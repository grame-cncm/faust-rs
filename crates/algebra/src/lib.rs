#![forbid(unsafe_code)]

//! Algebra support crate for Faust compiler passes.
//!
//! # Source provenance (C++)
//! - The porting plan maps `compiler/FaustAlgebra/FaustAlgebra.hh` here: an
//!   abstract per-primitive algebra that the C++ interval algebra derives from.
//!   It is not ported as such; the Rust interval operators are free functions
//!   in `interval::ops`, called by the typing rules of `sigtype`, and algebraic
//!   simplification lives in `normalize`.
//!
//! # Intended role
//! - Host reusable algebraic identities and canonicalization helpers shared by
//!   compiler passes.
//!
//! # Current status
//! - Scaffold only: the crate exposes only a stable crate identifier, and no
//!   workspace crate depends on it.
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
