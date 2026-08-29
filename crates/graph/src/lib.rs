#![forbid(unsafe_code)]

//! Graph algorithms crate placeholder.
//!
//! # Intended role
//! - Provide shared graph data structures/algorithms (topological order,
//!   SCC detection, scheduling helpers) reused by transform/backend passes.
//! - Isolate graph-specific logic from IR crates to keep crate boundaries clear.
//!
//! # Current status
//! - Scaffold only. No graph API is stabilized yet.
//!
//! # API mapping status
//! - `crate_id()` is `adapted` utility metadata (no direct C++ counterpart).

/// Stable identifier used by workspace-level crate discovery.
pub const CRATE_NAME: &str = "graph";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
