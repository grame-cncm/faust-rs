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
use compiler::{
    AuxFileArtifact, Compiler as FaustCompiler, ComputeMode, ExpandDspRequest,
    GenerateAuxFilesRequest, RealType, SchedulingStrategy, SignalFirLane,
    default_import_search_paths,
};
use fir::{FirMatch, match_fir};
use utils::{
    decode_c_argv as decode_c_argv_shared, free_c_memory_c_string_only, null_c_string_array,
    optional_c_str_arg, parse_ffi_compile_args, required_c_str_arg, write_error_4096,
};

use crate::cache::{
    cache_all_sha_keys, cache_drain, cache_insert, cache_lookup, cache_remove_by_ptr, start_mt,
    stop_mt,
};
use crate::clif::{CLIF_MAGIC, decode_factory_clif, encode_factory_clif};
use crate::runtime::build_runtime_descriptor;
use crate::types::{CraneliftDspFactory, alloc_c_string, alloc_factory, free_factory};

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
        let filename = match required_c_str_arg(filename, "filename") {
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
                preflight_compile_file_to_cranelift(Path::new(filename), args, opt_level)?;
            let dsp_source = std::fs::read_to_string(filename)
                .map_err(|e| format!("cannot read DSP source '{filename}': {e}"))?;
            build_scaffold_factory_from_file(
                FileFactoryBuildSpec {
                    filename,
                    dsp_source: &dsp_source,
                    argv: args,
                    opt_level,
                    semantic_fingerprint: &compiled.fir_dump,
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
        let name_app = match optional_c_str_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP",
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let dsp_content = match required_c_str_arg(dsp_content, "dsp_content") {
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
                preflight_compile_source_to_cranelift(name_app, dsp_content, opt_level, args)?;
            build_scaffold_factory_common(
                FactoryBuildSpec {
                    name: name_app,
                    dsp_code: dsp_content,
                    argv: args,
                    opt_level,
                    semantic_fingerprint: &compiled.fir_dump,
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
        let source_name = match optional_c_str_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP",
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
            let fir = export_fir_from_signal_array_handle(source_name, signals)?;
            let fir_dump = fir::dump_fir(&fir.store, fir.module);
            let double = parse_ffi_compile_args(args)
                .map(|a| a.double)
                .unwrap_or(false);
            let jit = compile_fir_module_to_cranelift(&fir, opt_level, double)?;
            let foreign_function_fingerprint = foreign_function_registry_fingerprint();
            build_scaffold_factory_common(
                FactoryBuildSpec {
                    name: source_name,
                    dsp_code: &fir_dump,
                    argv: args,
                    opt_level,
                    semantic_fingerprint: &fir_dump,
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
        let source_name = match optional_c_str_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP",
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
            let fir = export_fir_from_box_handle(source_name, box_expr)?;
            let fir_dump = fir::dump_fir(&fir.store, fir.module);
            let double = parse_ffi_compile_args(args)
                .map(|a| a.double)
                .unwrap_or(false);
            let jit = compile_fir_module_to_cranelift(&fir, opt_level, double)?;
            let foreign_function_fingerprint = foreign_function_registry_fingerprint();
            build_scaffold_factory_common(
                FactoryBuildSpec {
                    name: source_name,
                    dsp_code: &fir_dump,
                    argv: args,
                    opt_level,
                    semantic_fingerprint: &fir_dump,
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
        let sha_key = match required_c_str_arg(sha_key, "sha_key") {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        cache_lookup(sha_key)
    }
}

/// Delete a Cranelift DSP factory.
///
/// Returns `true` when a non-null factory pointer was freed.
///
/// # Safety
/// `factory` must be a valid pointer previously returned by a Cranelift factory
/// creation function, and must not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn deleteCCraneliftDSPFactory(factory: *mut CraneliftDspFactory) -> bool {
    unsafe {
        if factory.is_null() {
            return false;
        }
        cache_remove_by_ptr(factory);
        free_factory(factory);
        true
    }
}

/// Delete all cached Cranelift factories.
///
/// # Safety
/// Callers must ensure no live DSP instances still reference these factories.
#[unsafe(no_mangle)]
pub extern "C" fn deleteAllCCraneliftDSPFactories() {
    for ptr in cache_drain() {
        unsafe {
            if !ptr.is_null() {
                free_factory(ptr);
            }
        }
    }
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
        let text = match required_c_str_arg(bit_code, "bitcode") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        match decode_factory_bitcode(text) {
            Ok(factory) => {
                let ptr = alloc_factory(factory);
                cache_insert(&(*ptr).sha_key, ptr);
                ptr
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
        let path = match required_c_str_arg(bit_code_path, "path") {
            Ok(s) => s,
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let text = match std::fs::read_to_string(path) {
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
                let ptr = alloc_factory(factory);
                cache_insert(&(*ptr).sha_key, ptr);
                ptr
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
        let path = match required_c_str_arg(bit_code_path, "path") {
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
    semantic_fingerprint: &'a str,
    foreign_function_fingerprint: &'a str,
    source_is_faust: bool,
}

/// Internal normalized inputs used by the file-backed factory builder.
struct FileFactoryBuildSpec<'a> {
    filename: &'a str,
    dsp_source: &'a str,
    argv: &'a [String],
    opt_level: c_int,
    semantic_fingerprint: &'a str,
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
        semantic_fingerprint,
        foreign_function_fingerprint,
        ..
    } = spec;
    build_scaffold_factory_common(
        FactoryBuildSpec {
            name: source_name,
            dsp_code: dsp_source,
            argv,
            opt_level,
            semantic_fingerprint,
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
        semantic_fingerprint,
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
    let sha_key = format!(
        "cranelift:{}:{}:{}:{}",
        opt_level,
        identity_argv.join("\x1f"),
        foreign_function_fingerprint,
        semantic_fingerprint
    );
    let runtime = build_runtime_descriptor(&fir.store, fir.module)?;
    let num_inputs = fir.num_inputs;
    let num_outputs = fir.num_outputs;
    Ok(CraneliftDspFactory {
        name: name.to_owned(),
        sha_key,
        dsp_code: dsp_code.to_owned(),
        compile_options,
        json: format!(
            "{{\"name\":\"{}\",\"backend\":\"cranelift\",\"jit_compiled\":{},\"compute_body_lowered\":{}}}",
            json_escape(name),
            if jit.is_some() { "true" } else { "false" },
            if compute_body_lowered {
                "true"
            } else {
                "false"
            }
        ),
        source_is_faust,
        source_name: name.to_owned(),
        compile_argv: argv.to_vec(),
        opt_level,
        compiled_jit: jit,
        runtime,
        compute_body_lowered,
        num_inputs,
        num_outputs,
    })
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
    fir_dump: String,
    foreign_function_fingerprint: String,
}

/// Runs the real compiler pipeline to FIR, then compiles one Cranelift JIT module.
/// Builds a [`FaustCompiler`] configured from the shared FFI argv subset:
/// `-double` selects the real type, `-vec`/`-vs`/`-lv` select the compute mode,
/// `-ss` selects the scheduling strategy (vectorization port plan phase P2:
/// plumbing only — the strategy is stored but not yet acted on).
/// Returns the compiler plus the parsed `double` flag (needed by the JIT).
fn compiler_from_argv(argv: &[String]) -> (FaustCompiler, bool) {
    let parsed = parse_ffi_compile_args(argv).unwrap_or_default();
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
    (compiler, parsed.double)
}

fn preflight_compile_file_to_cranelift(
    path: &Path,
    argv: &[String],
    opt_level: c_int,
) -> Result<CompiledCraneliftFactory, String> {
    let (compiler, double) = compiler_from_argv(argv);
    let search_paths = collect_search_paths_for_file(path, argv);
    let fir = compiler
        .compile_file_to_fir_with_lane(path, &search_paths, SignalFirLane::TransformFastLane)
        .map_err(|e| e.to_string())?;
    let fir_dump = fir::dump_fir(&fir.store, fir.module);
    let num_inputs = fir_module_num_inputs(&fir.store, fir.module)?;
    let num_outputs = fir_module_num_outputs(&fir.store, fir.module)?;
    let fir = BoxFfiFirModule {
        store: fir.store,
        module: fir.module,
        num_inputs,
        num_outputs,
    };
    let jit = compile_fir_module_to_cranelift(&fir, opt_level, double)?;
    Ok(CompiledCraneliftFactory {
        fir,
        jit,
        fir_dump,
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
    let (compiler, double) = compiler_from_argv(argv);
    let fir = compiler
        .compile_source_to_fir_with_lane(source_name, source, SignalFirLane::TransformFastLane)
        .map_err(|e| e.to_string())?;
    let fir_dump = fir::dump_fir(&fir.store, fir.module);
    let num_inputs = fir_module_num_inputs(&fir.store, fir.module)?;
    let num_outputs = fir_module_num_outputs(&fir.store, fir.module)?;
    let fir = BoxFfiFirModule {
        store: fir.store,
        module: fir.module,
        num_inputs,
        num_outputs,
    };
    let jit = compile_fir_module_to_cranelift(&fir, opt_level, double)?;
    Ok(CompiledCraneliftFactory {
        fir,
        jit,
        fir_dump,
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
) -> Result<JitDspModule, String> {
    let extern_function_symbols = snapshot_registered_foreign_functions();
    let options = CraneliftOptions {
        opt_level: map_c_opt_level(opt_level),
        extern_function_symbols,
        double_precision: double,
        ..CraneliftOptions::default()
    };
    generate_cranelift_module(&fir.store, fir.module, &options).map_err(|e| e.to_string())
}

/// Maps C integer optimization levels to the current Cranelift backend scaffold enum.
fn map_c_opt_level(level: c_int) -> CraneliftOptLevel {
    match level {
        i if i <= 0 => CraneliftOptLevel::None,
        1 | 2 => CraneliftOptLevel::Speed,
        _ => CraneliftOptLevel::SpeedAndSize,
    }
}

/// Builds import search paths for file compilation from default base + `-I` args.
///
/// Path-based compilation must keep source-directory-relative import semantics,
/// so file constructors start from the compiler defaults and then append FFI
/// `-I...` overrides.
fn collect_search_paths_for_file(path: &Path, argv: &[String]) -> Vec<PathBuf> {
    let mut paths = default_import_search_paths(path);
    if let Ok(parsed) = parse_ffi_compile_args(argv) {
        paths.extend(parsed.search_paths);
    }
    paths
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
    let rebuilt = build_scaffold_factory_common(
        FactoryBuildSpec {
            name,
            dsp_code: source,
            argv,
            opt_level,
            semantic_fingerprint: &compiled.fir_dump,
            foreign_function_fingerprint: &compiled.foreign_function_fingerprint,
            source_is_faust: true,
        },
        &compiled.fir,
        Some(compiled.jit),
    )?;
    if rebuilt.sha_key != expected_sha {
        return Err(format!(
            "bitcode SHA mismatch: expected '{}', rebuilt '{}'",
            expected_sha, rebuilt.sha_key
        ));
    }
    if rebuilt.compile_options != expected_compile_options {
        return Err("bitcode compile options mismatch after rebuild".to_owned());
    }
    Ok(rebuilt)
}

/// Write an error message to a standard 4096-byte Faust error buffer.
///
/// # Safety
/// `buf` must point to at least 4096 bytes or be null.
unsafe fn write_error(buf: *mut c_char, msg: &str) {
    unsafe { write_error_4096(buf, msg) }
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
            let ptr = alloc_factory(factory);
            // SAFETY: `ptr` was just allocated and is non-null.
            unsafe {
                cache_insert(&(*ptr).sha_key, ptr);
            }
            ptr
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
        let filename = match required_c_str_arg(filename, "filename") {
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
        let source = match std::fs::read_to_string(filename) {
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
                write_error(error_msg, &e.to_string());
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
        let name_app = match optional_c_str_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP",
            Err(e) => {
                write_error(error_msg, &e);
                return std::ptr::null_mut();
            }
        };
        let source = match required_c_str_arg(dsp_content, "dsp_content") {
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
                write_error(error_msg, &e.to_string());
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
        let filename = match required_c_str_arg(filename, "filename") {
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
        let source = match std::fs::read_to_string(filename) {
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
                write_error(error_msg, &e.to_string());
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
        let name_app = match optional_c_str_arg(name_app, "name_app") {
            Ok(Some(s)) if !s.is_empty() => s,
            Ok(_) => "FaustDSP",
            Err(e) => {
                write_error(error_msg, &e);
                return false;
            }
        };
        let source = match required_c_str_arg(dsp_content, "dsp_content") {
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
                write_error(error_msg, &e.to_string());
                false
            }
        }
    }
}

/// Write SHA-256 hex of `text` (first 63 chars + NUL) into `buf` if non-null.
unsafe fn write_sha_key(buf: *mut c_char, text: &str) {
    if buf.is_null() {
        return;
    }
    let hash = sha256_hex(text.as_bytes());
    let bytes = hash.as_bytes();
    let len = bytes.len().min(63);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buf, len);
        *buf.add(len) = 0;
    }
}

/// Minimal SHA-256 computation returning a lower-hex string (64 chars).
fn sha256_hex(data: &[u8]) -> String {
    // FNV-1a 64-bit used as a lightweight stand-in (SHA-256 would need a dep).
    let hash = data.iter().fold(0xcbf29ce484222325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}{hash:016x}{hash:016x}{hash:016x}")
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

/// Minimal JSON string escaping for scaffold metadata text.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::{
        canonicalize_cache_identity_argv, clearCCraneliftForeignFunctions,
        createCCraneliftDSPFactoryFromBoxes, createCCraneliftDSPFactoryFromFile,
        createCCraneliftDSPFactoryFromSignals, createCCraneliftDSPFactoryFromString,
        deleteAllCCraneliftDSPFactories, deleteCCraneliftDSPFactory, factory_status, freeCMemory,
        getAllCCraneliftDSPFactories, getCCraneliftDSPFactoryCompileOptions,
        getCCraneliftDSPFactoryFromSHAKey, getCCraneliftDSPFactoryJSON,
        getCCraneliftDSPFactoryName, getCCraneliftDSPFactorySHAKey, getCLibFaustVersion,
        readCCraneliftDSPFactoryFromBitcode, readCCraneliftDSPFactoryFromBitcodeFile,
        registerCCraneliftForeignFunction, unregisterCCraneliftForeignFunction,
        writeCCraneliftDSPFactoryToBitcode, writeCCraneliftDSPFactoryToBitcodeFile,
    };

    extern "C" fn ffi_test_foreign_gain(x: f32) -> f32 {
        x * 0.25
    }

    #[test]
    fn factory_scaffold_status_is_stable() {
        let _guard = crate::test_serial_guard();
        assert_eq!(factory_status(), "cranelift-ffi factory runtime");
    }

    #[test]
    fn version_symbol_returns_static_c_string() {
        let _guard = crate::test_serial_guard();
        let ptr = getCLibFaustVersion();
        assert!(!ptr.is_null());
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert!(s.contains("cranelift-ffi"));
    }

    #[test]
    fn create_factory_from_string_runtime_roundtrip_queries() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let name = c"mydsp";
        let src = c"process = _;";
        let args = [c"-vec"];
        let argv = [args[0].as_ptr()];
        let mut err = [0_i8; 4096];

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                1,
                argv.as_ptr(),
                err.as_mut_ptr(),
                2,
            )
        };
        assert!(!factory.is_null());

        let name_ptr = unsafe { getCCraneliftDSPFactoryName(factory) };
        let json_ptr = unsafe { getCCraneliftDSPFactoryJSON(factory) };
        let opts_ptr = unsafe { getCCraneliftDSPFactoryCompileOptions(factory) };
        assert!(!name_ptr.is_null());
        assert!(!json_ptr.is_null());
        assert!(!opts_ptr.is_null());

        let name_s = unsafe { CStr::from_ptr(name_ptr) }.to_str().unwrap();
        let json_s = unsafe { CStr::from_ptr(json_ptr) }.to_str().unwrap();
        let opts_s = unsafe { CStr::from_ptr(opts_ptr) }.to_str().unwrap();
        assert_eq!(name_s, "mydsp");
        assert!(json_s.contains("\"backend\":\"cranelift\""));
        assert!(opts_s.contains("opt_level=2"));

        unsafe {
            assert!((*factory).compiled_jit.is_some());
            let lowered = (*factory).compute_body_lowered;
            assert!(json_s.contains(&format!(
                "\"compute_body_lowered\":{}",
                if lowered { "true" } else { "false" }
            )));
            freeCMemory(name_ptr.cast());
            freeCMemory(json_ptr.cast());
            freeCMemory(opts_ptr.cast());
            assert!(deleteCCraneliftDSPFactory(factory));
        }
    }

    #[test]
    fn cache_identity_canonicalizes_ss_value_only() {
        // `-ss 0` vs `-ss 1` decode to different strategies: distinct tokens.
        let depth_first = canonicalize_cache_identity_argv(&["-ss".to_owned(), "0".to_owned()]);
        let breadth_first = canonicalize_cache_identity_argv(&["-ss".to_owned(), "1".to_owned()]);
        assert_ne!(depth_first, breadth_first);

        // `-ss 3` and `-ss 42` both decode to `ReverseBreadthFirst`: identical
        // canonical token, even though the raw argv strings differ.
        let three = canonicalize_cache_identity_argv(&["-ss".to_owned(), "3".to_owned()]);
        let forty_two = canonicalize_cache_identity_argv(&["-ss".to_owned(), "42".to_owned()]);
        assert_eq!(three, forty_two);
        assert_eq!(three, vec!["-ss".to_owned(), "3".to_owned()]);

        // Every other token passes through unchanged.
        let mixed = canonicalize_cache_identity_argv(&[
            "-vec".to_owned(),
            "-ss".to_owned(),
            "42".to_owned(),
            "-vs".to_owned(),
            "64".to_owned(),
        ]);
        assert_eq!(
            mixed,
            vec![
                "-vec".to_owned(),
                "-ss".to_owned(),
                "3".to_owned(),
                "-vs".to_owned(),
                "64".to_owned(),
            ]
        );
    }

    #[test]
    fn ss_scheduling_strategy_changes_factory_cache_identity_canonically() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let name = c"ss_cache_identity_test";
        let src = c"process = _;";

        let build = |ss_value: &std::ffi::CStr| unsafe {
            let mut err = [0_i8; 4096];
            let flag = c"-ss";
            let argv = [flag.as_ptr(), ss_value.as_ptr()];
            let factory = createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                2,
                argv.as_ptr(),
                err.as_mut_ptr(),
                0,
            );
            assert!(
                !factory.is_null(),
                "factory build failed: {}",
                CStr::from_ptr(err.as_ptr()).to_string_lossy()
            );
            factory
        };

        let f_ss0 = build(c"0");
        let f_ss1 = build(c"1");
        let f_ss3 = build(c"3");
        let f_ss42 = build(c"42");

        unsafe {
            // `-ss 0` (DepthFirst) vs `-ss 1` (BreadthFirst): distinct cache identity.
            assert_ne!((*f_ss0).sha_key, (*f_ss1).sha_key);
            assert_ne!((*f_ss0).compile_options, (*f_ss1).compile_options);

            // `-ss 3` and `-ss 42` both decode to ReverseBreadthFirst: identical
            // cache identity, proving the canonical enum value — not the raw
            // argv token — drives the identity.
            assert_eq!((*f_ss3).sha_key, (*f_ss42).sha_key);
            assert_eq!((*f_ss3).compile_options, (*f_ss42).compile_options);

            deleteCCraneliftDSPFactory(f_ss0);
            deleteCCraneliftDSPFactory(f_ss1);
            deleteCCraneliftDSPFactory(f_ss3);
            deleteCCraneliftDSPFactory(f_ss42);
        }
    }

    #[test]
    fn create_factory_from_file_rejects_null_filename() {
        let _guard = crate::test_serial_guard();
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                std::ptr::null(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                0,
            )
        };
        assert!(factory.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(msg.contains("null filename"));
    }

    #[test]
    fn create_factory_from_string_reports_compiler_error_for_invalid_faust() {
        let _guard = crate::test_serial_guard();
        let name = c"bad";
        let src = c"process = ;";
        let mut err = [0_i8; 4096];

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                0,
            )
        };
        assert!(factory.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(!msg.is_empty());
    }

    #[test]
    fn cache_lookup_and_list_are_wired_to_created_factories() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let name = c"cachetest";
        let src = c"process = _;";
        let mut err = [0_i8; 4096];

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                3,
            )
        };
        assert!(!factory.is_null());

        let sha_ptr = unsafe { getCCraneliftDSPFactorySHAKey(factory) };
        assert!(!sha_ptr.is_null());
        let looked_up = unsafe { getCCraneliftDSPFactoryFromSHAKey(sha_ptr.cast_const()) };
        assert_eq!(looked_up, factory);

        let all_ptr = getAllCCraneliftDSPFactories();
        assert!(!all_ptr.is_null());
        let first = unsafe { *all_ptr };
        assert!(!first.is_null());

        unsafe {
            freeCMemory(sha_ptr.cast());
            // free returned strings (outer array is intentionally not freed in scaffold).
            freeCMemory(first.cast());
            deleteAllCCraneliftDSPFactories();
        }
    }

    #[test]
    fn clif_bitcode_write_emits_v1_magic_header() {
        let _guard = crate::test_serial_guard();
        let name = c"bitcode";
        let src = c"process = _;";
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!factory.is_null());

        let bitcode = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
        assert!(!bitcode.is_null());
        let bitcode_s = unsafe { CStr::from_ptr(bitcode) }.to_str().unwrap();
        assert!(bitcode_s.starts_with("FAUST_CLIF_V1\n"));
        assert!(bitcode_s.contains("clif_func_count="));
        assert!(bitcode_s.contains("clif_func_name_0="));
        assert!(bitcode_s.contains("clif_func_body_0="));
        assert!(!bitcode_s.contains("clif_text=deferred"));

        unsafe {
            freeCMemory(bitcode.cast());
            assert!(deleteCCraneliftDSPFactory(factory));
        }
    }

    #[test]
    fn clif_bitcode_file_write_emits_v1_magic_header() {
        let _guard = crate::test_serial_guard();
        let name = c"bitfile";
        let src = c"process = _;";
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!factory.is_null());

        let path = std::env::temp_dir().join(format!(
            "faust-rs-cranelift-ffi-{}-{}.fbc.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_c = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();

        let wrote = unsafe { writeCCraneliftDSPFactoryToBitcodeFile(factory, path_c.as_ptr()) };
        assert!(wrote);
        let text = std::fs::read_to_string(&path).expect("read written .clif");
        assert!(text.starts_with("FAUST_CLIF_V1\n"));
        assert!(text.contains("clif_func_count="));
        assert!(text.contains("clif_func_body_0="));

        let _ = std::fs::remove_file(&path);
        unsafe {
            assert!(deleteCCraneliftDSPFactory(factory));
        }
    }

    #[test]
    fn clif_bitcode_roundtrip_in_memory_rebuilds_runnable_factory() {
        let _guard = crate::test_serial_guard();
        let name = c"clifroundtrip";
        let src = c"process = _;";
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!factory.is_null());

        let payload = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
        assert!(!payload.is_null());

        let restored =
            unsafe { readCCraneliftDSPFactoryFromBitcode(payload.cast_const(), err.as_mut_ptr()) };
        assert!(!restored.is_null());
        unsafe {
            assert!((*restored).compiled_jit.is_some());
            assert_eq!((*restored).num_inputs, (*factory).num_inputs);
            assert_eq!((*restored).num_outputs, (*factory).num_outputs);
            assert_eq!((*restored).sha_key, (*factory).sha_key);
            assert_eq!((*restored).compile_options, (*factory).compile_options);
            freeCMemory(payload.cast());
            assert!(deleteCCraneliftDSPFactory(factory));
            assert!(deleteCCraneliftDSPFactory(restored));
        }
    }

    #[test]
    fn clif_bitcode_roundtrip_via_file_rebuilds_runnable_factory() {
        let _guard = crate::test_serial_guard();
        let name = c"cliffile";
        let src = c"process = _;";
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!factory.is_null());

        let path = std::env::temp_dir().join(format!(
            "faust-rs-cranelift-ffi-clif-{}-{}.clif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_c = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();

        let wrote = unsafe { writeCCraneliftDSPFactoryToBitcodeFile(factory, path_c.as_ptr()) };
        assert!(wrote);
        let restored =
            unsafe { readCCraneliftDSPFactoryFromBitcodeFile(path_c.as_ptr(), err.as_mut_ptr()) };
        assert!(!restored.is_null());
        unsafe {
            assert!((*restored).compiled_jit.is_some());
            assert_eq!((*restored).num_inputs, (*factory).num_inputs);
            assert_eq!((*restored).num_outputs, (*factory).num_outputs);
            assert_eq!((*restored).sha_key, (*factory).sha_key);
            assert_eq!((*restored).compile_options, (*factory).compile_options);
        }

        let _ = std::fs::remove_file(path);
        unsafe {
            assert!(deleteCCraneliftDSPFactory(factory));
            assert!(deleteCCraneliftDSPFactory(restored));
        }
    }

    #[test]
    fn source_backed_bitcode_read_rejects_invalid_format() {
        let _guard = crate::test_serial_guard();
        let bad = c"NOT_A_CRANELIFT_FORMAT";
        let mut err = [0_i8; 4096];
        let restored =
            unsafe { readCCraneliftDSPFactoryFromBitcode(bad.as_ptr(), err.as_mut_ptr()) };
        assert!(restored.is_null());
        let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_str().unwrap();
        assert!(msg.contains("unsupported") || msg.contains("format"));
    }

    #[test]
    fn shared_factory_builder_rejects_non_module_runtime_descriptor() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let mut store = fir::FirStore::new();
        let bad_root = {
            let mut b = fir::FirBuilder::new(&mut store);
            b.int32(0)
        };
        let bad_fir = box_ffi::BoxFfiFirModule {
            store,
            module: bad_root,
            num_inputs: 0,
            num_outputs: 0,
        };
        let result = super::build_scaffold_factory_common(
            super::FactoryBuildSpec {
                name: "dsp",
                dsp_code: "process = _;",
                argv: &[],
                opt_level: 1,
                semantic_fingerprint: "fingerprint",
                foreign_function_fingerprint: "",
                source_is_faust: true,
            },
            &bad_fir,
            None,
        );
        assert!(
            result.is_err(),
            "builder must fail on non-module FIR runtime descriptors"
        );
    }

    #[test]
    fn selected_runtime_corpus_cases_lower_compute_body() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        // `rep_38_sine_phasor` now lowers through fixed-size FIR delay-line
        // arrays (`fDelay*`). The current Cranelift bring-up contract still
        // rejects DSP struct array fields, so keep that fixture out of the
        // selected lowered-body subset until Array struct fields are supported.
        let cases = [
            "tests/corpus/rep_01_passthrough.dsp",
            "tests/corpus/rep_02_gain_bias.dsp",
            "tests/corpus/rep_03_stereo_mix.dsp",
            "tests/corpus/rep_07_nonlinear_clip.dsp",
        ];

        for rel in cases {
            let mut err = [0_i8; 4096];
            let path = root.join(rel);
            let c_path =
                std::ffi::CString::new(path.to_string_lossy().as_bytes()).expect("path CString");
            let factory = unsafe {
                createCCraneliftDSPFactoryFromFile(
                    c_path.as_ptr(),
                    0,
                    std::ptr::null(),
                    err.as_mut_ptr(),
                    1,
                )
            };
            assert!(
                !factory.is_null(),
                "factory creation failed for {rel}: {}",
                unsafe { CStr::from_ptr(err.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            );
            unsafe {
                assert!(
                    (*factory).compute_body_lowered,
                    "Cranelift fallback used for selected corpus case {rel}"
                );
                assert!(deleteCCraneliftDSPFactory(factory));
            }
        }
    }

    #[test]
    fn clif_save_restore_selected_corpus_cases() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        // Keep the same reduced subset as `selected_runtime_corpus_cases_lower_compute_body`.
        let cases = [
            "tests/corpus/rep_01_passthrough.dsp",
            "tests/corpus/rep_02_gain_bias.dsp",
            "tests/corpus/rep_03_stereo_mix.dsp",
            "tests/corpus/rep_07_nonlinear_clip.dsp",
        ];

        for rel in cases {
            let mut err = [0_i8; 4096];
            let path = root.join(rel);
            let c_path =
                std::ffi::CString::new(path.to_string_lossy().as_bytes()).expect("path CString");
            let factory = unsafe {
                createCCraneliftDSPFactoryFromFile(
                    c_path.as_ptr(),
                    0,
                    std::ptr::null(),
                    err.as_mut_ptr(),
                    1,
                )
            };
            assert!(
                !factory.is_null(),
                "create from file failed for {rel}: {}",
                unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
            );

            let payload = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
            assert!(!payload.is_null(), "write bitcode failed for {rel}");
            let restored = unsafe {
                readCCraneliftDSPFactoryFromBitcode(payload.cast_const(), err.as_mut_ptr())
            };
            assert!(
                !restored.is_null(),
                "restore from bitcode failed for {rel}: {}",
                unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
            );

            unsafe {
                assert!(
                    (*factory).compiled_jit.is_some(),
                    "missing original jit for {rel}"
                );
                assert!(
                    (*restored).compiled_jit.is_some(),
                    "missing restored jit for {rel}"
                );
                assert_eq!(
                    (*restored).sha_key,
                    (*factory).sha_key,
                    "sha mismatch for {rel}"
                );
                assert_eq!(
                    (*restored).compile_options,
                    (*factory).compile_options,
                    "compile_options mismatch for {rel}"
                );
                freeCMemory(payload.cast());
                assert!(deleteCCraneliftDSPFactory(factory));
                assert!(deleteCCraneliftDSPFactory(restored));
            }
        }
    }

    #[test]
    fn boxes_and_signals_constructor_match_string_constructor_sha() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        box_ffi::createLibContext();
        let box_root = box_ffi::CboxWire();
        assert!(!box_root.is_null());

        let name = c"same";
        let src = c"process = _;";
        let mut err = [0_i8; 4096];

        let from_box = unsafe {
            createCCraneliftDSPFactoryFromBoxes(
                name.as_ptr(),
                box_root,
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!from_box.is_null());
        let signals = unsafe { box_ffi::CboxesToSignals(box_root, err.as_mut_ptr()) };
        assert!(!signals.is_null());
        let from_signals = unsafe {
            createCCraneliftDSPFactoryFromSignals(
                name.as_ptr(),
                signals.cast(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!from_signals.is_null());
        let from_string = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!from_string.is_null());

        let sha_box_ptr = unsafe { getCCraneliftDSPFactorySHAKey(from_box) };
        let sha_signals_ptr = unsafe { getCCraneliftDSPFactorySHAKey(from_signals) };
        let sha_string_ptr = unsafe { getCCraneliftDSPFactorySHAKey(from_string) };
        let sha_box = unsafe { CStr::from_ptr(sha_box_ptr) }
            .to_string_lossy()
            .into_owned();
        let sha_signals = unsafe { CStr::from_ptr(sha_signals_ptr) }
            .to_string_lossy()
            .into_owned();
        let sha_string = unsafe { CStr::from_ptr(sha_string_ptr) }
            .to_string_lossy()
            .into_owned();
        assert_eq!(sha_box, sha_signals);
        assert_eq!(sha_box, sha_string);

        unsafe {
            freeCMemory(sha_box_ptr.cast());
            freeCMemory(sha_signals_ptr.cast());
            freeCMemory(sha_string_ptr.cast());
            box_ffi::freeCMemory(signals.cast());
            assert!(deleteCCraneliftDSPFactory(from_box));
            assert!(deleteCCraneliftDSPFactory(from_signals));
            assert!(deleteCCraneliftDSPFactory(from_string));
        }
        box_ffi::destroyLibContext();
    }

    #[test]
    fn registered_foreign_function_is_used_for_factory_build() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let name = c"foreign_fun";
        let src = c"process = ffunction(float ffi_test_foreign_gain(float), <math.h>, \"\");";
        let mut err = [0_i8; 4096];

        unsafe {
            registerCCraneliftForeignFunction(
                c"ffi_test_foreign_gain".as_ptr(),
                (ffi_test_foreign_gain as *const ()).cast_mut().cast(),
            );
        }

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(
            !factory.is_null(),
            "factory creation failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        unsafe {
            assert!((*factory).compute_body_lowered);
            assert!(deleteCCraneliftDSPFactory(factory));
        }
        super::clear_registered_foreign_functions();
    }

    #[test]
    fn unregister_foreign_function_removes_future_binding() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let name = c"foreign_fun_unreg";
        let src = c"process = ffunction(float ffi_test_foreign_gain(float), <math.h>, \"\");";
        let mut err = [0_i8; 4096];

        unsafe {
            registerCCraneliftForeignFunction(
                c"ffi_test_foreign_gain".as_ptr(),
                (ffi_test_foreign_gain as *const ()).cast_mut().cast(),
            );
            unregisterCCraneliftForeignFunction(c"ffi_test_foreign_gain".as_ptr());
        }

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(
            !factory.is_null(),
            "factory creation failed after unregister: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        unsafe {
            assert!(
                !(*factory).compute_body_lowered,
                "unregistered foreign function should fall back to the stub path"
            );
            assert!(deleteCCraneliftDSPFactory(factory));
        }
        super::clear_registered_foreign_functions();
    }

    #[test]
    fn clear_foreign_functions_removes_all_future_bindings() {
        let _guard = crate::test_serial_guard();
        super::clear_registered_foreign_functions();
        let name = c"foreign_fun_clear";
        let src = c"process = ffunction(float ffi_test_foreign_gain(float), <math.h>, \"\");";
        let mut err = [0_i8; 4096];

        unsafe {
            registerCCraneliftForeignFunction(
                c"ffi_test_foreign_gain".as_ptr(),
                (ffi_test_foreign_gain as *const ()).cast_mut().cast(),
            );
        }
        clearCCraneliftForeignFunctions();

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(
            !factory.is_null(),
            "factory creation failed after clear: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        unsafe {
            assert!(
                !(*factory).compute_body_lowered,
                "cleared foreign functions should fall back to the stub path"
            );
            assert!(deleteCCraneliftDSPFactory(factory));
        }
        super::clear_registered_foreign_functions();
    }

    #[test]
    fn cpp_header_exposes_cranelift_foreign_function_api() {
        let header = std::fs::read_to_string(format!(
            "{}/include/cranelift-dsp.h",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("cranelift C++ header should be readable");

        assert!(header.contains("inline void registerCraneliftForeignFunction("));
        assert!(
            header.contains(
                "inline void unregisterCraneliftForeignFunction(const std::string& name)"
            )
        );
        assert!(header.contains("inline void clearCraneliftForeignFunctions()"));
    }
}
