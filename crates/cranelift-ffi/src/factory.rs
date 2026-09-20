//! Factory-level `extern "C"` functions for `cranelift_dsp`.
//!
//! This module exports the runtime factory ABI for Cranelift and currently
//! includes:
//! - backend-prefixed naming (`createCCranelift...`)
//! - source creation keeps `opt_level`, omits LLVM `target`
//! - several LLVM-only families intentionally deferred in V1
//!
//! Runtime state:
//! - file/string constructors compile real FIR -> Cranelift JIT modules
//! - instances can execute real `compute` entry points
//! - signals/boxes constructors reuse `box-ffi` lowering bridges
//! - bitcode write now emits a textual `.clif` container payload (`FAUST_CLIF_V1`)
//!   while read-side migration is completed incrementally.

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_void};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use box_ffi::{BoxFfiFirModule, export_fir_from_box_handle, export_fir_from_signal_array_handle};
use codegen::backends::cranelift::{
    CraneliftOptLevel, CraneliftOptions, JitDspModule, generate_cranelift_module,
};
use codegen::json::{JsonBuildOptions, JsonMemoryDescription, build_json_description_from_fir};
use codegen::memory_layout::MemoryManagerMode;
use compiler::{
    AuxFileArtifact, Compiler as FaustCompiler, CompilerError, ComputeMode, ExpandDspRequest,
    FaustwasmServiceError, GenerateAuxFilesRequest, RealType, SchedulingStrategy, SignalFirLane,
    TableInitMode, merge_import_search_paths,
};
use ffi_common::{
    CompleteError, FaustMemoryManager, decode_c_argv as decode_c_argv_shared,
    free_c_memory_c_string_only, null_c_string_array, optional_c_string_arg,
    parse_ffi_compile_args, required_c_string_arg, write_error_4096,
};
use fir::{FirMatch, match_fir};

use crate::cache::{
    cache_all_sha_keys, cache_clear, cache_insert, cache_lookup, cache_release, start_mt, stop_mt,
};
use crate::clif::{CLIF_MAGIC, decode_factory_clif, encode_factory_clif};
use crate::runtime::build_runtime_descriptor;
use crate::types::{CraneliftDspFactory, FactoryMemoryState, MemoryManagerBinding, alloc_c_string};

/// Stable version string returned by [`getCLibFaustVersion`].
const CRANELIFT_FFI_VERSION: &str = concat!("faust-rs-cranelift-ffi/", env!("CARGO_PKG_VERSION"));

fn foreign_function_registry() -> &'static Mutex<HashMap<String, usize>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn snapshot_registered_foreign_functions() -> HashMap<String, *const c_void> {
    foreign_function_registry()
        .lock()
        .expect("foreign function registry mutex")
        .iter()
        .map(|(name, addr)| (name.clone(), (*addr as *const c_void)))
        .collect()
}

fn foreign_function_registry_fingerprint() -> String {
    let mut entries: Vec<_> = foreign_function_registry()
        .lock()
        .expect("foreign function registry mutex")
        .iter()
        .map(|(name, addr)| format!("{name}=0x{addr:x}"))
        .collect();
    entries.sort();
    entries.join(",")
}

#[cfg(test)]
fn clear_registered_foreign_functions() {
    foreign_function_registry()
        .lock()
        .expect("foreign function registry mutex")
        .clear();
}

/// Returns the Faust library version string.
///
/// This is a process-lifetime static C string.
///
/// # Safety
/// The returned pointer is process-static and must not be freed or mutated.
#[cfg_attr(feature = "standalone-capi-globals", unsafe(no_mangle))]
pub extern "C" fn getCLibFaustVersion() -> *const c_char {
    use std::sync::OnceLock;
    static VERSION_C: OnceLock<CString> = OnceLock::new();
    VERSION_C
        .get_or_init(|| CString::new(CRANELIFT_FFI_VERSION).expect("version contains no NUL"))
        .as_ptr()
}

/// Register one host foreign function for subsequent Cranelift factory builds.
///
/// The registration is process-global and must happen before compiling the DSP
/// factory that references the symbol through `ffunction(...)`.
///
/// # Safety
/// - `name` must be a valid null-terminated C string.
/// - `fn_ptr` must be a valid callable function address for the symbol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn registerCCraneliftForeignFunction(
    name: *const c_char,
    fn_ptr: *mut c_void,
) {
    if name.is_null() || fn_ptr.is_null() {
        return;
    }
    // SAFETY: caller provides a valid C string per the function contract.
    let Ok(name) = unsafe { std::ffi::CStr::from_ptr(name) }.to_str() else {
        return;
    };
    foreign_function_registry()
        .lock()
        .expect("foreign function registry mutex")
        .insert(name.to_owned(), fn_ptr as usize);
}

/// Unregister one previously registered host foreign function.
///
/// The operation is process-global and only affects future Cranelift factory
/// builds. Existing compiled factories are unchanged.
///
/// # Safety
/// - `name` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unregisterCCraneliftForeignFunction(name: *const c_char) {
    if name.is_null() {
        return;
    }
    // SAFETY: caller provides a valid C string per the function contract.
    let Ok(name) = unsafe { std::ffi::CStr::from_ptr(name) }.to_str() else {
        return;
    };
    foreign_function_registry()
        .lock()
        .expect("foreign function registry mutex")
        .remove(name);
}

/// Clear all previously registered host foreign functions.
///
/// The operation is process-global and only affects future Cranelift factory
/// builds. Existing compiled factories are unchanged.
#[unsafe(no_mangle)]
pub extern "C" fn clearCCraneliftForeignFunctions() {
    foreign_function_registry()
        .lock()
        .expect("foreign function registry mutex")
        .clear();
}

/// Create a Cranelift DSP factory from a Faust source file.
///
/// # Safety
/// - `filename` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid null-terminated C strings (or be null if `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn createCCraneliftDSPFactoryFromFile(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
    opt_level: c_int,
) -> *mut CraneliftDspFactory {
    unsafe {
        let filename = match required_c_string_arg(filename, "filename") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(args) => args,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        create_cranelift_factory_with_argv(&args, error_msg, |args| {
            let compiled =
                preflight_compile_file_to_cranelift(Path::new(&filename), args, opt_level)?;
            let dsp_source = std::fs::read_to_string(&filename)
                .map_err(|e| format!("cannot read DSP source '{filename}': {e}"))?;
            build_scaffold_factory_from_file(
                FileFactoryBuildSpec {
                    filename: &filename,
                    dsp_source: &dsp_source,
                    argv: args,
                    opt_level,
                    foreign_function_fingerprint: &compiled.foreign_function_fingerprint,
                },
                &compiled.fir,
                Some(compiled.jit),
            )
        })
    }
}

/// Create a Cranelift DSP factory from a Faust source string.
///
/// # Safety
/// - `dsp_content` must be a valid null-terminated C string.
/// - `name_app` may be null; if non-null it must be a valid C string.
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn createCCraneliftDSPFactoryFromString(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
    opt_level: c_int,
) -> *mut CraneliftDspFactory {
    unsafe {
        let name_app = match optional_c_string_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP".to_owned(),
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let dsp_content = match required_c_string_arg(dsp_content, "dsp_content") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(args) => args,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        create_cranelift_factory_with_argv(&args, error_msg, |args| {
            let compiled =
                preflight_compile_source_to_cranelift(&name_app, &dsp_content, opt_level, args)?;
            build_scaffold_factory_common(
                FactoryBuildSpec {
                    name: &name_app,
                    dsp_code: &dsp_content,
                    argv: args,
                    opt_level,
                    foreign_function_fingerprint: &compiled.foreign_function_fingerprint,
                    source_is_faust: true,
                },
                &compiled.fir,
                Some(compiled.jit),
            )
        })
    }
}

/// Create a Cranelift DSP factory from a null-terminated signal handle array.
///
/// The signal array contract matches `box-ffi` (`CboxesToSignals*`):
/// - `signals` points to a null-terminated `*mut c_void` array,
/// - each non-null entry is a signal handle managed by the `box-ffi` context.
///
/// # Safety
/// - `name_app` may be null; if non-null it must be a valid C string.
/// - `signals` must follow the handle-array contract above.
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn createCCraneliftDSPFactoryFromSignals(
    name_app: *const c_char,
    signals: *mut c_void,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
    opt_level: c_int,
) -> *mut CraneliftDspFactory {
    unsafe {
        let source_name = match optional_c_string_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP".to_owned(),
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(args) => args,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        if signals.is_null() {
            write_error(error_msg, "null signals pointer");
            return std::ptr::null_mut();
        }
        create_cranelift_factory_with_argv(&args, error_msg, |args| {
            let fir = export_fir_from_signal_array_handle(&source_name, signals)?;
            let fir_dump = fir::dump_fir(&fir.store, fir.module);
            let parsed = parse_ffi_compile_args(args)?;
            let jit = compile_fir_module_to_cranelift(
                &fir,
                opt_level,
                parsed.double,
                memory_manager_mode(parsed.memory_manager0),
            )?;
            let foreign_function_fingerprint = foreign_function_registry_fingerprint();
            build_scaffold_factory_common(
                FactoryBuildSpec {
                    name: &source_name,
                    dsp_code: &fir_dump,
                    argv: args,
                    opt_level,
                    foreign_function_fingerprint: &foreign_function_fingerprint,
                    source_is_faust: false,
                },
                &fir,
                Some(jit),
            )
        })
    }
}

/// Create a Cranelift DSP factory from one `box-ffi` box handle.
///
/// # Safety
/// - `name_app` may be null; if non-null it must be a valid C string.
/// - `box_expr` must be a valid `box-ffi` box handle.
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn createCCraneliftDSPFactoryFromBoxes(
    name_app: *const c_char,
    box_expr: *mut c_void,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
    opt_level: c_int,
) -> *mut CraneliftDspFactory {
    unsafe {
        let source_name = match optional_c_string_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP".to_owned(),
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(args) => args,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        if box_expr.is_null() {
            write_error(error_msg, "null box_expr pointer");
            return std::ptr::null_mut();
        }
        create_cranelift_factory_with_argv(&args, error_msg, |args| {
            let fir = export_fir_from_box_handle(&source_name, box_expr)?;
            let fir_dump = fir::dump_fir(&fir.store, fir.module);
            let parsed = parse_ffi_compile_args(args)?;
            let jit = compile_fir_module_to_cranelift(
                &fir,
                opt_level,
                parsed.double,
                memory_manager_mode(parsed.memory_manager0),
            )?;
            let foreign_function_fingerprint = foreign_function_registry_fingerprint();
            build_scaffold_factory_common(
                FactoryBuildSpec {
                    name: &source_name,
                    dsp_code: &fir_dump,
                    argv: args,
                    opt_level,
                    foreign_function_fingerprint: &foreign_function_fingerprint,
                    source_is_faust: false,
                },
                &fir,
                Some(jit),
            )
        })
    }
}

/// Returns a factory from the cache by SHA key.
///
/// # Safety
/// `sha_key` may be null; invalid UTF-8 returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryFromSHAKey(
    sha_key: *const c_char,
) -> *mut CraneliftDspFactory {
    unsafe {
        let sha_key = match required_c_string_arg(sha_key, "sha_key") {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        cache_lookup(&sha_key)
    }
}

/// Release one Cranelift DSP factory reference.
///
/// Returns `true` only when this was the last reference. Final release also
/// deletes any DSP instances that were not deleted manually.
///
/// # Safety
/// `factory` must be a valid pointer previously returned by a Cranelift factory
/// creation function, and must not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn deleteCCraneliftDSPFactory(factory: *mut CraneliftDspFactory) -> bool {
    if factory.is_null() {
        return false;
    }
    cache_release(factory)
}

/// Delete all cached Cranelift factories and their remaining DSP instances.
///
/// Every outstanding factory and instance pointer is invalid after this call,
/// regardless of its acquired reference count.
#[unsafe(no_mangle)]
pub extern "C" fn deleteAllCCraneliftDSPFactories() {
    cache_clear();
}

/// Return all cached Cranelift factory SHA keys as a null-terminated array.
///
/// The returned strings must be freed individually with `freeCMemory`. As in
/// the current `interp-ffi` implementation, the outer array deallocation path is
/// not yet modeled separately in the scaffold.
///
/// # Safety
/// The caller owns the returned allocations and must free each returned string;
/// the outer array ownership follows the crate's current scaffold contract.
#[unsafe(no_mangle)]
pub extern "C" fn getAllCCraneliftDSPFactories() -> *mut *mut c_char {
    let keys = cache_all_sha_keys();
    if keys.is_empty() {
        return std::ptr::null_mut();
    }
    let mut ptrs: Vec<*mut c_char> = keys.into_iter().map(|k| alloc_c_string(&k)).collect();
    ptrs.push(std::ptr::null_mut());
    let boxed: Box<[*mut c_char]> = ptrs.into_boxed_slice();
    Box::into_raw(boxed).cast::<*mut c_char>()
}

/// Return a factory JSON description string.
///
/// The returned string must be freed by the caller with [`freeCMemory`].
///
/// # Safety
/// `factory` must be a valid factory pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryJSON(
    factory: *mut CraneliftDspFactory,
) -> *mut c_char {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        alloc_c_string(&(*factory).json)
    }
}

/// Return the factory name as a heap C string.
///
/// # Safety
/// `factory` must be a valid factory pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryName(
    factory: *mut CraneliftDspFactory,
) -> *mut c_char {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        alloc_c_string(&(*factory).name)
    }
}

/// Return the factory SHA key as a heap C string.
///
/// # Safety
/// `factory` must be a valid factory pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactorySHAKey(
    factory: *mut CraneliftDspFactory,
) -> *mut c_char {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        alloc_c_string(&(*factory).sha_key)
    }
}

/// Return the expanded DSP code as a heap C string.
///
/// # Safety
/// `factory` must be a valid factory pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryDSPCode(
    factory: *mut CraneliftDspFactory,
) -> *mut c_char {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        alloc_c_string(&(*factory).dsp_code)
    }
}

/// Return the compile options string as a heap C string.
///
/// # Safety
/// `factory` must be a valid factory pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryCompileOptions(
    factory: *mut CraneliftDspFactory,
) -> *mut c_char {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        alloc_c_string(&(*factory).compile_options)
    }
}

/// Return the factory library dependency list.
///
/// # Safety
/// `factory` may be null; it is ignored by the current implementation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryLibraryList(
    _factory: *mut CraneliftDspFactory,
) -> *const *const c_char {
    null_c_string_array()
}

/// Return include pathnames used by the factory.
///
/// # Safety
/// `factory` may be null; it is ignored by the current implementation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryIncludePathnames(
    _factory: *mut CraneliftDspFactory,
) -> *const *const c_char {
    null_c_string_array()
}

/// Return warning messages produced during compilation.
///
/// # Safety
/// `factory` may be null; it is ignored by the current implementation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getCCraneliftDSPFactoryWarningMessages(
    _factory: *mut CraneliftDspFactory,
) -> *const *const c_char {
    null_c_string_array()
}

/// Read a Cranelift factory from a textual `.clif` bitcode payload in memory.
///
/// The current `FAUST_CLIF_V1` read path rebuilds a runnable JIT factory from
/// serialized source fallback + options while validating identity fields.
///
/// # Safety
/// `error_msg` follows the standard Faust C API error-buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readCCraneliftDSPFactoryFromBitcode(
    bit_code: *const c_char,
    error_msg: *mut c_char,
) -> *mut CraneliftDspFactory {
    unsafe {
        let text = match required_c_string_arg(bit_code, "bitcode") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        match decode_factory_bitcode(&text) {
            Ok(factory) => {
                let sha = factory.sha_key.clone();
                cache_insert(&sha, factory)
            }
            Err(e) => {
                write_error(error_msg, &e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Write a Cranelift factory to a textual `.clif` container string.
///
/// Returns null when the factory is not source-backed (for example created
/// from signal/box handles).
///
/// # Safety
/// `factory` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn writeCCraneliftDSPFactoryToBitcode(
    factory: *mut CraneliftDspFactory,
) -> *mut c_char {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        match encode_factory_clif(&*factory) {
            Ok(payload) => alloc_c_string(&payload),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// Read a Cranelift factory from a textual `.clif` bitcode file.
///
/// # Safety
/// `error_msg` follows the standard Faust C API error-buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readCCraneliftDSPFactoryFromBitcodeFile(
    bit_code_path: *const c_char,
    error_msg: *mut c_char,
) -> *mut CraneliftDspFactory {
    unsafe {
        let path = match required_c_string_arg(bit_code_path, "path") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                write_error(
                    error_msg,
                    &format!("cannot read bitcode file '{path}': {e}"),
                );
                return std::ptr::null_mut();
            }
        };
        match decode_factory_bitcode(&text) {
            Ok(factory) => {
                let sha = factory.sha_key.clone();
                cache_insert(&sha, factory)
            }
            Err(e) => {
                write_error(error_msg, &e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Write a Cranelift factory to a textual `.clif` container file.
///
/// # Safety
/// `factory` and `bit_code_path` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn writeCCraneliftDSPFactoryToBitcodeFile(
    factory: *mut CraneliftDspFactory,
    bit_code_path: *const c_char,
) -> bool {
    unsafe {
        if factory.is_null() || bit_code_path.is_null() {
            return false;
        }
        let path = match required_c_string_arg(bit_code_path, "path") {
            Ok(s) => s,
            Err(_) => return false,
        };
        let payload = match encode_factory_clif(&*factory) {
            Ok(payload) => payload,
            Err(_) => return false,
        };
        std::fs::write(path, payload).is_ok()
    }
}

/// Enable multi-thread-safe factory mode.
///
/// Returns `true` when multi-thread-safe cache mode is enabled.
///
/// # Safety
/// Callers must coordinate access mode transitions across all foreign threads.
#[cfg_attr(feature = "standalone-capi-globals", unsafe(no_mangle))]
pub extern "C" fn startMTDSPFactories() -> bool {
    start_mt()
}

/// Disable multi-thread-safe factory mode.
///
/// # Safety
/// Callers must coordinate access mode transitions across all foreign threads.
#[cfg_attr(feature = "standalone-capi-globals", unsafe(no_mangle))]
pub extern "C" fn stopMTDSPFactories() {
    stop_mt();
}

/// Free memory allocated by this library for C strings.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by a Cranelift FFI
/// function that documents `freeCMemory` ownership.
#[cfg_attr(feature = "standalone-capi-globals", unsafe(no_mangle))]
pub unsafe extern "C" fn freeCMemory(ptr: *mut c_void) {
    unsafe { free_c_memory_c_string_only(ptr) }
}

/// Factory runtime status string kept for module-presence tests.
#[must_use]
pub fn factory_status() -> &'static str {
    "cranelift-ffi factory runtime"
}

/// Internal normalized inputs used by the common factory builder.
///
/// The external C API exposes several constructors (file/string/boxes/signals)
/// that eventually converge to the same `CraneliftDspFactory` structure.
struct FactoryBuildSpec<'a> {
    name: &'a str,
    dsp_code: &'a str,
    argv: &'a [String],
    opt_level: c_int,
    foreign_function_fingerprint: &'a str,
    source_is_faust: bool,
}

/// Internal normalized inputs used by the file-backed factory builder.
struct FileFactoryBuildSpec<'a> {
    filename: &'a str,
    dsp_source: &'a str,
    argv: &'a [String],
    opt_level: c_int,
    foreign_function_fingerprint: &'a str,
}

/// Build one factory object from a source file path and compiled backend artifacts.
fn build_scaffold_factory_from_file(
    spec: FileFactoryBuildSpec<'_>,
    fir: &BoxFfiFirModule,
    jit: Option<JitDspModule>,
) -> Result<CraneliftDspFactory, String> {
    let source_name = Path::new(spec.filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("FaustDSP");
    let FileFactoryBuildSpec {
        dsp_source,
        argv,
        opt_level,
        foreign_function_fingerprint,
        ..
    } = spec;
    build_scaffold_factory_common(
        FactoryBuildSpec {
            name: source_name,
            dsp_code: dsp_source,
            argv,
            opt_level,
            foreign_function_fingerprint,
            source_is_faust: true,
        },
        fir,
        jit,
    )
}

/// Canonical `-ss <n>` token for one decoded [`SchedulingStrategy`], used only
/// for factory cache identity (see [`canonicalize_cache_identity_argv`]).
fn canonical_scheduling_strategy_token(strategy: SchedulingStrategy) -> &'static str {
    match strategy {
        SchedulingStrategy::DepthFirst => "0",
        SchedulingStrategy::BreadthFirst => "1",
        SchedulingStrategy::Special => "2",
        SchedulingStrategy::ReverseBreadthFirst => "3",
    }
}

/// Rewrites the `-ss <n>` value token in `argv` to its canonical decoded form
/// for factory cache identity (`compile_options`/`sha_key`) purposes only.
///
/// `SchedulingStrategy::decode` maps every `n >= 3` to the same
/// `ReverseBreadthFirst` strategy, so `-ss 3` and `-ss 42` must contribute the
/// same cache identity even though their raw argv tokens differ — otherwise
/// two factories that will compile and behave identically would land in
/// different cache slots. Every other token, and a malformed/missing `-ss`
/// value (left for [`parse_ffi_compile_args`] to reject during actual
/// compilation), passes through unchanged, so cache identity for every other
/// option is byte-identical to the pre-`-ss` behavior.
fn canonicalize_cache_identity_argv(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut i = 0;
    while i < argv.len() {
        if matches!(
            argv[i].as_str(),
            "-mem" | "-mem0" | "--memory-manager" | "--memory-manager0"
        ) {
            if !out.iter().any(|arg| arg == "-mem0") {
                out.push("-mem0".to_owned());
            }
            i += 1;
            continue;
        }
        out.push(argv[i].clone());
        if argv[i] == "-ss"
            && let Some(value) = argv.get(i + 1)
        {
            if let Ok(n) = value.parse::<u32>() {
                out.push(
                    canonical_scheduling_strategy_token(SchedulingStrategy::decode(n)).to_owned(),
                );
            } else {
                out.push(value.clone());
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

fn format_factory_sha_key(
    opt_level: c_int,
    identity_argv: &[String],
    foreign_function_fingerprint: &str,
    semantic_fingerprint: &str,
) -> String {
    format!(
        "cranelift:{}:{}:{}:{}",
        opt_level,
        identity_argv.join("\x1f"),
        foreign_function_fingerprint,
        semantic_fingerprint
    )
}

/// Shared factory object builder.
///
/// This is the point where FIR-derived runtime metadata, cache identity, JSON
/// summary text, and optional compiled JIT payload are assembled into the
/// exported opaque factory object.
fn build_scaffold_factory_common(
    spec: FactoryBuildSpec<'_>,
    fir: &BoxFfiFirModule,
    jit: Option<JitDspModule>,
) -> Result<CraneliftDspFactory, String> {
    let FactoryBuildSpec {
        name,
        dsp_code,
        argv,
        opt_level,
        foreign_function_fingerprint,
        source_is_faust,
    } = spec;
    let compute_body_lowered = jit
        .as_ref()
        .is_some_and(codegen::backends::cranelift::JitDspModule::compute_body_lowered);
    // `-ss`'s raw numeric token is canonicalized (see
    // `canonicalize_cache_identity_argv`) before it contributes to cache
    // identity; `compile_argv` below still stores the caller's raw argv.
    let identity_argv = canonicalize_cache_identity_argv(argv);
    let semantic_fingerprint = fir::canonical_fir_fingerprint(&fir.store, fir.module);
    let compile_options = if identity_argv.is_empty() {
        format!(
            "opt_level={opt_level}; compute_body_lowered={compute_body_lowered}; foreign_functions={foreign_function_fingerprint}"
        )
    } else {
        format!(
            "opt_level={opt_level}; compute_body_lowered={compute_body_lowered}; argv={}; foreign_functions={foreign_function_fingerprint}",
            identity_argv.join(" ")
        )
    };
    let sha_key = format_factory_sha_key(
        opt_level,
        &identity_argv,
        foreign_function_fingerprint,
        &semantic_fingerprint,
    );
    let runtime = build_runtime_descriptor(&fir.store, fir.module)?;
    let num_inputs = fir.num_inputs;
    let num_outputs = fir.num_outputs;
    let function_items = match match_fir(&fir.store, fir.module) {
        FirMatch::Module { functions, .. } => match match_fir(&fir.store, functions) {
            FirMatch::Block(items) => items,
            other => {
                return Err(format!(
                    "Cranelift JSON expected function block, got {other:?}"
                ));
            }
        },
        other => return Err(format!("Cranelift JSON expected module, got {other:?}")),
    };
    let memory = jit
        .as_ref()
        .and_then(JitDspModule::mem0_analysis)
        .cloned()
        .map(|analysis| JsonMemoryDescription {
            backend: "cranelift".to_owned(),
            manager_abi: "faust_memory_manager_v1".to_owned(),
            analysis,
        });
    let json = build_json_description_from_fir(
        &fir.store,
        &function_items,
        JsonBuildOptions {
            name: name.to_owned(),
            backend: Some("cranelift".to_owned()),
            jit_compiled: Some(jit.is_some()),
            compute_body_lowered: Some(compute_body_lowered),
            filename: None,
            version: Some(CRANELIFT_FFI_VERSION.to_owned()),
            compile_options: Some(compile_options.clone()),
            library_list: Vec::new(),
            include_pathnames: Vec::new(),
            top_level_meta: Vec::new(),
            size: None,
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: None,
            memory,
        },
        |_var| None,
    )
    .map_err(|error| format!("cannot build Cranelift factory JSON: {error}"))?
    .render();
    Ok(CraneliftDspFactory {
        name: name.to_owned(),
        sha_key,
        dsp_code: dsp_code.to_owned(),
        compile_options,
        json,
        source_is_faust,
        source_name: name.to_owned(),
        compile_argv: argv.to_vec(),
        opt_level,
        memory_state: Mutex::new(FactoryMemoryState::default()),
        compiled_jit: jit,
        runtime,
        compute_body_lowered,
        num_inputs,
        num_outputs,
    })
}

/// Binds a versioned custom memory manager to a `-mem0` Cranelift factory.
///
/// The callback table is copied; the caller may release the temporary table
/// after this call, but its context and callback targets must remain valid
/// until the factory and all its instances are destroyed. Description is a
/// complete transaction and happens before the binding is published.
///
/// # Safety
/// `factory` and `manager` must point to live values. `error_msg`, when
/// non-null, must reference the standard 4096-byte Faust error buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn setCCraneliftMemoryManager(
    factory: *mut CraneliftDspFactory,
    manager: *const FaustMemoryManager,
    error_msg: *mut c_char,
) -> bool {
    unsafe {
        let Some(factory) = factory.as_ref() else {
            write_error(error_msg, "null Cranelift factory");
            return false;
        };
        let Some(manager) = manager.as_ref() else {
            write_error(error_msg, "null faust_memory_manager table");
            return false;
        };
        let Some(analysis) = factory
            .compiled_jit
            .as_ref()
            .and_then(JitDspModule::mem0_analysis)
        else {
            write_error(error_msg, "factory was not compiled with -mem0");
            return false;
        };
        let binding = match MemoryManagerBinding::copy_from(manager) {
            Ok(binding) => binding,
            Err(error) => {
                write_error(error_msg, &error);
                return false;
            }
        };
        {
            let state = match factory.memory_state.lock() {
                Ok(state) => state,
                Err(_) => {
                    write_error(error_msg, "Cranelift memory-manager state is poisoned");
                    return false;
                }
            };
            if let Some(current) = state.binding {
                if current.same_identity(binding) {
                    return true;
                }
                if state.class_storage.is_some() || state.live_instances != 0 || state.class_busy {
                    write_error(
                        error_msg,
                        "cannot replace a Cranelift memory manager after allocation",
                    );
                    return false;
                }
            }
        }
        if let Err(error) = binding.describe(analysis) {
            write_error(error_msg, &error);
            return false;
        }
        let mut state = match factory.memory_state.lock() {
            Ok(state) => state,
            Err(_) => {
                write_error(error_msg, "Cranelift memory-manager state is poisoned");
                return false;
            }
        };
        if let Some(current) = state.binding {
            if current.same_identity(binding) {
                return true;
            }
            if state.class_storage.is_some() || state.live_instances != 0 || state.class_busy {
                write_error(
                    error_msg,
                    "cannot replace a Cranelift memory manager after allocation",
                );
                return false;
            }
        }
        state.binding = Some(binding);
        true
    }
}

/// Decode a conventional `argc`/`argv` C array into owned Rust strings.
///
/// This thin wrapper keeps the `unsafe` boundary local to the FFI layer while
/// reusing the shared utility implementation.
fn decode_c_argv(argc: c_int, argv: *const *const c_char) -> Result<Vec<String>, String> {
    unsafe { decode_c_argv_shared(argc, argv) }
}

/// Result of running the Faust compiler pipeline plus Cranelift JIT compilation.
#[derive(Debug)]
struct CompiledCraneliftFactory {
    fir: BoxFfiFirModule,
    jit: JitDspModule,
    foreign_function_fingerprint: String,
}

/// Runs the real compiler pipeline to FIR, then compiles one Cranelift JIT module.
/// Builds a [`FaustCompiler`] configured from the shared FFI argv subset:
/// `-double` selects the real type, `-vec`/`-vs`/`-lv` select the compute mode,
/// `-ss` selects the scheduling strategy (vectorization port plan phase P2:
/// plumbing only — the strategy is stored but not yet acted on).
/// Returns the compiler plus the parsed `double` flag (needed by the JIT).
fn compiler_from_argv(argv: &[String]) -> Result<(FaustCompiler, bool, MemoryManagerMode), String> {
    let parsed = parse_ffi_compile_args(argv)?;
    let compute_mode = if parsed.vec_mode {
        ComputeMode::Vector {
            vec_size: parsed.vec_size,
            loop_variant: parsed.loop_variant,
        }
    } else {
        ComputeMode::Scalar
    };
    let compiler = FaustCompiler::new()
        .with_real_type(if parsed.double {
            RealType::Float64
        } else {
            RealType::Float32
        })
        .with_compute_mode(compute_mode)
        .with_scheduling_strategy(SchedulingStrategy::decode(parsed.scheduling_strategy));
    let compiler = match parsed.table_init.as_deref() {
        Some("runtime") => compiler.with_table_init_mode(TableInitMode::Runtime),
        Some("const") => compiler.with_table_init_mode(TableInitMode::Const),
        _ => compiler,
    };
    let compiler = match parsed.table_init_sample_rate {
        Some(sample_rate) => compiler.with_table_init_sample_rate(sample_rate),
        None => compiler,
    };
    let compiler = match parsed.bra_tape {
        Some(samples) => compiler.with_bra_tape(samples),
        None => compiler,
    };
    Ok((
        compiler,
        parsed.double,
        memory_manager_mode(parsed.memory_manager0),
    ))
}

fn preflight_compile_file_to_cranelift(
    path: &Path,
    argv: &[String],
    opt_level: c_int,
) -> Result<CompiledCraneliftFactory, String> {
    let (compiler, double, memory_manager_mode) = compiler_from_argv(argv)?;
    let search_paths = collect_search_paths_for_file(path, argv);
    let fir = compiler
        .compile_file_to_fir_with_lane(path, &search_paths, SignalFirLane::TransformFastLane)
        .map_err(|e| summary_of(&e))?;
    let num_inputs = fir_module_num_inputs(&fir.store, fir.module)?;
    let num_outputs = fir_module_num_outputs(&fir.store, fir.module)?;
    let fir = BoxFfiFirModule {
        store: fir.store,
        module: fir.module,
        num_inputs,
        num_outputs,
    };
    let jit = compile_fir_module_to_cranelift(&fir, opt_level, double, memory_manager_mode)?;
    Ok(CompiledCraneliftFactory {
        fir,
        jit,
        foreign_function_fingerprint: foreign_function_registry_fingerprint(),
    })
}

/// Runs the real compiler pipeline on inline source to FIR, then compiles one
/// Cranelift JIT module.
fn preflight_compile_source_to_cranelift(
    source_name: &str,
    source: &str,
    opt_level: c_int,
    argv: &[String],
) -> Result<CompiledCraneliftFactory, String> {
    let (compiler, double, memory_manager_mode) = compiler_from_argv(argv)?;
    // Forward `-I` as import search paths. Without this the string path sees
    // only the built-in defaults, so `import("stdfaust.lib")` resolves while a
    // project-local `library("mine.lib")` does not — and the failure is
    // deferred until the library is actually *used*, since an unused
    // `library(...)` binding is never loaded.
    let search_paths = ffi_common::args::parse_ffi_compile_args(argv)
        .map(|parsed| parsed.search_paths)
        .unwrap_or_default();
    let fir = compiler
        .compile_source_to_fir_with_lane_and_search_paths(
            source_name,
            source,
            &search_paths,
            SignalFirLane::TransformFastLane,
        )
        .map_err(|e| summary_of(&e))?;
    let num_inputs = fir_module_num_inputs(&fir.store, fir.module)?;
    let num_outputs = fir_module_num_outputs(&fir.store, fir.module)?;
    let fir = BoxFfiFirModule {
        store: fir.store,
        module: fir.module,
        num_inputs,
        num_outputs,
    };
    let jit = compile_fir_module_to_cranelift(&fir, opt_level, double, memory_manager_mode)?;
    Ok(CompiledCraneliftFactory {
        fir,
        jit,
        foreign_function_fingerprint: foreign_function_registry_fingerprint(),
    })
}

/// Compiles one FIR module to Cranelift using one C ABI opt-level request.
///
/// `double` must match the precision the FIR was produced with so the backend
/// resolves `FAUSTFLOAT` to the same width (`F64` under `-double`).
fn compile_fir_module_to_cranelift(
    fir: &BoxFfiFirModule,
    opt_level: c_int,
    double: bool,
    memory_manager_mode: MemoryManagerMode,
) -> Result<JitDspModule, String> {
    let extern_function_symbols = snapshot_registered_foreign_functions();
    let options = CraneliftOptions {
        memory_manager_mode,
        opt_level: map_c_opt_level(opt_level),
        extern_function_symbols,
        double_precision: double,
        ..CraneliftOptions::default()
    };
    generate_cranelift_module(&fir.store, fir.module, &options).map_err(|e| e.to_string())
}

/// Converts the dependency-light FFI parser bit into the canonical codegen
/// mode before JIT compilation.
const fn memory_manager_mode(enabled: bool) -> MemoryManagerMode {
    if enabled {
        MemoryManagerMode::Mem0
    } else {
        MemoryManagerMode::None
    }
}

/// Maps C integer optimization levels to the current Cranelift backend scaffold enum.
fn map_c_opt_level(level: c_int) -> CraneliftOptLevel {
    match level {
        i if i <= 0 => CraneliftOptLevel::None,
        1 | 2 => CraneliftOptLevel::Speed,
        _ => CraneliftOptLevel::SpeedAndSize,
    }
}

/// Builds import search paths for file compilation: the `-I` args first, then
/// the compiler defaults (the source's directory, `FAUST_LIB_PATH`, the
/// installed libraries).
///
/// The order is the C++ compiler's and the CLI's: a `-I DIR` overrides a
/// library of the same name found later, which is what `faustprobe -I
/// some/faustlibraries` relies on to measure a checkout rather than the
/// installed copy. Appending the `-I` dirs after the defaults instead made
/// them dead for every standard library name.
fn collect_search_paths_for_file(path: &Path, argv: &[String]) -> Vec<PathBuf> {
    let extra = parse_ffi_compile_args(argv)
        .map(|parsed| parsed.search_paths)
        .unwrap_or_default();
    merge_import_search_paths(path, &extra)
}

/// Reads the FIR module input arity from one boxed FIR export.
fn fir_module_num_inputs(store: &fir::FirStore, module: fir::FirId) -> Result<usize, String> {
    match match_fir(store, module) {
        FirMatch::Module { num_inputs, .. } => Ok(num_inputs),
        other => Err(format!(
            "expected FIR Module when extracting input arity, got {other:?}"
        )),
    }
}

/// Reads the FIR module output arity from one boxed FIR export.
fn fir_module_num_outputs(store: &fir::FirStore, module: fir::FirId) -> Result<usize, String> {
    match match_fir(store, module) {
        FirMatch::Module { num_outputs, .. } => Ok(num_outputs),
        other => Err(format!(
            "expected FIR Module when extracting output arity, got {other:?}"
        )),
    }
}

/// Unescapes one field in the legacy textual bitcode container.
fn unesc_bitcode_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(ch) = it.next() {
        if ch == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Decodes a Cranelift source-backed bitcode payload and rebuilds a runnable
/// JIT factory.
///
/// Both legacy `CRANELIFT_FFI_V2_SOURCE` and current `FAUST_CLIF_V1` payloads
/// are accepted here so the read path can remain backwards-compatible during
/// the transition to the richer `.clif` container.
fn decode_factory_bitcode(text: &str) -> Result<CraneliftDspFactory, String> {
    if text.lines().next() == Some(CLIF_MAGIC) {
        let decoded = decode_factory_clif(text)?;
        if decoded.clif_functions.is_empty() {
            return Err("CLIF payload does not contain any generated function bodies".to_owned());
        }
        if !decoded
            .clif_functions
            .iter()
            .any(|(name, _)| name.ends_with("::compute") || name == "compute")
        {
            return Err("CLIF payload does not contain a compute function body".to_owned());
        }
        return rebuild_factory_from_source(
            &decoded.name,
            &decoded.source_fallback,
            &decoded.argv,
            decoded.opt_level,
            &decoded.expected_sha,
            &decoded.expected_compile_options,
        )
        .and_then(|rebuilt| {
            if rebuilt.num_inputs != decoded.num_inputs
                || rebuilt.num_outputs != decoded.num_outputs
            {
                return Err(format!(
                    "bitcode arity mismatch: payload in/out={}/{}, rebuilt in/out={}/{}",
                    decoded.num_inputs,
                    decoded.num_outputs,
                    rebuilt.num_inputs,
                    rebuilt.num_outputs
                ));
            }
            Ok(rebuilt)
        });
    }
    let mut lines = text.lines();
    match lines.next() {
        Some("CRANELIFT_FFI_V2_SOURCE") => {}
        Some(_) => return Err("unsupported cranelift bitcode format".to_owned()),
        None => return Err("empty bitcode payload".to_owned()),
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    for line in lines {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        fields.insert(k.to_owned(), unesc_bitcode_field(v));
    }

    let name = fields
        .remove("name")
        .ok_or_else(|| "missing 'name' field".to_owned())?;
    let expected_sha = fields
        .remove("sha")
        .ok_or_else(|| "missing 'sha' field".to_owned())?;
    let expected_compile_options = fields
        .remove("compile_options")
        .ok_or_else(|| "missing 'compile_options' field".to_owned())?;
    let source = fields
        .remove("source")
        .ok_or_else(|| "missing 'source' field".to_owned())?;
    let opt_level = fields
        .remove("opt_level")
        .ok_or_else(|| "missing 'opt_level' field".to_owned())?
        .parse::<c_int>()
        .map_err(|e| format!("invalid 'opt_level' field: {e}"))?;
    let argc = fields
        .remove("argc")
        .ok_or_else(|| "missing 'argc' field".to_owned())?
        .parse::<usize>()
        .map_err(|e| format!("invalid 'argc' field: {e}"))?;
    let mut argv = Vec::with_capacity(argc);
    for idx in 0..argc {
        let key = format!("arg{idx}");
        let arg = fields
            .remove(&key)
            .ok_or_else(|| format!("missing '{key}' field"))?;
        argv.push(arg);
    }

    rebuild_factory_from_source(
        &name,
        &source,
        &argv,
        opt_level,
        &expected_sha,
        &expected_compile_options,
    )
}

/// Rebuilds a runnable Cranelift factory from textual source plus expected
/// identity fields serialized in a bitcode payload.
fn rebuild_factory_from_source(
    name: &str,
    source: &str,
    argv: &[String],
    opt_level: c_int,
    expected_sha: &str,
    expected_compile_options: &str,
) -> Result<CraneliftDspFactory, String> {
    let compiled = preflight_compile_source_to_cranelift(name, source, opt_level, argv)?;
    // V1/V2 payloads written before allocation-independent fingerprints used
    // the diagnostic FIR dump directly. Keep accepting that identity so the
    // existing source-backed container compatibility contract remains intact.
    let legacy_sha = format_factory_sha_key(
        opt_level,
        &canonicalize_cache_identity_argv(argv),
        &compiled.foreign_function_fingerprint,
        &fir::dump_fir(&compiled.fir.store, compiled.fir.module),
    );
    let mut rebuilt = build_scaffold_factory_common(
        FactoryBuildSpec {
            name,
            dsp_code: source,
            argv,
            opt_level,
            foreign_function_fingerprint: &compiled.foreign_function_fingerprint,
            source_is_faust: true,
        },
        &compiled.fir,
        Some(compiled.jit),
    )?;
    if rebuilt.sha_key != expected_sha {
        if legacy_sha == expected_sha {
            rebuilt.sha_key = expected_sha.to_owned();
        } else {
            return Err(format!(
                "bitcode SHA mismatch: expected '{}', rebuilt '{}'",
                expected_sha, rebuilt.sha_key
            ));
        }
    }
    if rebuilt.compile_options != expected_compile_options {
        return Err("bitcode compile options mismatch after rebuild".to_owned());
    }
    Ok(rebuilt)
}

thread_local! {
    /// Complete text of the last error this thread reported; see
    /// [`ffi_common::complete_error`] for the contract.
    static COMPLETE_ERROR: CompleteError = const { CompleteError::new() };
}

/// The complete diagnostics-v2 JSON report of the last error reported on the
/// calling thread, for Rust callers of this crate:
/// [`getCCraneliftDSPFactoryErrorDiagnostics`] as an owned string.
#[must_use]
pub fn last_error_diagnostics_json() -> Option<String> {
    COMPLETE_ERROR.with(CompleteError::diagnostics)
}

/// Write an error message to a standard 4096-byte Faust error buffer, and
/// publish its complete text for [`getCCompleteCraneliftDSPFactoryError`].
///
/// # Safety
/// `buf` must point to at least 4096 bytes or be null.
unsafe fn write_error(buf: *mut c_char, msg: &str) {
    COMPLETE_ERROR.with(|record| record.report(msg));
    unsafe { write_error_4096(buf, msg) }
}

/// Flattens a compiler error to the one-line summary the error buffer
/// receives, keeping its rendered diagnostics for the report of that summary.
fn summary_of(error: &CompilerError) -> String {
    let summary = error.to_string();
    COMPLETE_ERROR.with(|record| {
        record.attach_with_diagnostics(
            &summary,
            &error.rendered_diagnostics(),
            &error.diagnostics_report_json(BACKEND),
        );
    });
    summary
}

/// [`summary_of`] for the helper-service errors (`expand`, auxiliary files).
fn service_summary_of(error: &FaustwasmServiceError) -> String {
    let summary = error.to_string();
    COMPLETE_ERROR.with(|record| match error.diagnostics_report_json(BACKEND) {
        Some(report) => {
            record.attach_with_diagnostics(&summary, &error.rendered_diagnostics(), &report);
        }
        None => record.attach(&summary, &error.rendered_diagnostics()),
    });
    summary
}

/// Returns the complete text of the last error reported on the calling thread
/// through an `error_msg` buffer: the message that buffer received, followed
/// by the compiler's rendered diagnostics (location, source snippet, notes,
/// fixes) when the failure had some. The buffer is 4096 bytes by contract and
/// its size cannot grow without breaking existing hosts; this text is not
/// truncated.
///
/// The pointer is owned by the library: do not free it. It is null while no
/// error was reported on this thread, and stays valid until the next error
/// reported on this thread. A call that succeeds does not reset it, so read it
/// after a call that failed.
#[unsafe(no_mangle)]
pub extern "C" fn getCCompleteCraneliftDSPFactoryError() -> *const c_char {
    COMPLETE_ERROR.with(CompleteError::as_ptr)
}

/// `request.backend` of this surface's diagnostics reports.
const BACKEND: &str = "cranelift";

/// Returns the typed form of the last error reported on the calling thread
/// through an `error_msg` buffer: the compiler's **diagnostics-v2 JSON
/// report**, with for each diagnostic its code, its labels with byte ranges in
/// each source, its facts, notes and help, and its fixes with their edits and
/// applicability, so that a host applies a machine-applicable fix without
/// reading the rendered text of [`getCCompleteCraneliftDSPFactoryError`].
///
/// Null when that error carried no typed diagnostics (an argument error, an
/// unreadable file's transport failure), **even if an earlier one did**: a
/// report never outlives the failure it describes. Otherwise the contract of
/// the complete text: owned by the library (do not free it), per thread, valid
/// until the next error reported on this thread, not reset by a success.
///
/// The document carries its own `schema_version` (2 today) and
/// `request.backend` (`"cranelift"`); fields may be added within a version, so
/// a host reads what it knows and checks the version rather than assuming it.
#[unsafe(no_mangle)]
pub extern "C" fn getCCraneliftDSPFactoryErrorDiagnostics() -> *const c_char {
    COMPLETE_ERROR.with(CompleteError::diagnostics_ptr)
}

/// Runs the shared post-`argv` FFI factory creation flow for Cranelift backend.
///
/// This centralizes common FFI mechanics (error buffer + cache insertion +
/// final allocation) while keeping file-vs-string compilation/preflight paths
/// separate so path-based import semantics remain backend-correct.
///
/// The callback builds a fully initialized Rust factory value; this helper then
/// performs the final opaque allocation and cache registration.
unsafe fn create_cranelift_factory_with_argv<F>(
    argv: &[String],
    error_msg: *mut c_char,
    build: F,
) -> *mut CraneliftDspFactory
where
    F: FnOnce(&[String]) -> Result<CraneliftDspFactory, String>,
{
    match build(argv) {
        Ok(factory) => {
            let sha = factory.sha_key.clone();
            cache_insert(&sha, factory)
        }
        Err(e) => {
            unsafe { write_error(error_msg, &e) };
            std::ptr::null_mut()
        }
    }
}

// ── expand / generateAuxFiles ─────────────────────────────────────────────

/// Validate and expand a Faust DSP source file.
///
/// Parses and evaluates the file.  On success writes the (unexpanded) source
/// text to a heap-allocated C string that the caller must free with
/// [`freeCMemory`].  `sha_key` (if non-null, at least 64 bytes) is populated
/// with the SHA-256 hex digest of the source.
///
/// # Safety
/// - `filename` must be a valid null-terminated C string (file path).
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `sha_key` may be null; if non-null it must reference at least 64 bytes.
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expandCCraneliftDSPFromFile(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    sha_key: *mut c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    unsafe {
        let filename = match required_c_string_arg(filename, "filename") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(a) => a,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let source = match std::fs::read_to_string(&filename) {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &format!("cannot read '{filename}': {e}"));
                return std::ptr::null_mut();
            }
        };
        let compiler = FaustCompiler::new();
        let request = ExpandDspRequest {
            source_name: filename.to_owned(),
            source,
            args: args.join(" "),
        };
        match compiler.expand_dsp(&request) {
            Ok(expanded) => {
                write_sha_key(sha_key, &expanded);
                alloc_c_string(&expanded)
            }
            Err(e) => {
                write_error(error_msg, &service_summary_of(&e));
                std::ptr::null_mut()
            }
        }
    }
}

/// Validate and expand a Faust DSP source string.
///
/// On success returns a heap-allocated C string (caller frees with
/// [`freeCMemory`]).  `sha_key` (if non-null, at least 64 bytes) is populated
/// with the SHA-256 hex digest of the source.
///
/// # Safety
/// - `name_app` may be null; if non-null it must be a valid C string.
/// - `dsp_content` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `sha_key` may be null; if non-null it must reference at least 64 bytes.
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expandCCraneliftDSPFromString(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    sha_key: *mut c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    unsafe {
        let name_app = match optional_c_string_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP".to_owned(),
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let source = match required_c_string_arg(dsp_content, "dsp_content") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(a) => a,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let compiler = FaustCompiler::new();
        let request = ExpandDspRequest {
            source_name: name_app.to_owned(),
            source: source.to_owned(),
            args: args.join(" "),
        };
        match compiler.expand_dsp(&request) {
            Ok(expanded) => {
                write_sha_key(sha_key, &expanded);
                alloc_c_string(&expanded)
            }
            Err(e) => {
                write_error(error_msg, &service_summary_of(&e));
                std::ptr::null_mut()
            }
        }
    }
}

/// Generate auxiliary output files from a Faust DSP source file.
///
/// Uses `-O <path>` from `argv` to determine the output directory (defaults to
/// `.`).  Requested formats (`-cpp`, `-c`, `-wasm`, `-json`, `-svg`) are
/// taken from `argv`.  Returns `true` on success.
///
/// # Safety
/// - `filename` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCCraneliftAuxFilesFromFile(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> bool {
    unsafe {
        let filename = match required_c_string_arg(filename, "filename") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return false;
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(a) => a,
            Err(e) => {
                write_error(error_msg, &e);
                return false;
            }
        };
        let source = match std::fs::read_to_string(&filename) {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &format!("cannot read '{filename}': {e}"));
                return false;
            }
        };
        let compiler = FaustCompiler::new();
        let request = GenerateAuxFilesRequest {
            source_name: filename.to_owned(),
            source,
            args: args.join(" "),
            ..Default::default()
        };
        match compiler.generate_aux_files(&request) {
            Ok(artifacts) => write_aux_artifacts_to_disk(&artifacts, &args, error_msg),
            Err(e) => {
                write_error(error_msg, &service_summary_of(&e));
                false
            }
        }
    }
}

/// Generate auxiliary output files from a Faust DSP source string.
///
/// Uses `-O <path>` from `argv` to determine the output directory (defaults to
/// `.`).  Requested formats (`-cpp`, `-c`, `-wasm`, `-json`, `-svg`) are
/// taken from `argv`.  Returns `true` on success.
///
/// # Safety
/// - `name_app` may be null; if non-null it must be a valid C string.
/// - `dsp_content` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null if `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCCraneliftAuxFilesFromString(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> bool {
    unsafe {
        let name_app = match optional_c_string_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP".to_owned(),
            Err(e) => {
                write_error(error_msg, &e);
                return false;
            }
        };
        let source = match required_c_string_arg(dsp_content, "dsp_content") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return false;
            }
        };
        let args = match decode_c_argv(argc, argv) {
            Ok(a) => a,
            Err(e) => {
                write_error(error_msg, &e);
                return false;
            }
        };
        let compiler = FaustCompiler::new();
        let request = GenerateAuxFilesRequest {
            source_name: name_app.to_owned(),
            source: source.to_owned(),
            args: args.join(" "),
            ..Default::default()
        };
        match compiler.generate_aux_files(&request) {
            Ok(artifacts) => write_aux_artifacts_to_disk(&artifacts, &args, error_msg),
            Err(e) => {
                write_error(error_msg, &service_summary_of(&e));
                false
            }
        }
    }
}

/// Writes the SHA-1 key of `text` into `buf` (40 characters plus NUL).
///
/// Mirrors C++ `generateSHA1`, whose result the caller receives in the
/// 64-character buffer the libfaust C API documents. The buffer is larger than
/// the digest; the extra room is not filled, exactly as in C++.
unsafe fn write_sha_key(buf: *mut c_char, text: &str) {
    if buf.is_null() {
        return;
    }
    let hash = ffi_common::sha1_hex(text.as_bytes());
    let bytes = hash.as_bytes();
    let len = bytes.len().min(63);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buf, len);
        *buf.add(len) = 0;
    }
}

/// Writes `artifacts` to the directory extracted from `-O <path>` in `argv`
/// (defaults to `.`), returning `true` if all writes succeed.
unsafe fn write_aux_artifacts_to_disk(
    artifacts: &[AuxFileArtifact],
    argv: &[String],
    error_msg: *mut c_char,
) -> bool {
    let out_dir = extract_output_dir(argv);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        unsafe { write_error(error_msg, &format!("cannot create output dir: {e}")) };
        return false;
    }
    for artifact in artifacts {
        let dest = out_dir.join(&artifact.path);
        if let Err(e) = std::fs::write(&dest, &artifact.content) {
            unsafe { write_error(error_msg, &format!("cannot write {}: {e}", dest.display())) };
            return false;
        }
    }
    true
}

/// Extracts the value of `-O <path>` from `argv`, defaulting to `.`.
fn extract_output_dir(argv: &[String]) -> PathBuf {
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "-O"
            && let Some(p) = argv.get(i + 1)
        {
            return PathBuf::from(p);
        }
        i += 1;
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests;
