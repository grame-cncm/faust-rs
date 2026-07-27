//! Global factory cache.
//!
//! Mirrors the `gFactoryDSPTable` cache in the C++ Faust implementation.
//! Maps SHA key strings to raw factory pointers.
//!
//! Thread safety: protected by a `Mutex`.  The `startMTDSPFactories` /
//! `stopMTDSPFactories` functions exist for API compatibility; actual thread
//! safety is provided by the `Mutex` regardless of the MT mode flag.

use std::sync::LazyLock;

use crate::types::InterpreterDspFactory;
use ffi_common::FactoryCache;

static FACTORY_CACHE: LazyLock<FactoryCache<InterpreterDspFactory>> =
    LazyLock::new(FactoryCache::new);

/// Insert a factory into the cache under its SHA key.
pub(crate) fn cache_insert(sha: &str, ptr: *mut InterpreterDspFactory) {
    FACTORY_CACHE.insert(sha, ptr);
}

/// Look up a factory by SHA key.  Returns `null` if not found.
pub(crate) fn cache_lookup(sha: &str) -> *mut InterpreterDspFactory {
    FACTORY_CACHE.lookup(sha)
}

/// Remove a factory from the cache by pointer value.
/// Does nothing if the pointer is not in the cache.
pub(crate) fn cache_remove_by_ptr(ptr: *mut InterpreterDspFactory) {
    FACTORY_CACHE.remove_by_ptr(ptr);
}

/// Drain the entire cache and return all factory pointers.
///
/// The caller is responsible for freeing them.
pub(crate) fn cache_drain() -> Vec<*mut InterpreterDspFactory> {
    FACTORY_CACHE.drain()
}

/// Return all SHA keys in the cache.
pub(crate) fn cache_all_sha_keys() -> Vec<String> {
    FACTORY_CACHE.all_sha_keys()
}

/// Enable multi-thread access mode.
pub(crate) fn start_mt() -> bool {
    FACTORY_CACHE.start_mt()
}

/// Disable multi-thread access mode.
pub(crate) fn stop_mt() {
    FACTORY_CACHE.stop_mt();
}
