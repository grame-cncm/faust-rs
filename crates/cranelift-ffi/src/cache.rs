//! Owned global factory/instance cache for the Cranelift C API.
//!
//! Cranelift follows the maintained Faust factory reference contract used by
//! the Interpreter and LLVM APIs. Rust synchronizes lifecycle operations in
//! both compatibility MT modes.

use std::sync::LazyLock;

use ffi_common::{FactoryCache, FactoryHandle, FactoryRelease};

use crate::types::{CraneliftDspFactory, CraneliftDspInstance};

static FACTORY_CACHE: LazyLock<FactoryCache<CraneliftDspFactory, CraneliftDspInstance>> =
    LazyLock::new(FactoryCache::new);

/// Inserts a factory or acquires the existing factory with the same SHA.
pub(crate) fn cache_insert(sha: &str, factory: CraneliftDspFactory) -> *mut CraneliftDspFactory {
    FACTORY_CACHE
        .insert_or_acquire(sha, factory)
        .map_or(std::ptr::null_mut(), FactoryHandle::as_ptr)
}

/// Looks up a factory by SHA and acquires a releasable reference.
#[must_use]
pub(crate) fn cache_lookup(sha: &str) -> *mut CraneliftDspFactory {
    FACTORY_CACHE
        .lookup_acquire(sha)
        .map_or(std::ptr::null_mut(), FactoryHandle::as_ptr)
}

/// Releases one factory reference and reports whether the allocation was freed.
pub(crate) fn cache_release(ptr: *mut CraneliftDspFactory) -> bool {
    FactoryHandle::from_raw(ptr)
        .is_some_and(|handle| FACTORY_CACHE.release(handle) == FactoryRelease::Removed)
}

/// Registers an instance for automatic deletion with its factory.
pub(crate) fn cache_register_instance(
    factory: *mut CraneliftDspFactory,
    instance: CraneliftDspInstance,
) -> *mut CraneliftDspInstance {
    FactoryHandle::from_raw(factory).map_or(std::ptr::null_mut(), |handle| {
        FACTORY_CACHE.register_instance(handle, instance)
    })
}

/// Removes and drops one manually deleted instance.
pub(crate) fn cache_remove_instance(ptr: *mut CraneliftDspInstance) -> bool {
    FACTORY_CACHE.remove_instance(ptr)
}

/// Drops all cached factories and instances regardless of outstanding handles.
pub(crate) fn cache_clear() {
    FACTORY_CACHE.clear();
}

/// Returns all factory SHA keys.
#[must_use]
pub(crate) fn cache_all_sha_keys() -> Vec<String> {
    FACTORY_CACHE.all_sha_keys()
}

/// Enables the public MT compatibility mode.
#[must_use]
pub(crate) fn start_mt() -> bool {
    FACTORY_CACHE.start_mt()
}

/// Disables the public MT compatibility mode.
pub(crate) fn stop_mt() {
    FACTORY_CACHE.stop_mt();
}

/// Returns a short status string used by tests to assert the module is present.
#[must_use]
pub fn cache_status() -> &'static str {
    "cranelift-ffi owned cache"
}
