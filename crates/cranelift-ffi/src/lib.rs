//! `cranelift-ffi` — C/C++ FFI export for the Cranelift backend.
//!
//! # Purpose
//! - Host the exported `cranelift_dsp` / `cranelift_dsp_factory` C and C++
//!   runtime API surface required by the Cranelift backend plan.
//! - Mirror the overall strategy used by `llvm_dsp` / `interpreter_dsp`
//!   families (factory cache + instance lifecycle + UI/meta callbacks).
//!
//! # Current status
//! - Runtime compute path is wired for file/string constructors:
//!   FIR -> Cranelift JIT -> callable instance `compute`.
//! - UI/meta/sample-rate/reset behavior is derived from FIR into a native
//!   runtime descriptor; no interpreter sidecar is used in the normal
//!   Cranelift factory/instance path anymore.
//! - Remaining deferred families are tracked in the Cranelift backend plan
//!   and parity matrix.
//!
//! # Planned modules
//! - [`types`] — opaque FFI wrapper types and callback glue structs.
//! - [`cache`] — global factory cache.
//! - [`factory`] — factory lifecycle and source compilation entry points.
//! - [`instance`] — instance lifecycle and `compute`/UI/meta dispatch entry points.
//! - `clif` — textual `.clif` container helpers used by V1 bitcode APIs.
//! - `runtime` — native runtime descriptor builder shared by factories/instances.

#![allow(unsafe_code)] // Future FFI implementation will require raw pointers.
#![allow(non_snake_case)] // FFI parity requires preserving C API symbol names.

pub mod cache;
pub(crate) mod clif;
pub mod factory;
pub mod instance;
pub mod probe;
pub(crate) mod runtime;
pub mod types;

#[cfg(test)]
mod diff;

#[cfg(test)]
/// Serializes integration-style tests that exercise global caches/JIT state.
///
/// Several `cranelift-ffi` tests mutate process-global state such as the
/// factory cache or imported JIT symbol tables. This helper provides one
/// coarse mutex so those tests can opt into serialized execution without
/// exposing test-only synchronization in the public API.
pub(crate) fn test_serial_guard() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("test mutex")
}
