//! Signal normalization and algebraic simplification.
//!
//! Ported from `compiler/normalize/` in the Faust C++ compiler.
//!
//! # Architecture
//!
//! The normalization pipeline follows a five-layer dependency order:
//!
//! ```text
//! normalform   ← pipeline coordinator (Phase 1: de-Bruijn → symbolic → typed → promoted)
//!   simplify   ← memoized rewrite engine
//!     normalize  ← add-term + delay-term normalization
//!       aterm    ← additive term (sum of mterms)
//!         mterm  ← multiplicative term (k · x^n · y^m / …)
//! ```
//!
//! # Current status
//! - `mterm`: complete.
//! - `aterm`, `normalize`, `simplify`, `normalform`: in progress.
//!
//! # API mapping status
//! - `crate_id()` is `adapted` utility metadata (no direct C++ counterpart).

pub(crate) mod aterm;
pub(crate) mod mterm;
pub mod normalform;
pub(crate) mod normalize;
pub mod rec_merge;
pub(crate) mod simplify;
pub mod table_promote;

pub use rec_merge::merge_isomorphic_symrec_groups;
pub use simplify::simplify_const;
pub use table_promote::{TableRangeWarning, promote_table_signals};

pub const CRATE_NAME: &str = "normalize";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
