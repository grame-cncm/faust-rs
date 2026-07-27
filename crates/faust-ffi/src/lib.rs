//! Unified C/C++ FFI distribution crate.
//!
//! This crate owns the canonical `libfaust-rs` artifacts (`staticlib` + `cdylib`).
//! Backend-specific FFI crates (`interp-ffi`, `cranelift-ffi`, `box-ffi`,
//! `signal-ffi`) are linked as Rust libraries and their exported `extern "C"`
//! symbols are distributed through this single top-level library.

/// Box manipulation C API surface.
pub use box_ffi as box_api;
/// Cranelift backend C API surface.
pub use cranelift_ffi as cranelift;
/// Interpreter backend C API surface.
pub use interp_ffi as interp;
/// Signal manipulation C API surface.
pub use signal_ffi as signal_api;
