//! Mid-level transform passes between signal propagation and backend emission.
//!
//! # Source provenance (C++)
//! - `compiler/transform/*`
//! - `compiler/generator/dag_instructions_compiler.cpp` / `compile_vect.cpp`
//!   (vector loop DAG and delay-word lowering)
//! - `compiler/generator/compile_scal.cpp` (scalar and `ondemand` lowering)
//! - `compiler/Dependencies/*`, `compiler/generator/occurrences.cpp`
//!   (dependency/occurrence rules)
//!
//! # Role in pipeline
//! `propagate → [signal_prepare] → [clk_env / hgraph / schedule] →
//! [signal_fir] → fir → codegen`
//!
//! The crate owns two production paths behind one facade:
//! - the **scalar** path: prepared forest → clock/dependency analysis →
//!   scheduled scalar lowering;
//! - the **checked vector** path (`-vec`): a producer/checker pipeline whose
//!   artifacts must each pass an independent checker before the emitted
//!   module is accepted; any named unsupported shape fails closed to scalar
//!   lowering with a stable, observable fallback reason.
//!
//! # Module map
//! - [`signal_prepare`] — arena-owning staging/verification boundary.
//! - [`clk_env`] — clock-environment inference for
//!   `ondemand`/`upsampling`/`downsampling` domains.
//! - [`hgraph`] — hierarchical dependency graph, effect orientation, audits.
//! - [`schedule`] — dependency scheduling shared by both paths
//!   ([`schedule::SchedulingStrategy`], `-ss 0..3`).
//! - [`signal_fir`] — signal→FIR lowering, vector selection, and fallback
//!   policy; see `signal_fir/vector` for the checked pipeline's stage map.
//!
//! # API mapping status
//! - `signal_fir` and `signal_prepare` public entry points are `adapted`:
//!   parity-driven behavior with Rust typed errors/options.
//! - `clk_env`/`hgraph`/`schedule` internals are `adapted` analysis stages
//!   exposed for diagnostics and workspace tests.

// `missing_docs` must be `deny`, not `warn`: an inner `#![warn(...)]`
// attribute overrides the command-line `-D warnings` clippy and CI already
// pass, so a plain `warn` here is invisible to every existing gate. `deny`
// makes an undocumented `pub` item fail `cargo build`/`check`/`clippy`/`test`
// directly, no extra command needed. Verified 2026-08-18 with a rejecting
// mutation: an undocumented `pub struct` compiled clean under `warn` and was
// rejected only after switching to `deny`.
#![deny(missing_docs)]

pub mod clk_env;
pub mod hgraph;
pub mod schedule;
pub mod signal_fir;
pub mod signal_prepare;

/// Stable crate identifier.
pub const CRATE_NAME: &str = "transform";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
