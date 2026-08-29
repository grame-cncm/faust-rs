#![forbid(unsafe_code)]

//! Algebra support crate for Faust compiler passes.
//!
//! # Source provenance (C++)
//! - Algebraic simplification and canonicalization logic is historically spread
//!   across `evaluate`, `normalize`, and typing/interval passes.
//!
//! # Intended role in pipeline
//! - Host reusable algebraic identities and normalization helpers shared by
//!   higher-level crates.
//! - Provide deterministic rewrite utilities that can be reused by
//!   `eval`/`normalize`/`interval` without duplicating rule tables.
//!
//! # Current status
//! - Phase scaffold only: no public rewrite API is stabilized yet.
//! - The crate currently exposes only a stable crate identifier so upper crates
//!   can declare dependencies without circular wiring.
//!
//! # API mapping status
//! - `crate_id()` is `adapted` (workspace/tooling utility), not a direct C++
//!   API counterpart.

/// Stable identifier used by workspace-level crate discovery.
pub const CRATE_NAME: &str = "algebra";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
