//! C string, argument-vector, and error-buffer marshalling.

use std::ffi::{CStr, CString, c_char, c_void};

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
            .map_err(|error| format!("invalid UTF-8 in argv[{index}]: {error}"))?;
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
        .map_err(|error| format!("invalid UTF-8 in {label}: {error}"))
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
        .map_err(|error| format!("invalid UTF-8 in {label}: {error}"))
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

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};

    use super::{
        alloc_c_string, decode_c_argv, free_c_memory_c_string_only, free_c_string,
        null_c_string_array, optional_c_str_arg, required_c_str_arg, write_error_4096,
    };

    #[test]
    fn c_string_helpers_roundtrip() {
        let pointer = alloc_c_string("ok");
        assert!(!pointer.is_null());
        unsafe {
            free_c_string(pointer);
        }
    }

    #[test]
    fn free_c_memory_string_only_handles_c_string() {
        let pointer = alloc_c_string("ok");
        unsafe {
            free_c_memory_c_string_only(pointer.cast());
        }
    }

    #[test]
    fn write_error_4096_writes_nul_terminated_message() {
        let mut buffer = [0_i8; 4096];
        unsafe {
            write_error_4096(buffer.as_mut_ptr(), "hello");
        }
        let value = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap();
        assert_eq!(value, "hello");
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
        let string = CString::new("abc").unwrap();
        let required = unsafe { required_c_str_arg(string.as_ptr(), "filename") }.unwrap();
        let optional = unsafe { optional_c_str_arg(string.as_ptr(), "name_app") }.unwrap();
        let none = unsafe { optional_c_str_arg(std::ptr::null(), "name_app") }.unwrap();
        assert_eq!(required, "abc");
        assert_eq!(optional, Some("abc"));
        assert_eq!(none, None);
    }

    #[test]
    fn null_c_string_array_returns_null_terminated_empty_array() {
        let pointer = null_c_string_array();
        assert!(!pointer.is_null());
        let first = unsafe { *pointer };
        assert!(first.is_null());
    }
}
