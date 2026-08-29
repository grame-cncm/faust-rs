#![forbid(unsafe_code)]

//! Box construction helpers backed by `tlib::TreeArena`.
//!
//! # Source provenance (C++)
//! - `compiler/boxes/boxes.hh`
//! - `compiler/boxes/boxes.cpp`
//!
//! # Public API mapping status
//! - Public construction API is `BoxBuilder`, which keeps 1:1 semantic mapping with
//!   the C++ box families (`box*` constructors in `compiler/boxes/boxes.hh/.cpp`).
//! - Public inspection API is `match_box` + `BoxMatch`.
//! - Legacy `node_*` / `is_node_*` helpers are kept internal to this crate.
//!
//! # Parity invariants
//! - Box nodes are represented as tagged trees with deterministic child order.
//! - Labels/identifiers are carried as `NodeKind::Symbol`.
//! - UI slider parameter payload keeps Faust list encoding (`list4(cur,min,max,step)`).
//!
//! # Integer convention
//! - Public box integer constructors/matchers are `i32`-based (`boxInt` parity).
//! - Storage remains `tlib::NodeKind::Int(i64)` internally; boundary conversion
//!   is explicit in this crate.

use tlib::TreeId;

mod builder;
mod complexity;
mod dump;
pub(crate) mod internals;
mod matcher;
mod print;
pub(crate) mod tags;

pub use builder::BoxBuilder;
pub use complexity::box_complexity;
pub use dump::dump_box;
pub use matcher::{BoxMatch, match_box};
pub use print::{BoxPrintError, FloatSize, SharedBoxProgram, box_pp, box_pp_shared, format_real};

/// Stable crate identifier used in workspace-level tooling and diagnostics.
pub const CRATE_NAME: &str = "boxes";

/// Box node identifier in `TreeArena`.
pub type BoxId = TreeId;

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}
