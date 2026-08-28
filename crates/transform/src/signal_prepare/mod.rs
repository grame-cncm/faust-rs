//! Signal-forest preparation before fast-lane FIR lowering.
//!
//! # Source provenance (C++)
//! - `compiler/normalize/normalform.cpp` (`deBruijn2Sym(...)`, `typeAnnotation(...)`)
//! - `compiler/box_signal_api.cpp` (`boxesToSignalsMLIR(...)`)
//! - `compiler/signals/sigtyperules.cpp` (reduced type inference subset)
//!
//! # Stage scope
//! The pipeline implemented here applies a **fixed linear sequence of ~16 passes**
//! to the propagated signal forest, converting it into the staged shape expected by
//! `signal_fir`.  The sequence is:
//!
//! ```text
//! 2.1  clone forest into a fresh private TreeArena
//! 2.2  de_bruijn_to_sym        (de Bruijn → SYMREC / SYMREF)
//! 2.3  canon_unary_rec         (canonicalize unary projection indices)
//!      └─ assert_sym           (W4: no de Bruijn remains)
//! 2.4  retype #1               (sig_types fresh before promote #1)
//! 2.5  promote #1              (insert SignalPromotion casts)
//! 2.6  retype #2               (sig_types fresh before simplify #1)
//! 2.7  simplify #1             (algebraic simplification)
//! 2.8  merge_iso_rec           (merge isomorphic SYMREC groups)
//! 2.9  retype #3               (sig_types fresh before simplify #2)
//! 2.10 simplify #2             (algebraic simplification)
//! 2.10a retype                 (sig_types fresh before table promotion; -ct only)
//! 2.10b promote_tables         (clamp unprovable table indexes; -ct only)
//! 2.11 canon_one_sample_delays (Delay(x,1) → Delay1(x))
//!      └─ assert_d1            (W4: no Delay(_, 1) remains)
//! 2.12 retype #4               (sig_types fresh before promote #2)
//! 2.13 promote #2              (second promotion pass)
//!      └─ assert_sym_d1        (W4: Sym + D1 preserved)
//! 2.14 retype #5               (final sig_types for PreparedSignals)
//! 2.15 derive_simple_types     (SigType → SimpleSigType)
//! 2.16 verify                  (postconditions; _verified entry only)
//! ```
//!
//! The five re-types (steps 2.4 / 2.6 / 2.9 / 2.12 / 2.14) — plus the
//! optional 2.10a one preceding table promotion under `-ct` — are driven by
//! the private `Staging` driver, which owns `sig_types` and guarantees it is
//! fresh before every typed pass.  No `sig_types_*` snapshot locals are
//! threaded by hand — the schedule is structural.
//!
//! Reduced typing deliberately stops short of the full C++ type lattice. The
//! goal is only to support the upcoming promotion pass and to feed `signal_fir`
//! with enough information for delay/recursion/table lowering.
//!
//! # Boundary contract
//! Input signals may still contain:
//! - de Bruijn recursion groups,
//! - mixed integer/real numeric expressions,
//! - table and clock-family nodes emitted by propagation.
//!
//! Output signals returned by [`prepare_signals_for_fir`] satisfy these
//! fast-lane invariants:
//! - recursion is rewritten to symbolic `SYMREC` / `SYMREF`,
//! - one reduced prepared type is available for each reachable node,
//! - casts needed by the current FIR lowerer have already been inserted,
//! - the original source arena is left untouched.
//!
//! The module also exposes an explicit verification boundary (implemented in
//! the `verify` submodule, `signal_prepare/verify.rs`):
//! - [`PreparedSignals::verify`] checks those postconditions on an already built
//!   staging forest,
//! - [`prepare_signals_for_fir_verified`] returns a wrapper that certifies the
//!   prepared forest passed the boundary checks before reaching FIR lowering.
//!
//! The two structural canonicalization rewrites (`canonicalize_unary_rec_projections`
//! and `canonicalize_one_sample_delays`) are implemented in the `rewrites` submodule
//! (`signal_prepare/rewrites.rs`). See that module for their specifications and
//! explicit C++ parity limitations.
//!
//! # Adaptation status
//! This is an adapted Rust staging phase rather than a 1:1 copy of one single
//! C++ class:
//! - `deBruijn2Sym(...)` is applied forest-wide like the C++ normalization path,
//! - reduced typing keeps only the distinctions currently needed by the
//!   fast-lane instead of the full C++ signal type lattice,
//! - the promotion pass ports only the `SignalPromotion` subset required before
//!   `signal_fir`,
//! - algebraic simplification then runs on the promoted forest before the
//!   final type snapshot is exposed to FIR lowering,
//! - `Delay(x, 1)` is then canonicalized back to `Delay1(x)` so downstream
//!   FIR consumers see one stable unary-delay form.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use normalize::normalform::{NormalFormError, promote_signals_fastlane, simplify_signals_fastlane};
pub use normalize::table_promote::TableRangeWarning;
use normalize::table_promote::promote_table_signals;
use signals::{SigId, SigMatch, match_sig};
use sigtype::{Nature, SigType, TypeAnnotator};
use tlib::{RecursionError, TreeArena, list_to_vec, vec_to_list};
use ui::UiProgram;

mod rewrites;
mod verify;
// Re-export the promotion-invariant checker so `tests.rs` can call it as
// `super::verify_promotion_invariant(...)` without knowing it lives in `verify`.
#[cfg(test)]
use verify::verify_promotion_invariant;

/// Reduced signal type domain used by the FIR-preparation pass.
///
/// Intentionally smaller than the C++ `sigtyperules` lattice: keeps only the
/// distinctions required by the `SignalPromotion` subset and FIR type selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimpleSigType {
    /// Integer-valued signal.
    Int,
    /// Real-valued signal.
    Real,
    /// Soundfile handle payload.
    Sound,
}

/// Result of preparing a propagated signal list for FIR lowering.
///
/// Owns a private staging arena and prepared output roots. Callers observe that
/// staged forest through read-only accessors so the preparation boundary stays
/// encapsulated and can be explicitly re-verified when needed.
#[derive(Debug)]
pub struct PreparedSignals {
    /// Private staging arena containing the prepared signal forest.
    arena: TreeArena,
    /// Prepared output roots interned in [`Self::arena`].
    outputs: Vec<SigId>,
    /// Reduced type annotation for prepared signal nodes (for promoter + FIR lowerer).
    types: HashMap<SigId, SimpleSigType>,
    /// Full signal type annotation from the `sigtype` type system.
    /// Carries interval bounds, variability, and all other lattice qualifiers.
    sig_types: HashMap<SigId, SigType>,
    /// Box derivations remapped into the private staging arena and inherited
    /// by nodes introduced by preparation rewrites.
    origins: propagate::SignalOrigins,
    /// Out-of-range table accesses clamped by the check-table pass (2.10b).
    table_warnings: Vec<TableRangeWarning>,
}

/// Prepared-signal forest that passed the explicit postcondition verifier.
///
/// This wrapper exists so downstream code can request a stronger boundary than
/// "the constructor should have produced something valid": the checked state is
/// represented in the type system and can be threaded through lowering code
/// without re-introducing ad hoc assumptions.
#[derive(Debug)]
pub struct VerifiedPreparedSignals {
    inner: PreparedSignals,
}

/// Options controlling the signal-preparation pipeline.
///
/// One value is threaded from `SignalFirOptions` down to the staging driver;
/// the `Default` implementation reproduces the reference compiler's default
/// behavior.
#[derive(Debug, Clone, Copy)]
pub struct PrepareOptions {
    /// Run the signal-level table access protection pass (steps 2.10a/2.10b),
    /// the Rust home of the C++ `-ct` option (`gGlobal->gCheckTable`,
    /// default on). When disabled, unprovable table indexes reach FIR
    /// lowering unclamped and the generated access is raw — exactly the C++
    /// `-ct 0` contract.
    pub check_table: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self { check_table: true }
    }
}

impl PreparedSignals {
    /// Returns the prepared staging arena.
    #[must_use]
    pub fn arena(&self) -> &TreeArena {
        &self.arena
    }

    /// Returns the prepared output roots.
    #[must_use]
    pub fn outputs(&self) -> &[SigId] {
        &self.outputs
    }

    /// Returns the reduced prepared type for one signal node, when available.
    #[must_use]
    pub fn ty(&self, sig: SigId) -> Option<SimpleSigType> {
        self.types.get(&sig).copied()
    }

    /// Returns the full `SigType` (with interval) for one signal node.
    #[must_use]
    pub fn sig_ty(&self, sig: SigId) -> Option<&SigType> {
        self.sig_types.get(&sig)
    }

    /// Read-only view of the reduced type map for consumers that need to
    /// iterate or pass it wholesale (e.g. FIR builders).  Prefer [`Self::ty`]
    /// for single-node lookups.
    #[must_use]
    pub fn types_map(&self) -> &HashMap<SigId, SimpleSigType> {
        &self.types
    }

    /// Read-only view of the full sig-type map for consumers that need to
    /// iterate or pass it wholesale.  Prefer [`Self::sig_ty`] for
    /// single-node lookups.  Use this map (rather than `types_map`) when
    /// interval bounds, variability, or other lattice qualifiers are needed —
    /// it is a strict superset of the reduced type map.
    #[must_use]
    pub fn sig_types_map(&self) -> &HashMap<SigId, SigType> {
        &self.sig_types
    }

    /// Returns Box-origin candidates for prepared Signal nodes.
    #[must_use]
    pub fn origins(&self) -> &propagate::SignalOrigins {
        &self.origins
    }

    /// Out-of-range table accesses clamped by the check-table pass
    /// (step 2.10b), for the semantic-warning channel. Empty when the pass
    /// was disabled or every access was provably in-bounds.
    #[must_use]
    pub fn table_warnings(&self) -> &[TableRangeWarning] {
        &self.table_warnings
    }
}

impl VerifiedPreparedSignals {
    /// Returns the verified staging arena.
    #[must_use]
    pub fn arena(&self) -> &TreeArena {
        &self.inner.arena
    }

    /// Returns the verified output roots.
    #[must_use]
    pub fn outputs(&self) -> &[SigId] {
        &self.inner.outputs
    }

    /// Returns the reduced prepared type for one signal node, when available.
    #[must_use]
    pub fn ty(&self, sig: SigId) -> Option<SimpleSigType> {
        self.inner.ty(sig)
    }

    /// Returns the full `SigType` for one signal node.
    #[must_use]
    pub fn sig_ty(&self, sig: SigId) -> Option<&SigType> {
        self.inner.sig_ty(sig)
    }

    /// Read-only view of the reduced type map.
    #[must_use]
    pub fn types_map(&self) -> &HashMap<SigId, SimpleSigType> {
        self.inner.types_map()
    }

    /// Read-only view of the full type map.
    #[must_use]
    pub fn sig_types_map(&self) -> &HashMap<SigId, SigType> {
        self.inner.sig_types_map()
    }

    /// Returns Box-origin candidates for verified prepared Signal nodes.
    #[must_use]
    pub fn origins(&self) -> &propagate::SignalOrigins {
        self.inner.origins()
    }

    /// Out-of-range table accesses clamped by the check-table pass.
    #[must_use]
    pub fn table_warnings(&self) -> &[TableRangeWarning] {
        self.inner.table_warnings()
    }

    /// Releases the verified wrapper and returns the inner prepared forest.
    #[must_use]
    pub fn into_inner(self) -> PreparedSignals {
        self.inner
    }
}

/// Typed errors returned while preparing signals for FIR lowering.
#[derive(Debug)]
pub enum SignalPrepareError {
    /// The output forest contains malformed or open de Bruijn recursion.
    Recursion(RecursionError),
    /// Structural type-inference or validation error (e.g. malformed recursion body).
    Typing(String),
    /// Typed failure returned by the canonical signal type annotator.
    Inference(sigtype::InferenceError),
    /// Explicit prepared-forest contract validation failed.
    Validation(String),
    /// Prepared-forest contract failure with an offending Signal.
    ValidationAt {
        /// Prepared Signal rejected by the verifier.
        signal: SigId,
        /// Structural invariant that failed.
        message: String,
        /// Box candidates inherited through preparation.
        box_origins: Vec<boxes::BoxId>,
    },
    /// The signal promotion pass failed (type-driven cast insertion).
    Promotion(NormalFormError),
    /// Algebraic simplification found a division by a constant zero.
    ///
    /// C++ equivalent: the `faustexception` thrown by `mterm::operator/=`,
    /// which aborts compilation with `ERROR : division by 0 in ...`.
    DivisionByZero(String),
    /// The check-table pass met a non-positive or non-constant table size.
    ///
    /// C++ equivalent: the `faustexception`s thrown by
    /// `SignalTablePromotion::safeSigRDTbl` / `safeSigWRTbl`
    /// (`ERROR : RDTbl size = N should be > 0`).
    TableAccess(String),
}

impl fmt::Display for SignalPrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recursion(err) => write!(
                f,
                "signal preparation failed during de_bruijn_to_sym: {err}"
            ),
            Self::Typing(msg) => write!(f, "signal preparation typing failed: {msg}"),
            Self::Inference(err) => write!(f, "signal preparation typing failed: {err}"),
            Self::Validation(msg) => {
                write!(f, "signal preparation postcondition failed: {msg}")
            }
            Self::ValidationAt { message, .. } => {
                write!(f, "signal preparation postcondition failed: {message}")
            }
            Self::Promotion(err) => write!(f, "signal preparation promotion failed: {err}"),
            Self::DivisionByZero(msg) | Self::TableAccess(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for SignalPrepareError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Recursion(e) => Some(e),
            Self::Inference(e) => Some(e),
            Self::Promotion(e) => Some(e),
            Self::Typing(_)
            | Self::Validation(_)
            | Self::ValidationAt { .. }
            | Self::DivisionByZero(_)
            | Self::TableAccess(_) => None,
        }
    }
}

impl SignalPrepareError {
    /// Prepared Signal most closely associated with the failure, when known.
    #[must_use]
    pub fn signal(&self) -> Option<SigId> {
        match self {
            Self::Inference(error) => error.signal(),
            Self::ValidationAt { signal, .. } => Some(*signal),
            _ => None,
        }
    }

    /// Ordered Box candidates associated with the offending Signal.
    #[must_use]
    pub fn box_origins(&self) -> &[boxes::BoxId] {
        match self {
            Self::ValidationAt { box_origins, .. } => box_origins,
            _ => &[],
        }
    }

    fn attach_origins(&mut self, origins: &propagate::SignalOrigins) {
        if let Self::ValidationAt {
            signal,
            box_origins,
            ..
        } = self
        {
            *box_origins = origins.origins_for(*signal).to_vec();
        }
    }
}

impl From<RecursionError> for SignalPrepareError {
    fn from(value: RecursionError) -> Self {
        Self::Recursion(value)
    }
}

impl From<NormalFormError> for SignalPrepareError {
    fn from(value: NormalFormError) -> Self {
        Self::Promotion(value)
    }
}

/// Clones one output forest into a private arena, converts de Bruijn recursion
/// to symbolic recursion with forest-wide sharing preserved, then infers one
/// reduced type per prepared signal node, applies the reduced promotion pass,
/// simplifies the promoted forest, then re-runs typing and promotion so the
/// final forest still satisfies the FIR lowerer's explicit cast invariants.
///
/// C++ parity note: both `deBruijn2Sym(...)` and the later type/promotion flow
/// conceptually operate on the whole output list, not independently per root.
/// This function mirrors that contract by cloning all outputs through one memo
/// table, converting one list root, and then typing the prepared forest.
///
/// Type-source note: recursive and arithmetic typing now come from the
/// canonical `sigtype::TypeAnnotator` / `normalize` path. `transform` no longer
/// runs a second local recursive typer. The remaining `SimpleSigType` map is a
/// reduced view derived from canonical `SigType` results for FIR type
/// selection.
///
/// Additional fast-lane note: after `de_bruijn_to_sym`, the staging forest is
/// normalized so degenerate symbolic recursion groups use a canonical physical
/// projection index (`0`). This mirrors the intent of the classic C++ pipeline,
/// where degenerate recursive projections are later normalized through
/// projection-definition rewriting (`inlineDegenerateRecursions(...)`), but it
/// does so earlier so the FIR preparer and lowerer can reason on dense slot
/// vectors only.
///
/// Limitation: this is a narrower compatibility step, not a full Rust port of
/// `inlineDegenerateRecursions(...)`. In particular, it does not analyze
/// recursive dependency graphs or rewrite projection definitions under delay
/// operators; it only canonicalizes logical projection indices once symbolic
/// recursion has already been built.
pub fn prepare_signals_for_fir(
    src_arena: &TreeArena,
    outputs: &[SigId],
    ui: &UiProgram,
) -> Result<PreparedSignals, SignalPrepareError> {
    let options = PrepareOptions::default();
    let prepared = prepare_signals_for_fir_unverified(src_arena, outputs, ui, None, &options)?;
    verify::verify_prepared_output_arity(outputs.len(), prepared.outputs.len())?;
    prepared.verify(ui)?;
    Ok(prepared)
}

/// Like [`prepare_signals_for_fir`], but returns a wrapper certifying that the
/// postcondition verifier has already run successfully.
pub fn prepare_signals_for_fir_verified(
    src_arena: &TreeArena,
    outputs: &[SigId],
    ui: &UiProgram,
) -> Result<VerifiedPreparedSignals, SignalPrepareError> {
    prepare_signals_for_fir_verified_with_options(
        src_arena,
        outputs,
        ui,
        &PrepareOptions::default(),
    )
}

/// Like [`prepare_signals_for_fir_verified`], with explicit
/// [`PrepareOptions`] (the `-ct` gate).
pub fn prepare_signals_for_fir_verified_with_options(
    src_arena: &TreeArena,
    outputs: &[SigId],
    ui: &UiProgram,
    options: &PrepareOptions,
) -> Result<VerifiedPreparedSignals, SignalPrepareError> {
    let prepared = prepare_signals_for_fir_unverified(src_arena, outputs, ui, None, options)?;
    verify::verify_prepared_output_arity(outputs.len(), prepared.outputs.len())?;
    prepared.into_verified(ui)
}

/// Prepares and verifies a Signal forest while preserving Box provenance.
///
/// This is the provenance-aware counterpart of
/// [`prepare_signals_for_fir_verified`]. It remaps arena-local Signal ids
/// during the staging clone, then conservatively propagates origins through
/// recursion conversion, promotion, simplification, recursion merging, and
/// delay canonicalization.
pub fn prepare_signals_for_fir_verified_with_origins(
    src_arena: &TreeArena,
    outputs: &[SigId],
    ui: &UiProgram,
    origins: &propagate::SignalOrigins,
    options: &PrepareOptions,
) -> Result<VerifiedPreparedSignals, SignalPrepareError> {
    let prepared =
        prepare_signals_for_fir_unverified(src_arena, outputs, ui, Some(origins), options)?;
    verify::verify_prepared_output_arity(outputs.len(), prepared.outputs.len())?;
    prepared.into_verified(ui)
}

/// Mutable staging value that owns the private arena, the current output roots,
/// and the current full-type snapshot throughout the preparation pipeline.
///
/// The driver guarantees `sig_types` is fresh before every typed pass and
/// re-infers after each structural change, reproducing the exact 5-infer schedule
/// (steps 2.4 / 2.6 / 2.9 / 2.12 / 2.14 in the module-level pipeline table).
/// No `sig_types_*` snapshot locals are threaded by hand — `retype` is the one
/// place that advances the snapshot, and it is called explicitly at those five
/// points in [`prepare_signals_for_fir_unverified`].
struct Staging<'ui> {
    arena: TreeArena,
    outputs: Vec<SigId>,
    /// Always "fresh for `outputs`" after each [`Self::retype`] call.
    sig_types: HashMap<SigId, SigType>,
    ui: &'ui UiProgram,
    origins: propagate::SignalOrigins,
    /// Out-of-range table accesses clamped by [`Self::promote_tables`].
    table_warnings: Vec<TableRangeWarning>,
}

impl<'ui> Staging<'ui> {
    /// Initializes staging from a freshly cloned forest.
    fn new(
        arena: TreeArena,
        outputs: Vec<SigId>,
        ui: &'ui UiProgram,
        origins: propagate::SignalOrigins,
    ) -> Self {
        Self {
            arena,
            outputs,
            sig_types: HashMap::new(),
            ui,
            origins,
            table_warnings: Vec::new(),
        }
    }

    fn inherit_origins(&mut self) {
        self.origins.inherit_forest(&self.arena, &self.outputs);
    }

    /// Refreshes `sig_types` via a full `TypeAnnotator` pass over `outputs`.
    ///
    /// Must be called at each of the five schedule points (2.4/2.6/2.9/2.12/2.14)
    /// so the next typed pass (promote / simplify) sees a consistent snapshot.
    fn retype(&mut self) -> Result<(), SignalPrepareError> {
        self.sig_types = infer_full_types(&self.arena, &self.outputs, self.ui)?;
        Ok(())
    }

    /// Pass 2.2: converts de Bruijn recursion to symbolic `SYMREC` / `SYMREF`.
    fn de_bruijn_to_sym(&mut self) -> Result<(), SignalPrepareError> {
        let before = self.outputs.clone();
        let list = vec_to_list(&mut self.arena, &self.outputs);
        let symbolic_list = tlib::de_bruijn_to_sym(&mut self.arena, list)?;
        self.outputs = list_to_vec(&self.arena, symbolic_list)
            .expect("de_bruijn_to_sym rebuilds a proper cons list");
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
        Ok(())
    }

    /// Pass 2.3: canonicalizes unary symbolic recursion projection indices to `0`.
    fn canon_unary_rec(&mut self) -> Result<(), SignalPrepareError> {
        let before = self.outputs.clone();
        let list = vec_to_list(&mut self.arena, &self.outputs);
        let canonical_list = rewrites::canonicalize_unary_rec_projections(&mut self.arena, list)?;
        self.outputs = list_to_vec(&self.arena, canonical_list)
            .expect("canonicalize_unary_rec_projections rebuilds a proper cons list");
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
        Ok(())
    }

    /// W4 contract: no de Bruijn recursion form remains after `de_bruijn_to_sym`.
    fn assert_sym(&self) {
        debug_assert!(
            !forest_has_de_bruijn(&self.arena, &self.outputs),
            "de_bruijn_to_sym must eliminate every de Bruijn recursion form before typing"
        );
    }

    /// Pass 2.5: first promotion pass — inserts `SignalPromotion` casts.
    fn promote(&mut self) -> Result<(), SignalPrepareError> {
        let before = self.outputs.clone();
        self.outputs = promote_signals_fastlane(&mut self.arena, &self.sig_types, &self.outputs)
            .map_err(SignalPrepareError::Promotion)?;
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
        Ok(())
    }

    /// Pass 2.7 / 2.10: algebraic simplification of the promoted / merged forest.
    ///
    /// Fails when simplification detects a division by a constant zero, which
    /// C++ reports as a fatal `ERROR : division by 0 in ...` from
    /// `mterm::operator/=`.
    fn simplify(&mut self) -> Result<(), SignalPrepareError> {
        let before = self.outputs.clone();
        self.outputs = simplify_signals_fastlane(&mut self.arena, &self.sig_types, &self.outputs)
            .map_err(|err| match err {
            NormalFormError::DivisionByZero(msg) => SignalPrepareError::DivisionByZero(msg),
            other => SignalPrepareError::Promotion(other),
        })?;
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
        Ok(())
    }

    /// Pass 2.8: merges isomorphic symbolic recursion groups.
    fn merge_iso_rec(&mut self) {
        let before = self.outputs.clone();
        self.outputs = normalize::merge_isomorphic_symrec_groups(&mut self.arena, &self.outputs);
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
    }

    /// Pass 2.10b: signal-level table access protection (`-ct`).
    ///
    /// Rewrites every table index the interval analysis cannot prove
    /// in-bounds into `max(0, min(index, size-1))`, recording one
    /// [`TableRangeWarning`] per rewrite. Requires a fresh [`Self::retype`]
    /// (step 2.10a): the clamp decision reads index intervals from
    /// `sig_types`.
    ///
    /// Fails on a non-positive or non-constant table size, which C++
    /// reports as a fatal `faustexception` from
    /// `SignalTablePromotion::safeSigRDTbl` / `safeSigWRTbl`.
    fn promote_tables(&mut self) -> Result<(), SignalPrepareError> {
        let before = self.outputs.clone();
        self.outputs = promote_table_signals(
            &mut self.arena,
            &self.sig_types,
            &self.outputs,
            &mut self.table_warnings,
        )
        .map_err(|err| match err {
            NormalFormError::TableAccess(msg) => SignalPrepareError::TableAccess(msg),
            other => SignalPrepareError::Promotion(other),
        })?;
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
        Ok(())
    }

    /// Pass 2.11: rewrites every `Delay(x, 1)` to the canonical `Delay1(x)` form.
    fn canon_one_sample_delays(&mut self) -> Result<(), SignalPrepareError> {
        let before = self.outputs.clone();
        self.outputs = rewrites::canonicalize_one_sample_delays(&mut self.arena, &self.outputs)?;
        self.origins.inherit_replacements(&before, &self.outputs);
        self.inherit_origins();
        Ok(())
    }

    /// W4 contract: no non-canonical one-sample delay `Delay(_, 1)` remains.
    fn assert_d1(&self) {
        debug_assert!(
            !forest_has_delay_of_one(&self.arena, &self.outputs),
            "canonicalize_one_sample_delays must rewrite every Delay(_, 1) to Delay1"
        );
    }

    /// W4 contract: both `Sym` and `D1` invariants are preserved after promote #2.
    fn assert_sym_d1(&self) {
        debug_assert!(
            !forest_has_de_bruijn(&self.arena, &self.outputs),
            "the staging pipeline must preserve the symbolic-recursion form (Sym)"
        );
        debug_assert!(
            !forest_has_delay_of_one(&self.arena, &self.outputs),
            "promotion must preserve the one-sample-delay canonical form (D1)"
        );
    }

    /// Consumes the staging value and returns the finished `PreparedSignals`.
    ///
    /// Must be called after the final `retype` (step 2.14) so `sig_types` is
    /// already the final snapshot.
    fn finish(self) -> PreparedSignals {
        let types = derive_simple_types(&self.arena, &self.sig_types);
        PreparedSignals {
            arena: self.arena,
            outputs: self.outputs,
            types,
            sig_types: self.sig_types,
            origins: self.origins,
            table_warnings: self.table_warnings,
        }
    }
}

fn prepare_signals_for_fir_unverified(
    src_arena: &TreeArena,
    outputs: &[SigId],
    ui: &UiProgram,
    source_origins: Option<&propagate::SignalOrigins>,
    options: &PrepareOptions,
) -> Result<PreparedSignals, SignalPrepareError> {
    // Step 2.1 — clone forest into a fresh private arena.
    let mut arena = TreeArena::new();
    let (cloned_outputs, node_map) = arena.clone_forest_from_with_mapping(src_arena, outputs);
    let origins = source_origins.map_or_else(propagate::SignalOrigins::default, |origins| {
        origins.remap(&node_map)
    });
    let mut s = Staging::new(arena, cloned_outputs, ui, origins);
    s.inherit_origins();

    // Step 2.2 — de Bruijn → SYMREC / SYMREF.
    s.de_bruijn_to_sym()?;
    // Step 2.3 — canonicalize unary projection indices.
    s.canon_unary_rec()?;
    s.assert_sym(); // W4: no de Bruijn remains

    // Step 2.4 — retype #1 (before promote #1).
    s.retype()?;
    // Step 2.5 — promote #1 (insert SignalPromotion casts).
    s.promote()?;

    // Step 2.6 — retype #2 (before simplify #1).
    s.retype()?;
    // Step 2.7 — simplify #1.
    s.simplify()?;

    // Step 2.8 — merge isomorphic SYMREC groups.
    s.merge_iso_rec();
    // Step 2.9 — retype #3 (before simplify #2).
    s.retype()?;
    // Step 2.10 — simplify #2.
    s.simplify()?;

    // Steps 2.10a/2.10b — check-table (`-ct`): retype so index intervals are
    // fresh, then clamp unprovable table accesses. Runs after simplify #2 so
    // table sizes are integer constants, mirroring the C++ ordering
    // (`signalTablePromote` after simplification in `simplifyToNormalForm`).
    if options.check_table {
        s.retype()?;
        s.promote_tables()?;
    }

    // Step 2.11 — Delay(x,1) → Delay1(x).
    s.canon_one_sample_delays()?;
    s.assert_d1(); // W4: no Delay(_, 1) remains

    // Step 2.12 — retype #4 (before promote #2).
    s.retype()?;
    // Step 2.13 — promote #2.
    s.promote()?;
    s.assert_sym_d1(); // W4: Sym + D1 preserved

    // Step 2.14 — retype #5 (final sig_types for PreparedSignals).
    s.retype()?;

    // Step 2.15 — derive SimpleSigType map and build PreparedSignals.
    Ok(s.finish())
}

/// Returns `true` if any node reachable from `outputs` satisfies `pred`.
///
/// A small structural DAG scan (memoized via `visited`, list children expanded)
/// backing the debug-only inter-pass contract assertions in
/// [`prepare_signals_for_fir_unverified`]. Malformed structures are ignored
/// here; the explicit [`PreparedSignals::verify`] boundary reports them with
/// precise diagnostics.
fn forest_any_node<P>(arena: &TreeArena, outputs: &[SigId], pred: P) -> bool
where
    P: Fn(&TreeArena, SigId) -> bool,
{
    fn walk<P: Fn(&TreeArena, SigId) -> bool>(
        arena: &TreeArena,
        sig: SigId,
        pred: &P,
        visited: &mut HashSet<SigId>,
    ) -> bool {
        if !visited.insert(sig) {
            return false;
        }
        if pred(arena, sig) {
            return true;
        }
        if arena.is_nil(sig) {
            return false;
        }
        let Some(node) = arena.node(sig) else {
            return false;
        };
        for &child in node.children.as_slice() {
            if arena.is_list(child) {
                if let Some(items) = list_to_vec(arena, child)
                    && items.iter().any(|&item| walk(arena, item, pred, visited))
                {
                    return true;
                }
            } else if walk(arena, child, pred, visited) {
                return true;
            }
        }
        false
    }

    let mut visited = HashSet::new();
    outputs
        .iter()
        .any(|&sig| walk(arena, sig, &pred, &mut visited))
}

/// Debug-contract predicate: any residual de Bruijn recursion form (`Sym` broken).
fn forest_has_de_bruijn(arena: &TreeArena, outputs: &[SigId]) -> bool {
    forest_any_node(arena, outputs, |a, s| {
        tlib::match_de_bruijn_rec(a, s).is_some() || tlib::match_de_bruijn_ref(a, s).is_some()
    })
}

/// Debug-contract predicate: any non-canonical one-sample delay `Delay(_, 1)` (`D1` broken).
fn forest_has_delay_of_one(arena: &TreeArena, outputs: &[SigId]) -> bool {
    forest_any_node(
        arena,
        outputs,
        |a, s| matches!(match_sig(a, s), SigMatch::Delay(_, amount) if matches!(match_sig(a, amount), SigMatch::Int(1))),
    )
}

/// Runs the full `TypeAnnotator` (sigtype crate) on the prepared output forest.
///
/// This produces interval bounds, variability, and all lattice qualifiers for
/// each node. Called in [`prepare_signals_for_fir`] before the first
/// `promote_signals_fastlane` (to guide cast-insertion decisions), again after
/// that promotion (to drive algebraic simplification on the promoted graph),
/// again after simplification (to re-establish canonical types before the
/// second promotion pass), and once more after the second promotion so the
/// final map reflects the forest that reaches FIR lowering. That last result
/// is what ends up in [`PreparedSignals::sig_types`].
fn infer_full_types(
    arena: &TreeArena,
    outputs: &[SigId],
    ui: &UiProgram,
) -> Result<HashMap<SigId, SigType>, SignalPrepareError> {
    let mut annotator = TypeAnnotator::new(arena, ui);
    annotator
        .annotate(outputs)
        .map_err(SignalPrepareError::Inference)
}

/// Reduces canonical `SigType` annotations to the smaller fast-lane domain.
///
/// `normalize`/`sigtype` are the single source of truth for recursion and
/// arithmetic typing. The fast-lane still keeps a smaller `Int / Real / Sound`
/// view for FIR type selection and assertions, derived directly and
/// homomorphically from the canonical map: no second fixpoint and no per-node
/// override, so the reduced type never contradicts the canonical nature. In
/// particular, a fully unconstrained self-recursion (`x = x`) closes to `Int`,
/// matching the C++ `TREC` result — the fast-lane no longer forces it to `Real`.
fn derive_simple_types(
    arena: &TreeArena,
    sig_types: &HashMap<SigId, SigType>,
) -> HashMap<SigId, SimpleSigType> {
    sig_types
        .iter()
        .map(|(sig, ty)| {
            let reduced = if matches!(match_sig(arena, *sig), SigMatch::Soundfile(_)) {
                SimpleSigType::Sound
            } else {
                match ty.nature() {
                    Nature::Int => SimpleSigType::Int,
                    Nature::Real | Nature::Any => SimpleSigType::Real,
                }
            };
            (*sig, reduced)
        })
        .collect()
}

#[cfg(test)]
mod tests;
