//! Verification boundary for the prepared signal forest.
//!
//! # What this module certifies
//!
//! After [`super::prepare_signals_for_fir_unverified`] builds a staged forest,
//! [`PreparedSignals::verify`] (implemented here) checks the postconditions that
//! the fast-lane FIR lowerer assumes hold on every input forest.  The postconditions
//! mirror the §0.2 boundary contract from the companion design document
//! `porting/signal-prepare-simplification-experiment-2026-06-22-en.md`:
//!
//! | Postcondition | What the checker enforces |
//! |---|---|
//! | **Sym** — no de Bruijn recursion remains | every reachable node is free of `SIGREC`/`DBRECREF`; `SYMREC`/`SYMREF` groups are well-formed |
//! | **Type coverage** — every reachable node is annotated | `types` and `sig_types` maps both cover every node reached by the traversal |
//! | **Type consistency** — stored types match derived types | the stored `SimpleSigType` equals what `derive_simple_types` computes from `SigType` |
//! | **P — promotion invariant** | every binary/delay/select operator receives operands in the domain it requires; no implicit cast is needed at FIR lowering time |
//! | **D1 — canonical one-sample delay** | no `Delay(_, 1)` survives; the unique one-sample-delay form is `Delay1` |
//! | **Control existence** | every UI control reference names a control present in the `UiProgram` |
//! | **Projection bounds** | every `Proj(k, group)` has `k` in `0..arity(group)`; unary groups use `k = 0` |
//!
//! The verifier is a pure read-only structural walk — it does not mutate the forest.
//! Any violation is reported as [`super::SignalPrepareError::Validation`].
//!
//! # Relationship to `mod.rs`
//!
//! The **producer pipeline** (`prepare_signals_for_fir_unverified` and friends) lives in
//! `mod.rs`; this file holds only the checker.  The W4 debug-only inter-pass contract
//! assertions (`forest_has_de_bruijn`, `forest_has_delay_of_one`) also live in `mod.rs`
//! because they back the pipeline's `debug_assert!` blocks.  The verifier here is the
//! official postcondition gate called from `prepare_signals_for_fir` and
//! `prepare_signals_for_fir_verified`.

use std::collections::{HashMap, HashSet};

use signals::{BinOp, SigId, SigMatch, match_sig};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};
use ui::UiProgram;

use super::{PreparedSignals, SignalPrepareError, SimpleSigType};

// ── impl PreparedSignals ─────────────────────────────────────────────────────

impl PreparedSignals {
    /// Verifies the documented postconditions of the prepared staging forest.
    ///
    /// This is intentionally a structural boundary verifier:
    /// - it checks only properties already guaranteed or already assumed by the
    ///   fast-lane,
    /// - it does not change the forest,
    /// - it fails close to the stage boundary when an invariant regresses.
    pub fn verify(&self, ui: &UiProgram) -> Result<(), SignalPrepareError> {
        let derived_types = super::derive_simple_types(&self.arena, &self.sig_types);
        let mut visited = HashSet::new();
        let mut reachable_typed_nodes = Vec::new();
        let mut sym_group_arities = HashMap::new();

        for &out in &self.outputs {
            verify_prepared_signal(
                &self.arena,
                ui,
                out,
                &mut visited,
                &mut sym_group_arities,
                &mut reachable_typed_nodes,
            )?;
        }

        for sig in reachable_typed_nodes {
            let Some(actual_reduced) = self.types.get(&sig).copied() else {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared signal {} is reachable but missing reduced type annotation",
                    sig.as_u32()
                )));
            };
            let Some(actual_full) = self.sig_types.get(&sig) else {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared signal {} is reachable but missing full SigType annotation",
                    sig.as_u32()
                )));
            };
            let Some(expected_reduced) = derived_types.get(&sig).copied() else {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared signal {} is reachable but has no derived reduced type",
                    sig.as_u32()
                )));
            };
            if actual_reduced != expected_reduced {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared signal {} has inconsistent reduced type: stored={actual_reduced:?}, derived={expected_reduced:?}, full={actual_full:?}",
                    sig.as_u32()
                )));
            }
            verify_promotion_invariant(&self.arena, &self.types, sig)?;
        }

        Ok(())
    }

    /// Consumes this prepared forest and returns a verified wrapper when the
    /// boundary checks succeed.
    pub fn into_verified(
        self,
        ui: &UiProgram,
    ) -> Result<super::VerifiedPreparedSignals, SignalPrepareError> {
        self.verify(ui)?;
        Ok(super::VerifiedPreparedSignals { inner: self })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

pub(super) fn verify_prepared_output_arity(
    expected: usize,
    actual: usize,
) -> Result<(), SignalPrepareError> {
    if expected == actual {
        return Ok(());
    }
    Err(SignalPrepareError::Validation(format!(
        "prepared output arity changed across staging: expected {expected}, got {actual}"
    )))
}

fn verify_prepared_signal(
    arena: &TreeArena,
    ui: &UiProgram,
    sig: SigId,
    visited: &mut HashSet<SigId>,
    sym_group_arities: &mut HashMap<SigId, usize>,
    reachable_typed_nodes: &mut Vec<SigId>,
) -> Result<(), SignalPrepareError> {
    if !visited.insert(sig) {
        return Ok(());
    }
    if arena.is_nil(sig) || arena.is_list(sig) {
        return Err(SignalPrepareError::Validation(format!(
            "prepared signal traversal reached unexpected list/nil node {}",
            sig.as_u32()
        )));
    }
    if tlib::match_de_bruijn_rec(arena, sig).is_some()
        || tlib::match_de_bruijn_ref(arena, sig).is_some()
    {
        return Err(SignalPrepareError::Validation(format!(
            "prepared signal {} still contains de Bruijn recursion form",
            sig.as_u32()
        )));
    }
    if match_sym_ref(arena, sig).is_some() {
        return Err(SignalPrepareError::Validation(format!(
            "prepared signal {} unexpectedly exposes a bare symbolic recursion reference",
            sig.as_u32()
        )));
    }
    if let Some((var, body_list)) = match_sym_rec(arena, sig) {
        reachable_typed_nodes.push(sig);
        let bodies = list_to_vec(arena, body_list).ok_or_else(|| {
            SignalPrepareError::Validation(
                "malformed symbolic recursion body list in prepared signals".to_owned(),
            )
        })?;
        if bodies.is_empty() {
            return Err(SignalPrepareError::Validation(format!(
                "symbolic recursion group {} has empty body list",
                sig.as_u32()
            )));
        }
        match sym_group_arities.insert(var, bodies.len()) {
            Some(previous) if previous != bodies.len() => {
                return Err(SignalPrepareError::Validation(format!(
                    "symbolic recursion variable {} was observed with inconsistent arities: {previous} vs {}",
                    var.as_u32(),
                    bodies.len()
                )));
            }
            _ => {}
        }
        for body in bodies {
            verify_prepared_signal(
                arena,
                ui,
                body,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        return Ok(());
    }

    reachable_typed_nodes.push(sig);
    match match_sig(arena, sig) {
        SigMatch::Unknown => {
            return Err(SignalPrepareError::Validation(format!(
                "prepared signal {} could not be decoded by match_sig",
                sig.as_u32()
            )));
        }
        SigMatch::Int(_)
        | SigMatch::Real(_)
        | SigMatch::Input(_)
        | SigMatch::Button(_)
        | SigMatch::Checkbox(_)
        | SigMatch::VSlider(_)
        | SigMatch::HSlider(_)
        | SigMatch::NumEntry(_)
        | SigMatch::Soundfile(_)
        | SigMatch::FConst(_, _, _)
        | SigMatch::FVar(_, _, _) => {}
        SigMatch::Output(_, inner)
        | SigMatch::Delay1(inner)
        | SigMatch::IntCast(inner)
        | SigMatch::BitCast(inner)
        | SigMatch::FloatCast(inner)
        | SigMatch::Gen(inner)
        | SigMatch::Lowest(inner)
        | SigMatch::Highest(inner)
        | SigMatch::Acos(inner)
        | SigMatch::Asin(inner)
        | SigMatch::Atan(inner)
        | SigMatch::Cos(inner)
        | SigMatch::Sin(inner)
        | SigMatch::Tan(inner)
        | SigMatch::Exp(inner)
        | SigMatch::Exp10(inner)
        | SigMatch::Log(inner)
        | SigMatch::Log10(inner)
        | SigMatch::Sqrt(inner)
        | SigMatch::Abs(inner)
        | SigMatch::Floor(inner)
        | SigMatch::Ceil(inner)
        | SigMatch::Rint(inner)
        | SigMatch::Round(inner)
        | SigMatch::TempVar(inner)
        | SigMatch::PermVar(inner) => verify_prepared_signal(
            arena,
            ui,
            inner,
            visited,
            sym_group_arities,
            reachable_typed_nodes,
        )?,
        // `Clocked(env, y)`: the first child is an opaque clock-environment
        // annotation (nil at the top-level rate or a `SIGCLOCKENV` token into
        // the propagation-owned clock-domain table), NOT a signal. Visiting it
        // as a signal is exactly the P0.1 bug: the env is not a signal node,
        // carries no type annotation, and must never enter the typed
        // reachability set. Only the wrapped signal `y` is verified.
        SigMatch::Clocked(env, y) => {
            let env_is_opaque =
                arena.is_nil(env) || matches!(match_sig(arena, env), SigMatch::ClockEnvToken(_));
            if !env_is_opaque {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared Clocked signal {} carries a malformed clock-env child {} \
                     (expected nil or a SIGCLOCKENV token)",
                    sig.as_u32(),
                    env.as_u32()
                )));
            }
            verify_prepared_signal(
                arena,
                ui,
                y,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        // A clock-env token reached directly in signal position is a
        // structural error: it is only legal as the first child of `Clocked`.
        SigMatch::ClockEnvToken(id) => {
            return Err(SignalPrepareError::Validation(format!(
                "clock-env token #{id} (node {}) reached signal position; \
                 it is only legal as the opaque first child of Clocked(env, y)",
                sig.as_u32()
            )));
        }
        SigMatch::Delay(x, y)
        | SigMatch::Prefix(x, y)
        | SigMatch::RdTbl(x, y)
        | SigMatch::Pow(x, y)
        | SigMatch::Min(x, y)
        | SigMatch::Max(x, y)
        | SigMatch::Atan2(x, y)
        | SigMatch::Fmod(x, y)
        | SigMatch::Remainder(x, y)
        | SigMatch::Attach(x, y)
        | SigMatch::Enable(x, y)
        | SigMatch::Control(x, y)
        | SigMatch::Seq(x, y)
        | SigMatch::ZeroPad(x, y) => {
            verify_prepared_signal(
                arena,
                ui,
                x,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
            verify_prepared_signal(
                arena,
                ui,
                y,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        SigMatch::Fir(coefs) | SigMatch::Iir(coefs) => {
            if coefs.is_empty() {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared filter carrier {} has an empty coefficient vector",
                    sig.as_u32()
                )));
            }
            for &child in coefs {
                verify_prepared_signal(
                    arena,
                    ui,
                    child,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
            }
        }
        SigMatch::BinOp(_, x, y) => {
            verify_prepared_signal(
                arena,
                ui,
                x,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
            verify_prepared_signal(
                arena,
                ui,
                y,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        SigMatch::Select2(selector, then_value, else_value)
        | SigMatch::AssertBounds(selector, then_value, else_value) => {
            verify_prepared_signal(
                arena,
                ui,
                selector,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
            verify_prepared_signal(
                arena,
                ui,
                then_value,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
            verify_prepared_signal(
                arena,
                ui,
                else_value,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        SigMatch::WrTbl(size, generator, write_index, write_signal) => {
            for child in [size, generator] {
                verify_prepared_signal(
                    arena,
                    ui,
                    child,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
            }
            let readonly = arena.is_nil(write_index) && arena.is_nil(write_signal);
            let malformed_write_pair = arena.is_nil(write_index) ^ arena.is_nil(write_signal);
            if malformed_write_pair {
                return Err(SignalPrepareError::Validation(format!(
                    "write table {} uses inconsistent readonly/write placeholders",
                    sig.as_u32()
                )));
            }
            if !readonly {
                for child in [write_index, write_signal] {
                    verify_prepared_signal(
                        arena,
                        ui,
                        child,
                        visited,
                        sym_group_arities,
                        reachable_typed_nodes,
                    )?;
                }
            }
        }
        SigMatch::FFun(_, arg_list) => {
            let args = list_to_vec(arena, arg_list).ok_or_else(|| {
                SignalPrepareError::Validation(
                    "malformed foreign-function argument list in prepared signals".to_owned(),
                )
            })?;
            for arg in args {
                verify_prepared_signal(
                    arena,
                    ui,
                    arg,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
            }
        }
        SigMatch::Proj(index, group) => {
            if index < 0 {
                return Err(SignalPrepareError::Validation(format!(
                    "projection {} uses negative index {index}",
                    sig.as_u32()
                )));
            }
            // Phase B0 RAD addition: a `Proj(slot, BlockReverseAD{..})` is
            // a legal projection over the carrier's `M + N` outputs
            // (M primals followed by N seed gradients). Validate the slot
            // bound here so downstream lowering can rely on it.
            if let SigMatch::BlockReverseAD {
                primal_count,
                seeds,
                ..
            } = match_sig(arena, group)
            {
                let seed_vec = list_to_vec(arena, seeds).ok_or_else(|| {
                    SignalPrepareError::Validation(format!(
                        "projection {} targets BlockReverseAD whose seed list is malformed",
                        sig.as_u32()
                    ))
                })?;
                let arity = i32::try_from(seed_vec.len())
                    .ok()
                    .and_then(|n| primal_count.checked_add(n))
                    .ok_or_else(|| {
                        SignalPrepareError::Validation(format!(
                            "projection {} targets BlockReverseAD whose total arity overflows i32",
                            sig.as_u32()
                        ))
                    })?;
                if index >= arity {
                    return Err(SignalPrepareError::Validation(format!(
                        "projection {} index {index} is out of range for BlockReverseAD arity {arity}",
                        sig.as_u32()
                    )));
                }
                verify_prepared_signal(
                    arena,
                    ui,
                    group,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
                return Ok(());
            }
            let reverse_group_body = match match_sig(arena, group) {
                SigMatch::ReverseTimeRec(body) => Some(body),
                _ => None,
            };
            let projection_group = reverse_group_body.unwrap_or(group);
            let arity = if let Some((var, _)) = match_sym_rec(arena, projection_group) {
                verify_prepared_signal(
                    arena,
                    ui,
                    group,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
                sym_group_arities.get(&var).copied().ok_or_else(|| {
                    SignalPrepareError::Validation(format!(
                        "projection {} targets recursion group {} without registered arity",
                        sig.as_u32(),
                        group.as_u32()
                    ))
                })?
            } else if let Some(var) = match_sym_ref(arena, projection_group) {
                sym_group_arities.get(&var).copied().ok_or_else(|| {
                    SignalPrepareError::Validation(format!(
                        "projection {} targets symbolic recursion ref {} before its group arity is known",
                        sig.as_u32(),
                        var.as_u32()
                    ))
                })?
            } else {
                return Err(SignalPrepareError::Validation(format!(
                    "projection {} does not target symbolic recursion",
                    sig.as_u32()
                )));
            };
            let index = usize::try_from(index).expect("negative indices rejected above");
            if index >= arity {
                return Err(SignalPrepareError::Validation(format!(
                    "projection {} index {index} is out of range for recursion arity {arity}",
                    sig.as_u32()
                )));
            }
            if arity == 1 && index != 0 {
                return Err(SignalPrepareError::Validation(format!(
                    "projection {} targets unary recursion with non-canonical index {index}",
                    sig.as_u32()
                )));
            }
        }
        SigMatch::Rec(_) => {
            return Err(SignalPrepareError::Validation(format!(
                "prepared signal {} still contains legacy SIGREC form",
                sig.as_u32()
            )));
        }
        SigMatch::BlockReverseAD {
            body,
            primal_count,
            seeds,
            cotangents,
            ..
        } => {
            // Phase B0 validation rules — the carrier is otherwise opaque
            // to downstream lowering, so we lock the structural invariants
            // that future RAD lowering will rely on:
            //
            //   1. body is a non-empty cons list of M signals;
            //   2. cotangents is a cons list of exactly M signals;
            //   3. primal_count matches body.len() and is non-negative;
            //   4. every child signal is itself well-formed.
            //
            // Source provenance: original Rust design in
            // `porting/rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`,
            // section "11.1 Phase B0 — Signal carrier + minimal validation".
            if primal_count < 0 {
                return Err(SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} declares negative primal_count {primal_count}",
                    sig.as_u32()
                )));
            }
            let body_vec = list_to_vec(arena, body).ok_or_else(|| {
                SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} body is not a well-formed cons list",
                    sig.as_u32()
                ))
            })?;
            let seed_vec = list_to_vec(arena, seeds).ok_or_else(|| {
                SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} seed list is malformed",
                    sig.as_u32()
                ))
            })?;
            let cot_vec = list_to_vec(arena, cotangents).ok_or_else(|| {
                SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} cotangent list is malformed",
                    sig.as_u32()
                ))
            })?;
            if body_vec.is_empty() {
                return Err(SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} requires at least one primal output",
                    sig.as_u32()
                )));
            }
            if body_vec.len() != cot_vec.len() {
                return Err(SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} body length {} does not match cotangent length {}",
                    sig.as_u32(),
                    body_vec.len(),
                    cot_vec.len()
                )));
            }
            if usize::try_from(primal_count).ok() != Some(body_vec.len()) {
                return Err(SignalPrepareError::Validation(format!(
                    "BlockReverseAD {} primal_count {primal_count} disagrees with body length {}",
                    sig.as_u32(),
                    body_vec.len()
                )));
            }
            for child in body_vec.into_iter().chain(seed_vec).chain(cot_vec) {
                verify_prepared_signal(
                    arena,
                    ui,
                    child,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
            }
        }
        SigMatch::ReverseTimeRec(body) => {
            verify_prepared_signal(
                arena,
                ui,
                body,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        SigMatch::VBargraph(control, inner) | SigMatch::HBargraph(control, inner) => {
            verify_control_exists(ui, control, sig)?;
            verify_prepared_signal(
                arena,
                ui,
                inner,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        SigMatch::Waveform(values)
        | SigMatch::OnDemand(values)
        | SigMatch::Upsampling(values)
        | SigMatch::Downsampling(values) => {
            for &child in values {
                verify_prepared_signal(
                    arena,
                    ui,
                    child,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
            }
        }
        SigMatch::SoundfileLength(soundfile, part) | SigMatch::SoundfileRate(soundfile, part) => {
            verify_prepared_signal(
                arena,
                ui,
                soundfile,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
            verify_prepared_signal(
                arena,
                ui,
                part,
                visited,
                sym_group_arities,
                reachable_typed_nodes,
            )?;
        }
        SigMatch::SoundfileBuffer(soundfile, chan, part, ridx) => {
            for child in [soundfile, chan, part, ridx] {
                verify_prepared_signal(
                    arena,
                    ui,
                    child,
                    visited,
                    sym_group_arities,
                    reachable_typed_nodes,
                )?;
            }
        }
    }

    match match_sig(arena, sig) {
        SigMatch::Button(control)
        | SigMatch::Checkbox(control)
        | SigMatch::VSlider(control)
        | SigMatch::HSlider(control)
        | SigMatch::NumEntry(control)
        | SigMatch::Soundfile(control) => verify_control_exists(ui, control, sig),
        _ => Ok(()),
    }
}

fn verify_control_exists(
    ui: &UiProgram,
    control: ui::ControlId,
    sig: SigId,
) -> Result<(), SignalPrepareError> {
    if ui.control(control).is_some() {
        return Ok(());
    }
    Err(SignalPrepareError::Validation(format!(
        "prepared signal {} references missing UI control id {}",
        sig.as_u32(),
        control
    )))
}

/// Verifies the promotion invariant `P` and the one-sample-delay canonical form
/// `D1` for one prepared node, using the reduced type map.
///
/// `P` is the precondition the FIR lowering rules are documented against (see
/// `transform::signal_fir::module::build_module`): every operator already
/// receives operands in the domain it needs, so the lowerer never inserts
/// implicit casts. Until this check, the staging verifier enforced a predicate
/// strictly weaker than that precondition — `P` was caught only lazily inside
/// `lower_binop`, and `D1` was never re-guarded after canonicalization. See
/// `porting/signal-to-fir-rewriting-calculus-2026-06-20-en.md` §8.1.
///
/// The checks compare the reduced `SimpleSigType` domains, which map 1:1 onto the
/// FIR operand types used by lowering. A violation means a previous staging pass
/// regressed the contract, so it is reported close to the boundary rather than
/// deep inside FIR emission.
pub(super) fn verify_promotion_invariant(
    arena: &TreeArena,
    types: &HashMap<SigId, SimpleSigType>,
    sig: SigId,
) -> Result<(), SignalPrepareError> {
    let dom = |s: SigId| types.get(&s).copied();
    let int = Some(SimpleSigType::Int);

    match match_sig(arena, sig) {
        // D1: one-sample delays must be canonicalized to `Delay1`.
        SigMatch::Delay(_, amount) => {
            if matches!(match_sig(arena, amount), SigMatch::Int(1)) {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared signal {} is a non-canonical one-sample delay Delay(_, 1); expected Delay1",
                    sig.as_u32()
                )));
            }
            if dom(amount) != int {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared Delay {} has non-integer amount domain {:?}",
                    sig.as_u32(),
                    dom(amount)
                )));
            }
        }
        SigMatch::Prefix(init, value) => {
            if dom(init) != dom(value) {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared Prefix {} has mismatched init/value domains {:?} vs {:?}",
                    sig.as_u32(),
                    dom(init),
                    dom(value)
                )));
            }
        }
        SigMatch::RdTbl(_, index) => {
            if dom(index) != int {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared RdTbl {} has non-integer index domain {:?}",
                    sig.as_u32(),
                    dom(index)
                )));
            }
        }
        SigMatch::WrTbl(_, _, write_index, _) => {
            if !arena.is_nil(write_index) && dom(write_index) != int {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared WrTbl {} has non-integer write-index domain {:?}",
                    sig.as_u32(),
                    dom(write_index)
                )));
            }
        }
        SigMatch::Select2(selector, then_value, else_value) => {
            if dom(selector) != int {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared Select2 {} has non-integer selector domain {:?}",
                    sig.as_u32(),
                    dom(selector)
                )));
            }
            if dom(then_value) != dom(else_value) {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared Select2 {} has mismatched branch domains {:?} vs {:?}",
                    sig.as_u32(),
                    dom(then_value),
                    dom(else_value)
                )));
            }
        }
        SigMatch::Enable(_, gate) => {
            if dom(gate) != int {
                return Err(SignalPrepareError::Validation(format!(
                    "prepared Enable {} has non-integer gate domain {:?}",
                    sig.as_u32(),
                    dom(gate)
                )));
            }
        }
        SigMatch::BinOp(op, lhs, rhs) => {
            let is_comparison = matches!(
                op,
                BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne
            );
            if is_comparison {
                // Comparisons keep same-typed numeric operands; the result is Int.
                if dom(lhs) != dom(rhs)
                    || !matches!(dom(lhs), Some(SimpleSigType::Int | SimpleSigType::Real))
                {
                    return Err(SignalPrepareError::Validation(format!(
                        "prepared comparison {} has inconsistent operand domains {:?} vs {:?}",
                        sig.as_u32(),
                        dom(lhs),
                        dom(rhs)
                    )));
                }
            } else {
                // Arithmetic / Div / Rem / bitwise / shift: both operands must
                // already share the node's result domain (no implicit cast at
                // lowering).
                let node = dom(sig);
                if dom(lhs) != node || dom(rhs) != node {
                    return Err(SignalPrepareError::Validation(format!(
                        "prepared BinOp {} operands {:?}, {:?} do not match result domain {:?}",
                        sig.as_u32(),
                        dom(lhs),
                        dom(rhs),
                        node
                    )));
                }
            }
        }
        _ => {}
    }
    Ok(())
}
