//! Shared mechanism for SHA-keyed FFI factory caches.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

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
            .map_or(std::ptr::null_mut(), |value| value as *mut T)
    }

    /// Remove all entries matching the provided pointer value.
    pub fn remove_by_ptr(&self, ptr: *mut T) {
        let mut guard = self.map.lock().unwrap();
        guard.retain(|_, value| *value != ptr as usize);
    }

    /// Drain the cache and return all stored pointers.
    #[must_use]
    pub fn drain(&self) -> Vec<*mut T> {
        let mut guard = self.map.lock().unwrap();
        guard.drain().map(|(_, value)| value as *mut T).collect()
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
    use super::FactoryCache;

    #[test]
    fn roundtrip_raw_pointer() {
        let cache = FactoryCache::<u8>::new();
        let pointer = 0x1234usize as *mut u8;
        cache.insert("sha", pointer);
        assert_eq!(cache.lookup("sha"), pointer);
        assert_eq!(cache.all_sha_keys(), vec!["sha".to_string()]);
        cache.remove_by_ptr(pointer);
        assert!(cache.lookup("sha").is_null());
        cache.insert("sha2", pointer);
        let drained = cache.drain();
        assert_eq!(drained, vec![pointer]);
        assert!(cache.lookup("sha2").is_null());
        assert!(cache.start_mt());
        cache.stop_mt();
    }
}
