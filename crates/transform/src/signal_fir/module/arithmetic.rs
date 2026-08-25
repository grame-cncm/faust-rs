//! Binary operator, math function, cast, select, and projection lowering.
//!
//! Defines [`UsedPrototypes`], the sub-state struct that tracks which math
//! helpers and extern symbols the generated module depends on.
//!
//! Covers the arithmetic spine of the signal-to-FIR lowering:
//! - `lower_binop` — maps Faust `BinOp` nodes to typed FIR binop instructions;
//! - `lower_math1` / `lower_math2` — unary and binary math intrinsics;
//! - `lower_minmax` — min/max with integer-vs-real type handling;
//! - `lower_abs` — absolute value with domain-appropriate FIR form;
//! - `lower_cast` / `lower_bitcast` — integer↔real type coercions;
//! - `lower_select2` — conditional selection;
//! - `lower_proj` — recursion projection decoding.
//!
//! Relies on the promoter invariant that all operands already carry explicit
//! cast wrappers; no implicit coercion is performed here.
use crate::signal_fir::FirId;
use crate::signal_fir::FirType;
use crate::signal_fir::SigId;
use crate::signal_fir::SignalFirError;
use crate::signal_fir::SignalFirErrorCode;
use crate::signal_fir::leaf_emit;
use crate::signal_fir::module::AccessType;
use crate::signal_fir::module::BTreeMap;
use crate::signal_fir::module::BinOp;
use crate::signal_fir::module::FirBuilder;
use crate::signal_fir::module::FirMathOp;
use crate::signal_fir::module::ForeignFunProto;
use crate::signal_fir::module::HashSet;
use crate::signal_fir::module::SigMatch;
use crate::signal_fir::module::SignalToFirLower;
use crate::signal_fir::module::dump_sig_readable;
use crate::signal_fir::module::list_to_vec;
use crate::signal_fir::module::match_sig;
use crate::signal_fir::module::match_sym_ref;
use crate::signal_fir::recursion::RecArrayInfo;
use crate::signal_fir::recursion::RecursionAllocCtx;
use crate::signal_fir::recursion::RecursionCarrierRef;
use crate::signal_fir::recursion::RecursionGroupProjection;
use crate::signal_fir::recursion::RecursionLoweringCtx;
use crate::signal_fir::recursion::RecursionStorageStrategy;
use crate::signal_fir::recursion::decode_group_projection;
use crate::signal_fir::recursion::decode_symbolic_group_bodies;
use crate::signal_fir::recursion::resolve_active_recursion_carrier;

/// Prototype registration state — tracks which math helpers and extern symbols
/// have been referenced, so the module assembler can emit exactly the needed
/// declarations.
#[derive(Default)]
pub(super) struct UsedPrototypes {
    /// Set of math operations used; drives prototype emission order.
    pub(super) math_ops: HashSet<FirMathOp>,
    /// Set of integer helper function names used (`abs`, `min_i`, `max_i`).
    pub(super) int_fun_names: HashSet<&'static str>,
    /// Extern prototypes requested by `SIGFFUN` lowering, keyed by callee name.
    pub(super) foreign_fun_protos: BTreeMap<String, ForeignFunProto>,
    /// Extern globals requested by `SIGFVAR` lowering, keyed by symbol name.
    pub(super) foreign_vars: BTreeMap<String, FirType>,
}

impl leaf_emit::LeafPrototypes for UsedPrototypes {
    fn note_math_op(&mut self, op: FirMathOp) {
        self.math_ops.insert(op);
    }
    fn note_int_helper(&mut self, name: &'static str) {
        self.int_fun_names.insert(name);
    }
}

impl<'a> SignalToFirLower<'a> {
    /// Lowers one binary signal operator to FIR binop.
    ///
    /// Relies on the promoter invariant: every `BinOp` operand already has the
    /// correct domain type (mixed Int/Real pairs wrapped in `FloatCast`; bitwise
    /// and shift operands in `IntCast`; `Div` operands always Real).
    /// Comparisons keep same-typed numeric operands and produce `Int32` results
    /// for C++ parity.  No implicit coercion is performed here.
    pub(super) fn lower_binop(
        &mut self,
        node: SigId,
        op: BinOp,
        lhs_sig: SigId,
        rhs_sig: SigId,
    ) -> Result<FirId, SignalFirError> {
        let result_ty = self.signal_fir_type(node)?;
        let lhs = self.lower_signal(lhs_sig)?;
        let rhs = self.lower_signal(rhs_sig)?;
        leaf_emit::emit_binop(&mut self.store, op, result_ty, lhs, rhs).map_err(|error| {
            match error {
                leaf_emit::LeafBinopError::UnsupportedOperator => SignalFirError::new(
                    SignalFirErrorCode::UnsupportedBinOp,
                    format!("unsupported SIGBINOP operator `{}` in Step 2A", op.name()),
                ),
                leaf_emit::LeafBinopError::MissingOperandType { is_lhs, .. } => {
                    SignalFirError::new(
                        SignalFirErrorCode::UnsupportedBinOp,
                        format!(
                            "missing FIR type for {} operand of `{}` in Step 2A",
                            if is_lhs { "left" } else { "right" },
                            op.name()
                        ),
                    )
                }
                leaf_emit::LeafBinopError::OperandContract { lhs, rhs, expected } => {
                    SignalFirError::new(
                        SignalFirErrorCode::UnsupportedBinOp,
                        format!(
                            "prepared SIGBINOP operands for `{}` violate fast-lane typing contract: lhs={lhs:?}, rhs={rhs:?}, result={expected:?} (expr={})",
                            op.name(),
                            dump_sig_readable(self.arena, node)
                        ),
                    )
                }
            }
        })
    }

    /// Lowers one unary math intrinsic call.
    pub(super) fn lower_math1(
        &mut self,
        op: FirMathOp,
        value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let value = self.lower_signal(value)?;
        let real_ty = self.real_ty();
        Ok(leaf_emit::emit_math_call1(
            &mut self.store,
            &mut self.used_protos,
            op,
            value,
            real_ty,
        ))
    }

    /// Lowers one binary math intrinsic call.
    pub(super) fn lower_math2(
        &mut self,
        op: FirMathOp,
        lhs: SigId,
        rhs: SigId,
    ) -> Result<FirId, SignalFirError> {
        let lhs = self.lower_signal(lhs)?;
        let rhs = self.lower_signal(rhs)?;
        let real_ty = self.real_ty();
        Ok(leaf_emit::emit_math_call2(
            &mut self.store,
            &mut self.used_protos,
            op,
            lhs,
            rhs,
            real_ty,
        ))
    }

    /// Lowers `min`/`max`, preserving integer recursion/state when the reduced
    /// typer kept both operands in the integer domain.
    ///
    /// Source provenance (C++):
    /// - `compiler/extended/minprim.hh`
    /// - `compiler/extended/maxprim.hh`
    ///
    /// Integer `min/max` remain explicit FIR function calls (`min_i` / `max_i`)
    /// so backends can apply the same target-local renaming policy as the C++
    /// compiler instead of hardwiring a branch synthesis here.
    pub(super) fn lower_minmax(
        &mut self,
        node: SigId,
        lhs_sig: SigId,
        rhs_sig: SigId,
        is_min: bool,
    ) -> Result<FirId, SignalFirError> {
        let result_ty = self.signal_fir_type(node)?;
        let lhs = self.lower_signal(lhs_sig)?;
        let rhs = self.lower_signal(rhs_sig)?;
        let real_ty = self.real_ty();
        Ok(leaf_emit::emit_minmax(
            &mut self.store,
            &mut self.used_protos,
            is_min,
            &result_ty,
            real_ty,
            lhs,
            rhs,
        ))
    }

    /// Lowers `abs`, preserving integer recursion/state when the reduced typer
    /// kept the operand in the integer domain.
    ///
    /// Source provenance (C++):
    /// - `compiler/extended/absprim.hh`
    ///
    /// Integer `abs` stays an explicit function call so backends can preserve
    /// the target-local parity spelling and overflow contract.
    pub(super) fn lower_abs(
        &mut self,
        node: SigId,
        value_sig: SigId,
    ) -> Result<FirId, SignalFirError> {
        let result_ty = self.signal_fir_type(node)?;
        let value = self.lower_signal(value_sig)?;
        let real_ty = self.real_ty();
        Ok(leaf_emit::emit_abs(
            &mut self.store,
            &mut self.used_protos,
            &result_ty,
            real_ty,
            value,
        ))
    }

    /// Lowers one numeric cast.
    pub(super) fn lower_cast(
        &mut self,
        typ: FirType,
        value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let value = self.lower_signal(value)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.cast(typ, value))
    }

    /// Lowers one bitcast operation.
    pub(super) fn lower_bitcast(
        &mut self,
        typ: FirType,
        value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let value = self.lower_signal(value)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.bitcast(typ, value))
    }

    /// Lowers `select2` with explicit result-type selection.
    pub(super) fn lower_select2(
        &mut self,
        node: SigId,
        cond: SigId,
        then_value: SigId,
        else_value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let cond = self.lower_signal(cond)?;
        let then_value = self.lower_signal(then_value)?;
        let else_value = self.lower_signal(else_value)?;
        let real_ty = self.signal_fir_type(node)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.select2(cond, then_value, else_value, real_ty))
    }

    /// Lowers recursion projection nodes after the mandatory
    /// `de_bruijn_to_sym` preparation step.
    ///
    /// Expects symbolic recursion payloads (`SYMREC` / `SYMREF`) — the normal
    /// fast-lane input form produced by `signal_prepare`.
    ///
    /// **Scheduled body evaluation**: a delayed `Proj(SYMREF)` may allocate the
    /// group's carriers before the owning `Proj(SYMREC)` is reached. Body
    /// expressions themselves follow the global same-tick schedule. The owning
    /// projection emits the simultaneous carrier updates exactly once.
    ///
    /// **Fast path** (active reference inside a body being lowered): when the
    /// canonical recursion-carrier resolver finds the group on the stack, the
    /// current-slot value is read directly — no recursion into `lower_signal`
    /// occurs, which breaks the cycle.
    pub(super) fn lower_proj(
        &mut self,
        node: SigId,
        index: i32,
        group: SigId,
    ) -> Result<FirId, SignalFirError> {
        let index_usize = usize::try_from(index).map_err(|_| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("negative SIGPROJ index {index} in Step 2C.2"),
            )
        })?;
        // ── Fast path: active reference inside a body being lowered ──
        if let Some(rec_ref) =
            resolve_active_recursion_carrier(self.arena, &self.recursion, group, index_usize)?
        {
            return self.load_recursion_carrier_storage(node, &rec_ref);
        }

        let canonical_group = self.recursion.canonical_group(self.arena, group);
        let clock_context = self.current_clock_context();
        let is_symbolic_reference = match_sym_ref(self.arena, group).is_some();

        // C++ permits a delayed recursive reference to appear before the
        // owning projection in the schedule. Reserve all group carriers now,
        // but do not emit the current-sample body update yet.
        if is_symbolic_reference {
            let canonical_group = canonical_group.ok_or_else(|| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!(
                        "unbound symbolic recursion reference in projection {}",
                        node.as_u32()
                    ),
                )
            })?;
            let _ = self.ensure_recursion_group_carriers(canonical_group)?;
            let rec_ref = self
                .recursion
                .resolve_materialized_carrier(
                    self.arena,
                    canonical_group,
                    index_usize,
                    clock_context,
                )
                .expect("registered symbolic group must have an allocated carrier");
            return self.load_recursion_carrier_storage(node, &rec_ref);
        }

        // A preallocated top-level carrier is not a completed projection: the
        // body update still has to be emitted. Reuse is valid only after that
        // group has been scheduled in the current sample.
        let group_is_scheduled = canonical_group.is_some_and(|canonical| {
            self.recursion
                .scheduled_groups
                .contains(&(canonical, clock_context))
        });
        if group_is_scheduled {
            if let Some(current_value) =
                self.load_scalar_recursion_current_value(group, index_usize)?
            {
                return Ok(current_value);
            }
            if let Some(rec_ref) = self.recursion.resolve_materialized_carrier(
                self.arena,
                group,
                index_usize,
                clock_context,
            ) {
                return self.load_recursion_carrier_storage(node, &rec_ref);
            }
        }

        // ── Fast path: SigBlockReverseAD carrier ──
        if let SigMatch::BlockReverseAD {
            body,
            primal_count,
            seeds,
            cotangents,
            policy: _,
        } = match_sig(self.arena, group)
        {
            let pc = usize::try_from(primal_count).map_err(|_| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!("negative primal_count in BlockReverseAD Proj({index})"),
                )
            })?;
            let body_sigs = list_to_vec(self.arena, body).ok_or_else(|| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    "malformed body list in BlockReverseAD".to_string(),
                )
            })?;
            let seed_sigs = list_to_vec(self.arena, seeds).ok_or_else(|| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    "malformed seed list in BlockReverseAD".to_string(),
                )
            })?;
            let cotangent_sigs = list_to_vec(self.arena, cotangents).ok_or_else(|| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    "malformed cotangent list in BlockReverseAD".to_string(),
                )
            })?;
            return self.lower_block_reverse_ad_proj(
                node,
                group,
                index_usize,
                pc,
                &body_sigs,
                &seed_sigs,
                &cotangent_sigs,
            );
        }

        // ── Decode all body signals from the group ──
        let RecursionGroupProjection {
            var,
            bodies,
            canonical_index,
        } = decode_group_projection(self.arena, node, index, group)?;

        let (_, _, group_arrays) = self.ensure_recursion_group_carriers(group)?;

        self.schedule_recursion_group_bodies(group, var, &bodies, &group_arrays, clock_context)?;

        self.load_projection_result(node, group, canonical_index, &group_arrays)
    }

    /// Schedules one symbolic group's simultaneous body pass exactly once
    /// per clock context: lowers every body inside the active-group scope,
    /// snapshots multi-output lanes before the carrier stores, emits the
    /// carrier updates, and persists scalar carriers in post-output.
    fn schedule_recursion_group_bodies(
        &mut self,
        group: SigId,
        var: SigId,
        bodies: &[SigId],
        group_arrays: &[RecArrayInfo],
        clock_context: Option<u32>,
    ) -> Result<(), SignalFirError> {
        // ── Push group context, lower ALL bodies, emit stores ──
        // Use recursion-owned scheduling so each group's body pass runs only once.
        if self.recursion.mark_group_scheduled(group, clock_context) {
            self.with_active_recursion_group(var, group_arrays.to_vec(), |this, active_arrays| {
                let zero = this.lower_int32_const(0);
                let mut body_values = Vec::with_capacity(bodies.len());
                let mut current_indexes = Vec::with_capacity(active_arrays.len());
                for (i, body) in bodies.iter().enumerate() {
                    body_values.push(this.lower_signal(*body)?);
                    let current_index = match active_arrays[i].storage_strategy() {
                        RecursionStorageStrategy::SingleScalar => {
                            this.bind_scalar_recursion_current_value(
                                group,
                                i,
                                &active_arrays[i],
                                body_values[i],
                            );
                            zero
                        }
                        RecursionStorageStrategy::ExactShift => zero,
                        RecursionStorageStrategy::Circular => {
                            this.global_circular_current_index(active_arrays[i].size)
                        }
                    };
                    current_indexes.push(current_index);
                }
                if active_arrays.len() > 1 {
                    // Multi-output recursion is a simultaneous update. Snapshot
                    // every body before carrier stores so one lane cannot read
                    // another lane's already-updated current slot.
                    for (i, body_value) in body_values.iter_mut().enumerate() {
                        let typ = active_arrays[i].typ.clone();
                        let prefix = if typ == FirType::Int32 {
                            "iRecBody"
                        } else {
                            "fRecBody"
                        };
                        let name = format!("{prefix}{}", this.name_gen.next_loop_var_id);
                        this.name_gen.next_loop_var_id += 1;
                        let declare = {
                            let mut b = FirBuilder::new(&mut this.store);
                            b.declare_var(
                                name.clone(),
                                typ.clone(),
                                AccessType::Stack,
                                Some(*body_value),
                            )
                        };
                        this.regions.current_phases_mut().immediate.push(declare);
                        *body_value = {
                            let mut b = FirBuilder::new(&mut this.store);
                            b.load_var(name, AccessType::Stack, typ)
                        };
                    }
                }
                let phases = this.regions.current_phases_mut();
                let mut recursion_ctx = RecursionLoweringCtx {
                    store: &mut this.store,
                    immediate_statements: &mut phases.immediate,
                    post_output_statements: &mut phases.post_output,
                    next_loop_var_id: &mut this.name_gen.next_loop_var_id,
                };
                recursion_ctx.emit_group_body_updates(
                    active_arrays,
                    &body_values,
                    &current_indexes,
                );
                for (i, info) in active_arrays.iter().enumerate() {
                    if info.storage_strategy() == RecursionStorageStrategy::SingleScalar {
                        let binding = this
                            .recursion
                            .current_value_binding(
                                this.arena,
                                group,
                                i,
                                this.current_clock_context(),
                            )
                            .expect("scalar recursion binding should be recorded before finalize");
                        let current_value = {
                            let mut b = FirBuilder::new(&mut this.store);
                            b.load_var(binding.name, AccessType::Stack, binding.typ.clone())
                        };
                        let store_state = {
                            let mut b = FirBuilder::new(&mut this.store);
                            b.store_var(info.name.clone(), AccessType::Struct, current_value)
                        };
                        this.regions
                            .current_phases_mut()
                            .post_output
                            .push(store_state);
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Loads the requested projection's current value from its carrier:
    /// scalar binding, exact-shift slot 0, or the circular current index.
    fn load_projection_result(
        &mut self,
        node: SigId,
        group: SigId,
        canonical_index: usize,
        group_arrays: &[RecArrayInfo],
    ) -> Result<FirId, SignalFirError> {
        let info = &group_arrays[canonical_index];
        let out_ty = self.signal_fir_type(node)?;
        if info.storage_strategy() == RecursionStorageStrategy::SingleScalar {
            let current_value = self
                .load_scalar_recursion_current_value(group, canonical_index)?
                .expect("scalar recursion current value should be available after scheduling");
            debug_assert_eq!(
                info.typ, out_ty,
                "SIGPROJ type mismatch: carrier={:?}, node={:?}",
                info.typ, out_ty
            );
            return Ok(current_value);
        }
        let zero = self.lower_int32_const(0);
        let circular_index = if info.storage_strategy() == RecursionStorageStrategy::ExactShift {
            zero
        } else {
            self.global_circular_current_index(info.size)
        };
        let phases = self.regions.current_phases_mut();
        let mut recursion_ctx = RecursionLoweringCtx {
            store: &mut self.store,
            immediate_statements: &mut phases.immediate,
            post_output_statements: &mut phases.post_output,
            next_loop_var_id: &mut self.name_gen.next_loop_var_id,
        };
        let current_index = recursion_ctx.current_index_for_carrier(info, zero, circular_index);
        let out = recursion_ctx.load_feedback_carrier(info, current_index, info.typ.clone());
        debug_assert_eq!(
            info.typ, out_ty,
            "SIGPROJ type mismatch: array={:?}, node={:?}",
            info.typ, out_ty
        );
        Ok(out)
    }

    /// Allocates all carriers for one symbolic group without lowering its
    /// body. This is the Rust equivalent of C++ reserving a vector-name
    /// property from a delayed access before `generateRecProj` runs.
    pub(super) fn ensure_recursion_group_carriers(
        &mut self,
        group: SigId,
    ) -> Result<(SigId, Vec<SigId>, Vec<RecArrayInfo>), SignalFirError> {
        let (var, bodies) = decode_symbolic_group_bodies(self.arena, group).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("expected symbolic recursion group {}", group.as_u32()),
            )
        })?;
        let mut body_infos = Vec::with_capacity(bodies.len());
        for body in &bodies {
            let state_ty = self.signal_fir_type(*body)?;
            let init = match state_ty {
                FirType::Int32 => self.lower_int32_const(0),
                FirType::Float32 | FirType::Float64 | FirType::FaustFloat => self.float_const(0.0),
                other => {
                    return Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        format!("unsupported recursive state type in Step 2C.2: {other:?}"),
                    ));
                }
            };
            body_infos.push((state_ty, init));
        }
        let clock_context = self.current_clock_context();
        let arrays = {
            let mut ctx = RecursionAllocCtx {
                arena: self.arena,
                delay: &self.delay,
                store: &mut self.store,
                struct_declarations: &mut self.sections.struct_declarations,
                clear_statements: &mut self.sections.clear_statements,
                clear_init_seen: &mut self.sections.clear_init_seen,
                next_loop_var_id: &mut self.name_gen.next_loop_var_id,
                recursion: &mut self.recursion,
                clock_context,
            };
            ctx.allocate_group_arrays(group, &body_infos)?
        };
        Ok((var, bodies, arrays))
    }

    fn load_recursion_carrier_storage(
        &mut self,
        node: SigId,
        rec_ref: &RecursionCarrierRef,
    ) -> Result<FirId, SignalFirError> {
        let real_ty = self.signal_fir_type(node)?;
        let current_index = match rec_ref.strategy {
            RecursionStorageStrategy::ExactShift | RecursionStorageStrategy::SingleScalar => {
                self.lower_int32_const(0)
            }
            RecursionStorageStrategy::Circular => {
                self.global_circular_current_index(rec_ref.info.size)
            }
        };
        let phases = self.regions.current_phases_mut();
        let mut recursion_ctx = RecursionLoweringCtx {
            store: &mut self.store,
            immediate_statements: &mut phases.immediate,
            post_output_statements: &mut phases.post_output,
            next_loop_var_id: &mut self.name_gen.next_loop_var_id,
        };
        Ok(recursion_ctx.load_feedback_carrier(&rec_ref.info, current_index, real_ty))
    }
}
