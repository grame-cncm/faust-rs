//! Ownership helpers for opaque Rust values exposed through C handles.

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

#[cfg(test)]
mod tests {
    use super::{alloc_opaque, free_opaque};

    #[test]
    fn opaque_helpers_roundtrip() {
        let pointer = alloc_opaque(123_u32);
        assert!(!pointer.is_null());
        unsafe {
            assert_eq!(*pointer, 123);
            free_opaque(pointer);
        }
    }
}
