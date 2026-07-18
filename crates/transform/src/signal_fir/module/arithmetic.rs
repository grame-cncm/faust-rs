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

use super::*;

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
        let (fir_op, typ) = map_binop(op, result_ty).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedBinOp,
                format!("unsupported SIGBINOP operator `{}` in Step 2A", op.name()),
            )
        })?;
        let lhs_ty = self.store.value_type(lhs).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedBinOp,
                format!(
                    "missing FIR type for left operand of `{}` in Step 2A",
                    op.name()
                ),
            )
        })?;
        let rhs_ty = self.store.value_type(rhs).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedBinOp,
                format!(
                    "missing FIR type for right operand of `{}` in Step 2A",
                    op.name()
                ),
            )
        })?;
        let operands_ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                lhs_ty == typ && rhs_ty == typ
            }
            BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Lsh | BinOp::ARsh | BinOp::LRsh => {
                lhs_ty == FirType::Int32 && rhs_ty == FirType::Int32
            }
            BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne => {
                lhs_ty == rhs_ty
                    && matches!(lhs_ty, FirType::Int32 | FirType::Float32 | FirType::Float64)
            }
        };
        if !operands_ok {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedBinOp,
                format!(
                    "prepared SIGBINOP operands for `{}` violate fast-lane typing contract: lhs={lhs_ty:?}, rhs={rhs_ty:?}, result={typ:?} (expr={})",
                    op.name(),
                    dump_sig_readable(self.arena, node)
                ),
            ));
        }
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.binop(fir_op, lhs, rhs, typ))
    }

    /// Lowers one unary math intrinsic call.
    pub(super) fn lower_math1(
        &mut self,
        op: FirMathOp,
        value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let value = self.lower_signal(value)?;
        self.used_protos.math_ops.insert(op);
        // Math calls operate on and return the internal real type.
        let real_ty = self.real_ty();
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.math_call(op, &[value], real_ty))
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
        self.used_protos.math_ops.insert(op);
        // Math calls operate on and return the internal real type.
        let real_ty = self.real_ty();
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.math_call(op, &[lhs, rhs], real_ty))
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
        if result_ty == FirType::Int32 {
            let lhs = self.lower_signal(lhs_sig)?;
            let rhs = self.lower_signal(rhs_sig)?;
            self.used_protos
                .int_fun_names
                .insert(if is_min { "min_i" } else { "max_i" });
            let mut b = FirBuilder::new(&mut self.store);
            return Ok(b.fun_call(
                if is_min { "min_i" } else { "max_i" },
                &[lhs, rhs],
                FirType::Int32,
            ));
        }
        self.lower_math2(
            if is_min {
                FirMathOp::Min
            } else {
                FirMathOp::Max
            },
            lhs_sig,
            rhs_sig,
        )
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
        if result_ty == FirType::Int32 {
            let value = self.lower_signal(value_sig)?;
            self.used_protos.int_fun_names.insert("abs");
            let mut b = FirBuilder::new(&mut self.store);
            return Ok(b.fun_call("abs", &[value], FirType::Int32));
        }
        self.lower_math1(FirMathOp::Abs, value_sig)
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
                .resolve_materialized_carrier(self.arena, canonical_group, index_usize)
                .expect("registered symbolic group must have an allocated carrier");
            return self.load_recursion_carrier_storage(node, &rec_ref);
        }

        // A preallocated top-level carrier is not a completed projection: the
        // body update still has to be emitted. Reuse is valid only after that
        // group has been scheduled in the current sample.
        let group_is_scheduled = canonical_group
            .is_some_and(|canonical| self.recursion.scheduled_groups.contains(&canonical));
        if group_is_scheduled {
            if let Some(current_value) =
                self.load_scalar_recursion_current_value(group, index_usize)?
            {
                return Ok(current_value);
            }
            if let Some(rec_ref) =
                self.recursion
                    .resolve_materialized_carrier(self.arena, group, index_usize)
            {
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

        // ── Push group context, lower ALL bodies, emit stores ──
        // Use recursion-owned scheduling so each group's body pass runs only once.
        if self.recursion.mark_group_scheduled(group) {
            self.with_active_recursion_group(var, group_arrays.clone(), |this, active_arrays| {
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
                            .current_value_binding(this.arena, group, i)
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

        // ── Return the result for the requested index ──
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

/// Maps signal-level operators to FIR operators with result typing policy.
///
/// `real_ty` is the internal DSP computation type (e.g. `Float32` / `Float64`).
/// It is used for arithmetic operators whose result is a real-valued sample.
/// Comparison operators produce `Int32` in the fast-lane, matching the normal
/// C++ signal typing path where comparisons are "boolean int" values. This is
/// distinct from the optional backend-specific `SignalBool2IntPromotion` pass:
/// the fast-lane does not rely on that pass and must preserve the standard
/// signal semantics directly. Bitwise operators also produce `Int32`.
pub(in crate::signal_fir) fn map_binop(op: BinOp, real_ty: FirType) -> Option<(FirBinOp, FirType)> {
    match op {
        // Arithmetic operators: result is the internal real type.
        BinOp::Add => Some((FirBinOp::Add, real_ty)),
        BinOp::Sub => Some((FirBinOp::Sub, real_ty)),
        BinOp::Mul => Some((FirBinOp::Mul, real_ty)),
        BinOp::Div => Some((FirBinOp::Div, real_ty)),
        BinOp::Rem => Some((FirBinOp::Rem, real_ty)),
        // Comparison operators: result is Int32 ("boolean int") for parity
        // with the standard C++ signal typing path.
        BinOp::Gt => Some((FirBinOp::Gt, FirType::Int32)),
        BinOp::Lt => Some((FirBinOp::Lt, FirType::Int32)),
        BinOp::Ge => Some((FirBinOp::Ge, FirType::Int32)),
        BinOp::Le => Some((FirBinOp::Le, FirType::Int32)),
        BinOp::Eq => Some((FirBinOp::Eq, FirType::Int32)),
        BinOp::Ne => Some((FirBinOp::Ne, FirType::Int32)),
        // Bitwise operators: result is Int32 — independent of real_ty.
        BinOp::And => Some((FirBinOp::And, FirType::Int32)),
        BinOp::Or => Some((FirBinOp::Or, FirType::Int32)),
        BinOp::Xor => Some((FirBinOp::Xor, FirType::Int32)),
        BinOp::Lsh => Some((FirBinOp::Lsh, FirType::Int32)),
        BinOp::ARsh => Some((FirBinOp::ARsh, FirType::Int32)),
        BinOp::LRsh => Some((FirBinOp::LRsh, FirType::Int32)),
    }
}
