//! Block Reverse AD (BRA) lowering — backward sweep, adjoint accumulation, tapes.
//!
//! Defines [`BraState`], the sub-state struct that groups all BRA-specific
//! fields previously scattered across `SignalToFirLower`.
//!
//! Implements the `impl SignalToFirLower` methods that lower `ReverseTimeRec`
//! recursion groups and their associated BRA tape stores, backward-sweep
//! loops, adjoint variables, and carry propagation.  The BRA pattern is the
//! Faust-specific realisation of reverse-mode automatic differentiation in a
//! block-processing DSP context.
//!
//! Note: as of 2026-05-10 the primary RAD dispatcher uses the forward-mode
//! algebraic RAD path; the BRA lowering here is preserved for the LTI
//! adjoint fast-path revival.

use super::*;

/// Grouped state for Block Reverse AD lowering.
#[derive(Default)]
pub(super) struct BraState {
    /// Guards against re-emitting the backward sweep for a `SigBlockReverseAD`
    /// group that has already been scheduled.  Keyed by the group `SigId`.
    pub(super) scheduled: HashSet<SigId>,
    /// Per-seed gradient `FirId` cache for emitted `SigBlockReverseAD` sweeps.
    ///
    /// Key: `(group_sig, seed_index)` where `seed_index` is the position of
    /// the seed in the carrier's seed list.  Populated by
    /// `ensure_bra_backward_sweep` and consumed by `lower_block_reverse_ad_proj`.
    pub(super) grad_cache: HashMap<(SigId, usize), FirId>,
    /// Carry variable names for `Delay1` nodes encountered inside a
    /// `SigBlockReverseAD` backward sweep.  Keyed by the `Delay1` node `SigId`.
    ///
    /// Each carry variable persists in the DSP struct and is zeroed by
    /// `emit_bra_compute_resets` before every reverse sample loop so that
    /// no adjoint state leaks across host `compute()` calls.
    pub(super) delay1_carry_vars: HashMap<SigId, String>,
    /// Carry array variable names and sizes for `Delay(c, x)` nodes (c > 1)
    /// encountered inside a `SigBlockReverseAD` backward sweep.
    ///
    /// Key: `Delay` node `SigId`.  Value: `(name, c)` where `name` is the
    /// struct-field name of the `Array(real_ty, c)` circular carry buffer.
    ///
    /// The carry implements the anti-causal adjoint: at reverse step n,
    /// `carry[n % c]` holds `adj[y][n + c]` from the previous c-th reverse
    /// step, contributing `adj[x][n] += carry[n % c]`.  The buffer is zeroed
    /// by `emit_bra_compute_resets` before each reverse sample loop.
    pub(super) delay_array_carry_vars: HashMap<SigId, (String, usize)>,
    /// Tape array variable names for signals recorded during the forward loop.
    ///
    /// Key: signal `SigId` whose forward value must be replayed in the reverse
    /// loop.  Value: the struct-field name of the `Array(real_ty,
    /// MAX_BRA_TAPE_BLOCK_SIZE)` used to store/load it.
    ///
    /// Populated by `ensure_bra_tape_stores` and consumed by
    /// `load_bra_fwd_value`.  Acts as a per-signal idempotency guard: a
    /// signal is never taped twice even when `ensure_bra_tape_stores` is
    /// called once per primal body slot.
    pub(super) tape_store_var: HashMap<SigId, String>,
}

impl<'a> SignalToFirLower<'a> {
    /// Emits `compute()`-preamble resets for `ReverseTimeRec` (LTI adjoint)
    /// recursion carriers.
    ///
    /// Dormant under the 2026-05-10 RAD dispatcher change; kept compilable for
    /// a future LTI fast-path revival.
    ///
    /// `ReverseTimeRec` has block-local adjoint semantics: the state one frame
    /// past `count - 1` is terminal-zero for every `compute()` call. Ordinary
    /// SYMREC primal carriers are only cleared by `instanceClear()` (they are
    /// persistent DSP state); only the LTI adjoint carriers belonging to
    /// `ReverseTimeRec` groups must be zeroed per-block.
    ///
    /// The distinction is made via `recursion.reverse_time_rec_group_ids`,
    /// which is populated by `allocate_group_arrays` when it sees a
    /// `SigMatch::ReverseTimeRec` group.  SYMREC carriers for BRA primal
    /// bodies are NOT in that set and are therefore skipped here.
    pub(super) fn emit_reverse_time_rec_compute_resets(&mut self) {
        let reverse_ids = self.recursion.reverse_time_rec_group_ids.clone();
        let mut carriers: Vec<_> = self
            .recursion
            .rec_array_by_group_index
            .iter()
            .filter(|&(&(group_id, _), _)| reverse_ids.contains(&group_id))
            .map(|(_, info)| info.clone())
            .collect();
        carriers.sort_by(|a, b| a.name.cmp(&b.name));
        carriers.dedup_by(|a, b| a.name == b.name);

        for info in carriers {
            let init = match info.typ {
                FirType::Int32 => self.lower_int32_const(0),
                FirType::Float32 | FirType::Float64 | FirType::FaustFloat => self.float_const(0.0),
                _ => continue,
            };
            if info.size == 1 {
                let mut b = FirBuilder::new(&mut self.store);
                self.sections.control_statements.push(b.store_var(
                    info.name,
                    AccessType::Struct,
                    init,
                ));
            } else {
                let loop_var = self.fresh_loop_var("lRevRec");
                let upper = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.int32(i32::try_from(info.size).unwrap_or(i32::MAX))
                };
                let body = {
                    let index = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.load_var(loop_var.clone(), AccessType::Loop, FirType::Int32)
                    };
                    let store = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.store_table(info.name, AccessType::Struct, index, init)
                    };
                    let mut b = FirBuilder::new(&mut self.store);
                    b.block(&[store])
                };
                let mut b = FirBuilder::new(&mut self.store);
                self.sections
                    .control_statements
                    .push(b.simple_for_loop(loop_var, upper, body, false));
            }
        }
    }

    // ── BlockReverseAD (Phase B3) ─────────────────────────────────────────

    /// Emits `compute()`-preamble resets for `SigBlockReverseAD` adjoint carry
    /// variables.
    ///
    /// Each carry variable stores the anti-causal adjoint contribution for a
    /// `Delay1` node inside the BRA body across reverse-loop samples.  Like
    /// `ReverseTimeRec` adjoint carriers, these must be zeroed before each
    /// reverse sample loop so no adjoint state leaks across host `compute()`
    /// calls.
    pub(super) fn emit_bra_compute_resets(&mut self) {
        // Scalar Delay1 / Prefix carry resets.
        let mut names: Vec<String> = self.bra.delay1_carry_vars.values().cloned().collect();
        names.sort();
        for name in names {
            let zero = self.float_const(0.0);
            let mut b = FirBuilder::new(&mut self.store);
            self.sections
                .control_statements
                .push(b.store_var(name, AccessType::Struct, zero));
        }
        // Array Delay(c) carry resets: zero c elements via a small for-loop.
        let mut array_entries: Vec<(String, usize)> =
            self.bra.delay_array_carry_vars.values().cloned().collect();
        array_entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, c) in array_entries {
            let zero = self.float_const(0.0);
            let loop_var = self.fresh_loop_var("lBraDlyRst");
            let upper = {
                let mut b = FirBuilder::new(&mut self.store);
                b.int32(i32::try_from(c).unwrap_or(i32::MAX))
            };
            let body = {
                let idx = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.load_var(loop_var.clone(), AccessType::Loop, FirType::Int32)
                };
                let store = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.store_table(name, AccessType::Struct, idx, zero)
                };
                let mut b = FirBuilder::new(&mut self.store);
                b.block(&[store])
            };
            let mut b = FirBuilder::new(&mut self.store);
            self.sections
                .control_statements
                .push(b.simple_for_loop(loop_var, upper, body, false));
        }
    }

    /// Lowers a `Proj(index, BlockReverseAD)` node.
    ///
    /// - Slots `0 .. primal_count - 1` are **primal** outputs: the body
    ///   expression at that index is lowered directly in the forward sample
    ///   loop.
    /// - Slots `primal_count .. primal_count + seeds.len() - 1` are
    ///   **gradient** outputs: `ensure_bra_backward_sweep` is called once to
    ///   emit the TBPTT(BS, BS) adjoint sweep in the **current** sample-loop
    ///   slice, and the per-seed adjoint `FirId` is returned from the cache.
    ///
    /// “Current” is deliberate.  For a public gradient output the current slice
    /// is the reverse loop built by `build_module`.  For an internal gradient
    /// used by a forward recursive update, the current slice is the forward
    /// loop body currently being lowered.  The sweep code itself is the same;
    /// only its placement differs.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_block_reverse_ad_proj(
        &mut self,
        _node: SigId,
        group: SigId,
        index: usize,
        primal_count: usize,
        body_sigs: &[SigId],
        seed_sigs: &[SigId],
        cotangent_sigs: &[SigId],
    ) -> Result<FirId, SignalFirError> {
        if index < primal_count {
            // Primal projection: lower the body signal and schedule tape stores
            // for signals reachable from THIS body only (Phase B4).
            //
            // Each body is lowered in its own SYMREC recursion context, so
            // `lower_signal` for body[index] works correctly under that body's
            // recursion variable.  Passing only `body_sigs[index]` ensures that
            // `ensure_bra_tape_stores` never tries to lower signals from a
            // different SYMREC group whose recursion variable is not yet on the
            // stack.  A per-signal guard inside the function prevents duplicate
            // tape declarations when bodies share sub-expressions.
            let val = self.lower_signal(body_sigs[index])?;
            self.ensure_bra_tape_stores(group, &[body_sigs[index]], seed_sigs, cotangent_sigs)?;
            return Ok(val);
        }
        let seed_index = index - primal_count;
        self.ensure_bra_backward_sweep(group, body_sigs, seed_sigs, cotangent_sigs)?;
        self.bra
            .grad_cache
            .get(&(group, seed_index))
            .copied()
            .ok_or_else(|| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!(
                        "BRA backward sweep did not produce gradient for seed index {seed_index}"
                    ),
                )
            })
    }

    /// Ensures the TBPTT(BS, BS) backward adjoint sweep for `group` has been
    /// emitted into the current sample-loop phase.
    ///
    /// The phase may be the explicit reverse loop for public RAD gradient
    /// outputs, or the forward loop when the gradient projection is an internal
    /// operand of a causal expression.  This function should therefore avoid
    /// assuming `self.lowering_reverse_loop == true`; it emits a local transpose
    /// program against the loop variable `i0` and lets the caller's scheduling
    /// context determine whether `i0` advances forward or backward in generated
    /// C++.
    ///
    /// The sweep is emitted **at most once** per group per loop slice; the
    /// `bra_state_scheduled` guard prevents re-emission when multiple gradient
    /// projection slots for the same carrier are lowered.
    ///
    /// # Algorithm
    ///
    /// 1. Build a unified postorder over all body roots (shared `visited` set
    ///    handles DAG-shared sub-expressions).
    /// 2. Lower each cotangent signal into a FIR value (constant `1.0` in the
    ///    all-ones B1 convention).
    ///    3a. Pre-seed recursive feedback carries (Phase B6).  For each
    ///    `Delay1(Proj(slot, SYMREF(var)))` node in the postorder, load the
    ///    corresponding carry struct field (written by the previous reverse step)
    ///    and accumulate it into `adj[body_sigs[slot]]`.  This ensures the total
    ///    TBPTT adjoint `cotangent[n] + carry_from_step_n+1` is available when
    ///    the `Proj(slot, SYMREC)` node is processed first in the reverse
    ///    postorder.
    ///    3b. Seed the adjoint map: `adj[body_sigs[k]] += cotangent_firs[k]`.
    /// 4. Walk the postorder in reverse, calling `propagate_bra_adj` for each
    ///    node to distribute its accumulated adjoint to its children.
    /// 5. Store per-seed gradient `FirId`s into `bra_grad_cache`.
    pub(super) fn ensure_bra_backward_sweep(
        &mut self,
        group: SigId,
        body_sigs: &[SigId],
        seed_sigs: &[SigId],
        cotangent_sigs: &[SigId],
    ) -> Result<(), SignalFirError> {
        if !self.bra.scheduled.insert(group) {
            return Ok(());
        }

        // 1. Collect unified postorder.
        let mut visited = std::collections::HashSet::new();
        let mut postorder = Vec::new();
        for &body in body_sigs {
            collect_bra_postorder(self.arena, body, &mut visited, &mut postorder);
        }

        // 2. Lower cotangent signals.
        let mut cot_firs = Vec::with_capacity(cotangent_sigs.len());
        for &c in cotangent_sigs {
            cot_firs.push(self.lower_signal(c)?);
        }

        // 3. Seed the adjoint map.
        let mut adj: std::collections::HashMap<SigId, FirId> = std::collections::HashMap::new();

        // 3a. Pre-seed recursive feedback carries.
        //
        // In TBPTT the total adjoint of a recursive output `y[slot][n]` is:
        //
        //   adj[y[slot][n]] = cotangent[slot][n] + carry_from_step_n+1
        //
        // The carry from step n+1 encodes `adj[y[slot][n+1]] · ∂y[n+1]/∂y[n]`
        // and is stored in a struct field written during the previous reverse-loop
        // iteration.  We load it here — before the reverse-postorder walk — and
        // accumulate it into the matching `body_sig` so that when the Proj-SYMREC
        // node is processed first in the reverse postorder its `y_bar` already
        // includes the feedback contribution.
        //
        // `Delay1(Proj(slot, SYMREF(var)))` is the structural signal that
        // introduces the one-sample feedback delay; its carry variable represents
        // the anti-causal adjoint flowing from step n+1 back to step n.
        //
        // For circuits with multiple independent SYMREC groups (e.g., two
        // separate recursive poles), each group has its own SYMREF variable and
        // its own SYMREC variable.  We must match `SYMREF(var)` against the
        // corresponding `Proj(slot, SYMREC(var, ...))` in `body_sigs` by
        // comparing the symbolic recursion variable — NOT by using `slot` as a
        // flat index into `body_sigs` (which would be wrong when multiple groups
        // all have slot=0).
        //
        // Build: (SYMREC var TreeId, proj slot) → body_sig  from body_sigs.
        let mut var_slot_to_body_sig: HashMap<(TreeId, usize), SigId> = HashMap::new();
        for &body_sig in body_sigs {
            if let SigMatch::Proj(bslot, bgroup) = match_sig(self.arena, body_sig)
                && let Some((bvar, _)) = match_sym_rec(self.arena, bgroup)
            {
                let bslot_usize = usize::try_from(bslot).unwrap_or(usize::MAX);
                var_slot_to_body_sig.insert((bvar, bslot_usize), body_sig);
            }
        }

        for &sig in &postorder {
            if let SigMatch::Delay1(x) = match_sig(self.arena, sig)
                && let SigMatch::Proj(slot, inner_group) = match_sig(self.arena, x)
                && let Some(ref_var) = match_sym_ref(self.arena, inner_group)
            {
                let slot_usize = usize::try_from(slot).unwrap_or(usize::MAX);
                // Look up the body_sig whose SYMREC var matches this SYMREF var.
                if let Some(&proj_symrec) = var_slot_to_body_sig.get(&(ref_var, slot_usize)) {
                    let carry_name = self.ensure_bra_delay1_carry(sig, group)?;
                    let carry_load = {
                        let rt = self.real_ty();
                        let mut b = FirBuilder::new(&mut self.store);
                        b.load_var(carry_name, AccessType::Struct, rt)
                    };
                    let real_ty = self.real_ty.clone();
                    Self::add_to_adjoint(
                        &mut self.store,
                        &mut adj,
                        proj_symrec,
                        carry_load,
                        real_ty,
                    );
                }
            }
        }

        // 3b. Seed cotangent contributions.
        for (k, &body_sig) in body_sigs.iter().enumerate() {
            let cot = cot_firs[k];
            Self::add_to_adjoint(
                &mut self.store,
                &mut adj,
                body_sig,
                cot,
                self.real_ty.clone(),
            );
        }

        // 4. Backward propagation in reverse postorder.
        for &sig in postorder.iter().rev() {
            let y_bar = match adj.get(&sig).copied() {
                Some(fir) => fir,
                None => continue,
            };
            self.propagate_bra_adj(sig, y_bar, &mut adj, group)?;
        }

        // 5. Cache gradient FirIds.
        for (j, &seed) in seed_sigs.iter().enumerate() {
            let grad = adj
                .get(&seed)
                .copied()
                .unwrap_or_else(|| self.float_const(0.0));
            self.bra.grad_cache.insert((group, j), grad);
        }

        Ok(())
    }

    /// Propagates the adjoint `y_bar` of `sig` to the signal's children,
    /// updating `adj` according to the chain rule for each supported node kind.
    ///
    /// **Delay1** is anti-causal: rather than contributing directly to `adj[x]`,
    /// it reads the carry variable (written by the *next* reverse-loop step)
    /// as `adj[x]` and schedules a carry write to `post_output` for the
    /// *previous* reverse-loop step.  This matches the TBPTT(BS, BS) reference
    /// executor in `crates/compiler/tests/block_reverse_ad.rs`.
    ///
    /// **Phase B4 tape**: for `Mul`, `Div`, and unary math nodes whose operand
    /// value must be replayed from the forward pass, this method uses
    /// [`Self::load_bra_fwd_value`] instead of `lower_signal`.  When a tape
    /// array was declared by `ensure_bra_tape_stores` for that signal, the
    /// tape load is emitted; otherwise `lower_signal` is called (safe for
    /// trivially reverse-evaluable signals).
    ///
    /// Unsupported node kinds return a `SignalFirError::UnsupportedSignalNode`.
    pub(super) fn propagate_bra_adj(
        &mut self,
        sig: SigId,
        y_bar: FirId,
        adj: &mut std::collections::HashMap<SigId, FirId>,
        group: SigId,
    ) -> Result<(), SignalFirError> {
        let real_ty = self.real_ty.clone();
        let decoded = match_sig(self.arena, sig);
        if let Some((rule, x)) = rad_unary_math_rule(&decoded) {
            return self.propagate_bra_unary_math_adj(rule, sig, x, y_bar, adj);
        }
        if let Some((rule, lhs, rhs)) = rad_binary_math_rule(&decoded) {
            return self.propagate_bra_binary_math_adj(rule, lhs, rhs, sig, y_bar, adj);
        }
        match decoded {
            // ── Leaves ─────────────────────────────────────────────────────
            SigMatch::Real(_)
            | SigMatch::Int(_)
            | SigMatch::Input(_)
            | SigMatch::HSlider(_)
            | SigMatch::VSlider(_)
            | SigMatch::NumEntry(_)
            | SigMatch::Button(_)
            | SigMatch::Checkbox(_)
            // Foreign constants (e.g. `ma.SR`, `ma.PI` pulled in via stdfaust.lib)
            // and foreign variables are external scalars with no differentiable children.
            // Gradient contribution is zero; nothing to propagate.
            | SigMatch::FConst(..)
            | SigMatch::FVar(..) => {
                // Seeds, constants, or external scalars: no children to propagate into.
            }

            // ── Casts: identity rule for real casts, gradient stop for int→real ─
            SigMatch::FloatCast(x) => {
                // `signalPromotion` inserts `FloatCast` where an Int-valued
                // signal is used in a Real context.  Example:
                //
                //   i[n]  = 1103515245*i[n-1] + 12345     // Int LCG state
                //   x[n]  = float(i[n]) * 4.656612873e-10 // Real noise sample
                //
                // RAD differentiates the Real expression starting at `x[n]`;
                // it does not reinterpret the upstream Int recurrence as Real
                // arithmetic.  Propagating a Float32 adjoint into the Int32
                // LCG subtree would both change the DSP semantics and produce
                // invalid mixed-domain FIR (`BinOp(Float32, Int32)`) in the
                // reverse sweep.  Therefore FloatCast is an identity only for
                // float-to-float casts; for int→real casts it is a gradient
                // boundary.
                let x_is_int = matches!(
                    self.signal_fir_type(x),
                    Ok(FirType::Int32) | Ok(FirType::Int64)
                );
                if !x_is_int {
                    Self::add_to_adjoint(&mut self.store, adj, x, y_bar, real_ty);
                }
            }
            SigMatch::IntCast(x) | SigMatch::BitCast(x) => {
                Self::add_to_adjoint(&mut self.store, adj, x, y_bar, real_ty);
            }

            // ── BinOp ───────────────────────────────────────────────────────
            SigMatch::BinOp(op, lhs, rhs) => {
                let rule = rad_binop_rule(op);
                if !matches!(rule, RadBinOpRule::Rem | RadBinOpRule::Zero) {
                    // `Mul` and `Div` need tape-aware forward operand values.
                    // `Add`/`Sub` do not use operands, so `y_bar` is a harmless
                    // placeholder that avoids needless tape traffic.
                    let lhs_val = if matches!(rule, RadBinOpRule::Mul | RadBinOpRule::Div) {
                        self.load_bra_fwd_value(lhs)?
                    } else {
                        y_bar
                    };
                    let rhs_val = if matches!(rule, RadBinOpRule::Mul | RadBinOpRule::Div) {
                        self.load_bra_fwd_value(rhs)?
                    } else {
                        y_bar
                    };
                    let mut b = FirRadFormulaBuilder::new(self, real_ty.clone());
                    if let Some((lhs_adj, rhs_adj)) =
                        rad_binop_contributions(&mut b, rule, lhs_val, rhs_val, y_bar)
                    {
                        Self::add_to_adjoint(
                            &mut self.store,
                            adj,
                            lhs,
                            lhs_adj,
                            real_ty.clone(),
                        );
                        Self::add_to_adjoint(&mut self.store, adj, rhs, rhs_adj, real_ty);
                    }
                }
            }

            // ── Delay1: anti-causal carry ───────────────────────────────────
            SigMatch::Delay1(x) => {
                // y[n] = x[n-1].  Adjoint: adj[x][n-1] += adj[y][n].
                //
                // In the reverse sample loop at step n:
                //   carry_load  = struct field written at step n+1  → adj[x] contribution
                //   carry_store = y_bar → struct field for step n-1 to read
                //
                // Ordering: `immediate` runs before `post_output` within one
                // iteration, so the load always reads the value stored at n+1.
                //
                // Special case — `Delay1(Proj(slot, SYMREF(var)))`:
                //   This is the one-sample feedback in a recursive body.  The carry
                //   load was already emitted during the pre-scan in
                //   `ensure_bra_backward_sweep` (step 3a) and accumulated into
                //   `adj[body_sigs[slot]]` so that the total TBPTT adjoint
                //   `cotangent[n] + carry_from_n+1` is set before the Proj-SYMREC
                //   node is processed in the reverse postorder.  Here we only need
                //   to store the new carry for step n-1.
                let is_recursive_feedback =
                    if let SigMatch::Proj(_slot, inner_group) = match_sig(self.arena, x) {
                        match_sym_ref(self.arena, inner_group).is_some()
                    } else {
                        false
                    };
                let carry_name = self.ensure_bra_delay1_carry(sig, group)?;
                let carry_store = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.store_var(carry_name.clone(), AccessType::Struct, y_bar)
                };
                self.regions.current_phases_mut().post_output.push(carry_store);
                if !is_recursive_feedback {
                    let carry_load = {
                        let rt = self.real_ty();
                        let mut b = FirBuilder::new(&mut self.store);
                        b.load_var(carry_name, AccessType::Struct, rt)
                    };
                    Self::add_to_adjoint(&mut self.store, adj, x, carry_load, real_ty);
                }
            }

            // ── Floor / Ceil / Rint / Round: zero gradient ──────────────────
            SigMatch::Floor(x) | SigMatch::Ceil(x) | SigMatch::Rint(x) | SigMatch::Round(x) => {
                let _ = (x, y_bar); // Rounding ops: gradient is 0 almost everywhere.
            }

            // ── Delay(c, x): anti-causal carry with circular buffer ──────────
            SigMatch::Delay(sig_inner, amount) => {
                // Forward: y[n] = x[n-c].  Backward: adj[x][n] += adj[y][n+c].
                //
                // At reverse step n:
                //   carry[n % c] holds adj[y][n+c] written c steps ago.
                //   We load it → adj[sig_inner] += carry[n%c].
                //   We store y_bar to carry[n%c] for step n-c to read.
                let c_raw = tree_to_int(self.arena, amount).unwrap_or(0);
                let c = usize::try_from(c_raw).unwrap_or(0);
                if c == 0 {
                    // Zero delay: y = x.
                    Self::add_to_adjoint(&mut self.store, adj, sig_inner, y_bar, real_ty);
                } else {
                    let carry_name = self.ensure_bra_delay_array_carry(sig, c)?;
                    let c_fir = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.int32(i32::try_from(c).unwrap_or(i32::MAX))
                    };
                    let i0 = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.load_var("i0", AccessType::Loop, FirType::Int32)
                    };
                    let slot = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.binop(FirBinOp::Rem, i0, c_fir, FirType::Int32)
                    };
                    let rt = self.real_ty();
                    let carry_load = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.load_table(carry_name.clone(), AccessType::Struct, slot, rt)
                    };
                    let carry_store = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.store_table(carry_name, AccessType::Struct, slot, y_bar)
                    };
                    self.regions.current_phases_mut().post_output.push(carry_store);
                    Self::add_to_adjoint(&mut self.store, adj, sig_inner, carry_load, real_ty);
                }
            }

            // ── Prefix(init, sig): Delay1 semantics + init contribution ─────
            SigMatch::Prefix(init, sig_inner) => {
                // Forward: y[0] = init, y[n] = x[n-1] for n ≥ 1.
                // Backward (same as Delay1 for x):
                //   adj[sig_inner][n] += adj[y][n+1]  (anti-causal carry)
                //   adj[init]         += adj[y][0]    (only at frame 0)
                //
                // The i0==0 condition for the init contribution is emitted as
                // a FIR Select2: contrib = y_bar * (i0 == 0 ? 1 : 0).
                let carry_name = self.ensure_bra_delay1_carry(sig, sig)?;
                let rt = self.real_ty();
                let carry_load = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.load_var(carry_name.clone(), AccessType::Struct, rt)
                };
                let carry_store = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.store_var(carry_name, AccessType::Struct, y_bar)
                };
                self.regions.current_phases_mut().post_output.push(carry_store);
                Self::add_to_adjoint(&mut self.store, adj, sig_inner, carry_load, real_ty.clone());
                // Conditional init contribution: y_bar when i0 == 0, else 0.
                let i0 = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.load_var("i0", AccessType::Loop, FirType::Int32)
                };
                let zero_i = self.lower_int32_const(0);
                let is_frame0 = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.binop(FirBinOp::Eq, i0, zero_i, FirType::Int32)
                };
                let zero_r = self.float_const(0.0);
                let init_contrib = {
                    let mut b = FirBuilder::new(&mut self.store);
                    // Select2(cond, y_bar, 0.0): when is_frame0 != 0, use y_bar
                    b.select2(is_frame0, y_bar, zero_r, real_ty.clone())
                };
                Self::add_to_adjoint(&mut self.store, adj, init, init_contrib, real_ty);
            }

            // ── Proj(slot, SYMREC/SYMREF) — recursive carrier projection ────────
            //
            // Two symbolic Proj forms appear after `de_bruijn_to_sym`:
            //
            // • `Proj(slot, SYMREC(var, body_list))` — the top-level recursive
            //   output.  Its primal value equals `body_list[slot]`, so the adjoint
            //   flows identically to that body (identity Jacobian = 1).
            //   The pre-scan in `ensure_bra_backward_sweep` (step 3a) already
            //   accumulated the feedback carry into `adj[this_node]` before the
            //   reverse-postorder walk, so `y_bar` here is the full TBPTT adjoint
            //   `cotangent[n] + carry_from_step_n+1`.
            //
            // • `Proj(slot, SYMREF(var))` — a back-reference inside the recursive
            //   body.  This always appears as `Delay1(Proj(slot, SYMREF))`, and
            //   its adjoint carry was pre-loaded into `adj[body_sigs[slot]]` during
            //   the pre-scan.  The `Delay1` arm above stores the new carry to the
            //   struct field (for step n-1).  Nothing more to propagate here.
            SigMatch::Proj(slot, group_sig) => {
                if let Some((_var, body_list)) = match_sym_rec(self.arena, group_sig) {
                    // SYMREC top-level output: propagate adjoint to body[slot].
                    let slot_usize = usize::try_from(slot).map_err(|_| {
                        SignalFirError::new(
                            SignalFirErrorCode::UnsupportedSignalNode,
                            format!(
                                "negative Proj slot {slot} in BlockReverseAD backward pass (B6)"
                            ),
                        )
                    })?;
                    let bodies = list_to_vec(self.arena, body_list).ok_or_else(|| {
                        SignalFirError::new(
                            SignalFirErrorCode::UnsupportedSignalNode,
                            "malformed SYMREC body list in BlockReverseAD backward pass (B6)"
                                .to_string(),
                        )
                    })?;
                    let &body = bodies.get(slot_usize).ok_or_else(|| {
                        SignalFirError::new(
                            SignalFirErrorCode::UnsupportedSignalNode,
                            format!(
                                "Proj slot {slot_usize} out of range (SYMREC body count \
                                 {}) in BlockReverseAD backward pass (B6)",
                                bodies.len()
                            ),
                        )
                    })?;
                    Self::add_to_adjoint(&mut self.store, adj, body, y_bar, real_ty);
                } else if match_sym_ref(self.arena, group_sig).is_some() {
                    // SYMREF back-reference: carry pre-loaded in pre-scan; nothing to do.
                } else {
                    return Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        format!(
                            "Proj over non-SYMREC/SYMREF group ({:?}) not supported in \
                             BlockReverseAD backward pass (B6)",
                            match_sig(self.arena, group_sig)
                        ),
                    ));
                }
            }

            other => {
                return Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!("signal {other:?} not supported in BlockReverseAD backward pass (B6)"),
                ));
            }
        }
        Ok(())
    }

    /// Declares and returns the name of the adjoint carry variable for a
    /// `Delay1` node encountered inside a `SigBlockReverseAD` backward sweep.
    ///
    /// The carry is stored as a real-typed DSP struct field named
    /// `fBraCarryN` where N comes from the monotonic loop-var counter.  It is
    /// zeroed by `emit_bra_compute_resets` before each reverse sample loop so
    /// no adjoint state leaks across host `compute()` calls.
    ///
    /// Idempotent: subsequent calls for the same `delay1_node` return the same
    /// name without emitting a second declaration.
    pub(super) fn ensure_bra_delay1_carry(
        &mut self,
        delay1_node: SigId,
        _group: SigId,
    ) -> Result<String, SignalFirError> {
        if let Some(name) = self.bra.delay1_carry_vars.get(&delay1_node) {
            return Ok(name.clone());
        }
        let name = format!("fBraCarry{}", self.name_gen.next_loop_var_id);
        self.name_gen.next_loop_var_id += 1;
        let real_ty = self.real_ty.clone();
        // Declare the struct field without a reset-time init: BRA carry variables
        // are internal DSP state, not UI-controlled parameters, and must NOT appear
        // in `instanceResetUserInterface`.  Only `instanceClear` zeroes them (below).
        self.ensure_named_struct_var(&name, real_ty, None);
        // Register a clear-time zero init for `instanceClear`.
        let zero2 = self.float_const(0.0);
        self.register_clear_init(name.clone(), zero2);
        self.bra.delay1_carry_vars.insert(delay1_node, name.clone());
        Ok(name)
    }

    /// Declares and returns the name of the circular carry buffer for a
    /// `Delay(c, x)` node encountered inside a `SigBlockReverseAD` backward
    /// sweep, where `c > 1` is the constant delay amount.
    ///
    /// The buffer is a `Array(real_ty, c)` struct field named `fBraDelayCarryN`.
    /// At reverse step n, slot `n % c` holds the adjoint contribution from step
    /// `n + c` (written c iterations ago), implementing the anti-causal rule
    /// `adj_x[n] += adj_y[n + c]`.
    ///
    /// Idempotent: subsequent calls for the same `delay_node` return the same
    /// name without emitting a second declaration.
    pub(super) fn ensure_bra_delay_array_carry(
        &mut self,
        delay_node: SigId,
        c: usize,
    ) -> Result<String, SignalFirError> {
        if let Some((name, _)) = self.bra.delay_array_carry_vars.get(&delay_node) {
            return Ok(name.clone());
        }
        let name = format!("fBraDelayCarry{}", self.name_gen.next_loop_var_id);
        self.name_gen.next_loop_var_id += 1;
        let real_ty = self.real_ty.clone();
        let arr_ty = FirType::Array(Box::new(real_ty), c);
        self.ensure_named_struct_var(&name, arr_ty, None);
        self.bra
            .delay_array_carry_vars
            .insert(delay_node, (name.clone(), c));
        Ok(name)
    }

    /// Schedules forward-tape stores for tape-needed signals reachable from
    /// the given `body_sigs` roots.
    ///
    /// Called from `lower_block_reverse_ad_proj` once per primal slot, with
    /// only the body for that slot.  This ensures that `lower_signal` is
    /// called exclusively within the SYMREC recursion context that is active
    /// for the current primal slot — signals from a different SYMREC group
    /// (with a different recursion variable on the stack) must **not** be
    /// lowered here.
    ///
    /// Idempotency is maintained per-signal via `bra_tape_store_var`: if a
    /// signal has already been taped (e.g. because it is shared across bodies),
    /// a second call for a different body silently skips it.
    ///
    /// # Steps
    ///
    /// 1. Build the postorder for the supplied `body_sigs` roots.
    /// 2. Call [`collect_tape_needed_values`] to determine which forward values
    ///    require a tape.
    /// 3. For each tape-needed signal `v` not yet in `bra_tape_store_var`:
    ///    a. Allocate a fresh struct-field name `fBraTapeN`.
    ///    b. Declare the field as `Array(real_ty, MAX_BRA_TAPE_BLOCK_SIZE)`.
    ///    c. Lower `v` via `lower_signal` (runs in the forward loop context).
    ///    d. Emit `store_table(fBraTapeN, Struct, i0, v_fir)` to
    ///    `sample_phases.immediate` so it captures the forward value
    ///    **before** `post_output` updates delay/state variables (placing
    ///    it in `sample_end` would read post-update state and produce the
    ///    wrong tape entry for signals like `Delay1`).
    ///    e. Record the mapping `v → fBraTapeN` in `bra_tape_store_var`.
    ///
    /// In the split public-output schedule these stores appear in the forward
    /// loop and the matching loads appear in a later reverse loop.  In the
    /// inline adaptive schedule both the stores and the adjoint statements can
    /// be emitted into the same forward loop body.  The phase ordering still
    /// matters: tape stores are pushed to `immediate`, before state updates and
    /// before any later BRA sweep statements for the same carrier can consume
    /// the recorded values.
    ///
    /// # Interaction with `signalPromotion`
    ///
    /// The input signal forest has already been promoted before FIR lowering.
    /// BRA therefore must not perform ad-hoc integer-to-real promotion by
    /// casting values at the tape store.  The tape is a backend object, not a
    /// Signal-IR node, so such a cast would bypass normalform's `signalPromotion`
    /// rules and could hide a missing promotion bug.
    ///
    /// `collect_tape_needed_values` is intentionally conservative and
    /// structural: it may see integer/discrete nodes that are present upstream
    /// of a promoted `FloatCast`.  Those upstream nodes keep their original
    /// integer semantics (for instance the LCG recurrence used to generate
    /// pseudo-noise) and no adjoint rule crosses the int→real cast.  They are
    /// skipped here.  The promoted real `FloatCast` result, or a real expression
    /// derived from it, is the value that may be taped and later loaded by the
    /// reverse sweep.
    pub(super) fn ensure_bra_tape_stores(
        &mut self,
        _group: SigId,
        body_sigs: &[SigId],
        _seed_sigs: &[SigId],
        _cotangent_sigs: &[SigId],
    ) -> Result<(), SignalFirError> {
        // 1. Build postorder over the supplied body roots.
        let mut visited = std::collections::HashSet::new();
        let mut postorder = Vec::new();
        for &body in body_sigs {
            collect_bra_postorder(self.arena, body, &mut visited, &mut postorder);
        }

        // 2. Determine which values need to be taped.
        let tape_needed = collect_tape_needed_values(self.arena, &postorder);
        if tape_needed.is_empty() {
            return Ok(());
        }

        // 3. Emit tape stores in deterministic (postorder) order.
        let mut tape_sigs: Vec<SigId> = tape_needed.into_iter().collect();
        // Sort by SigId for deterministic emission.
        tape_sigs.sort();
        for v in tape_sigs {
            // Per-signal idempotency: skip signals already taped by a prior call
            // (e.g. a signal shared between two SYMREC bodies).
            if self.bra.tape_store_var.contains_key(&v) {
                continue;
            }
            let real_ty = self.real_ty.clone();
            let v_ty = self.signal_fir_type(v)?;
            if v_ty != real_ty {
                // `collect_tape_needed_values` is structural: it walks the full
                // body postorder and can see integer islands below a
                // `FloatCast`, notably LCG-style noise recursions.  Those
                // integer subgraphs are not differentiable and
                // `propagate_bra_adj` stops at the int->float cast, so no
                // reverse rule will ever load them from a BRA tape.  The
                // real-valued use site must already be represented by a
                // promoted `FloatCast` node; that node is the candidate to tape
                // when needed.  Skip non-real candidates here rather than
                // silently casting and hiding a missing Signal-level promotion.
                continue;
            }
            let tape_name = format!("fBraTape{}", self.name_gen.next_loop_var_id);
            self.name_gen.next_loop_var_id += 1;
            // Declare as a fixed-size array struct field.
            let tape_ty = FirType::Array(Box::new(real_ty.clone()), MAX_BRA_TAPE_BLOCK_SIZE);
            self.ensure_named_struct_var(&tape_name, tape_ty, None);
            // Lower the value in the current (forward) loop context.
            // BRA tapes are homogeneous `real_ty` arrays because the reverse
            // rules consume recorded forward values in real adjoint arithmetic.
            let v_fir = self.lower_signal(v)?;
            if self.store.value_type(v_fir) != Some(real_ty.clone()) {
                let sig_text = dump_sig_readable(self.arena, v);
                let got = self.store.value_type(v_fir);
                return Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!(
                        "BlockReverseAD real tape-needed signal {sig_text} lowered to FIR type {got:?}, expected {real_ty:?}; integer/real promotion must be resolved before FIR lowering"
                    ),
                ));
            }
            // Tape stores go in `immediate` so they capture the forward value
            // BEFORE `post_output` updates delay/state variables.  Placing them
            // in `sample_end` would re-read post-update state (e.g. the updated
            // Delay1 register) and produce the wrong tape entry.
            // Bounded tape index: a no-op for the supported block size, and a
            // safe wrap (never an out-of-bounds write) if the host exceeds it.
            let idx = self.bra_tape_index();
            let store_stmt = {
                let mut b = FirBuilder::new(&mut self.store);
                b.store_table(tape_name.clone(), AccessType::Struct, idx, v_fir)
            };
            self.regions.current_phases_mut().immediate.push(store_stmt);
            self.bra.tape_store_var.insert(v, tape_name);
        }
        Ok(())
    }

    /// Returns the FIR value for `sig` in the **reverse** sample loop.
    ///
    /// - If `sig` has a tape array (recorded by `ensure_bra_tape_stores`),
    ///   emits `load_table(fBraTapeN, Struct, i0)` and returns that value.
    /// - Otherwise falls back to `lower_signal(sig)`, which is correct when
    ///   `sig` is trivially reverse-evaluable (stateless leaf or pure
    ///   combinator of leaves).
    ///
    /// The loop variable `i0` used for the tape load is the same reverse-loop
    /// counter driven by the outer `build_module` reverse iteration; loading
    /// tape[i0] during the backward sweep at step `n` retrieves the forward
    /// value stored at forward step `n`.
    pub(super) fn load_bra_fwd_value(&mut self, sig: SigId) -> Result<FirId, SignalFirError> {
        if let Some(tape_name) = self.bra.tape_store_var.get(&sig).cloned() {
            let real_ty = self.real_ty();
            let idx = self.bra_tape_index();
            let load = {
                let mut b = FirBuilder::new(&mut self.store);
                b.load_table(tape_name, AccessType::Struct, idx, real_ty)
            };
            Ok(load)
        } else {
            self.lower_signal(sig)
        }
    }

    /// Builds the bounded BRA tape index `i0 & (MAX_BRA_TAPE_BLOCK_SIZE - 1)`.
    ///
    /// `MAX_BRA_TAPE_BLOCK_SIZE` is a power of two, so the mask is a **no-op** for
    /// the supported block size (`count ≤ MAX_BRA_TAPE_BLOCK_SIZE`): there,
    /// `i0 < MAX` and `i0 & (MAX - 1) == i0`. For an over-long block it keeps the
    /// access in bounds — the forward store and the reverse load use the same
    /// wrapped slot — instead of reading/writing past the tape array. The
    /// out-of-range tail then carries aliased (approximate) gradients rather than
    /// triggering undefined behaviour; the exact fix is chunked TBPTT or a
    /// dynamically sized tape (analysis W5 / rewriting-calculus §8.5).
    fn bra_tape_index(&mut self) -> FirId {
        let i0 = {
            let mut b = FirBuilder::new(&mut self.store);
            b.load_var("i0", AccessType::Loop, FirType::Int32)
        };
        let mask = {
            let mut b = FirBuilder::new(&mut self.store);
            b.int32(i32::try_from(MAX_BRA_TAPE_BLOCK_SIZE - 1).unwrap_or(i32::MAX))
        };
        let mut b = FirBuilder::new(&mut self.store);
        b.binop(FirBinOp::And, i0, mask, FirType::Int32)
    }

    /// Accumulates `new_term` into the adjoint of `sig`, building an `Add`
    /// node when a prior term already exists.
    ///
    /// This is the FIR-level equivalent of `adj[sig] += new_term` in the
    /// scalar BPTT executor.
    pub(super) fn add_to_adjoint(
        store: &mut FirStore,
        adj: &mut std::collections::HashMap<SigId, FirId>,
        sig: SigId,
        new_term: FirId,
        real_ty: FirType,
    ) {
        let entry = adj.entry(sig);
        match entry {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let old = *e.get();
                let sum = {
                    let mut b = FirBuilder::new(store);
                    b.binop(FirBinOp::Add, old, new_term, real_ty)
                };
                *e.get_mut() = sum;
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(new_term);
            }
        }
    }

    /// Emits one floating-point constant at the internal real precision.
    ///
    /// Uses `Float32` or `Float64` depending on `real_ty`.  Never emits
    /// `FaustFloat` — that type is reserved for external interface points.
    pub(super) fn float_const(&mut self, value: f64) -> FirId {
        let mut b = FirBuilder::new(&mut self.store);
        match self.real_ty {
            FirType::Float64 => b.float64(value),
            _ => b.float32(value as f32),
        }
    }

    /// Derives an initial state value from a signal if constant, otherwise `0`.
    pub(super) fn initial_state_from_signal(&mut self, sig: SigId) -> FirId {
        match match_sig(self.arena, sig) {
            SigMatch::Int(v) => self.lower_int32_const(v),
            SigMatch::Real(v) => self.float_const(v),
            _ => self.float_const(0.0),
        }
    }
}
