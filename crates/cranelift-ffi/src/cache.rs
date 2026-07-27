//! Global factory cache for `cranelift_dsp` (scaffold implementation).
//!
//! This mirrors the basic shape of the interpreter/LLVM factory cache APIs:
//! SHA key -> factory pointer, with `get/deleteAll/getAll` entry points.
//!
//! # Current limitations
//! - No refcount semantics yet (unlike the production C++ behavior).
//! - Returns raw pointers directly from the cache.
//! - Intended for FFI/lifecycle smoke validation during early Cranelift porting.

use std::sync::LazyLock;

use crate::types::CraneliftDspFactory;
use ffi_common::FactoryCache;

static FACTORY_CACHE: LazyLock<FactoryCache<CraneliftDspFactory>> =
    LazyLock::new(FactoryCache::new);

/// Insert or replace a factory pointer under a SHA key.
///
/// The cache owns only the key mapping, not the pointed factory allocation.
/// Callers remain responsible for eventual `free_factory` after removal/drain.
pub(crate) fn cache_insert(sha: &str, ptr: *mut CraneliftDspFactory) {
    FACTORY_CACHE.insert(sha, ptr);
}

/// Look up a factory pointer by SHA key.
///
/// Returns null when the key is absent.
#[must_use]
pub(crate) fn cache_lookup(sha: &str) -> *mut CraneliftDspFactory {
    FACTORY_CACHE.lookup(sha)
}

/// Remove a factory from the cache by pointer value.
///
/// This is used by delete paths that do not necessarily know the SHA key.
pub(crate) fn cache_remove_by_ptr(ptr: *mut CraneliftDspFactory) {
    FACTORY_CACHE.remove_by_ptr(ptr);
}

/// Drain the entire cache and return factory pointers for caller-side freeing.
#[must_use]
pub(crate) fn cache_drain() -> Vec<*mut CraneliftDspFactory> {
    FACTORY_CACHE.drain()
}

/// Return all SHA keys currently stored in the cache.
#[must_use]
pub(crate) fn cache_all_sha_keys() -> Vec<String> {
    FACTORY_CACHE.all_sha_keys()
}

/// Enable multi-thread mode (compatibility flag only in the scaffold).
///
/// The underlying `ffi_common::FactoryCache` keeps this flag for API parity with
/// the classic Faust C API families, even though the Rust implementation is
/// already guarded by synchronization primitives.
#[must_use]
pub(crate) fn start_mt() -> bool {
    FACTORY_CACHE.start_mt()
}

/// Disable multi-thread mode (compatibility flag only in the scaffold).
///
/// No teardown is required beyond forwarding to the shared cache helper.
pub(crate) fn stop_mt() {
    FACTORY_CACHE.stop_mt();
}

/// Returns a short status string used by tests to assert the scaffold module is present.
#[must_use]
pub fn cache_status() -> &'static str {
    "cranelift-ffi cache scaffold"
}

#[cfg(test)]
mod tests {
    use super::{cache_all_sha_keys, cache_drain, cache_insert, cache_lookup, cache_status};

    #[test]
    fn cache_scaffold_status_is_stable() {
        assert_eq!(cache_status(), "cranelift-ffi cache scaffold");
    }

    #[test]
    fn cache_roundtrip_raw_pointer() {
        let p = 0x1234usize as *mut crate::types::CraneliftDspFactory;
        cache_insert("sha-test", p);
        assert_eq!(cache_lookup("sha-test"), p);
        assert!(cache_all_sha_keys().iter().any(|s| s == "sha-test"));
        let drained = cache_drain();
        assert!(drained.contains(&p));
        assert!(cache_lookup("sha-test").is_null());
    }
}
