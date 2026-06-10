//! `wasm-ffi` — raw WASM exports for the Rust `faustwasm` compile service.
//!
//! # Purpose
//! This crate is the Phase 2 binding layer from
//! `porting/faustwasm-dual-mode-rust-interface-plan-2026-03-26-en.md`.
//! It exposes a minimal request/response ABI on top of the pure-Rust compiler
//! compile service from `crates/compiler`.
//!
//! # Design
//! - raw exports only: no `wasm-bindgen` glue;
//! - one compile request in, one owned result handle out;
//! - JS reads returned buffers through pointer/length accessors, then frees the
//!   result handle explicitly.
//!
//! # Source provenance (C++)
//! - `compiler/generator/wasm/bindings/adapter.cpp`
//! - `compiler/generator/wasm/bindings/adapter.h`
//!
//! # Mapping status
//! `adapted` relative to the historical `libFaustWasm` Emscripten binding:
//! - preserved semantics: compile Faust source -> `{ wasm, json }`;
//! - adapted ABI: integer result handles + raw ptr/len accessors instead of C++
//!   vectors/factory pointers;
//! - partial helper compatibility:
//!   - implemented now: `getInfos(version|help)`
//!   - explicit stubs: `expandDSP`, `generateAuxFiles`, and the remaining
//!     `getInfos(...)` keys
//! - deferred: compatibility naming shim.
//!
//! # Packaging
//! The intended compiler-module artifact is built with:
//! `cargo run -p xtask -- build-faustwasm-compiler-module`
//!
//! That workflow produces and validates the standalone
//! `target/wasm32-unknown-unknown/release/faust_wasm_ffi.wasm` module consumed
//! by the future `faustwasm` embedded-compiler loader.
//!
//! # Embedded Faust libraries
//! The compiler-module can embed a read-only bundle of Faust library sources
//! discovered at build time. The raw compile export installs that bundle into
//! [`WasmArtifactRequest::virtual_sources`], so:
//! - parser-side `import("stdfaust.lib")` works from a source string,
//! - evaluator-side `library("maths.lib")` / `component("...")` can keep
//!   resolving against the same in-memory bundle,
//! - the shipped WASM compiler-module stays self-contained for the standard
//!   library set plus any explicitly embedded local `.lib` roots, without
//!   recreating an Emscripten-style virtual filesystem.

#![allow(non_snake_case)]
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::slice;
use std::str;
use std::sync::{Mutex, OnceLock};

use codegen::backends::wasm::WasmOptions;
use compiler::{Compiler, RealType, WasmArtifactBundle, WasmArtifactRequest};
use parser::VirtualSourceMap;

include!(concat!(env!("OUT_DIR"), "/embedded_faust_libraries.rs"));

const WASM_FFI_VERSION: &str = concat!("faust-rs-wasm-ffi/", env!("CARGO_PKG_VERSION"));

/// Stored compile-service outcome kept behind an integer handle.
///
/// The raw WASM ABI never returns owned Rust objects directly. Instead, the
/// host gets a numeric handle, then reads the payload through the exported
/// `ptr`/`len` accessors until it explicitly calls
/// [`faust_wasm_result_free`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredCompileResult {
    Ok(WasmArtifactBundle),
    Err(String),
}

/// Stored text-helper outcome kept behind an integer handle.
///
/// This is used by helper APIs such as `get_info`, `expand_dsp`, and
/// `generate_aux_files` so they can share the same explicit-lifetime pattern as
/// compile results.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredTextResult {
    Ok(String),
    Err(String),
}

/// Registry for compile result handles exposed to the host.
///
/// Handles start at `1`; `0` is naturally reserved as an invalid/null-like
/// value on the JS side.
#[derive(Default)]
struct ResultRegistry {
    next_handle: u32,
    entries: HashMap<u32, StoredCompileResult>,
}

impl ResultRegistry {
    /// Store one compile result and return the exported handle.
    fn insert(&mut self, result: StoredCompileResult) -> u32 {
        let next = self.next_handle.saturating_add(1).max(1);
        self.next_handle = next;
        self.entries.insert(next, result);
        next
    }

    /// Borrow one stored compile result by handle.
    fn get(&self, handle: u32) -> Option<&StoredCompileResult> {
        self.entries.get(&handle)
    }

    /// Drop one stored compile result and release its owned buffers.
    fn remove(&mut self, handle: u32) {
        self.entries.remove(&handle);
    }
}

/// Global compile-result registry used by the raw WASM ABI.
///
/// A process-global mutex is sufficient here because the compiler-module is
/// instantiated as a single WASM module and the host drives requests through
/// explicit handles.
fn registry() -> &'static Mutex<ResultRegistry> {
    static REGISTRY: OnceLock<Mutex<ResultRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(ResultRegistry::default()))
}

/// Global text-result registry used by the raw helper APIs.
fn text_registry() -> &'static Mutex<ResultRegistryText> {
    static REGISTRY: OnceLock<Mutex<ResultRegistryText>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(ResultRegistryText::default()))
}

/// Split `faustwasm`-style argument strings using the historical C++ binding
/// convention.
///
/// This intentionally keeps empty tokens created by repeated spaces because the
/// compatibility layer in `faustwasm` historically forwarded raw strings rather
/// than a shell-escaped argv array.
fn split_faustwasm_args(args: &str) -> Vec<String> {
    args.split(' ').map(str::to_owned).collect()
}

/// Registry for text-result handles exposed to the host.
#[derive(Default)]
struct ResultRegistryText {
    next_handle: u32,
    entries: HashMap<u32, StoredTextResult>,
}

impl ResultRegistryText {
    /// Store one text result and return the exported handle.
    fn insert(&mut self, result: StoredTextResult) -> u32 {
        let next = self.next_handle.saturating_add(1).max(1);
        self.next_handle = next;
        self.entries.insert(next, result);
        next
    }

    /// Borrow one stored text result by handle.
    fn get(&self, handle: u32) -> Option<&StoredTextResult> {
        self.entries.get(&handle)
    }

    /// Drop one stored text result and release its owned payload.
    fn remove(&mut self, handle: u32) {
        self.entries.remove(&handle);
    }
}

/// Parse one raw compile request from host strings into the typed compiler
/// service request.
///
/// Besides option parsing, this is where the embedded standard-library bundle
/// is attached so source-string compilation can resolve `import("stdfaust.lib")`
/// without a host filesystem.
/// Build a `VirtualSourceMap` from the embedded stdlib bundle extended with
/// any `--virtual-source name=base64` entries found in `argv`.
/// Builds the compiler for one helper request, honoring `-pn <name>`
/// (process entry-point selection — e.g. `-pn effect` compiles the
/// `effect =` declaration, matching the C++ compiler's option).
fn compiler_with_process_name_from_argv(argv: &[String]) -> Compiler {
    let process_name = argv
        .iter()
        .position(|arg| arg == "-pn")
        .and_then(|position| argv.get(position + 1))
        .cloned();
    let compiler = Compiler::new();
    match process_name {
        Some(process_name) => compiler.with_process_name(process_name),
        None => compiler,
    }
}

fn virtual_sources_from_argv(argv: &[String]) -> VirtualSourceMap {
    let mut vsources = embedded_standard_library_sources();
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--virtual-source"
            && let Some(spec) = argv.get(i + 1)
        {
            if let Some(eq) = spec.find('=') {
                let lib_name = &spec[..eq];
                let b64 = &spec[eq + 1..];
                if let Ok(bytes) = base64_decode(b64.as_bytes())
                    && let Ok(content) = String::from_utf8(bytes)
                {
                    vsources = vsources.with_source(lib_name, content);
                }
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    vsources
}

fn parse_compile_request(
    name: &str,
    source: &str,
    args: &str,
    internal_memory: bool,
) -> Result<WasmArtifactRequest, String> {
    let _embedded_root = embedded_standard_library_root();
    let _embedded_roots = embedded_standard_library_roots();
    let argv = split_faustwasm_args(args);
    let parsed = utils::parse_ffi_compile_args(&argv)?;
    let mut request = WasmArtifactRequest::new(name, source);
    request.import_dirs = parsed.search_paths;
    request.virtual_sources = virtual_sources_from_argv(&argv);
    request.wasm_options = WasmOptions {
        double_precision: parsed.double,
        internal_memory,
        ..WasmOptions::default()
    };
    Ok(request)
}

/// Materialize the build-time embedded standard-library bundle as a virtual
/// source map.
///
/// The logical paths match the names used in Faust imports, for example
/// `stdfaust.lib`, `maths.lib`, or `oscillators.lib`.
fn embedded_standard_library_sources() -> VirtualSourceMap {
    VirtualSourceMap::new(
        EMBEDDED_FAUST_LIBRARIES
            .iter()
            .map(|(path, source)| (std::path::PathBuf::from(path), (*source).to_owned())),
    )
}

/// Return the filesystem root used at build time to assemble the embedded
/// standard library bundle, when known.
///
/// This is retained mostly for diagnostics/tests; runtime resolution itself is
/// done purely through [`embedded_standard_library_sources`].
fn embedded_standard_library_root() -> Option<&'static str> {
    EMBEDDED_FAUST_LIB_ROOT
}

/// Return the filesystem roots used at build time to assemble the embedded
/// `.lib` bundle.
fn embedded_standard_library_roots() -> &'static [&'static str] {
    EMBEDDED_FAUST_LIB_ROOTS
}

/// Compile one request into a stored success/error payload.
///
/// This is the bridge between the raw string ABI and the typed compiler crate.
/// It chooses the compiler real type from the requested WASM float mode, then
/// delegates to [`compiler::Compiler::compile_wasm_artifact`].
fn compile_to_stored_result(
    name: &str,
    source: &str,
    args: &str,
    internal_memory: bool,
) -> StoredCompileResult {
    let request = match parse_compile_request(name, source, args, internal_memory) {
        Ok(request) => request,
        Err(error) => return StoredCompileResult::Err(error),
    };
    let compiler = Compiler::new().with_real_type(if request.wasm_options.double_precision {
        RealType::Float64
    } else {
        RealType::Float32
    });
    match compiler.compile_wasm_artifact(&request) {
        Ok(bundle) => StoredCompileResult::Ok(bundle),
        Err(error) => StoredCompileResult::Err(error.to_string()),
    }
}

/// Decode one UTF-8 argument slice passed from the host.
///
/// All raw exports use `ptr + len` pairs so the ABI remains independent from
/// C-string termination rules.
unsafe fn decode_utf8_arg<'a>(ptr: *const u8, len: usize, label: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("null {label} pointer"));
    }
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes).map_err(|error| format!("invalid UTF-8 in {label}: {error}"))
}

/// Borrow one compile result handle for the duration of `f`.
fn with_result<R>(handle: u32, f: impl FnOnce(Option<&StoredCompileResult>) -> R) -> R {
    let guard = registry().lock().expect("wasm-ffi registry poisoned");
    f(guard.get(handle))
}

/// Store one compile result and return its exported handle.
fn store_result(result: StoredCompileResult) -> u32 {
    let mut guard = registry().lock().expect("wasm-ffi registry poisoned");
    guard.insert(result)
}

/// Borrow one text result handle for the duration of `f`.
fn with_text_result<R>(handle: u32, f: impl FnOnce(Option<&StoredTextResult>) -> R) -> R {
    let guard = text_registry()
        .lock()
        .expect("wasm-ffi text registry poisoned");
    f(guard.get(handle))
}

/// Store one text result and return its exported handle.
fn store_text_result(result: StoredTextResult) -> u32 {
    let mut guard = text_registry()
        .lock()
        .expect("wasm-ffi text registry poisoned");
    guard.insert(result)
}

/// Read the compiled WASM payload pointer for one stored compile result.
///
/// The returned pointer stays valid until [`faust_wasm_result_free`] is called
/// for the same handle.
fn result_bytes_ptr(handle: u32) -> *const u8 {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(bundle)) => bundle.wasm_bytes.as_ptr(),
        _ => std::ptr::null(),
    })
}

/// Read the compiled WASM payload length for one stored compile result.
fn result_bytes_len(handle: u32) -> usize {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(bundle)) => bundle.wasm_bytes.len(),
        _ => 0,
    })
}

/// Read the companion JSON payload pointer for one stored compile result.
fn result_json_ptr(handle: u32) -> *const u8 {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(bundle)) => bundle.dsp_json.as_ptr(),
        _ => std::ptr::null(),
    })
}

/// Read the companion JSON payload length for one stored compile result.
fn result_json_len(handle: u32) -> usize {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(bundle)) => bundle.dsp_json.len(),
        _ => 0,
    })
}

/// Read the `compile_options` payload pointer for one stored compile result.
fn result_compile_options_ptr(handle: u32) -> *const u8 {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(bundle)) => bundle.compile_options.as_ptr(),
        _ => std::ptr::null(),
    })
}

/// Read the `compile_options` payload length for one stored compile result.
fn result_compile_options_len(handle: u32) -> usize {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(bundle)) => bundle.compile_options.len(),
        _ => 0,
    })
}

/// Read the error payload pointer for one stored compile result.
fn result_error_ptr(handle: u32) -> *const u8 {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Err(message)) => message.as_ptr(),
        _ => std::ptr::null(),
    })
}

/// Read the error payload length for one stored compile result.
fn result_error_len(handle: u32) -> usize {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Err(message)) => message.len(),
        _ => 0,
    })
}

/// Read the UTF-8 payload pointer for one stored text result.
fn text_result_ptr(handle: u32) -> *const u8 {
    with_text_result(handle, |result| match result {
        Some(StoredTextResult::Ok(text)) | Some(StoredTextResult::Err(text)) => text.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Read the UTF-8 payload length for one stored text result.
fn text_result_len(handle: u32) -> usize {
    with_text_result(handle, |result| match result {
        Some(StoredTextResult::Ok(text)) | Some(StoredTextResult::Err(text)) => text.len(),
        None => 0,
    })
}

/// Allocates a writable region in the module linear memory for host-written
/// request payloads.
///
/// The caller must later release the region with [`faust_wasm_dealloc`].
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_alloc(len: usize) -> *mut u8 {
    let mut bytes = Vec::<u8>::with_capacity(len);
    let ptr = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    ptr
}

/// Releases a region previously allocated by [`faust_wasm_alloc`].
///
/// `len` must match the original allocation capacity.
///
/// # Safety
/// - `ptr` must have been returned by [`faust_wasm_alloc`].
/// - `len` must match the original allocation capacity passed to
///   [`faust_wasm_alloc`].
/// - the region must not be used again after this call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn faust_wasm_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Vec::from_raw_parts(ptr, 0, len));
    }
}

/// Returns the static binding version string pointer.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_version_ptr() -> *const u8 {
    WASM_FFI_VERSION.as_ptr()
}

/// Returns the static binding version string length.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_version_len() -> usize {
    WASM_FFI_VERSION.len()
}

/// Compiles one DSP source string to a stored `{ wasm, json }` result handle.
///
/// The `args` string follows the current C++ `libFaustWasm` convention:
/// it is split on plain spaces before parsing the shared Rust FFI option
/// subset (`-I`, `-cn`, `-double`).
///
/// On success the returned handle exposes three independent byte payloads:
/// compiled WASM bytes, companion JSON, and the backend-aware
/// `compile_options` string.
///
/// # Safety
/// - `name_ptr`, `source_ptr`, and `args_ptr` must point to readable byte
///   ranges of lengths `name_len`, `source_len`, and `args_len`.
/// - each byte range must contain valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn faust_wasm_compile_dsp(
    name_ptr: *const u8,
    name_len: usize,
    source_ptr: *const u8,
    source_len: usize,
    args_ptr: *const u8,
    args_len: usize,
    internal_memory: u32,
) -> u32 {
    let result = unsafe {
        let name = match decode_utf8_arg(name_ptr, name_len, "name") {
            Ok(name) => name,
            Err(error) => return store_result(StoredCompileResult::Err(error)),
        };
        let source = match decode_utf8_arg(source_ptr, source_len, "source") {
            Ok(source) => source,
            Err(error) => return store_result(StoredCompileResult::Err(error)),
        };
        let args = match decode_utf8_arg(args_ptr, args_len, "args") {
            Ok(args) => args,
            Err(error) => return store_result(StoredCompileResult::Err(error)),
        };
        compile_to_stored_result(name, source, args, internal_memory != 0)
    };
    store_result(result)
}

/// Returns `1` for a successful compile result and `0` for an error result or
/// unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_is_ok(handle: u32) -> u32 {
    with_result(handle, |result| match result {
        Some(StoredCompileResult::Ok(_)) => 1,
        _ => 0,
    })
}

/// Returns the pointer to the compiled WASM bytes for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_wasm_ptr(handle: u32) -> *const u8 {
    result_bytes_ptr(handle)
}

/// Returns the compiled WASM byte length for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_wasm_len(handle: u32) -> usize {
    result_bytes_len(handle)
}

/// Returns the pointer to the companion JSON UTF-8 bytes for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_json_ptr(handle: u32) -> *const u8 {
    result_json_ptr(handle)
}

/// Returns the companion JSON byte length for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_json_len(handle: u32) -> usize {
    result_json_len(handle)
}

/// Returns the pointer to the `compile_options` UTF-8 bytes for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_compile_options_ptr(handle: u32) -> *const u8 {
    result_compile_options_ptr(handle)
}

/// Returns the `compile_options` byte length for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_compile_options_len(handle: u32) -> usize {
    result_compile_options_len(handle)
}

/// Returns the pointer to the error message UTF-8 bytes for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_error_ptr(handle: u32) -> *const u8 {
    result_error_ptr(handle)
}

/// Returns the error message byte length for `handle`.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_error_len(handle: u32) -> usize {
    result_error_len(handle)
}

/// Releases a stored compile result handle and all owned buffers behind it.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_result_free(handle: u32) {
    let mut guard = registry().lock().expect("wasm-ffi registry poisoned");
    guard.remove(handle);
}

/// Queries one `faustwasm` helper-info string and returns a stored text-result
/// handle.
///
/// Supported now: `version`, `help`. Other known keys return explicit
/// `unsupported` errors.
///
/// # Safety
/// - `what_ptr` must point to a readable UTF-8 byte range of length
///   `what_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn faust_wasm_get_info(what_ptr: *const u8, what_len: usize) -> u32 {
    let result = unsafe {
        let what = match decode_utf8_arg(what_ptr, what_len, "what") {
            Ok(what) => what,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let compiler = Compiler::new();
        match compiler.get_faustwasm_info(what) {
            Ok(text) => StoredTextResult::Ok(text),
            Err(error) => StoredTextResult::Err(error.to_string()),
        }
    };
    store_text_result(result)
}

/// Export for the `expandDSP(...)` helper service.
///
/// The raw ABI mirrors the text-result handle convention used elsewhere in the
/// module. The underlying compiler service may still return `unsupported`
/// errors for requests outside the currently implemented Rust subset.
///
/// # Safety
/// - `name_ptr`, `source_ptr`, and `args_ptr` must point to readable UTF-8 byte
///   ranges of lengths `name_len`, `source_len`, and `args_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn faust_wasm_expand_dsp(
    name_ptr: *const u8,
    name_len: usize,
    source_ptr: *const u8,
    source_len: usize,
    args_ptr: *const u8,
    args_len: usize,
) -> u32 {
    let result = unsafe {
        let name = match decode_utf8_arg(name_ptr, name_len, "name") {
            Ok(name) => name,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let source = match decode_utf8_arg(source_ptr, source_len, "source") {
            Ok(source) => source,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let args = match decode_utf8_arg(args_ptr, args_len, "args") {
            Ok(args) => args,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let compiler = Compiler::new();
        match compiler.expand_dsp(&compiler::ExpandDspRequest {
            source_name: name.to_owned(),
            source: source.to_owned(),
            args: args.to_owned(),
        }) {
            Ok(text) => StoredTextResult::Ok(text),
            Err(error) => StoredTextResult::Err(error.to_string()),
        }
    };
    store_text_result(result)
}

/// Transfer object for one auxiliary file artifact returned by
/// [`faust_wasm_generate_aux_files_json`].
///
/// Serialised as a JSON object with three fields:
///
/// - `"path"` — relative filename within the `<name>-svg/` hierarchy
///   (e.g. `"process.svg"`, `"process_0x1234.svg"`).  Matches the `href`
///   attribute values embedded in sibling SVG files so cross-file links can
///   be resolved by a simple map lookup.
/// - `"binary"` — `true` for opaque binary payloads (e.g. `.wasm`), `false`
///   for text-encodable files (SVG, JSON, C/C++ source).
/// - `"content_base64"` — base64-encoded file content.  All artifacts use
///   base64 regardless of the `binary` flag to keep the outer JSON valid for
///   arbitrary byte sequences.
struct WasmAuxFileArtifact {
    path: String,
    binary: bool,
    content_base64: String,
}

/// Encode `bytes` as standard base64 (RFC 4648, alphabet `A-Za-z0-9+/`, `=`
/// padding).
///
/// Pure-Rust implementation kept inline to avoid introducing a new dependency
/// into a crate compiled for `wasm32-unknown-unknown`.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in chunks.by_ref() {
        let b0 = chunk[0] as usize;
        let b1 = chunk[1] as usize;
        let b2 = chunk[2] as usize;
        out.push(ALPHABET[b0 >> 2] as char);
        out.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        out.push(ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        out.push(ALPHABET[b2 & 0x3f] as char);
    }
    match chunks.remainder() {
        [b0] => {
            let b0 = *b0 as usize;
            out.push(ALPHABET[b0 >> 2] as char);
            out.push(ALPHABET[(b0 & 0x03) << 4] as char);
            out.push('=');
            out.push('=');
        }
        [b0, b1] => {
            let b0 = *b0 as usize;
            let b1 = *b1 as usize;
            out.push(ALPHABET[b0 >> 2] as char);
            out.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            out.push(ALPHABET[(b1 & 0x0f) << 2] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Escape a string for embedding inside a JSON double-quoted value.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Decode a standard base64 byte slice back to bytes.
///
/// Used for `--virtual-source` arg injection and in `#[cfg(test)]` round-trip assertions.
fn base64_decode(s: &[u8]) -> Result<Vec<u8>, ()> {
    const DEC: [u8; 128] = {
        let mut t = [255u8; 128];
        let mut i = 0u8;
        loop {
            let ch =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"[i as usize];
            t[ch as usize] = i;
            i += 1;
            if i == 64 {
                break;
            }
        }
        t
    };
    let bytes: Vec<u8> = s.iter().copied().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let v: Vec<u8> = chunk
            .iter()
            .map(|&b| {
                if (b as usize) < 128 {
                    DEC[b as usize]
                } else {
                    255
                }
            })
            .collect();
        match v.as_slice() {
            [a, b, c, d] if *a != 255 && *b != 255 && *c != 255 && *d != 255 => {
                out.push((a << 2) | (b >> 4));
                out.push((b << 4) | (c >> 2));
                out.push((c << 6) | d);
            }
            [a, b, c] if *a != 255 && *b != 255 && *c != 255 => {
                out.push((a << 2) | (b >> 4));
                out.push((b << 4) | (c >> 2));
            }
            [a, b] if *a != 255 && *b != 255 => {
                out.push((a << 2) | (b >> 4));
            }
            _ => return Err(()),
        }
    }
    Ok(out)
}

/// Serialise a slice of [`WasmAuxFileArtifact`] as a UTF-8 JSON array.
///
/// Each element is rendered as:
/// ```json
/// {"path":"...","binary":false,"content_base64":"..."}
/// ```
fn artifacts_to_json(artifacts: &[WasmAuxFileArtifact]) -> String {
    let items: Vec<String> = artifacts
        .iter()
        .map(|a| {
            format!(
                r#"{{"path":"{}","binary":{},"content_base64":"{}"}}"#,
                json_escape(&a.path),
                a.binary,
                a.content_base64
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// Export for the `generateAuxFiles(...)` helper service.
///
/// This API intentionally keeps the coarse historical success/failure shape
/// used by `faustwasm`, even though the Rust compiler service underneath can
/// represent richer aux-file payloads.
///
/// Returns `1` only when aux-file generation succeeds.
///
/// # Safety
/// - `name_ptr`, `source_ptr`, and `args_ptr` must point to readable UTF-8 byte
///   ranges of lengths `name_len`, `source_len`, and `args_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn faust_wasm_generate_aux_files(
    name_ptr: *const u8,
    name_len: usize,
    source_ptr: *const u8,
    source_len: usize,
    args_ptr: *const u8,
    args_len: usize,
) -> u32 {
    let name = match unsafe { decode_utf8_arg(name_ptr, name_len, "name") } {
        Ok(name) => name,
        Err(_) => return 0,
    };
    let source = match unsafe { decode_utf8_arg(source_ptr, source_len, "source") } {
        Ok(source) => source,
        Err(_) => return 0,
    };
    let args = match unsafe { decode_utf8_arg(args_ptr, args_len, "args") } {
        Ok(args) => args,
        Err(_) => return 0,
    };
    let argv = split_faustwasm_args(args);
    let compiler = compiler_with_process_name_from_argv(&argv);
    match compiler.generate_aux_files(&compiler::GenerateAuxFilesRequest {
        source_name: name.to_owned(),
        source: source.to_owned(),
        args: args.to_owned(),
        virtual_sources: virtual_sources_from_argv(&argv),
    }) {
        Ok(_files) => 1,
        Err(_error) => 0,
    }
}

/// Export for the `generateAuxFilesJson(...)` helper service.
///
/// Generates auxiliary output files for the given DSP source, encodes every
/// artifact as base64, and returns a UTF-8 JSON array through the
/// text-result handle system.  The array contains one object per file:
///
/// ```json
/// [
///   {"path":"process.svg","binary":false,"content_base64":"PHN2Zy4uLg=="},
///   {"path":"process_0x1234.svg","binary":false,"content_base64":"PHN2Zy4uLg=="}
/// ]
/// ```
///
/// `process.svg` is always the first element when SVG output is requested.
/// The `path` values match the `href` attributes embedded in the SVG source
/// so cross-file links can be resolved by a simple map lookup.
///
/// On failure, the returned handle is an error text result carrying the
/// compiler diagnostic.  Use [`faust_wasm_text_result_is_ok`] to
/// distinguish the two cases before reading the payload.
///
/// The caller must release the handle with [`faust_wasm_text_result_free`].
///
/// # Safety
/// - `name_ptr`, `source_ptr`, and `args_ptr` must point to readable UTF-8
///   byte ranges of lengths `name_len`, `source_len`, and `args_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn faust_wasm_generate_aux_files_json(
    name_ptr: *const u8,
    name_len: usize,
    source_ptr: *const u8,
    source_len: usize,
    args_ptr: *const u8,
    args_len: usize,
) -> u32 {
    let result = unsafe {
        let name = match decode_utf8_arg(name_ptr, name_len, "name") {
            Ok(name) => name,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let source = match decode_utf8_arg(source_ptr, source_len, "source") {
            Ok(source) => source,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let args = match decode_utf8_arg(args_ptr, args_len, "args") {
            Ok(args) => args,
            Err(error) => return store_text_result(StoredTextResult::Err(error)),
        };
        let argv = split_faustwasm_args(args);
        let compiler = compiler_with_process_name_from_argv(&argv);
        match compiler.generate_aux_files(&compiler::GenerateAuxFilesRequest {
            source_name: name.to_owned(),
            source: source.to_owned(),
            args: args.to_owned(),
            virtual_sources: virtual_sources_from_argv(&argv),
        }) {
            Ok(files) => {
                let wasm_artifacts: Vec<WasmAuxFileArtifact> = files
                    .into_iter()
                    .map(|f| WasmAuxFileArtifact {
                        path: f.path,
                        binary: f.binary,
                        content_base64: base64_encode(&f.content),
                    })
                    .collect();
                StoredTextResult::Ok(artifacts_to_json(&wasm_artifacts))
            }
            Err(error) => StoredTextResult::Err(error.to_string()),
        }
    };
    store_text_result(result)
}

/// Returns `1` for a successful text result and `0` for an error result or
/// unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_text_result_is_ok(handle: u32) -> u32 {
    with_text_result(handle, |result| match result {
        Some(StoredTextResult::Ok(_)) => 1,
        _ => 0,
    })
}

/// Returns the UTF-8 pointer stored behind one text result handle.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_text_result_ptr(handle: u32) -> *const u8 {
    text_result_ptr(handle)
}

/// Returns the UTF-8 byte length stored behind one text result handle.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_text_result_len(handle: u32) -> usize {
    text_result_len(handle)
}

/// Releases a stored text result handle and the owned payload behind it.
#[unsafe(no_mangle)]
pub extern "C" fn faust_wasm_text_result_free(handle: u32) {
    let mut guard = text_registry()
        .lock()
        .expect("wasm-ffi text registry poisoned");
    guard.remove(handle);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        StoredCompileResult, StoredTextResult, compile_to_stored_result, parse_compile_request,
        split_faustwasm_args, store_result, store_text_result,
    };

    fn temp_root(test_name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock drift")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "faust_rs_wasm_ffi_{test_name}_{}_{}",
            std::process::id(),
            stamp
        ));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    #[test]
    fn split_faustwasm_args_matches_cxx_space_tokenization_shape() {
        assert_eq!(
            split_faustwasm_args("-double  -I dsp"),
            vec!["-double", "", "-I", "dsp"]
        );
    }

    #[test]
    fn parse_compile_request_maps_double_and_import_dirs() {
        let root = temp_root("parse_request");
        let request = parse_compile_request(
            "osc.dsp",
            "process = 0;",
            &format!("-double -I {}", root.display()),
            true,
        )
        .expect("request should parse");

        assert!(request.wasm_options.double_precision);
        assert!(request.wasm_options.internal_memory);
        assert_eq!(request.import_dirs, vec![root]);
    }

    #[test]
    fn compile_to_stored_result_returns_wasm_and_json_payloads() {
        let result = compile_to_stored_result("osc.dsp", "process = 0;", "", true);
        let StoredCompileResult::Ok(bundle) = result else {
            panic!("compile should succeed");
        };

        assert!(bundle.wasm_bytes.starts_with(b"\0asm"));
        assert!(bundle.dsp_json.contains("\"filename\": \"osc.dsp\""));
        assert_eq!(bundle.compile_options, "-lang wasm -single");
    }

    #[test]
    fn compile_to_stored_result_supports_string_source_import_dirs() {
        let root = temp_root("memory_import_dir");
        let child = root.join("child.lib");
        fs::write(&child, "process = _;\n").expect("write child");

        let result = compile_to_stored_result(
            "main.dsp",
            "process = component(\"child.lib\");",
            &format!("-I {}", root.display()),
            true,
        );
        let StoredCompileResult::Ok(bundle) = result else {
            panic!("compile with import dir should succeed");
        };

        assert!(bundle.wasm_bytes.starts_with(b"\0asm"));
        assert!(bundle.dsp_json.contains("child.lib"));
    }

    #[test]
    fn parse_compile_request_injects_virtual_source_from_flag() {
        // Encode a tiny Faust library as base64 and verify it resolves.
        let lib_content = "myconst = 42;";
        let b64 = super::base64_encode(lib_content.as_bytes());
        let args = format!("--virtual-source mylib.lib={}", b64);
        let result = compile_to_stored_result(
            "test.dsp",
            "import(\"mylib.lib\");\nprocess = myconst;",
            &args,
            true,
        );
        assert!(
            matches!(result, StoredCompileResult::Ok(_)),
            "compile with --virtual-source flag should succeed"
        );
    }

    #[test]
    fn embedded_standard_library_bundle_exposes_stdfaust_when_available() {
        let bundle = super::embedded_standard_library_sources();
        if super::embedded_standard_library_root().is_none() {
            assert!(bundle.is_empty());
            return;
        }
        assert!(bundle.contains(Path::new("stdfaust.lib")));
    }

    #[test]
    fn compile_to_stored_result_supports_embedded_stdfaust_when_available() {
        if super::embedded_standard_library_root().is_none() {
            return;
        }

        let result = compile_to_stored_result(
            "probe.dsp",
            "import(\"stdfaust.lib\");\nprocess = 0;\n",
            "",
            true,
        );
        let StoredCompileResult::Ok(bundle) = result else {
            panic!("compile with embedded stdfaust should succeed when bundled");
        };

        assert!(bundle.wasm_bytes.starts_with(b"\0asm"));
        assert!(bundle.dsp_json.contains("stdfaust.lib"));
    }

    #[test]
    fn stored_handles_keep_error_payloads_addressable_until_free() {
        let handle = store_result(StoredCompileResult::Err("boom".to_owned()));
        assert_eq!(super::faust_wasm_result_is_ok(handle), 0);
        assert_eq!(super::faust_wasm_result_error_len(handle), 4);
        assert!(!super::faust_wasm_result_error_ptr(handle).is_null());
        super::faust_wasm_result_free(handle);
        assert_eq!(super::faust_wasm_result_error_len(handle), 0);
    }

    #[test]
    fn text_result_handles_keep_payloads_addressable_until_free() {
        let handle = store_text_result(StoredTextResult::Ok("help".to_owned()));
        assert_eq!(super::faust_wasm_text_result_is_ok(handle), 1);
        assert_eq!(super::faust_wasm_text_result_len(handle), 4);
        assert!(!super::faust_wasm_text_result_ptr(handle).is_null());
        super::faust_wasm_text_result_free(handle);
        assert_eq!(super::faust_wasm_text_result_len(handle), 0);
    }

    // ── base64 helpers ──────────────────────────────────────────────────────

    #[test]
    fn base64_encode_empty_input_produces_empty_string() {
        assert_eq!(super::base64_encode(b""), "");
    }

    #[test]
    fn base64_encode_one_byte_produces_four_chars_with_double_padding() {
        // 0x4d = 'M' → "TQ=="
        assert_eq!(super::base64_encode(b"M"), "TQ==");
    }

    #[test]
    fn base64_encode_two_bytes_produces_four_chars_with_single_padding() {
        // "Ma" → "TWE="
        assert_eq!(super::base64_encode(b"Ma"), "TWE=");
    }

    #[test]
    fn base64_encode_three_bytes_produces_four_chars_no_padding() {
        assert_eq!(super::base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn base64_encode_known_rfc4648_vector() {
        // RFC 4648 §10 test vector
        assert_eq!(super::base64_encode(b"foobar"), "Zm9vYmFy");
    }

    // ── JSON serialisation ───────────────────────────────────────────────────

    #[test]
    fn artifacts_to_json_empty_slice_produces_empty_array() {
        assert_eq!(super::artifacts_to_json(&[]), "[]");
    }

    #[test]
    fn artifacts_to_json_single_text_artifact_round_trips() {
        let artifact = super::WasmAuxFileArtifact {
            path: "process.svg".to_owned(),
            binary: false,
            content_base64: "PHN2Zy8+".to_owned(),
        };
        let json = super::artifacts_to_json(&[artifact]);
        assert_eq!(
            json,
            r#"[{"path":"process.svg","binary":false,"content_base64":"PHN2Zy8+"}]"#
        );
    }

    #[test]
    fn artifacts_to_json_escapes_special_chars_in_path() {
        let artifact = super::WasmAuxFileArtifact {
            path: r#"a"b\c"#.to_owned(),
            binary: false,
            content_base64: String::new(),
        };
        let json = super::artifacts_to_json(&[artifact]);
        assert!(json.contains(r#""path":"a\"b\\c""#));
    }

    // ── generate_aux_files_json export ──────────────────────────────────────

    /// Helper: call faust_wasm_generate_aux_files_json and return the decoded
    /// result text (panics on UTF-8 error).
    fn call_generate_aux_files_json(name: &str, source: &str, args: &str) -> (u32, String) {
        let handle = unsafe {
            super::faust_wasm_generate_aux_files_json(
                name.as_ptr(),
                name.len(),
                source.as_ptr(),
                source.len(),
                args.as_ptr(),
                args.len(),
            )
        };
        let is_ok = super::faust_wasm_text_result_is_ok(handle);
        let ptr = super::faust_wasm_text_result_ptr(handle);
        let len = super::faust_wasm_text_result_len(handle);
        let text = unsafe {
            let bytes = std::slice::from_raw_parts(ptr, len);
            std::str::from_utf8(bytes)
                .expect("result must be UTF-8")
                .to_owned()
        };
        super::faust_wasm_text_result_free(handle);
        (is_ok, text)
    }

    #[test]
    fn generate_aux_files_json_with_svg_flag_returns_svg_artifact() {
        let (is_ok, json) = call_generate_aux_files_json("osc.dsp", "process = 0;", "-svg");
        assert_eq!(is_ok, 1, "expected success, got error: {json}");

        // Must be a JSON array.
        assert!(json.starts_with('['), "expected JSON array, got: {json}");
        // Must contain at least one SVG entry.
        assert!(
            json.contains("process.svg"),
            "process.svg missing from: {json}"
        );
        // The path field must come before the content for process.svg.
        assert!(
            json.contains(r#""path":"process.svg""#),
            "process.svg not first in: {json}"
        );
        // content_base64 must be present and non-empty.
        assert!(
            !json.contains(r#""content_base64":""#) || json.contains(r#""content_base64":"P"#),
            "content_base64 looks empty in: {json}"
        );
    }

    #[test]
    fn generate_aux_files_json_svg_content_base64_decodes_to_valid_svg() {
        let (is_ok, json) = call_generate_aux_files_json("osc.dsp", "process = 0;", "-svg");
        assert_eq!(is_ok, 1, "expected success, got error: {json}");

        // Extract the first content_base64 value by simple string search.
        let key = r#""content_base64":""#;
        let start = json.find(key).expect("content_base64 key missing") + key.len();
        let end = json[start..].find('"').expect("closing quote missing") + start;
        let b64 = &json[start..end];

        // Decode and verify it starts with the SVG magic bytes.
        let decoded = super::base64_decode(b64.as_bytes()).expect("base64 must decode");
        let text = std::str::from_utf8(&decoded).expect("SVG must be valid UTF-8");
        assert!(
            text.contains("<svg") || text.starts_with("<?xml"),
            "decoded content is not SVG: {}",
            &text[..text.len().min(120)]
        );
    }

    #[test]
    fn generate_aux_files_json_invalid_source_returns_error_handle() {
        let (is_ok, text) = call_generate_aux_files_json(
            "bad.dsp",
            "this is not valid faust source !!!§§§",
            "-svg",
        );
        assert_eq!(is_ok, 0, "expected error handle, got success with: {text}");
        assert!(!text.is_empty(), "error message must not be empty");
    }

    #[test]
    fn generate_aux_files_boolean_wrapper_still_returns_success_for_valid_source() {
        let name = "osc.dsp";
        let source = "process = 0;";
        let args = "-svg";
        let result = unsafe {
            super::faust_wasm_generate_aux_files(
                name.as_ptr(),
                name.len(),
                source.as_ptr(),
                source.len(),
                args.as_ptr(),
                args.len(),
            )
        };
        assert_eq!(result, 1, "boolean wrapper must still return 1 on success");
    }
}
