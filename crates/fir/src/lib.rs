#![forbid(unsafe_code)]

//! FIR construction and matching helpers.
//!
//! # Source provenance (C++)
//! - `compiler/generator/instructions.hh`
//! - `compiler/generator/instructions_type.hh`
//! - `compiler/generator/instructions.cpp`
//! - `compiler/generator/fir/fir_code_checker.hh`
//!
//! # Public API mapping status
//! - Public construction API is [`FirBuilder`], aligned with the canonical
//!   `BoxBuilder` and `SigBuilder` style used in `crates/boxes` and
//!   `crates/signals`.
//! - Public inspection API is [`match_fir`] + [`FirMatch`].
//!
//! # Type model parity notes
//! - `FirType::UI`, `FirType::Sound`, and `FirType::Meta` represent the
//!   C++ FIR API handle layer historically spelled through pointer kinds
//!   (`kUI_ptr`, `kSound_ptr`, `kMeta_ptr`) in `instructions_type.hh`.
//! - Generic pointer nesting remains explicit with `FirType::Ptr(...)`
//!   (for example `FAUSTFLOAT**` is `Ptr(Ptr(FaustFloat))`).
//! - Canonical DSP API signatures should therefore use:
//!   - `metadata(Meta)` (pointer-shaped handle),
//!   - `buildUserInterface(UI)` (pointer-shaped handle),
//!   - `compute(Int32, Ptr(Ptr(FaustFloat)), Ptr(Ptr(FaustFloat)))`.
//!
//! # Parity invariants
//! - FIR nodes are represented as hash-consed trees in `tlib::TreeArena`.
//! - Identical FIR nodes are structurally shared automatically by interning.
//! - FIR value nodes carry explicit result types, so backend passes do not need
//!   a separate type-reconstruction phase.
//! - Dispatch is explicit and exhaustive via `match_fir`, no RTTI/dynamic-cast.

pub mod checker;
#[path = "../helpers.rs"]
pub mod helpers;
pub mod inliner;

use std::collections::HashSet;
use std::fmt::Write as _;

use tlib::{NodeKind, TreeArena, TreeId, tree_to_double, tree_to_int};

pub const CRATE_NAME: &str = "fir";

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// FIR node identifier in [`FirStore`].
pub type FirId = TreeId;

mod builder;
mod dump;
mod encoding;
mod matcher;
mod store;
pub mod subcontainer;
mod types;

pub use builder::FirBuilder;
pub use dump::{canonical_fir_fingerprint, dump_fir};
pub use matcher::{FirMatch, fir_match_children, match_fir};
pub use store::FirStore;
pub use types::{
    AccessType, BargraphType, ButtonType, FirBinOp, FirMathOp, FirType, NamedType, SliderRange,
    SliderType, UiBoxType,
};

pub(crate) use dump::child_ids;
pub(crate) use encoding::*;

#[cfg(test)]
mod tests;
