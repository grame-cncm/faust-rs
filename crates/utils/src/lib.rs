//! Shared utility crate placeholder.
//!
//! # Intended role
//! - Provide small, dependency-light helpers reused across crates (formatting,
//!   path helpers, stable ordering helpers, etc.).
//! - Keep cross-cutting helpers out of domain crates to preserve boundaries.
//!
//! # Current status
//! - Small shared helpers are being introduced as duplication appears in
//!   backend/FFI crates.
//!
//! # API mapping status
//! - `crate_id()` is `adapted` utility metadata (no direct C++ counterpart).

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const CRATE_NAME: &str = "utils";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// `FAUSTFLOAT` type used by current Rust FFI exports (`f32`).
///
/// This is intentionally defined in `utils` so FFI callback-table layouts can
/// be shared by multiple backend-specific FFI crates without duplicating the
/// `#[repr(C)]` definitions.
pub type FfiFaustFloat = f32;

/// C-ABI UI callback table used by generated/runtime DSP code (mirrors Faust `UIGlue`).
///
/// Backend FFI crates re-export this type so the external C ABI remains stable
/// while the callback-table definition is maintained in a single place.
#[repr(C)]
pub struct UIGlue {
    pub ui_interface: *mut c_void,
    pub open_tab_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub open_horizontal_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub open_vertical_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub close_box: Option<unsafe extern "C" fn(*mut c_void)>,
    pub add_button: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *mut FfiFaustFloat)>,
    pub add_check_button:
        Option<unsafe extern "C" fn(*mut c_void, *const c_char, *mut FfiFaustFloat)>,
    pub add_vertical_slider: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_horizontal_slider: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_num_entry: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_horizontal_bargraph: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_vertical_bargraph: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_soundfile:
        Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_void)>,
    pub declare:
        Option<unsafe extern "C" fn(*mut c_void, *mut FfiFaustFloat, *const c_char, *const c_char)>,
}

/// C-ABI metadata callback table used by generated/runtime DSP code (mirrors Faust `MetaGlue`).
#[repr(C)]
pub struct MetaGlue {
    pub meta_interface: *mut c_void,
    pub declare: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char)>,
}

/// Allocates a heap C string for FFI return values.
///
/// Embedded NUL bytes are replaced with the textual sequence `\\0`.
#[must_use]
pub fn alloc_c_string(s: &str) -> *mut c_char {
    let safe = s.replace('\0', "\\0");
    match CString::new(safe) {
        Ok(cs) => cs.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Boxes a value and returns an owning raw pointer for FFI opaque handles.
///
/// Backend FFI crates should keep backend-specific wrapper functions around
/// this helper so public docs and ownership contracts remain explicit.
#[must_use]
pub fn alloc_opaque<T>(value: T) -> *mut T {
    Box::into_raw(Box::new(value))
}

/// Frees an opaque pointer previously returned by [`alloc_opaque`].
///
/// # Safety
/// `ptr` must be a valid non-null pointer returned by [`alloc_opaque`], and it
/// must not be used after this call.
pub unsafe fn free_opaque<T>(ptr: *mut T) {
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

/// Frees a pointer previously returned by [`alloc_c_string`].
///
/// # Safety
/// `ptr` must be null or a valid pointer returned by [`alloc_c_string`].
pub unsafe fn free_c_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

/// Implements the common `freeCMemory` behavior used by Faust-style FFI crates
/// for heap strings returned through the C ABI.
///
/// This helper only handles the "pointer was allocated as a C string" case.
/// For `char**` arrays, callers must free elements first, then free the outer
/// array pointer according to backend-specific allocation strategy.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by an API documented as
/// freeable via `freeCMemory`.
pub unsafe fn free_c_memory_c_string_only(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            free_c_string(ptr.cast::<c_char>());
        }
    }
}

/// Writes an error message into a conventional Faust 4096-byte error buffer.
///
/// # Safety
/// `buf` must be null or point to at least 4096 writable bytes.
pub unsafe fn write_error_4096(buf: *mut c_char, msg: &str) {
    if buf.is_null() {
        return;
    }
    let bytes = msg.as_bytes();
    let len = bytes.len().min(4095);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buf, len);
        *buf.add(len) = 0;
    }
}

/// Decodes a conventional `argc`/`argv` pair into owned UTF-8 Rust strings.
///
/// # Safety
/// - When `argc > 0`, `argv` must be non-null and reference at least `argc`
///   entries.
/// - Each entry must be a valid null-terminated C string.
pub unsafe fn decode_c_argv(argc: i32, argv: *const *const c_char) -> Result<Vec<String>, String> {
    if argc < 0 {
        return Err("negative argc".to_owned());
    }
    if argc == 0 {
        return Ok(Vec::new());
    }
    if argv.is_null() {
        return Err("argv is null while argc > 0".to_owned());
    }
    let argc = usize::try_from(argc).map_err(|_| "argc out of range".to_owned())?;
    let raw_args = unsafe { std::slice::from_raw_parts(argv, argc) };
    let mut result = Vec::with_capacity(raw_args.len());
    for (index, ptr) in raw_args.iter().copied().enumerate() {
        if ptr.is_null() {
            return Err(format!("argv[{index}] is null"));
        }
        let value = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map_err(|e| format!("invalid UTF-8 in argv[{index}]: {e}"))?;
        result.push(value.to_owned());
    }
    Ok(result)
}

/// Decodes a required C string argument as UTF-8.
///
/// Error messages follow the common Faust FFI wording pattern:
/// - `null <label> pointer`
/// - `invalid UTF-8 in <label>: ...`
///
/// # Safety
/// `ptr` must be null or point to a valid null-terminated C string.
pub unsafe fn required_c_str_arg<'a>(ptr: *const c_char, label: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("null {label} pointer"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| format!("invalid UTF-8 in {label}: {e}"))
}

/// Decodes an optional C string argument as UTF-8.
///
/// Returns `Ok(None)` when `ptr` is null.
///
/// # Safety
/// `ptr` must be null or point to a valid null-terminated C string.
pub unsafe fn optional_c_str_arg<'a>(
    ptr: *const c_char,
    label: &str,
) -> Result<Option<&'a str>, String> {
    if ptr.is_null() {
        return Ok(None);
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(Some)
        .map_err(|e| format!("invalid UTF-8 in {label}: {e}"))
}

/// Returns a process-lifetime static null-terminated empty `char**` array.
#[must_use]
pub fn null_c_string_array() -> *const *const c_char {
    struct SyncNullArray([*const c_char; 1]);
    // SAFETY: Immutable static null pointer array.
    unsafe impl Sync for SyncNullArray {}
    static NULL_ARRAY: SyncNullArray = SyncNullArray([std::ptr::null()]);
    NULL_ARRAY.0.as_ptr()
}

/// Minimal shared subset of Faust CLI-like options accepted by Rust FFI crates.
///
/// Supported options: `-I <path>`, `-cn <name>`, `-double`, the vector-mode
/// trio `-vec` / `-vs <n>` / `-lv <n>`, and the scheduling-strategy option
/// `-ss <n>`.
/// Unknown options are ignored so backend FFI crates can accept broader argv
/// vectors while incrementally extending support.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FfiCompileArgs {
    /// Extra import search paths collected from `-I`.
    pub search_paths: Vec<PathBuf>,
    /// Optional class/module name override from `-cn`.
    pub module_name: Option<String>,
    /// Use double-precision (64-bit) floating-point for internal DSP arithmetic.
    ///
    /// Set by the `-double` flag in the `argv` vector passed to FFI factory
    /// constructors.  Mirrors the reference Faust compiler's `-double` option.
    pub double: bool,
    /// Vector mode requested (`-vec`). When false, `vec_size`/`loop_variant` are
    /// ignored (scalar codegen).
    pub vec_mode: bool,
    /// Vector chunk size (`-vs <n>`; Faust default 32). Only meaningful when
    /// [`Self::vec_mode`] is set.
    pub vec_size: u32,
    /// Vector loop variant (`-lv <n>`; 0 = fastest/default, 1 = simple). Only
    /// meaningful when [`Self::vec_mode`] is set.
    pub loop_variant: u8,
    /// Raw signal/loop scheduling-strategy value (`-ss <n>`; Faust default
    /// `0`, depth-first).
    ///
    /// Kept as the raw non-negative integer here (rather than a decoded
    /// enum) because `utils` is a dependency-light leaf crate and the
    /// `SchedulingStrategy` enum lives in `transform`. Callers that depend on
    /// `compiler`/`transform` decode it with
    /// `transform::schedule::SchedulingStrategy::decode` (re-exported as
    /// `compiler::SchedulingStrategy`): `0 -> DepthFirst`,
    /// `1 -> BreadthFirst`, `2 -> Special`, `n >= 3 -> ReverseBreadthFirst`.
    /// Vectorization port plan phase P2: parsed and threaded, but no FFI
    /// compile path invokes the scheduler yet.
    pub scheduling_strategy: u32,
}

/// Parses the shared FFI option subset (`-I`, `-cn`, `-double`,
/// `-vec`/`-vs`/`-lv`, `-ss`) from an argv vector. `vec_size` defaults to 32
/// when `-vec` is given without `-vs`, matching the Faust CLI.
/// `scheduling_strategy` defaults to `0` (depth-first) when `-ss` is absent,
/// mirroring the CLI's `--scheduling-strategy` default.
pub fn parse_ffi_compile_args(argv: &[String]) -> Result<FfiCompileArgs, String> {
    let mut parsed = FfiCompileArgs {
        vec_size: 32,
        ..FfiCompileArgs::default()
    };
    let mut i = 0usize;
    while i < argv.len() {
        let arg = &argv[i];
        if arg == "-I" {
            let Some(value) = argv.get(i + 1) else {
                return Err("missing path after -I".to_owned());
            };
            parsed.search_paths.push(PathBuf::from(value));
            i += 2;
            continue;
        }
        if arg == "-cn" {
            let Some(value) = argv.get(i + 1) else {
                return Err("missing class name after -cn".to_owned());
            };
            parsed.module_name = Some(value.clone());
            i += 2;
            continue;
        }
        if arg == "-double" {
            parsed.double = true;
            i += 1;
            continue;
        }
        if arg == "-vec" {
            parsed.vec_mode = true;
            i += 1;
            continue;
        }
        if arg == "-vs" {
            let Some(value) = argv.get(i + 1) else {
                return Err("missing value after -vs".to_owned());
            };
            parsed.vec_size = value.parse().map_err(|e| format!("bad -vs value: {e}"))?;
            i += 2;
            continue;
        }
        if arg == "-lv" {
            let Some(value) = argv.get(i + 1) else {
                return Err("missing value after -lv".to_owned());
            };
            parsed.loop_variant = value.parse().map_err(|e| format!("bad -lv value: {e}"))?;
            i += 2;
            continue;
        }
        if arg == "-ss" {
            let Some(value) = argv.get(i + 1) else {
                return Err("missing value after -ss".to_owned());
            };
            parsed.scheduling_strategy =
                value.parse().map_err(|e| format!("bad -ss value: {e}"))?;
            i += 2;
            continue;
        }
        i += 1;
    }
    Ok(parsed)
}

/// Minimal SHA-keyed factory cache used by FFI crates.
///
/// The cache stores raw pointers as `usize` under a `Mutex<HashMap<...>>` so it
/// remains `Send + Sync` without exposing `*mut T` in shared mutable statics.
///
/// # Scope
/// - This is intentionally a small mechanism helper (insert/lookup/remove/drain
///   + optional MT compatibility flag).
/// - Backend-specific crates keep their own wrapper functions and semantics.
///
/// # Current limitations
/// - No refcount/coalescing semantics.
/// - Returns raw pointers directly.
/// - MT mode flag is compatibility metadata only; thread safety is provided by
///   the mutex regardless of the flag state.
pub struct FactoryCache<T> {
    map: Mutex<HashMap<String, usize>>,
    mt_mode: AtomicBool,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Default for FactoryCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FactoryCache<T> {
    /// Creates an empty cache with MT mode disabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            mt_mode: AtomicBool::new(false),
            _marker: PhantomData,
        }
    }

    /// Insert or replace a pointer under a SHA key.
    pub fn insert(&self, sha: &str, ptr: *mut T) {
        let mut guard = self.map.lock().unwrap();
        guard.insert(sha.to_owned(), ptr as usize);
    }

    /// Look up a pointer by SHA key, returning null if absent.
    #[must_use]
    pub fn lookup(&self, sha: &str) -> *mut T {
        let guard = self.map.lock().unwrap();
        guard
            .get(sha)
            .copied()
            .map_or(std::ptr::null_mut(), |v| v as *mut T)
    }

    /// Remove all entries matching the provided pointer value.
    pub fn remove_by_ptr(&self, ptr: *mut T) {
        let mut guard = self.map.lock().unwrap();
        guard.retain(|_, v| *v != ptr as usize);
    }

    /// Drain the cache and return all stored pointers.
    #[must_use]
    pub fn drain(&self) -> Vec<*mut T> {
        let mut guard = self.map.lock().unwrap();
        guard.drain().map(|(_, v)| v as *mut T).collect()
    }

    /// Returns all SHA keys currently present in the cache.
    #[must_use]
    pub fn all_sha_keys(&self) -> Vec<String> {
        let guard = self.map.lock().unwrap();
        guard.keys().cloned().collect()
    }

    /// Enable MT mode (compatibility flag only) and return success.
    #[must_use]
    pub fn start_mt(&self) -> bool {
        self.mt_mode.store(true, Ordering::SeqCst);
        true
    }

    /// Disable MT mode (compatibility flag only).
    pub fn stop_mt(&self) {
        self.mt_mode.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};
    use std::path::PathBuf;

    use super::{
        FactoryCache, MetaGlue, UIGlue, alloc_c_string, alloc_opaque, crate_id, decode_c_argv,
        free_c_memory_c_string_only, free_c_string, free_opaque, null_c_string_array,
        optional_c_str_arg, parse_ffi_compile_args, required_c_str_arg, write_error_4096,
    };

    #[test]
    fn crate_id_is_stable() {
        assert_eq!(crate_id(), "utils");
    }

    #[test]
    fn factory_cache_roundtrip_raw_pointer() {
        let cache = FactoryCache::<u8>::new();
        let p = 0x1234usize as *mut u8;
        cache.insert("sha", p);
        assert_eq!(cache.lookup("sha"), p);
        assert_eq!(cache.all_sha_keys(), vec!["sha".to_string()]);
        cache.remove_by_ptr(p);
        assert!(cache.lookup("sha").is_null());
        cache.insert("sha2", p);
        let drained = cache.drain();
        assert_eq!(drained, vec![p]);
        assert!(cache.lookup("sha2").is_null());
        assert!(cache.start_mt());
        cache.stop_mt();
    }

    #[test]
    fn ffi_glue_types_are_constructible() {
        let _ = std::mem::size_of::<UIGlue>();
        let _ = std::mem::size_of::<MetaGlue>();
    }

    #[test]
    fn c_string_helpers_roundtrip() {
        let p = alloc_c_string("ok");
        assert!(!p.is_null());
        unsafe {
            free_c_string(p);
        }
    }

    #[test]
    fn opaque_helpers_roundtrip() {
        let p = alloc_opaque(123_u32);
        assert!(!p.is_null());
        unsafe {
            assert_eq!(*p, 123);
            free_opaque(p);
        }
    }

    #[test]
    fn free_c_memory_string_only_handles_c_string() {
        let p = alloc_c_string("ok");
        unsafe {
            free_c_memory_c_string_only(p.cast());
        }
    }

    #[test]
    fn write_error_4096_writes_nul_terminated_message() {
        let mut buf = [0_i8; 4096];
        unsafe {
            write_error_4096(buf.as_mut_ptr(), "hello");
        }
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn decode_c_argv_roundtrip() {
        let args = [CString::new("-Ilib").unwrap(), CString::new("-cn").unwrap()];
        let argv = [args[0].as_ptr(), args[1].as_ptr()];
        let decoded = unsafe { decode_c_argv(2, argv.as_ptr()) }.unwrap();
        assert_eq!(decoded, vec!["-Ilib".to_string(), "-cn".to_string()]);
    }

    #[test]
    fn c_str_arg_helpers_decode_required_and_optional() {
        let s = CString::new("abc").unwrap();
        let req = unsafe { required_c_str_arg(s.as_ptr(), "filename") }.unwrap();
        let opt = unsafe { optional_c_str_arg(s.as_ptr(), "name_app") }.unwrap();
        let none = unsafe { optional_c_str_arg(std::ptr::null(), "name_app") }.unwrap();
        assert_eq!(req, "abc");
        assert_eq!(opt, Some("abc"));
        assert_eq!(none, None);
    }

    #[test]
    fn null_c_string_array_returns_null_terminated_empty_array() {
        let p = null_c_string_array();
        assert!(!p.is_null());
        let first = unsafe { *p };
        assert!(first.is_null());
    }

    #[test]
    fn parse_ffi_compile_args_accepts_i_cn_and_vec_options() {
        let argv = vec![
            "-I".to_owned(),
            "lib1".to_owned(),
            "-I".to_owned(),
            "lib2".to_owned(),
            "-cn".to_owned(),
            "MyDSP".to_owned(),
            "-vec".to_owned(),
            "-vs".to_owned(),
            "64".to_owned(),
            "-lv".to_owned(),
            "1".to_owned(),
        ];
        let parsed = parse_ffi_compile_args(&argv).unwrap();
        assert_eq!(parsed.search_paths.len(), 2);
        assert_eq!(parsed.search_paths[0], PathBuf::from("lib1"));
        assert_eq!(parsed.search_paths[1], PathBuf::from("lib2"));
        assert_eq!(parsed.module_name.as_deref(), Some("MyDSP"));
        assert!(parsed.vec_mode);
        assert_eq!(parsed.vec_size, 64);
        assert_eq!(parsed.loop_variant, 1);
    }

    #[test]
    fn parse_ffi_compile_args_uses_default_vec_size() {
        let argv = vec!["-vec".to_owned()];
        let parsed = parse_ffi_compile_args(&argv).unwrap();
        assert!(parsed.vec_mode);
        assert_eq!(parsed.vec_size, 32);
        assert_eq!(parsed.loop_variant, 0);
    }

    #[test]
    fn parse_ffi_compile_args_accepts_ss_option() {
        let argv = vec!["-ss".to_owned(), "3".to_owned()];
        let parsed = parse_ffi_compile_args(&argv).unwrap();
        assert_eq!(parsed.scheduling_strategy, 3);
    }

    #[test]
    fn parse_ffi_compile_args_ss_defaults_to_zero_when_absent() {
        let parsed = parse_ffi_compile_args(&[]).unwrap();
        assert_eq!(parsed.scheduling_strategy, 0);
    }

    #[test]
    fn parse_ffi_compile_args_ss_is_independent_of_vec() {
        // `-ss` must parse without `-vec` present, and must not perturb the
        // vector-mode defaults.
        let argv = vec!["-ss".to_owned(), "1".to_owned()];
        let parsed = parse_ffi_compile_args(&argv).unwrap();
        assert_eq!(parsed.scheduling_strategy, 1);
        assert!(!parsed.vec_mode);
        assert_eq!(parsed.vec_size, 32);
        assert_eq!(parsed.loop_variant, 0);
    }

    #[test]
    fn parse_ffi_compile_args_rejects_missing_ss_value() {
        let argv = vec!["-ss".to_owned()];
        let err = parse_ffi_compile_args(&argv).unwrap_err();
        assert!(err.contains("missing value after -ss"), "{err}");
    }

    #[test]
    fn parse_ffi_compile_args_rejects_non_integer_ss_value() {
        let argv = vec!["-ss".to_owned(), "abc".to_owned()];
        let err = parse_ffi_compile_args(&argv).unwrap_err();
        assert!(err.contains("bad -ss value"), "{err}");
    }

    #[test]
    fn parse_ffi_compile_args_rejects_negative_ss_value() {
        let argv = vec!["-ss".to_owned(), "-1".to_owned()];
        let err = parse_ffi_compile_args(&argv).unwrap_err();
        assert!(err.contains("bad -ss value"), "{err}");
    }
}
