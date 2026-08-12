//! Unified C/C++ FFI distribution crate.
//!
//! This crate owns the canonical `libfaust-rs` artifacts (`staticlib` + `cdylib`).
//! Backend-specific FFI crates (`interp-ffi`, `cranelift-ffi`, `box-ffi`,
//! `signal-ffi`) are linked as Rust libraries and their exported `extern "C"`
//! symbols are distributed through this single top-level library.

// Same allocator as the `faust-rs` binary, for the same measured reason: the
// platform allocator is roughly half of propagation self time on macOS, and a
// `libfaust-rs` host gets the identical compiler workload.
//
// Permitted here where it would not be in an rlib: this crate is a
// cdylib/staticlib, so it is the final artifact for its own Rust code. It is
// sound because the FFI memory contract never crosses allocators — every
// pointer returned to a host comes from `CString::into_raw` and goes back
// through the exported `freeCMemory`, and no path frees host-allocated memory.
//
// The one shape to keep in mind is `staticlib` linked into a *Rust* host that
// also selects an allocator: the allocator shim symbols are not per-crate, so
// that combination conflicts at link time. Linking a Rust staticlib into a Rust
// binary already duplicates std and is not a supported use of this crate.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Box manipulation C API surface.
pub use box_ffi as box_api;
/// Cranelift backend C API surface.
pub use cranelift_ffi as cranelift;
/// Interpreter backend C API surface.
pub use interp_ffi as interp;
/// Backend-agnostic libfaust C API surface (`expandDSP*`, `generateAuxFiles*`,
/// `generateSHA1`).
pub use libfaust_ffi as libfaust;
/// Signal manipulation C API surface.
pub use signal_ffi as signal_api;
