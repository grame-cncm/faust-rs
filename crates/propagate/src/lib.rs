//! Box-to-signal propagation (Phase 4, section 2.4).
//!
//! # Source provenance (C++)
//! - `compiler/propagate/propagate.hh`
//! - `compiler/propagate/propagate.cpp`
//! - `compiler/boxes/boxtype.cpp`
//!
//! # Current scope
//! - Core box arity inference for supported box families.
//! - Primitive lowering from `boxes::BoxMatch` to `signals::SigBuilder`.
//! - Composition algebra: `seq`, `par`, `split`, `merge`.
//! - Explicit typed errors for unsupported nodes and arity mismatches.
//! - Recursive composition lowering with De Bruijn-style placeholders (`DEBRUIJNREC`/`DEBRUIJNREF` tag nodes, converted to `sigRec`/`sigProj` by `signal_prepare`).
//! - Typed `FlatBoxId` boundary that validates the post-`eval/a2sb` flat box subset.
//!
//! # Public API mapping status
//! - [`box_arity_typed`] and [`propagate_typed`] are the primary Rust entry
//!   points for the post-`eval/a2sb` flat-box contract.
//! - [`PropagateOutput`], [`propagate_typed_with_ui`], and [`propagate_typed_with_ui_options`]
//!   are the grouped-UI ownership extensions introduced by the UI IR rewrite.
//! - `make_sig_input_list(...)` mirrors C++ `makeSigInputList(...)`.
//! - `FlatBoxId` / [`try_build_flat_box`] are an adapted Rust boundary: they make the
//!   C++ post-`evalprocess -> a2sb -> propagate` flat-box contract explicit while
//!   preserving `TreeArena` node sharing through `TreeId`.
//! - `route`, `ffun`, `soundfile`, `ondemand`, `upsampling`, and
//!   `downsampling` now lower through the typed flat boundary.
//!
//! # Integer convention
//! - Integer signals emitted by this pass are `i32`-semantic.
//! - Conversions from container sizes/indices (`usize`) are explicit and
//!   fallible to preserve deterministic diagnostics on overflow.
//!
//! # Forward-mode automatic differentiation (FAD)
//!
//! When `box_tree` contains a `fad(expr, seed)` node, propagation expands the
//! primal output bundle into:
//! ```text
//! [primal₀, ∂primal₀/∂s₀, ∂primal₀/∂s₁, …,
//!  primal₁, ∂primal₁/∂s₀, ∂primal₁/∂s₁, …]
//! ```
//! where `s₀, s₁, …` are the outputs of the `seed` box (one independent
//! differentiation variable per lane), in the order the seed produces them.
//! A single-output seed degenerates to the canonical `[primal, tangent]`
//! pair; multi-output seeds bundle several independent variables through a
//! single `fad` node.
//!
//! ## Output arity
//!
//! [`box_arity_typed`] computes expanded arity:
//! ```text
//! outputs = body_outputs × (1 + seed_outputs)
//! ```
//! This matches the C++ `getBoxType` logic (`compiler/boxes/boxtype.cpp:371`)
//! for the single-seed case.
//!
//! Under the explicit-seed model, `[autodiff:false]` metadata is parsed but
//! does not gate differentiation; the seed list alone decides which signals
//! are differentiated.
//!
//! ## Differentiation algorithm
//!
//! Implemented in the internal `forward_ad` module. Each primal output is differentiated
//! independently for every seed; the full rule table (constants, BinOp,
//! transcendentals, delays, recursion, …) lives in the `forward_ad` module
//! doc.
//!
//! Key algorithmic points:
//! - FAD runs directly on de Bruijn-form recursion nodes (`DEBRUIJNREC` /
//!   `DEBRUIJNREF`); the `de_bruijn_to_sym` conversion is deferred to
//!   `signal_prepare`, where it runs once over all process outputs so
//!   shared sub-terms keep a single symbolic name across primal and tangent
//!   lanes.
//! - One internal `ForwardADTransform` instance per seed; a memoization
//!   cache prevents exponential blow-up on reused DAG subgraphs and breaks
//!   recursion cycles.
//! - Seed recognition is `SigId` equality: the transform short-circuits at
//!   any node whose `SigId` matches the seed and never descends into the
//!   seed's own recursive body.
//!
//! ## Interaction with the `Rec` combinator
//!
//! Recursive boxes (`sigRec`) require special treatment because there are now
//! two distinct valid FAD modes in recursion:
//!
//! 1. **Expand-after-Rec** — when a `ForwardAD` node is structurally present in
//!    a recursive branch but none of its expanded outputs are consumed locally
//!    before the `Rec` boundary, branch propagation keeps it arity-transparent.
//!    `box_arity_wiring` is used for the internal port algebra, and
//!    `forward_ad::generate_fad_signals_multi(...)` runs after the recursive
//!    group has been built.
//! 2. **Augmented-state Rec** — when a recursive branch locally consumes
//!    `[primal, tangent]` outputs (for example `fad(loss, prev) : !, _` inside
//!    the feedback function), the `Rec` must propagate on the real expanded AD
//!    arity. In that mode the recursive group itself carries augmented
//!    primal+tangent lanes and no post-`Rec` expansion step is performed.
//!
//! ## Reverse-mode AD (`rad`)
//!
//! `rad(expr, seeds)` lowers through the internal `reverse_ad` module. Feed-forward bodies use a
//! local symbolic reverse sweep and produce `[primals…, gradients…]` with an
//! implicit all-ones cotangent over primal outputs. Temporal and recursive
//! bodies leave that symbolic sweep and are routed to the `BlockReverseAD`
//! finite-block fallback; hard unsupported families still surface typed
//! diagnostics.

use std::fmt::{Display, Formatter};

use ahash::{AHashMap, AHashSet};
use boxes::{BoxId, BoxMatch, match_box};
use errors::codes;
use errors::{Diagnostic, IntoDiagnostic, Severity, Stage};
use signals::{SigBuilder, SigId, SigMatch, match_sig};
use tlib::{
    NodeKind, TreeArena, TreeId, de_bruijn_aperture_with_memo, list_to_vec, tree_to_int,
    vec_to_list,
};
use ui::{
    ControlId, ControlKind, ControlRange, ControlSpec, UiGroupKind, UiGroupPathSegment,
    UiGroupSpec, UiMatch, UiMetadata, UiNormalizedGroupPath, UiProgram, UiProgramBuilder,
    UiRootOrigin, canonicalize_group_spec, match_ui, normalize_group_label_navigation,
    normalize_widget_label_path, split_label_metadata,
};

pub mod clock_domain;
mod forward_ad;
mod reverse_ad;
pub mod stateful_rad;
pub mod transpose_ad;

pub use clock_domain::{ClockDomain, ClockDomainId, ClockDomainKind, ClockDomainTable};

/// Memoization cache for [`box_arity_typed`] results, keyed by validated flat boxes.
pub type ArityCache = AHashMap<FlatBoxId, Result<BoxArity, PropagateError>>;
/// Environment mapping route/slot placeholders to propagated signals.
type SlotEnv = AHashMap<BoxId, SigId>;
/// Context-aware mapping from (source widget box node, group-path hash) to stable control ids.
/// The group-path hash distinguishes the same structural widget appearing in different UI groups.
type ControlIds = AHashMap<(BoxId, u64), ControlId>;

/// Computes a stable hash over a stack of [`UiGroupPathSegment`] values.
///
/// Used to distinguish widget nodes that share the same `BoxId` due to hash-consing but live
/// in different UI group contexts (e.g. two `hslider("X", …)` with identical parameters placed
/// inside different `hgroup`/`vgroup` wrappers).
fn group_path_hash(groups: &[UiGroupPathSegment]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = ahash::AHasher::default();
    groups.hash(&mut hasher);
    hasher.finish()
}

pub const CRATE_NAME: &str = "propagate";
const DEBRUIJNREC_TAG: &str = "DEBRUIJNREC";
const DEBRUIJNREF_TAG: &str = "DEBRUIJNREF";

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// Input/output arity of one box expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxArity {
    /// Number of required input signals.
    pub inputs: usize,
    /// Number of produced output signals.
    pub outputs: usize,
}

/// Explicit products of the post-`eval/a2sb` propagation boundary.
///
/// # Source provenance (C++)
/// - `compiler/propagate/propagate.cpp`
/// - `compiler/signals/signals.hh`
/// - `compiler/signals/signals.cpp`
///
/// Mapping status:
/// - `adapted` relative to the C++ clock-environment/path ownership.
/// - Behaviorally equivalent target: DSP signals and grouped UI become
///   explicit products of propagation instead of backend-local heuristics.
#[derive(Debug)]
pub struct PropagateOutput {
    /// Final propagated output signal list (`box_arity.outputs` items).
    pub signals: Vec<SigId>,
    /// Canonical grouped UI artifact extracted from the same propagated box
    /// tree.
    ///
    /// This is the Rust ownership split that replaces the earlier
    /// backend-local UI reconstruction heuristic: signals carry only control
    /// references, while grouped layout and metadata are owned here.
    pub ui: UiProgram,
    /// Clock-domain instances allocated by `ondemand` / `upsampling` /
    /// `downsampling` wrappers during this propagation run (roadmap P0.2).
    ///
    /// Empty for programs without clocked wrappers. In-graph `SIGCLOCKENV`
    /// tokens index into this table via [`ClockDomainId::from_u32`].
    pub clock_domains: ClockDomainTable,
}

/// Canonical grouped-UI construction policy applied during propagation.
///
/// Source provenance (C++):
/// - `compiler/generator/compile.cpp`
/// - `compiler/generator/instructions_compiler.cpp`
///
/// Parity note:
/// - when the root UI group has an empty label, C++ rewrites it to the
///   canonical compilation name (top-level `declare name` or source stem)
///   before backend emission.
/// - Rust threads that canonical root label into grouped UI construction so
///   `UiProgram` is already the source of truth before FIR/backend lowering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropagateUiOptions {
    /// Canonical label used when propagation must synthesize or rename the root
    /// group.
    ///
    /// This should already reflect the C++ root-label policy:
    /// `declare name` from the master document when present, otherwise source
    /// filename stem.
    pub synthesized_root_label: Box<str>,
}

impl PropagateUiOptions {
    #[must_use]
    /// Creates one grouped-UI construction policy with the provided root label.
    pub fn new(synthesized_root_label: impl Into<Box<str>>) -> Self {
        Self {
            synthesized_root_label: synthesized_root_label.into(),
        }
    }
}

mod api;
mod arity;
mod engine;
mod error;
mod flat;
mod ui_build;

pub use api::{propagate_typed, propagate_typed_with_ui, propagate_typed_with_ui_options};
pub use arity::{box_arity_typed, make_sig_input_list};
pub use error::PropagateError;
pub use flat::{FlatBoxBuildError, FlatBoxId, try_build_flat_box};

pub(crate) use arity::box_arity_wiring;
pub(crate) use engine::{
    PropagateContext, PropagateMemo, ffunction_arity, list_length, merge_compatible,
    propagate_in_slot_env, split_compatible, usize_from_int_node,
};
pub(crate) use flat::{
    FlatNodeKind, RecFadMode, contains_forward_ad, count_fad_nodes, flat_node_kind, rec_fad_mode,
};
pub(crate) use ui_build::{build_ui_program, decode_box_label};

#[cfg(test)]
mod tests;
