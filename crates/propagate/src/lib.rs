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
use diagnostics::codes;
use diagnostics::{Diagnostic, Severity, Stage, ToDiagnostic};
use signals::{SigBuilder, SigId, SigMatch, match_sig};
use tlib::{
    NodeKind, TreeArena, TreeId, de_bruijn_aperture_with_memo, list_to_vec, tree_to_int,
    tree_to_str, vec_to_list,
};
use ui::{
    ControlId, ControlKind, ControlRange, ControlSpec, UiGroupKind, UiGroupPathSegment,
    UiGroupSpec, UiMatch, UiMetadata, UiNormalizedGroupPath, UiProgram, UiProgramBuilder,
    UiRootOrigin, canonicalize_group_spec, match_ui, normalize_group_label_navigation,
    normalize_widget_label_path, split_label_metadata,
};

pub mod clock_domain;
mod context_id;
mod forward_ad;
mod profile;
mod result_memo;
mod reverse_ad;
pub mod stateful_rad;
pub mod transpose_ad;

pub use clock_domain::{ClockDomain, ClockDomainId, ClockDomainKind, ClockDomainTable};

/// Memoization cache for [`box_arity_typed`] results, keyed by validated flat boxes.
pub type ArityCache = AHashMap<FlatBoxId, Result<BoxArity, PropagateError>>;
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
    /// Source-neutral derivation links from generated signals to the Box nodes
    /// whose propagation produced them.
    ///
    /// The table is deliberately external to `TreeArena`: attaching origins to
    /// hash-consed Signal nodes would make source position part of semantic
    /// identity and would destroy DAG sharing. A shared `SigId` therefore owns
    /// an ordered set of candidate Box origins.
    pub signal_origins: SignalOrigins,
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

/// Ordered Box-origin candidates retained for generated Signal nodes.
///
/// C++ Faust relies on mutable Tree properties for comparable bookkeeping.
/// Rust uses this explicit side table so provenance ownership is visible,
/// testable, and independent from Signal hash-consing. Parser source locations
/// remain owned by `parser::BoxProvenance`; the compiler facade joins both
/// tables only when it builds a diagnostic.
#[derive(Clone, Debug)]
pub struct SignalOrigins {
    by_signal: AHashMap<SigId, Vec<BoxId>>,
    /// When `false`, every recording operation is a no-op and the table stays
    /// empty. See [`SignalOrigins::disabled`].
    recording: bool,
}

impl Default for SignalOrigins {
    fn default() -> Self {
        Self {
            by_signal: AHashMap::new(),
            recording: true,
        }
    }
}

impl SignalOrigins {
    /// Returns a table that records nothing.
    ///
    /// Provenance exists to build a diagnostic. A caller that discards the
    /// table cannot observe it, so building it is pure cost: every recording
    /// entry point below returns early, and in particular
    /// [`Self::record_derived_forest`] skips its DFS over the reachable signal
    /// forest, which propagation performs once per box node.
    ///
    /// This is what `propagate_typed` uses: it returns only the signal list, so
    /// the origins it used to accumulate were dropped unread. `eval` calls it
    /// for every constant fold (the C++ `boxPropagateSig` path), which made
    /// evaluation pay a full provenance walk per folded expression.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            by_signal: AHashMap::new(),
            recording: false,
        }
    }

    /// Returns `true` when this table accumulates recordings.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.recording
    }

    /// Maximum Box candidates retained per signal.
    ///
    /// Origins are bounded diagnostic evidence, not an exhaustive occurrence
    /// index. Hash-consed signals are shared by many boxes, so without a cap
    /// the per-pass `inherit_forest` unions accumulate candidate lists that
    /// grow with program size, and the linear-scan dedup in [`Self::record`]
    /// turns preparation super-quadratic. Diagnostic-time occurrence choice
    /// only ever consults the leading candidates.
    ///
    /// Matches the bound proposed in `faust-rs#15`, which targets `main`; this
    /// branch needs it too, and the correction plan's later steps cannot be
    /// measured until it is in place.
    pub const MAX_ORIGINS_PER_SIGNAL: usize = 8;

    /// Records that `signal` was produced while propagating `box_node`.
    ///
    /// Retains at most [`Self::MAX_ORIGINS_PER_SIGNAL`] candidates per signal,
    /// in first-recorded order.
    pub fn record(&mut self, signal: SigId, box_node: BoxId) {
        if !self.recording {
            return;
        }
        let origins = self.by_signal.entry(signal).or_default();
        if origins.len() < Self::MAX_ORIGINS_PER_SIGNAL && !origins.contains(&box_node) {
            origins.push(box_node);
        }
    }

    /// Records the same Box derivation for every signal in one output bus.
    pub fn record_outputs(&mut self, signals: &[SigId], box_node: BoxId) {
        if !self.recording {
            return;
        }
        for signal in signals {
            self.record(*signal, box_node);
        }
    }

    /// Records a Box origin for newly created nodes reachable from an output
    /// bus while preserving more specific origins already assigned by child
    /// propagation calls.
    /// Propagation calls this once per box node, innermost first, so the walk
    /// must stay proportional to what that box actually added rather than to
    /// everything below it.
    ///
    /// **Attribution closure.** When this returns, every node reachable from
    /// `signals` carries an origin. An already-attributed node was therefore
    /// covered by the inner call that attributed it, together with its whole
    /// reachable subgraph — and Signal nodes are hash-consed and immutable, so
    /// that subgraph cannot have grown since. Descending past such a node can
    /// only re-confirm origins that already exist.
    ///
    /// Pruning there is what keeps the cost linear. Without it, a node deep in
    /// the graph is re-walked once per enclosing box, which is the quadratic
    /// factor that made `dx.algorithm(5)` and its corpus neighbours regress.
    /// The resulting table is unchanged: this prunes redundant traversal, not
    /// recording.
    pub fn record_derived_forest(&mut self, arena: &TreeArena, signals: &[SigId], box_node: BoxId) {
        if !self.recording {
            return;
        }
        let mut stack = signals.to_vec();
        let mut visited = AHashSet::new();
        while let Some(signal) = stack.pop() {
            if !visited.insert(signal) {
                continue;
            }
            if !self.origins_for(signal).is_empty() {
                continue;
            }
            self.record(signal, box_node);
            if let Some(children) = arena.children(signal) {
                stack.extend(children.iter().copied());
            }
        }
        self.record_outputs(signals, box_node);
    }

    /// Returns candidate Box origins in deterministic propagation order.
    #[must_use]
    pub fn origins_for(&self, signal: SigId) -> &[BoxId] {
        self.by_signal.get(&signal).map_or(&[], Vec::as_slice)
    }

    /// Propagates all origins from `sources` to a newly derived signal.
    ///
    /// Normalization, recursion conversion, and AD rewrites can use this
    /// operation without depending on parser types.
    pub fn inherit(&mut self, derived: SigId, sources: &[SigId]) {
        if !self.recording {
            return;
        }
        let inherited = sources
            .iter()
            .flat_map(|source| self.origins_for(*source))
            .copied()
            .collect::<Vec<_>>();
        for origin in inherited {
            self.record(derived, origin);
        }
    }

    /// Transfers root provenance across a lane-preserving forest rewrite.
    ///
    /// Preparation passes preserve output arity. Pairing old and new roots
    /// retains the source of an operator even when the replacement node no
    /// longer contains the old node as a structural child.
    pub fn inherit_replacements(&mut self, before: &[SigId], after: &[SigId]) {
        if !self.recording {
            return;
        }
        for (source, derived) in before.iter().zip(after) {
            self.inherit(*derived, &[*source]);
        }
    }

    /// Remaps this table after a Signal forest is cloned to another arena.
    ///
    /// Sources are visited in ascending `SigId` order rather than in the hash
    /// order of `node_map`. The clone mapping is expected to be injective, in
    /// which case order is irrelevant — but nothing in the type enforces that,
    /// and if two sources ever share a destination, [`Self::record`] keeps only
    /// the first [`Self::MAX_ORIGINS_PER_SIGNAL`] candidates it sees. Hash order
    /// would then decide *which* candidates a diagnostic can name, and vary from
    /// run to run. Sorting makes the result a function of the inputs alone,
    /// which is cheap here: `remap` runs once per compilation.
    #[must_use]
    pub fn remap(&self, node_map: &std::collections::HashMap<SigId, SigId>) -> Self {
        if !self.recording {
            return Self::disabled();
        }
        let mut pairs = node_map.iter().collect::<Vec<_>>();
        pairs.sort_unstable_by_key(|(source, _)| source.as_u32());
        let mut remapped = Self::default();
        for (source, destination) in pairs {
            for origin in self.origins_for(*source) {
                remapped.record(*destination, *origin);
            }
        }
        remapped
    }

    /// Ensures every reachable derived node inherits origins from its children.
    ///
    /// Rewriting passes call this after rebuilding a forest. Existing explicit
    /// origins win; a newly interned parent receives the ordered union of all
    /// reachable child origins. This is conservative by design: exact
    /// occurrence choice remains a diagnostic-time operation.
    pub fn inherit_forest(&mut self, arena: &TreeArena, roots: &[SigId]) {
        if !self.recording {
            return;
        }
        fn visit(
            origins: &mut SignalOrigins,
            arena: &TreeArena,
            signal: SigId,
            visited: &mut std::collections::HashSet<SigId>,
        ) {
            if !visited.insert(signal) {
                return;
            }
            let children = arena
                .children(signal)
                .map_or_else(Vec::new, |children| children.to_vec());
            for child in &children {
                visit(origins, arena, *child, visited);
            }
            if origins.origins_for(signal).is_empty() {
                origins.inherit(signal, &children);
            }
        }
        let mut visited = std::collections::HashSet::new();
        for root in roots {
            visit(self, arena, *root, &mut visited);
        }
    }

    /// Number of Signal identities carrying at least one origin.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_signal.len()
    }

    /// Returns `true` when no signal provenance was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_signal.is_empty()
    }
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

pub use api::{propagate_typed, propagate_typed_with_ui};
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
