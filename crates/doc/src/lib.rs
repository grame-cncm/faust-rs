#![forbid(unsafe_code)]

//! Mathematical documentation crate placeholder.
//!
//! # Intended role
//! - Port of the C++ automatic mathematical documentation generator,
//!   `compiler/documentator/` (the `-mdoc` option), as mapped by the porting
//!   plan.
//!
//! # Current status
//! - Not ported: empty scaffold, with no dependents and no `-mdoc` CLI option.
//!
//! # API mapping status
//! - `crate_id()` is `adapted` utility metadata (no direct C++ counterpart).

/// Stable identifier used by workspace-level crate discovery.
pub const CRATE_NAME: &str = "doc";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
