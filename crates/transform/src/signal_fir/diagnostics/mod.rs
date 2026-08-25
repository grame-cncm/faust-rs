//! Observation-only diagnostic surfaces — never on the production path.
//!
//! Nothing in this tree influences planning, scheduling, lowering, or the
//! emitted FIR. Every module here either re-derives facts for comparison or
//! exercises a frozen prototype, and is consumed only by workspace
//! integration tests and explicit diagnostic entry points:
//!
//! - [`pv_slice`] — the frozen `PV` proof-of-concept vector pre-slice
//!   (consumed by `crates/compiler/tests/pv_vector_slice.rs`); the real
//!   `-vec` pipeline lives in [`super::vector`].
//! - [`shadow`] — schedule-conformance reports comparing the recorded
//!   first-lowering order with the selected hierarchical schedule
//!   (consumed by `crates/compiler/tests/p3_shadow_mode.rs` and the
//!   `FAUST_RS_SHADOW_REPORT` environment variable).
//!
//! When deciding whether code is load-bearing, a path under `diagnostics/`
//! is the answer: it is not.

pub mod pv_slice;
pub mod shadow;
