//! Owned, reference-counted factory cache for Faust FFI backends.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Opaque factory identity held while the cache owns the allocation.
///
/// The handle intentionally exposes no Rust reference or dereference
/// operation. FFI adapters may derive the raw pointer at the ABI edge, while
/// cache operations use this typed identity to avoid integer-erased pointers.
pub struct FactoryHandle<T> {
    pointer: NonNull<T>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> FactoryHandle<T> {
    /// Builds a typed identity from a non-null raw ABI pointer.
    #[must_use]
    pub fn from_raw(pointer: *mut T) -> Option<Self> {
        NonNull::new(pointer).map(|pointer| Self {
            pointer,
            _marker: PhantomData,
        })
    }

    /// Returns the raw pointer used at the ABI edge.
    #[must_use]
    pub fn as_ptr(self) -> *mut T {
        self.pointer.as_ptr()
    }
}

impl<T> Clone for FactoryHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for FactoryHandle<T> {}

impl<T> std::fmt::Debug for FactoryHandle<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("FactoryHandle")
            .field(&self.pointer)
            .finish()
    }
}

impl<T> PartialEq for FactoryHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.pointer == other.pointer
    }
}

impl<T> Eq for FactoryHandle<T> {}

impl<T> std::hash::Hash for FactoryHandle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.pointer.hash(state);
    }
}

// SAFETY: `FactoryHandle` only carries pointer identity and provides no safe
// access to `T`. The cache mutex governs lifetime operations; dereferencing the
// raw ABI pointer remains the explicit responsibility of unsafe FFI code.
unsafe impl<T> Send for FactoryHandle<T> {}
// SAFETY: same reasoning as the `Send` implementation; shared handle access
// cannot read or mutate the pointee through the safe API.
unsafe impl<T> Sync for FactoryHandle<T> {}

/// Result of releasing one externally acquired factory handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryRelease {
    /// The pointer is not owned by this cache.
    NotFound,
    /// Another acquired handle still keeps the factory alive.
    Retained,
    /// This was the last handle; instances and factory were dropped.
    Removed,
}

struct FactoryEntry<T, I> {
    // Keep instances first so Rust drops them before their parent factory.
    instances: HashMap<FactoryHandle<I>, Box<I>>,
    factory: Box<T>,
    external_references: usize,
}

/// SHA-keyed owner for FFI factories and their live DSP instances.
///
/// This mirrors the maintained Faust C++ lifecycle:
///
/// - first creation inserts one externally releasable handle;
/// - repeated creation and SHA lookup coalesce and acquire another handle;
/// - deletion returns `Removed` only for the last handle;
/// - final deletion drops any instances not manually deleted;
/// - clearing invalidates every outstanding factory and instance pointer.
///
/// Lifecycle operations are always protected by a mutex. The public
/// `startMTDSPFactories`/`stopMTDSPFactories` compatibility flag is retained,
/// but Rust provides the stronger guarantee that cache operations remain
/// synchronized in both modes.
pub struct FactoryCache<T, I> {
    map: Mutex<HashMap<String, FactoryEntry<T, I>>>,
    mt_mode: AtomicBool,
}

impl<T, I> Default for FactoryCache<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I> FactoryCache<T, I> {
    /// Creates an empty cache with the compatibility MT flag disabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            mt_mode: AtomicBool::new(false),
        }
    }

    /// Inserts a new owned factory or coalesces with an existing SHA entry.
    ///
    /// Coalescing drops the candidate and increments the existing entry's
    /// external reference count. `None` is returned only on counter overflow.
    #[must_use]
    pub fn insert_or_acquire(&self, sha: &str, factory: T) -> Option<FactoryHandle<T>> {
        let mut guard = self.map.lock().unwrap();
        if let Some(entry) = guard.get_mut(sha) {
            entry.external_references = entry.external_references.checked_add(1)?;
            return FactoryHandle::from_raw(entry.factory.as_mut());
        }

        let mut factory = Box::new(factory);
        let handle = FactoryHandle::from_raw(factory.as_mut())
            .expect("Box always yields a non-null factory pointer");
        guard.insert(
            sha.to_owned(),
            FactoryEntry {
                instances: HashMap::new(),
                factory,
                external_references: 1,
            },
        );
        Some(handle)
    }

    /// Looks up a SHA entry and acquires one externally releasable handle.
    #[must_use]
    pub fn lookup_acquire(&self, sha: &str) -> Option<FactoryHandle<T>> {
        let mut guard = self.map.lock().unwrap();
        let entry = guard.get_mut(sha)?;
        entry.external_references = entry.external_references.checked_add(1)?;
        FactoryHandle::from_raw(entry.factory.as_mut())
    }

    /// Releases one handle and drops the entry on its last external reference.
    #[must_use]
    pub fn release(&self, handle: FactoryHandle<T>) -> FactoryRelease {
        let mut guard = self.map.lock().unwrap();
        let Some(key) = guard.iter().find_map(|(key, entry)| {
            std::ptr::eq(entry.factory.as_ref(), handle.as_ptr().cast_const()).then(|| key.clone())
        }) else {
            return FactoryRelease::NotFound;
        };

        let entry = guard
            .get_mut(&key)
            .expect("key originated from the same locked map");
        if entry.external_references > 1 {
            entry.external_references -= 1;
            return FactoryRelease::Retained;
        }

        let removed = guard
            .remove(&key)
            .expect("key originated from the same locked map");
        drop(guard);
        drop(removed);
        FactoryRelease::Removed
    }

    /// Transfers one DSP instance into its factory's ownership list.
    ///
    /// Returns null and drops the instance if the factory is no longer cached.
    #[must_use]
    pub fn register_instance(&self, factory: FactoryHandle<T>, instance: I) -> *mut I {
        let mut guard = self.map.lock().unwrap();
        let Some(entry) = guard
            .values_mut()
            .find(|entry| std::ptr::eq(entry.factory.as_ref(), factory.as_ptr().cast_const()))
        else {
            // Host-backed instance destructors may call arbitrary callbacks;
            // never run them while the process-global cache mutex is held.
            drop(guard);
            drop(instance);
            return std::ptr::null_mut();
        };

        let mut instance = Box::new(instance);
        let pointer = FactoryHandle::from_raw(instance.as_mut())
            .expect("Box always yields a non-null instance pointer");
        entry.instances.insert(pointer, instance);
        pointer.as_ptr()
    }

    /// Removes and drops one manually deleted DSP instance.
    #[must_use]
    pub fn remove_instance(&self, pointer: *mut I) -> bool {
        let Some(pointer) = FactoryHandle::from_raw(pointer) else {
            return false;
        };
        let mut guard = self.map.lock().unwrap();
        let removed = guard
            .values_mut()
            .find_map(|entry| entry.instances.remove(&pointer));
        drop(guard);
        let found = removed.is_some();
        drop(removed);
        found
    }

    /// Drops all instances and factories regardless of outstanding handles.
    pub fn clear(&self) {
        let mut guard = self.map.lock().unwrap();
        let removed = std::mem::take(&mut *guard);
        drop(guard);
        drop(removed);
    }

    /// Returns all cached SHA keys in deterministic order.
    #[must_use]
    pub fn all_sha_keys(&self) -> Vec<String> {
        let guard = self.map.lock().unwrap();
        let mut keys: Vec<String> = guard.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Enables the public MT compatibility mode.
    #[must_use]
    pub fn start_mt(&self) -> bool {
        self.mt_mode.store(true, Ordering::SeqCst);
        true
    }

    /// Disables the public MT compatibility mode.
    pub fn stop_mt(&self) {
        self.mt_mode.store(false, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn reference_count(&self, handle: FactoryHandle<T>) -> Option<usize> {
        let guard = self.map.lock().unwrap();
        guard.values().find_map(|entry| {
            std::ptr::eq(entry.factory.as_ref(), handle.as_ptr().cast_const())
                .then_some(entry.external_references)
        })
    }

    #[cfg(test)]
    fn instance_count(&self, handle: FactoryHandle<T>) -> Option<usize> {
        let guard = self.map.lock().unwrap();
        guard.values().find_map(|entry| {
            std::ptr::eq(entry.factory.as_ref(), handle.as_ptr().cast_const())
                .then_some(entry.instances.len())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};

    use super::{FactoryCache, FactoryRelease};

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct OrderedDrop {
        label: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Drop for OrderedDrop {
        fn drop(&mut self) {
            self.order.lock().unwrap().push(self.label);
        }
    }

    #[test]
    fn repeated_insert_and_lookup_share_one_owned_factory() {
        let drops = Arc::new(AtomicUsize::new(0));
        let cache = FactoryCache::<DropProbe, DropProbe>::new();
        let first = cache
            .insert_or_acquire("sha", DropProbe(Arc::clone(&drops)))
            .unwrap();
        let repeated = cache
            .insert_or_acquire("sha", DropProbe(Arc::clone(&drops)))
            .unwrap();
        let lookup = cache.lookup_acquire("sha").unwrap();

        assert_eq!(first, repeated);
        assert_eq!(first, lookup);
        assert_eq!(cache.reference_count(first), Some(3));
        assert_eq!(drops.load(Ordering::SeqCst), 1, "candidate is dropped");
        assert_eq!(cache.release(repeated), FactoryRelease::Retained);
        assert_eq!(cache.release(lookup), FactoryRelease::Retained);
        assert_eq!(cache.release(first), FactoryRelease::Removed);
        assert_eq!(drops.load(Ordering::SeqCst), 2, "owned factory is dropped");
    }

    #[test]
    fn final_release_and_clear_drop_registered_instances() {
        let factory_drops = Arc::new(AtomicUsize::new(0));
        let instance_drops = Arc::new(AtomicUsize::new(0));
        let cache = FactoryCache::<DropProbe, DropProbe>::new();
        let first = cache
            .insert_or_acquire("first", DropProbe(Arc::clone(&factory_drops)))
            .unwrap();
        let instance = cache.register_instance(first, DropProbe(Arc::clone(&instance_drops)));
        assert!(!instance.is_null());
        assert_eq!(cache.instance_count(first), Some(1));
        assert!(cache.remove_instance(instance));
        assert_eq!(instance_drops.load(Ordering::SeqCst), 1);

        let second_instance =
            cache.register_instance(first, DropProbe(Arc::clone(&instance_drops)));
        assert!(!second_instance.is_null());
        assert_eq!(cache.release(first), FactoryRelease::Removed);
        assert_eq!(instance_drops.load(Ordering::SeqCst), 2);

        let second = cache
            .insert_or_acquire("second", DropProbe(Arc::clone(&factory_drops)))
            .unwrap();
        assert!(
            !cache
                .register_instance(second, DropProbe(Arc::clone(&instance_drops)))
                .is_null()
        );
        cache.clear();
        assert_eq!(factory_drops.load(Ordering::SeqCst), 2);
        assert_eq!(instance_drops.load(Ordering::SeqCst), 3);
        assert_eq!(cache.release(second), FactoryRelease::NotFound);
    }

    #[test]
    fn final_release_drops_instances_before_their_factory() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let cache = FactoryCache::<OrderedDrop, OrderedDrop>::new();
        let factory = cache
            .insert_or_acquire(
                "sha",
                OrderedDrop {
                    label: "factory",
                    order: Arc::clone(&order),
                },
            )
            .unwrap();
        assert!(
            !cache
                .register_instance(
                    factory,
                    OrderedDrop {
                        label: "instance",
                        order: Arc::clone(&order),
                    },
                )
                .is_null()
        );

        assert_eq!(cache.release(factory), FactoryRelease::Removed);
        assert_eq!(*order.lock().unwrap(), ["instance", "factory"]);
    }

    #[test]
    fn concurrent_lookup_and_release_preserve_a_live_reference() {
        for _ in 0..128 {
            let cache = Arc::new(FactoryCache::<usize, ()>::new());
            let initial = cache.insert_or_acquire("sha", 7).unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let worker_cache = Arc::clone(&cache);
            let worker_barrier = Arc::clone(&barrier);
            let worker = std::thread::spawn(move || {
                worker_barrier.wait();
                worker_cache.lookup_acquire("sha").map(|handle| {
                    assert_eq!(*unsafe { handle.as_ptr().as_ref().unwrap() }, 7);
                    worker_cache.release(handle)
                })
            });

            barrier.wait();
            let initial_release = cache.release(initial);
            let worker_release = worker.join().unwrap();
            match (initial_release, worker_release) {
                (FactoryRelease::Removed, None)
                | (FactoryRelease::Removed, Some(FactoryRelease::Retained))
                | (FactoryRelease::Retained, Some(FactoryRelease::Removed)) => {}
                unexpected => panic!("unexpected concurrent lifecycle result: {unexpected:?}"),
            }
        }
    }
}
