//! `faust-probe` — generic DSP probing: set controls, render offline, measure.
//!
//! # Why this lives here
//! `impulse-runner` can be its own crate because the interpreter it drives is
//! safe Rust reachable from the compiler core. Instantiating and calling
//! Cranelift-compiled code is not: it needs the factory/instance handles and
//! the `UIGlue` callback table this crate exports. The workspace layering
//! contract (`cargo run -p xtask -- ffi-boundary-check`) forbids a core crate
//! from depending rightward on an FFI adapter, which settles the question the
//! design left open: the probe belongs beside `impulse_cranelift`, inside the
//! adapter that owns the runtime it drives.
//!
//! # Why a third runner
//! `impulse-runner` and `impulse_cranelift` answer *did the behaviour change?*
//! They render a fixed protocol — sample rate 44100, block 64, an impulse on
//! frame 0, buttons held for the first block only, sliders left at their
//! defaults — and emit `.ir` for `filesCompare -part` against the C++ oracle.
//! The protocol is fixed by construction: regression testing requires that
//! nothing varies.
//!
//! Neither can answer *is the behaviour correct?*, which needs the opposite:
//! varying exactly the thing under test. Comparing a ladder filter against its
//! analytic transfer function needs a chosen cutoff and resonance; measuring a
//! release time needs a gate held for tens of thousands of samples; observing a
//! ping-pong delay needs an impulse on one channel alone.
//!
//! Cranelift rather than the interpreter because rendering there is roughly
//! five times faster once JIT cost is amortised, which is what makes parameter
//! sweeps inside one process worthwhile rather than merely possible.
//!
//! Full rationale, phases and validation criteria:
//! `porting/faust-probe-generic-test-tool-design-2026-08-14-en.md`.
//!
//! # Layout
//! - [`params`] — control discovery, the `UIGlue` walk producing a path map.
//! - [`render`] — excitation and reductions; FFI-free, so unit-testable.
//! - [`engine`] — JIT lifecycle and the block loop.

pub mod engine;
pub mod params;
pub mod render;
