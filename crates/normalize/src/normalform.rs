//! Normal-form pipeline coordinator — Phase 1.
//!
//! Ported (Phase 1 scope) from C++ `compiler/normalize/normalform.cpp`.
//!
//! # Phase 1 scope
//!
//! This module implements the three preparation steps that
//! `crates/transform/signal_prepare` currently performs before FIR lowering,
//! now exposed as a general-purpose, backend-agnostic API:
//!
//! 0. **De Bruijn coherence check** (`check_de_bruijn_coherence`): reject
//!    locally escaping recursive references before any symbolic conversion.
//! 1. **de Bruijn → symbolic** (`de_bruijn_to_sym`): rewrite all de-Bruijn
//!    recursion groups into symbolic `SymRec`/`SymRef` form.
//! 2. **Type annotation** (`TypeAnnotator::annotate`): compute the full
//!    `SigType` for every reachable node using the C++ type lattice.
//! 3. **Signal promotion** (see [`promote_signals`]): insert numeric casts
//!    where the signal graph mixes integer and real computations.
//!
//! Parent nodes own context-sensitive promotion requirements, following the
//! C++ `SignalPromotion::transformation(...)` rules. Integer-only contexts such
//! as `select2` selectors, delay/table indices, and `enable` gates are rebuilt
//! through explicit integer-context helpers instead of relying on backend-side
//! repairs.
//!
//! # Phase 2 (deferred)
//!
//! The full C++ `simplifyToNormalForm` pipeline (UI promotion, FTZ wrapping,
//! auto-differentiation, double-promotion, causality check) is deferred to a
//! future Phase 2 implementation.
//!
//! # API mapping status
//! - `simplifyToNormalForm(sig)` → [`prepare_signals`] (Phase 1 only)
//! - `simplifyToNormalForm2(sigs)` → [`prepare_signals_multi`]

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use signals::{BinOp, SigBuilder, SigId, SigMatch, dump_sig_readable, match_sig};
use sigtype::{Nature, SigType, TypeAnnotator};
use tlib::{
    RecursionError, TreeArena, check_de_bruijn_coherence, de_bruijn_to_sym, match_sym_rec,
    match_sym_ref,
};
use ui::UiProgram;

use crate::simplify::{SimplifyCache, simplify_with_cache};

// ─── Options ──────────────────────────────────────────────────────────────────

/// Configuration for the normal-form preparation pipeline.
///
/// Corresponds to the subset of `gGlobal` options consumed by Phase 1.
#[derive(Debug, Clone, Default)]
pub struct NormalFormOpts {
    /// Skip the signal-promotion pass (useful for testing individual steps).
    pub skip_promotion: bool,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Error returned by the normal-form pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalFormError {
    /// De-Bruijn-to-symbolic conversion failed (open recursion group).
    Recursion(String),
    /// Full-type annotation failed.
    Type(String),
}

impl std::fmt::Display for NormalFormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recursion(e) => write!(f, "de-Bruijn conversion error: {e}"),
            Self::Type(e) => write!(f, "type annotation error: {e}"),
        }
    }
}

impl std::error::Error for NormalFormError {}

impl From<RecursionError> for NormalFormError {
    fn from(e: RecursionError) -> Self {
        Self::Recursion(format!("{e}"))
    }
}

impl From<sigtype::TypeError> for NormalFormError {
    fn from(e: sigtype::TypeError) -> Self {
        Self::Type(format!("{e}"))
    }
}

impl From<sigtype::rules::TypeError> for NormalFormError {
    fn from(e: sigtype::rules::TypeError) -> Self {
        Self::Type(format!("{e}"))
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Phase 1 normal-form preparation for a single signal.
///
/// Runs the normal-form preparation pipeline:
/// 0. `check_de_bruijn_coherence(arena, sig)`
/// 1. `de_bruijn_to_sym(arena, sig)`
/// 2. `TypeAnnotator::annotate(&[sym_sig])` -> `types`
/// 3. `promote_signals(arena, &types, &[sym_sig])` -> promoted output
///
/// Returns the promoted signal root and the type map produced by step 2.
///
/// C++: Phase 1 subset of `Tree simplifyToNormalForm(Tree sig)`.
pub fn prepare_signals(
    arena: &mut TreeArena,
    ui: &UiProgram,
    sig: SigId,
    opts: &NormalFormOpts,
) -> Result<(SigId, HashMap<SigId, SigType>), NormalFormError> {
    let results = prepare_signals_multi(arena, ui, &[sig], opts)?;
    let (sigs, types) = results;
    Ok((sigs[0], types))
}

/// Phase 1 normal-form preparation for a slice of output signals.
///
/// Applies the preparation pipeline to the entire signal forest rooted at
/// `sigs`, treating them as co-dependent outputs (type annotation sees all
/// roots).
///
/// Returns the promoted signal roots (same order as input) and the type map.
///
/// C++: Phase 1 subset of `void simplifyToNormalForm2(tvec& sigs)`.
pub fn prepare_signals_multi(
    arena: &mut TreeArena,
    ui: &UiProgram,
    sigs: &[SigId],
    opts: &NormalFormOpts,
) -> Result<(Vec<SigId>, HashMap<SigId, SigType>), NormalFormError> {
    // ── Step 0: De Bruijn coherence pre-check ─────────────────────────────
    for &s in sigs {
        check_de_bruijn_coherence(arena, s)?;
    }

    // ── Step 1: de Bruijn → symbolic ──────────────────────────────────────
    let mut sym_sigs: Vec<SigId> = Vec::with_capacity(sigs.len());
    for &s in sigs {
        sym_sigs.push(de_bruijn_to_sym(arena, s)?);
    }

    // ── Step 2: type annotation ────────────────────────────────────────────
    // `TypeAnnotator` borrows `arena` immutably; that borrow ends when `types`
    // is returned and the annotator drops.
    let types = {
        let mut annotator = TypeAnnotator::new(arena, ui);
        annotator.annotate(&sym_sigs)?
    };

    // ── Step 3: signal promotion ───────────────────────────────────────────
    if opts.skip_promotion {
        return Ok((sym_sigs, types));
    }
    let promoted_sigs = promote_signals(arena, &types, &sym_sigs);

    // Re-annotate after promotion so callers get an up-to-date type map.
    let types2 = {
        let mut annotator = TypeAnnotator::new(arena, ui);
        annotator.annotate(&promoted_sigs)?
    };

    Ok((promoted_sigs, types2))
}

// ─── Signal promotion ─────────────────────────────────────────────────────────

/// Insert numeric promotion casts into a signal forest.
///
/// Walks each signal tree and inserts `FloatCast` nodes where an integer signal
/// feeds into a context that requires a real value, and vice versa.  This is a
/// simplified version of the C++ `SignalPromotion` pass sufficient for Phase 1.
///
/// Current scope: wrap integer-valued output signals with `FloatCast` when
/// the Faust process output is expected to be real (variability = Konst with
/// Int nature).  This matches the minimal promotion the fast-lane needed.
///
/// C++: `signalPromotion(sig)` in `normalform.cpp`.
pub fn promote_signals(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sigs: &[SigId],
) -> Vec<SigId> {
    promote_signals_fastlane(arena, types, sigs)
        .expect("signal promotion should succeed after canonical type annotation")
}

/// Fast-lane promotion pass shared by `normalize` and FIR preparation.
///
/// This is the current Rust home of the detailed promotion subset previously
/// implemented in `transform::signal_prepare`. It consumes the canonical
/// `SigType` map produced by `TypeAnnotator`, so recursive widening and mixed
/// numeric expressions are decided from the same source of truth as the rest
/// of the normalization pipeline.
pub fn promote_signals_fastlane(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sigs: &[SigId],
) -> Result<Vec<SigId>, NormalFormError> {
    let mut promoter = SignalPromoter::new(arena, types);
    sigs.iter().map(|sig| promoter.promote(*sig)).collect()
}

/// Simplify a prepared signal forest using the canonical `SigType` context.
pub fn simplify_signals_fastlane(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sigs: &[SigId],
) -> Vec<SigId> {
    let mut cache = SimplifyCache::new();
    sigs.iter()
        .map(|sig| {
            match catch_unwind(AssertUnwindSafe(|| {
                simplify_with_cache(arena, &mut cache, types, *sig)
            })) {
                Ok(simplified) => simplified,
                Err(_) => {
                    cache.clear();
                    *sig
                }
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReducedSigKind {
    Int,
    Real,
    Sound,
}

/// Replays the Phase-1 subset of C++ `SignalPromotion::transformation(...)`.
///
/// Provenance:
/// - C++ `compiler/transform/sigPromotion.cpp`
/// - C++ type source `compiler/signals/sigtyperules.cpp`
///
/// Parity contract:
/// - `promote(...)` memoizes only the context-free reconstruction of one
///   original `SigId`.
/// - Parent rules own context-sensitive coercions through
///   [`Self::promote_as_int`], [`Self::promote_as_float`], and
///   [`Self::promote_like`].
/// - The cached value for `promote(sig)` must therefore stay valid under any
///   parent, and parent-specific wrappers apply casts exactly where the C++
///   rule does.
///
/// Current rule inventory:
/// - `1:1 enough for Phase 1`: arithmetic/binops, `select2`, delay/table
///   indices, `enable`, bargraphs, waveform numeric homogenization, clocked
///   family integer clocks.
/// - `adapted but context-invariant`: list reconstruction, symbolic recursion,
///   `output`, `attach`, `control`, `seq`, `temp_var`, `perm_var`,
///   `assert_bounds`, `lowest`, `highest`, foreign-function argument lists.
/// - `deferred to Phase 2`: passes outside the current normalization subset
///   (`simplify`, FTZ wrapping, auto-diff, UI promotion beyond numeric casts,
///   and the rest of full `simplifyToNormalForm`).
struct SignalPromoter<'a> {
    arena: &'a mut TreeArena,
    types: &'a HashMap<SigId, SigType>,
    memo: HashMap<SigId, SigId>,
}

impl<'a> SignalPromoter<'a> {
    fn new(arena: &'a mut TreeArena, types: &'a HashMap<SigId, SigType>) -> Self {
        Self {
            arena,
            types,
            memo: HashMap::new(),
        }
    }

    /// Rebuild one signal in a context-free way and memoize that result.
    ///
    /// This deliberately does not encode parent-owned integer/real coercions.
    /// Those remain at the parent call site, mirroring C++
    /// `SignalPromotion::transformation(...)`.
    fn promote(&mut self, sig: SigId) -> Result<SigId, NormalFormError> {
        if let Some(promoted) = self.memo.get(&sig) {
            return Ok(*promoted);
        }

        let promoted = if self.arena.is_nil(sig) {
            sig
        } else if self.arena.is_list(sig) {
            let head = self.arena.hd(sig).ok_or_else(|| {
                NormalFormError::Type("malformed list during promotion".to_owned())
            })?;
            let tail = self.arena.tl(sig).ok_or_else(|| {
                NormalFormError::Type("malformed list during promotion".to_owned())
            })?;
            let promoted_head = self.promote(head)?;
            let promoted_tail = self.promote(tail)?;
            self.arena.cons(promoted_head, promoted_tail)
        } else if let Some((var, body_list)) = match_sym_rec(self.arena, sig) {
            let promoted_body = self.promote(body_list)?;
            tlib::sym_rec(self.arena, var, promoted_body)
        } else if let Some(var) = match_sym_ref(self.arena, sig) {
            tlib::sym_ref(self.arena, var)
        } else {
            self.promote_signal(sig)?
        };

        self.memo.insert(sig, promoted);
        Ok(promoted)
    }

    fn promote_signal(&mut self, sig: SigId) -> Result<SigId, NormalFormError> {
        let promoted = match match_sig(self.arena, sig) {
            SigMatch::Unknown => self.clone_generic(sig)?,
            SigMatch::Int(_)
            | SigMatch::Real(_)
            | SigMatch::Input(_)
            | SigMatch::Button(_)
            | SigMatch::Checkbox(_) => sig,
            SigMatch::Output(index, inner) => {
                let inner = self.promote(inner)?;
                SigBuilder::new(self.arena).output(index, inner)
            }
            SigMatch::Delay1(value) => {
                let value = self.promote(value)?;
                SigBuilder::new(self.arena).delay1(value)
            }
            SigMatch::Delay(value, amount) => {
                let value = self.promote(value)?;
                let amount_promoted = self.promote_as_int(amount)?;
                SigBuilder::new(self.arena).delay(value, amount_promoted)
            }
            SigMatch::Fir(coefs) => {
                let coefs = coefs.to_vec();
                self.promote_filter_carrier(&coefs, |b, coefs| b.fir(coefs))?
            }
            SigMatch::Iir(coefs) => {
                let coefs = coefs.to_vec();
                self.promote_filter_carrier(&coefs, |b, coefs| b.iir(coefs))?
            }
            SigMatch::Prefix(init, value) => {
                let init_promoted = self.promote(init)?;
                let value_promoted = self.promote(value)?;
                let (init_promoted, value_promoted) = if self.same_type(init, value)? {
                    (init_promoted, value_promoted)
                } else {
                    (self.promote_as_float(init)?, self.promote_as_float(value)?)
                };
                SigBuilder::new(self.arena).prefix(init_promoted, value_promoted)
            }
            SigMatch::IntCast(inner) => {
                let inner_promoted = self.promote(inner)?;
                self.smart_int_cast(inner_promoted, inner)?
            }
            SigMatch::BitCast(inner) => {
                let inner = self.promote(inner)?;
                SigBuilder::new(self.arena).bit_cast(inner)
            }
            SigMatch::FloatCast(inner) => self.promote_as_float(inner)?,
            SigMatch::Gen(inner) => {
                let inner = self.promote(inner)?;
                SigBuilder::new(self.arena).generate(inner)
            }
            SigMatch::RdTbl(table, index) => {
                let table = self.promote(table)?;
                let index_promoted = self.promote_as_int(index)?;
                SigBuilder::new(self.arena).rdtbl(table, index_promoted)
            }
            SigMatch::WrTbl(size, generator, write_index, write_signal) => {
                let size = self.promote(size)?;
                let generator_promoted = self.promote(generator)?;
                if self.arena.is_nil(write_index) && self.arena.is_nil(write_signal) {
                    SigBuilder::new(self.arena).wrtbl_readonly(size, generator_promoted)
                } else {
                    let write_index_promoted = self.promote_as_int(write_index)?;
                    let write_signal_promoted = self.promote_like(generator, write_signal)?;
                    SigBuilder::new(self.arena).wrtbl(
                        size,
                        generator_promoted,
                        write_index_promoted,
                        write_signal_promoted,
                    )
                }
            }
            SigMatch::Select2(selector, then_value, else_value) => {
                let selector_promoted = self.promote_as_int(selector)?;
                let then_promoted = self.promote(then_value)?;
                let else_promoted = self.promote(else_value)?;
                let (then_promoted, else_promoted) = if self.same_type(then_value, else_value)? {
                    (then_promoted, else_promoted)
                } else {
                    (
                        self.promote_as_float(then_value)?,
                        self.promote_as_float(else_value)?,
                    )
                };
                SigBuilder::new(self.arena).select2(selector_promoted, then_promoted, else_promoted)
            }
            SigMatch::AssertBounds(min, max, current) => {
                let min = self.promote(min)?;
                let max = self.promote(max)?;
                let current = self.promote(current)?;
                SigBuilder::new(self.arena).assert_bounds(min, max, current)
            }
            SigMatch::Lowest(inner) => {
                let inner = self.promote(inner)?;
                SigBuilder::new(self.arena).lowest(inner)
            }
            SigMatch::Highest(inner) => {
                let inner = self.promote(inner)?;
                SigBuilder::new(self.arena).highest(inner)
            }
            SigMatch::BinOp(op, left, right) => self.promote_binop(sig, op, left, right)?,
            SigMatch::Pow(left, right) => {
                self.promote_real_binary(|b, l, r| b.pow(l, r), left, right)?
            }
            SigMatch::Min(left, right) => {
                self.promote_minmax(|b, l, r| b.min(l, r), left, right)?
            }
            SigMatch::Max(left, right) => {
                self.promote_minmax(|b, l, r| b.max(l, r), left, right)?
            }
            SigMatch::Acos(inner) => self.promote_real_unary(|b, x| b.acos(x), inner)?,
            SigMatch::Asin(inner) => self.promote_real_unary(|b, x| b.asin(x), inner)?,
            SigMatch::Atan(inner) => self.promote_real_unary(|b, x| b.atan(x), inner)?,
            SigMatch::Atan2(left, right) => {
                self.promote_real_binary(|b, l, r| b.atan2(l, r), left, right)?
            }
            SigMatch::Cos(inner) => self.promote_real_unary(|b, x| b.cos(x), inner)?,
            SigMatch::Sin(inner) => self.promote_real_unary(|b, x| b.sin(x), inner)?,
            SigMatch::Tan(inner) => self.promote_real_unary(|b, x| b.tan(x), inner)?,
            SigMatch::Exp(inner) => self.promote_real_unary(|b, x| b.exp(x), inner)?,
            SigMatch::Exp10(inner) => self.promote_real_unary(|b, x| b.exp10(x), inner)?,
            SigMatch::Log(inner) => self.promote_real_unary(|b, x| b.log(x), inner)?,
            SigMatch::Log10(inner) => self.promote_real_unary(|b, x| b.log10(x), inner)?,
            SigMatch::Sqrt(inner) => self.promote_real_unary(|b, x| b.sqrt(x), inner)?,
            SigMatch::Abs(inner) => self.promote_abs(inner)?,
            SigMatch::Fmod(left, right) => {
                self.promote_real_binary(|b, l, r| b.fmod(l, r), left, right)?
            }
            SigMatch::Remainder(left, right) => {
                self.promote_real_binary(|b, l, r| b.remainder(l, r), left, right)?
            }
            SigMatch::Floor(inner) => self.promote_real_unary(|b, x| b.floor(x), inner)?,
            SigMatch::Ceil(inner) => self.promote_real_unary(|b, x| b.ceil(x), inner)?,
            SigMatch::Rint(inner) => self.promote_real_unary(|b, x| b.rint(x), inner)?,
            SigMatch::Round(inner) => self.promote_real_unary(|b, x| b.round(x), inner)?,
            SigMatch::FFun(ff, largs) => {
                let largs = self.promote(largs)?;
                SigBuilder::new(self.arena).ffun(ff, largs)
            }
            SigMatch::FConst(ty, name, file) => SigBuilder::new(self.arena).fconst(ty, name, file),
            SigMatch::FVar(ty, name, file) => SigBuilder::new(self.arena).fvar(ty, name, file),
            SigMatch::Proj(index, group) => {
                let group = self.promote(group)?;
                SigBuilder::new(self.arena).proj(index, group)
            }
            SigMatch::Rec(body) => {
                let body = self.promote(body)?;
                SigBuilder::new(self.arena).rec(body)
            }
            SigMatch::ReverseTimeRec(body) => {
                let body = self.promote(body)?;
                SigBuilder::new(self.arena).reverse_time_rec(body)
            }
            SigMatch::BlockReverseAD {
                body,
                seeds,
                cotangents,
                policy,
                ..
            } => {
                // Phase B0 promotion is a structural pass-through: each
                // child list is promoted independently (cons traversal is
                // built into `promote`), and the carrier is rebuilt from
                // the materialised slices so subsequent passes see a
                // canonical layout.
                let body = self.promote(body)?;
                let seeds = self.promote(seeds)?;
                let cotangents = self.promote(cotangents)?;
                let body_vec = tlib::list_to_vec(self.arena, body).ok_or_else(|| {
                    NormalFormError::Type("malformed body list inside BlockReverseAD".to_owned())
                })?;
                let seed_vec = tlib::list_to_vec(self.arena, seeds).ok_or_else(|| {
                    NormalFormError::Type("malformed seed list inside BlockReverseAD".to_owned())
                })?;
                let cot_vec = tlib::list_to_vec(self.arena, cotangents).ok_or_else(|| {
                    NormalFormError::Type(
                        "malformed cotangent list inside BlockReverseAD".to_owned(),
                    )
                })?;
                SigBuilder::new(self.arena).block_reverse_ad(&body_vec, &seed_vec, &cot_vec, policy)
            }
            SigMatch::VSlider(control) => SigBuilder::new(self.arena).vslider(control),
            SigMatch::HSlider(control) => SigBuilder::new(self.arena).hslider(control),
            SigMatch::NumEntry(control) => SigBuilder::new(self.arena).numentry(control),
            SigMatch::VBargraph(control, value) => {
                let value = self.promote_as_float(value)?;
                SigBuilder::new(self.arena).vbargraph(control, value)
            }
            SigMatch::HBargraph(control, value) => {
                let value = self.promote_as_float(value)?;
                SigBuilder::new(self.arena).hbargraph(control, value)
            }
            SigMatch::Attach(left, right) => {
                let left = self.promote(left)?;
                let right = self.promote(right)?;
                SigBuilder::new(self.arena).attach(left, right)
            }
            SigMatch::Enable(left, right) => {
                let left = self.promote(left)?;
                let right_promoted = self.promote_as_int(right)?;
                SigBuilder::new(self.arena).enable(left, right_promoted)
            }
            SigMatch::Control(left, right) => {
                let left = self.promote(left)?;
                let right = self.promote(right)?;
                SigBuilder::new(self.arena).control(left, right)
            }
            SigMatch::Waveform(values) => {
                let values = values.to_vec();
                self.promote_waveform(&values)?
            }
            SigMatch::Soundfile(control) => SigBuilder::new(self.arena).soundfile(control),
            SigMatch::SoundfileLength(soundfile, part) => {
                let soundfile = self.promote(soundfile)?;
                let part_promoted = self.promote_as_int(part)?;
                SigBuilder::new(self.arena).soundfile_length(soundfile, part_promoted)
            }
            SigMatch::SoundfileRate(soundfile, part) => {
                let soundfile = self.promote(soundfile)?;
                let part_promoted = self.promote_as_int(part)?;
                SigBuilder::new(self.arena).soundfile_rate(soundfile, part_promoted)
            }
            SigMatch::SoundfileBuffer(soundfile, chan, part, index) => {
                let soundfile = self.promote(soundfile)?;
                let chan = self.promote(chan)?;
                let part_promoted = self.promote_as_int(part)?;
                let index_promoted = self.promote_as_int(index)?;
                SigBuilder::new(self.arena).soundfile_buffer(
                    soundfile,
                    chan,
                    part_promoted,
                    index_promoted,
                )
            }
            SigMatch::TempVar(value) => {
                let value = self.promote(value)?;
                SigBuilder::new(self.arena).temp_var(value)
            }
            SigMatch::PermVar(value) => {
                let value = self.promote(value)?;
                SigBuilder::new(self.arena).perm_var(value)
            }
            SigMatch::Seq(left, right) => {
                let left = self.promote(left)?;
                let right = self.promote(right)?;
                SigBuilder::new(self.arena).seq(left, right)
            }
            SigMatch::ZeroPad(value, amount) => {
                let value = self.promote(value)?;
                let amount_promoted = self.promote_as_int(amount)?;
                SigBuilder::new(self.arena).zero_pad(value, amount_promoted)
            }
            SigMatch::OnDemand(items) => {
                let items = items.to_vec();
                self.promote_clocked_family(&items, |b, items| b.on_demand(items))?
            }
            SigMatch::Upsampling(items) => {
                let items = items.to_vec();
                self.promote_clocked_family(&items, |b, items| b.upsampling(items))?
            }
            SigMatch::Downsampling(items) => {
                let items = items.to_vec();
                self.promote_clocked_family(&items, |b, items| b.downsampling(items))?
            }
            SigMatch::Clocked(clock_env, value) => {
                // The clock-env child is an opaque annotation (nil at the
                // top-level rate or a `SIGCLOCKENV` token), never a signal:
                // pass it through untouched (roadmap P0.1).
                let value = self.promote(value)?;
                SigBuilder::new(self.arena).clocked(clock_env, value)
            }
            // Opaque clock-env token: keep as-is; only reachable through
            // the generic list walk when a clock env is shared as a child.
            SigMatch::ClockEnvToken(_) => sig,
        };
        Ok(promoted)
    }

    fn promote_binop(
        &mut self,
        node: SigId,
        op: BinOp,
        left: SigId,
        right: SigId,
    ) -> Result<SigId, NormalFormError> {
        let left_promoted = self.promote(left)?;
        let right_promoted = self.promote(right)?;
        let node_ty = self.kind(node)?;
        let out = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                if node_ty == ReducedSigKind::Int {
                    SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
                } else {
                    let left_promoted = self.promote_as_float(left)?;
                    let right_promoted = self.promote_as_float(right)?;
                    SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
                }
            }
            BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne => {
                if self.same_type(left, right)? {
                    SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
                } else {
                    let left_promoted = self.promote_as_float(left)?;
                    let right_promoted = self.promote_as_float(right)?;
                    SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
                }
            }
            BinOp::Rem => {
                if self.same_type(left, right)?
                    && self.kind(left)? == ReducedSigKind::Int
                    && self.kind(right)? == ReducedSigKind::Int
                {
                    SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
                } else {
                    let left_promoted = self.promote_as_float(left)?;
                    let right_promoted = self.promote_as_float(right)?;
                    SigBuilder::new(self.arena).fmod(left_promoted, right_promoted)
                }
            }
            BinOp::Div => {
                let left_promoted = self.promote_as_float(left)?;
                let right_promoted = self.promote_as_float(right)?;
                SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
            }
            BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Lsh | BinOp::ARsh | BinOp::LRsh => {
                let left_promoted = self.promote_as_int(left)?;
                let right_promoted = self.promote_as_int(right)?;
                SigBuilder::new(self.arena).binop(op, left_promoted, right_promoted)
            }
        };
        self.memo.insert(node, out);
        Ok(out)
    }

    fn promote_real_unary(
        &mut self,
        build: impl FnOnce(&mut SigBuilder<'_>, SigId) -> SigId,
        inner: SigId,
    ) -> Result<SigId, NormalFormError> {
        let inner_promoted = self.promote_as_float(inner)?;
        Ok(build(&mut SigBuilder::new(self.arena), inner_promoted))
    }

    fn promote_real_binary(
        &mut self,
        build: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId) -> SigId,
        left: SigId,
        right: SigId,
    ) -> Result<SigId, NormalFormError> {
        let left_promoted = self.promote_as_float(left)?;
        let right_promoted = self.promote_as_float(right)?;
        Ok(build(
            &mut SigBuilder::new(self.arena),
            left_promoted,
            right_promoted,
        ))
    }

    fn promote_minmax(
        &mut self,
        build: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId) -> SigId,
        left: SigId,
        right: SigId,
    ) -> Result<SigId, NormalFormError> {
        let left_promoted = self.promote(left)?;
        let right_promoted = self.promote(right)?;
        let (left_promoted, right_promoted) = if self.same_type(left, right)? {
            (left_promoted, right_promoted)
        } else {
            (self.promote_as_float(left)?, self.promote_as_float(right)?)
        };
        Ok(build(
            &mut SigBuilder::new(self.arena),
            left_promoted,
            right_promoted,
        ))
    }

    fn promote_abs(&mut self, inner: SigId) -> Result<SigId, NormalFormError> {
        let inner_promoted = self.promote(inner)?;
        let inner_promoted = if self.kind(inner)? == ReducedSigKind::Int {
            inner_promoted
        } else {
            self.promote_as_float(inner)?
        };
        Ok(SigBuilder::new(self.arena).abs(inner_promoted))
    }

    fn promote_waveform(&mut self, values: &[SigId]) -> Result<SigId, NormalFormError> {
        let all_int = values
            .iter()
            .all(|value| self.kind(*value).is_ok_and(|ty| ty == ReducedSigKind::Int));
        let mut promoted = Vec::with_capacity(values.len());
        for value in values {
            let promoted_value = self.promote(*value)?;
            let promoted_value = if all_int {
                promoted_value
            } else {
                self.smart_float_cast_preserving_clocked(promoted_value, *value)?
            };
            promoted.push(promoted_value);
        }
        Ok(SigBuilder::new(self.arena).waveform(&promoted))
    }

    fn promote_clocked_family(
        &mut self,
        items: &[SigId],
        build: impl FnOnce(&mut SigBuilder<'_>, &[SigId]) -> SigId,
    ) -> Result<SigId, NormalFormError> {
        let mut promoted = Vec::with_capacity(items.len());
        for (index, item) in items.iter().copied().enumerate() {
            let promoted_item = self.promote(item)?;
            let promoted_item = if index == 0 {
                self.smart_clock_cast(promoted_item, item)?
            } else {
                promoted_item
            };
            promoted.push(promoted_item);
        }
        Ok(build(&mut SigBuilder::new(self.arena), &promoted))
    }

    /// Promotes children of compact `sigFIR`/`sigIIR` filter carriers while
    /// preserving the carrier node itself.
    ///
    /// C++ provenance:
    /// - `compiler/signals/sigFIR.cpp`
    /// - `compiler/signals/sigIIR.cpp`
    ///
    /// Filter algebra helpers use these carriers as structured views over
    /// delayed sample expressions. Normal form should normalize their children,
    /// but it should not expand them back into raw delay/add/mul syntax.
    fn promote_filter_carrier(
        &mut self,
        coefs: &[SigId],
        build: impl FnOnce(&mut SigBuilder<'_>, &[SigId]) -> SigId,
    ) -> Result<SigId, NormalFormError> {
        let mut promoted = Vec::with_capacity(coefs.len());
        for &coef in coefs {
            promoted.push(self.promote(coef)?);
        }
        Ok(build(&mut SigBuilder::new(self.arena), &promoted))
    }

    fn smart_clock_cast(
        &mut self,
        promoted: SigId,
        original: SigId,
    ) -> Result<SigId, NormalFormError> {
        if self.kind(original)? == ReducedSigKind::Int {
            return Ok(promoted);
        }
        if let SigMatch::Clocked(clock_env, clock) = match_sig(self.arena, promoted) {
            let clock = SigBuilder::new(self.arena).int_cast(clock);
            return Ok(SigBuilder::new(self.arena).clocked(clock_env, clock));
        }
        Ok(SigBuilder::new(self.arena).int_cast(promoted))
    }

    fn smart_int_cast(
        &mut self,
        promoted: SigId,
        original: SigId,
    ) -> Result<SigId, NormalFormError> {
        let needs_int_cast = self.kind(original)? == ReducedSigKind::Real
            || matches!(match_sig(self.arena, promoted), SigMatch::FloatCast(_))
            || matches!(
                match_sig(self.arena, promoted),
                SigMatch::Clocked(_, value) if matches!(match_sig(self.arena, value), SigMatch::FloatCast(_))
            );
        if !needs_int_cast {
            return Ok(promoted);
        }
        if let SigMatch::Clocked(clock_env, clock) = match_sig(self.arena, promoted) {
            let clock = SigBuilder::new(self.arena).int_cast(clock);
            return Ok(SigBuilder::new(self.arena).clocked(clock_env, clock));
        }
        Ok(SigBuilder::new(self.arena).int_cast(promoted))
    }

    /// Rebuilds one child for a parent that requires an integer-domain input.
    ///
    /// This mirrors the C++ pattern where the parent rule applies
    /// `smartIntCast(...)` directly at the call site, for example in
    /// `select2`, delay/table indices, and `enable`.
    fn promote_as_int(&mut self, original: SigId) -> Result<SigId, NormalFormError> {
        let promoted = self.promote(original)?;
        self.smart_int_cast(promoted, original)
    }

    /// Rebuilds one child for a parent that requires a real-domain input.
    ///
    /// This mirrors the C++ pattern where arithmetic and math parent rules
    /// apply `smartFloatCast(...)` directly from the parent site.
    fn promote_as_float(&mut self, original: SigId) -> Result<SigId, NormalFormError> {
        let promoted = self.promote(original)?;
        self.smart_float_cast(promoted, original)
    }

    fn smart_float_cast(
        &mut self,
        promoted: SigId,
        original: SigId,
    ) -> Result<SigId, NormalFormError> {
        self.smart_float_cast_preserving_clocked(promoted, original)
    }

    fn smart_float_cast_preserving_clocked(
        &mut self,
        promoted: SigId,
        original: SigId,
    ) -> Result<SigId, NormalFormError> {
        if self.kind(original)? != ReducedSigKind::Int {
            return Ok(promoted);
        }
        if let SigMatch::Clocked(clock_env, value) = match_sig(self.arena, promoted) {
            let value = SigBuilder::new(self.arena).float_cast(value);
            return Ok(SigBuilder::new(self.arena).clocked(clock_env, value));
        }
        Ok(SigBuilder::new(self.arena).float_cast(promoted))
    }

    fn smart_cast(
        &mut self,
        target: SigId,
        source: SigId,
        promoted_source: SigId,
    ) -> Result<SigId, NormalFormError> {
        let target_ty = self.kind(target)?;
        let source_ty = self.kind(source)?;
        if target_ty == source_ty {
            Ok(promoted_source)
        } else if target_ty == ReducedSigKind::Real && source_ty == ReducedSigKind::Int {
            self.smart_float_cast(promoted_source, source)
        } else if target_ty == ReducedSigKind::Int && source_ty == ReducedSigKind::Real {
            self.smart_int_cast(promoted_source, source)
        } else {
            Ok(promoted_source)
        }
    }

    /// Rebuilds `source` in the numeric domain expected by `target`.
    ///
    /// This mirrors parent rules in the C++ promoter that coerce a child
    /// according to another operand or declaration type, such as mutable table
    /// writes matching the generator/table element domain.
    fn promote_like(&mut self, target: SigId, source: SigId) -> Result<SigId, NormalFormError> {
        let promoted_source = self.promote(source)?;
        self.smart_cast(target, source, promoted_source)
    }

    fn same_type(&self, left: SigId, right: SigId) -> Result<bool, NormalFormError> {
        Ok(self.kind(left)? == self.kind(right)?)
    }

    fn kind(&self, sig: SigId) -> Result<ReducedSigKind, NormalFormError> {
        match match_sig(self.arena, sig) {
            SigMatch::Int(_) => return Ok(ReducedSigKind::Int),
            SigMatch::Real(_) => return Ok(ReducedSigKind::Real),
            SigMatch::Soundfile(_) => return Ok(ReducedSigKind::Sound),
            _ => {}
        }
        let ty = self.types.get(&sig).ok_or_else(|| {
            NormalFormError::Type(format!(
                "missing canonical type for signal {} during promotion: {}",
                sig.as_u32(),
                dump_sig_readable(self.arena, sig)
            ))
        })?;
        Ok(match ty.nature() {
            Nature::Int => ReducedSigKind::Int,
            Nature::Real | Nature::Any => ReducedSigKind::Real,
        })
    }

    fn clone_generic(&mut self, sig: SigId) -> Result<SigId, NormalFormError> {
        let node = self.arena.node(sig).cloned().ok_or_else(|| {
            NormalFormError::Type(format!("missing node {} during promotion", sig.as_u32()))
        })?;
        let mut promoted_children = Vec::with_capacity(node.children.len());
        for child in node.children.as_slice() {
            promoted_children.push(self.promote(*child)?);
        }
        Ok(self.arena.intern(node.kind, &promoted_children))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use signals::{SigBuilder, SigMatch, match_sig};
    use tlib::TreeArena;
    use ui::UiProgram;

    use super::*;

    fn arena() -> TreeArena {
        TreeArena::new()
    }

    fn ui() -> UiProgram {
        UiProgram::empty()
    }

    // ── prepare_signals — trivial signals ─────────────────────────────────

    #[test]
    fn prepare_signals_integer_constant() {
        // A plain integer constant passes through unchanged.
        let mut a = arena();
        let u = ui();
        let i42 = SigBuilder::new(&mut a).int(42);
        let (r, types) = prepare_signals(&mut a, &u, i42, &NormalFormOpts::default()).unwrap();
        assert_eq!(match_sig(&a, r), SigMatch::Int(42));
        assert!(types.contains_key(&r));
    }

    #[test]
    fn prepare_signals_audio_input() {
        // An input signal passes through; types must be available.
        let mut a = arena();
        let u = ui();
        let x = SigBuilder::new(&mut a).input(0);
        let (r, types) = prepare_signals(&mut a, &u, x, &NormalFormOpts::default()).unwrap();
        // Input may stay as-is or be promoted; types must have an entry for r.
        assert!(types.contains_key(&r) || types.contains_key(&x));
    }

    #[test]
    fn prepare_signals_add_expression() {
        // An add expression on two inputs: all three nodes typed.
        let mut a = arena();
        let u = ui();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let add = SigBuilder::new(&mut a).add(x, y);
        let opts = NormalFormOpts {
            skip_promotion: true,
        };
        let (_r, types) = prepare_signals(&mut a, &u, add, &opts).unwrap();
        // At minimum, the de-Bruijn-converted form should be typed.
        assert!(!types.is_empty());
    }

    // ── prepare_signals_multi ─────────────────────────────────────────────

    #[test]
    fn prepare_signals_multi_two_outputs() {
        let mut a = arena();
        let u = ui();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let opts = NormalFormOpts {
            skip_promotion: true,
        };
        let (rs, _types) = prepare_signals_multi(&mut a, &u, &[x, y], &opts).unwrap();
        assert_eq!(rs.len(), 2);
    }

    #[test]
    fn prepare_signals_multi_rejects_incoherent_de_bruijn_before_conversion() {
        let mut a = arena();
        let u = ui();
        let zero_level = tlib::de_bruijn_ref(&mut a, 0);
        let root = tlib::de_bruijn_rec(&mut a, zero_level);
        let opts = NormalFormOpts {
            skip_promotion: true,
        };

        let err = prepare_signals_multi(&mut a, &u, &[root], &opts)
            .expect_err("zero-level de Bruijn ref must fail the coherence pre-check");
        assert!(
            err.to_string().contains("DEBRUIJNREF(level=0) at depth=1"),
            "unexpected error: {err}"
        );
    }

    // ── promote_signals ───────────────────────────────────────────────────

    #[test]
    fn promote_signals_no_change_for_same_type_binop() {
        // int + int should not insert casts.
        let mut a = arena();
        let u = ui();
        let i3 = SigBuilder::new(&mut a).int(3);
        let i4 = SigBuilder::new(&mut a).int(4);
        let add = SigBuilder::new(&mut a).add(i3, i4);
        let opts = NormalFormOpts {
            skip_promotion: true,
        };
        let (_r, types) = prepare_signals(&mut a, &u, add, &opts).unwrap();
        let promoted = promote_signals(&mut a, &types, &[add]);
        // With same-type operands, the promoted result should not wrap in a cast.
        assert_eq!(promoted.len(), 1);
    }

    #[test]
    fn promote_signals_preserves_filter_carriers_and_normalizes_children() {
        let mut a = arena();
        let u = ui();
        let x = SigBuilder::new(&mut a).input(0);
        let i_coef = SigBuilder::new(&mut a).int(1);
        let r_coef = SigBuilder::new(&mut a).real(0.5);
        let fir = SigBuilder::new(&mut a).fir(&[x, i_coef, r_coef]);
        let opts = NormalFormOpts {
            skip_promotion: true,
        };
        let (_r, types) = prepare_signals(&mut a, &u, fir, &opts).unwrap();

        let promoted = promote_signals(&mut a, &types, &[fir]);

        let SigMatch::Fir(coefs) = match_sig(&a, promoted[0]) else {
            panic!("expected promoted FIR carrier");
        };
        assert_eq!(coefs[0], x);
        assert_eq!(coefs[1], i_coef);
        assert_eq!(coefs[2], r_coef);
    }

    #[test]
    fn promote_signals_preserves_iir_carrier_shape() {
        let mut a = arena();
        let u = ui();
        let target = SigBuilder::new(&mut a).input(0);
        let input = SigBuilder::new(&mut a).input(1);
        let fb = SigBuilder::new(&mut a).real(-0.5);
        let iir = SigBuilder::new(&mut a).iir(&[target, input, fb]);
        let opts = NormalFormOpts {
            skip_promotion: true,
        };
        let (_r, types) = prepare_signals(&mut a, &u, iir, &opts).unwrap();

        let promoted = promote_signals(&mut a, &types, &[iir]);

        let SigMatch::Iir(coefs) = match_sig(&a, promoted[0]) else {
            panic!("expected promoted IIR carrier");
        };
        assert_eq!(coefs, [target, input, fb]);
    }

    // ── NormalFormError conversion ────────────────────────────────────────

    #[test]
    fn error_display() {
        let e = NormalFormError::Type("bad type".to_string());
        assert!(e.to_string().contains("bad type"));
        let e2 = NormalFormError::Recursion("open group".to_string());
        assert!(e2.to_string().contains("open group"));
    }
}
