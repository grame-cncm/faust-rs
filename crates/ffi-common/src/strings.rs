//! C string, argument-vector, and error-buffer marshalling.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

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

/// Frees a pointer as a C string, without consulting the pointer-array
/// registry.
///
/// Only correct for a pointer known to come from [`alloc_c_string`]. The
/// exported `freeCMemory` must use [`free_c_memory`] instead: a host hands it
/// pointer arrays too.
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

/// Pointer arrays handed to a host (`Signal*`, `char**`), keyed by address,
/// with the element count their release needs.
///
/// Process-global rather than owned by a tree context, for two reasons:
/// `freeCMemory` receives a bare pointer, so whichever adapter exports it must
/// recognize an array allocated by any other adapter; and an array stays the
/// host's to free after `destroyLibContext`, as with C++ `malloc`/`free`.
static OWNED_PTR_ARRAYS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();

fn owned_ptr_arrays() -> MutexGuard<'static, HashMap<usize, usize>> {
    // Every operation leaves the map consistent, so a panic elsewhere while it
    // was held does not invalidate it: recover instead of unwinding into C.
    OWNED_PTR_ARRAYS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Allocates a null-terminated pointer array for a C return value.
///
/// The terminator is appended here. The array is released by [`free_c_memory`]
/// (the exported `freeCMemory`); the pointers it holds are not.
#[must_use]
pub fn alloc_ptr_array<T>(mut values: Vec<*mut T>) -> *mut *mut T {
    values.push(std::ptr::null_mut());
    let len = values.len();
    let raw = Box::into_raw(values.into_boxed_slice()).cast::<*mut T>();
    owned_ptr_arrays().insert(raw as usize, len);
    raw
}

/// Releases `ptr` if it is a live array returned by [`alloc_ptr_array`].
///
/// Returns `false`, and does nothing, for any other pointer, including an
/// array already released.
pub fn free_if_owned_ptr_array(ptr: *mut c_void) -> bool {
    let Some(len) = owned_ptr_arrays().remove(&(ptr as usize)) else {
        return false;
    };
    // SAFETY: the registry holds exactly the arrays `alloc_ptr_array` leaked
    // from a boxed slice of `len` thin pointers, and removing the entry above
    // makes this the only release. `*mut c_void` has the layout of every
    // `*mut T` the array was allocated with.
    drop(unsafe {
        Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            ptr.cast::<*mut c_void>(),
            len,
        ))
    });
    true
}

/// The `freeCMemory` of every adapter: releases a pointer array returned by
/// [`alloc_ptr_array`], otherwise a C string returned by [`alloc_c_string`].
///
/// C++ Faust allocates both with `malloc` and frees them with `free`; here the
/// two allocations differ, so the array registry decides which release applies.
///
/// # Safety
/// `ptr` must be null, a live pointer array returned by [`alloc_ptr_array`], or
/// a C string returned by [`alloc_c_string`] and not yet freed.
pub unsafe fn free_c_memory(ptr: *mut c_void) {
    if ptr.is_null() || free_if_owned_ptr_array(ptr) {
        return;
    }
    unsafe { free_c_string(ptr.cast::<c_char>()) }
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

/// Decodes a required C string argument into an owned UTF-8 value.
///
/// Error messages follow the common Faust FFI wording pattern:
/// - `null <label> pointer`
/// - `invalid UTF-8 in <label>: ...`
///
/// # Safety
/// `ptr` must be null or point to a valid null-terminated C string.
pub unsafe fn required_c_string_arg(ptr: *const c_char, label: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("null {label} pointer"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_owned)
        .map_err(|error| format!("invalid UTF-8 in {label}: {error}"))
}

/// Decodes an optional C string argument into an owned UTF-8 value.
///
/// Returns `Ok(None)` when `ptr` is null.
///
/// # Safety
/// `ptr` must be null or point to a valid null-terminated C string.
pub unsafe fn optional_c_string_arg(
    ptr: *const c_char,
    label: &str,
) -> Result<Option<String>, String> {
    if ptr.is_null() {
        return Ok(None);
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_owned)
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
        alloc_c_string, alloc_ptr_array, decode_c_argv, free_c_memory, free_c_memory_c_string_only,
        free_c_string, free_if_owned_ptr_array, null_c_string_array, optional_c_string_arg,
        required_c_string_arg, write_error_4096,
    };
    use std::ffi::{c_char, c_void};

    #[test]
    fn pointer_arrays_are_null_terminated_and_released_once() {
        let elements = [alloc_c_string("a"), alloc_c_string("b")];
        let array = alloc_ptr_array(elements.to_vec());
        unsafe {
            assert_eq!(*array, elements[0]);
            assert_eq!(*array.add(1), elements[1]);
            assert!((*array.add(2)).is_null());
        }
        assert!(free_if_owned_ptr_array(array.cast()));
        assert!(!free_if_owned_ptr_array(array.cast()), "released twice");
        for element in elements {
            assert!(
                !free_if_owned_ptr_array(element.cast()),
                "a string is no array"
            );
            unsafe { free_c_string(element) };
        }
    }

    /// The exported `freeCMemory` contract: one entry point for strings and
    /// for the arrays the C headers return (`Signal*`, `char**`).
    #[test]
    fn free_c_memory_releases_arrays_and_strings() {
        let element = alloc_c_string("sha");
        let array: *mut *mut c_char = alloc_ptr_array(vec![element]);
        let empty: *mut *mut c_void = alloc_ptr_array(Vec::new());
        unsafe {
            free_c_memory(element.cast());
            free_c_memory(array.cast());
            free_c_memory(empty.cast());
            free_c_memory(std::ptr::null_mut());
        }
        assert!(!free_if_owned_ptr_array(array.cast()));
        assert!(!free_if_owned_ptr_array(empty.cast()));
    }

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
        let mut buffer = [0 as c_char; 4096];
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
    fn c_string_arg_helpers_return_owned_required_and_optional_values() {
        let string = CString::new("abc").unwrap();
        let required = unsafe { required_c_string_arg(string.as_ptr(), "filename") }.unwrap();
        let optional = unsafe { optional_c_string_arg(string.as_ptr(), "name_app") }.unwrap();
        let none = unsafe { optional_c_string_arg(std::ptr::null(), "name_app") }.unwrap();
        drop(string);
        assert_eq!(required, "abc");
        assert_eq!(optional.as_deref(), Some("abc"));
        assert_eq!(none, None);
    }

    #[test]
    fn c_string_arg_helpers_report_null_and_invalid_utf8() {
        let null_error =
            unsafe { required_c_string_arg(std::ptr::null(), "filename") }.unwrap_err();
        assert_eq!(null_error, "null filename pointer");

        let invalid = [0xff_u8, 0];
        let required_error =
            unsafe { required_c_string_arg(invalid.as_ptr().cast(), "filename") }.unwrap_err();
        let optional_error =
            unsafe { optional_c_string_arg(invalid.as_ptr().cast(), "name_app") }.unwrap_err();
        assert!(required_error.starts_with("invalid UTF-8 in filename:"));
        assert!(optional_error.starts_with("invalid UTF-8 in name_app:"));
    }

    #[test]
    fn allocated_c_strings_escape_embedded_nul_bytes() {
        let pointer = alloc_c_string("left\0right");
        let value = unsafe { CStr::from_ptr(pointer) }.to_str().unwrap();
        assert_eq!(value, r"left\0right");
        unsafe {
            free_c_string(pointer);
        }
    }

    #[test]
    fn error_buffer_truncates_to_4095_bytes_and_terminates() {
        let mut buffer = [1 as c_char; 4096];
        let message = "x".repeat(5000);
        unsafe {
            write_error_4096(buffer.as_mut_ptr(), &message);
        }
        assert_eq!(buffer[4095], 0);
        let value = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
        assert_eq!(value.len(), 4095);
        assert!(value.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn null_c_string_array_returns_null_terminated_empty_array() {
        let pointer = null_c_string_array();
        assert!(!pointer.is_null());
        let first = unsafe { *pointer };
        assert!(first.is_null());
    }
}
