#![forbid(unsafe_code)]

//! Signal normalization and algebraic simplification.
//!
//! Ported from `compiler/normalize/` in the Faust C++ compiler.
//!
//! # Architecture
//!
//! The normalization modules follow a five-layer dependency order:
//!
//! ```text
//! normalform   ← promotion, simplification entry points, normal-form preparation
//!   simplify   ← memoized rewrite engine
//!     normalize  ← add-term + delay-term normalization
//!       aterm    ← additive term (sum of mterms)
//!         mterm  ← multiplicative term (k · x^n · y^m / …)
//! ```
//!
//! Two further passes stand beside them: [`rec_merge`] (merging isomorphic
//! `SYMREC` groups, no C++ counterpart) and [`table_promote`] (`-ct` table
//! index clamping).
//!
//! # Callers
//! - `transform::signal_prepare` runs promotion, simplification, `rec_merge`
//!   and `table_promote` between its re-typings, on every compilation.
//! - `eval` folds constant expressions with [`try_simplify_const`] /
//!   [`try_simplify_const_cached`].
//!
//! # API mapping status
//! - `crate_id()` is `adapted` utility metadata (no direct C++ counterpart).

pub(crate) mod aterm;
pub(crate) mod mterm;
pub mod normalform;
pub(crate) mod normalize;
pub mod rec_merge;
pub(crate) mod simplify;
pub(crate) mod sum_chain;
pub mod table_promote;

pub use mterm::DivisionByZero;
pub use rec_merge::merge_isomorphic_symrec_groups;
pub use simplify::{
    ConstSimplifyCache, simplify_const, try_simplify_const, try_simplify_const_cached,
};
pub use table_promote::{TableRangeWarning, promote_table_signals};

pub const CRATE_NAME: &str = "normalize";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
