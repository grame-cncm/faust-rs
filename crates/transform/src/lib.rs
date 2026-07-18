//! Mid-level transform passes over signals and FIR.
//!
//! # Source provenance (C++)
//! - `compiler/transform/*`
//! - selected lowering logic from `compiler/generator/*` for FIR-oriented paths
//!
//! # Role in pipeline
//! - Hosts transformations that are neither parser/eval/propagate concerns nor
//!   backend emitters.
//! - Current active slice is signal-to-FIR lowering under [`signal_fir`].
//!
//! # Current status
//! - [`signal_fir`] is implemented for the fast-lane prototype and exercised by
//!   integration tests/golden checks.
//! - [`signal_prepare`] now owns the pre-FIR staging boundary used to clone the
//!   output forest and run forest-wide `de_bruijn_to_sym` conversion.
//! - Additional transform families (scheduling/vectorization/rewrites) are
//!   planned but not yet exposed as stable public APIs.
//!
//! # API mapping status
//! - `signal_fir` public entry points are `adapted`: parity-driven behavior with
//!   Rust typed errors/options.

pub mod clk_env;
pub mod hgraph;
pub mod schedule;
pub mod signal_fir;
pub mod signal_prepare;

pub const CRATE_NAME: &str = "transform";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
