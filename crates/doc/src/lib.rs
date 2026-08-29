#![forbid(unsafe_code)]

//! Documentation and report-integration crate placeholder.
//!
//! # Intended role
//! - Host programmatic documentation builders shared by developer tooling
//!   (`xtask`, phase reports, parity dashboards).
//! - Provide typed renderers/formatters for markdown or machine-readable
//!   project status outputs.
//!
//! # Current status
//! - Scaffold only. No runtime/reporting APIs are stabilized yet.
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
