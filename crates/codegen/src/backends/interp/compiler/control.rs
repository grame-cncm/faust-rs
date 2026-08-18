//! `control` half of the FBC compiler.
//!
//! Branches, switches, loops, and function calls.
//!
//! Split out of `compiler.rs` on 2026-08-18, where all 54 methods sat in one
//! 1891-line `impl`. The method bodies are moved verbatim; only their
//! visibility widened from private to `pub(super)` so the sibling modules can
//! still reach them.

use super::*;

impl<R: FbcReal> FirToFbcCompiler<R> {
    /// # Source provenance (C++)
    /// - `visit(IfInst*)` — block-switching for if/else.
    pub(super) fn compile_if(
        &mut self,
        store: &FirStore,
        cond: FirId,
        then_block: FirId,
        else_block: Option<FirId>,
    ) -> Result<(), CompileError> {
        // Compile condition.
        self.compile_node(store, cond)?;

        // Compile 'then' in a new sub-block.
        self.begin_sub_block();
        self.compile_node(store, then_block)?;
        let then_block_id = self.end_sub_block();

        // Compile 'else' in a (possibly empty) new sub-block.
        self.begin_sub_block();
        if let Some(else_id) = else_block {
            self.compile_node(store, else_id)?;
        }
        let else_block_id = self.end_sub_block();

        // Emit kIf.
        self.current_block.push(FbcInstruction::full(
            FbcOpcode::If,
            "",
            0,
            R::default(),
            0,
            0,
            Some(then_block_id),
            Some(else_block_id),
        ));
        Ok(())
    }

    /// Compiles `Switch(cond, cases, default)` as a nested `If` chain.
    ///
    /// This backend lowering currently assumes integer-like switch conditions
    /// and case labels, which matches the active FIR fixtures and the most
    /// common control dispatch patterns.
    pub(super) fn compile_switch(
        &mut self,
        store: &FirStore,
        cond: FirId,
        cases: &[(i64, FirId)],
        default: Option<FirId>,
    ) -> Result<(), CompileError> {
        self.compile_switch_cases(store, cond, cases, default)
    }

    /// Lowers one `switch` case list recursively as a right-nested `if` chain.
    pub(super) fn compile_switch_cases(
        &mut self,
        store: &FirStore,
        cond: FirId,
        cases: &[(i64, FirId)],
        default: Option<FirId>,
    ) -> Result<(), CompileError> {
        if let Some((&(case_value, case_block), rest)) = cases.split_first() {
            // Evaluate `cond == case_value` then branch.
            self.compile_node(store, cond)?;
            self.compile_int32(i32::try_from(case_value).map_err(|_| {
                CompileError::UnsupportedNode {
                    description: format!("switch case value out of i32 range: {case_value}"),
                }
            })?)?;
            self.current_block
                .push(FbcInstruction::new(FbcOpcode::EQInt));

            // Then branch: compile case block.
            self.begin_sub_block();
            self.compile_node(store, case_block)?;
            let then_block_id = self.end_sub_block();

            // Else branch: recurse on remaining cases or compile default.
            self.begin_sub_block();
            self.compile_switch_cases(store, cond, rest, default)?;
            let else_block_id = self.end_sub_block();

            self.current_block.push(FbcInstruction::full(
                FbcOpcode::If,
                "",
                0,
                R::default(),
                0,
                0,
                Some(then_block_id),
                Some(else_block_id),
            ));
            Ok(())
        } else {
            if let Some(default_block) = default {
                self.compile_node(store, default_block)?;
            }
            Ok(())
        }
    }

    /// # Source provenance (C++)
    /// - `visit(ForLoopInst*)` — block-switching for loop with init + body.
    ///
    /// A general `ForLoop` carries an explicit loop variable plus `init`/`end`/
    /// `step` nodes and a direction. `init` is a `DeclareVar` that allocates and
    /// seeds the variable; `step` and `end` are the (signed) increment value and
    /// the exclusive bound. The loop runs `var = init; do { body; var += step }
    /// while (is_reverse ? var > end : var < end)`.
    ///
    /// Earlier this compiled `step`/`end` as plain expressions and never updated
    /// the loop variable or built a real condition, so reverse loops (the
    /// shift-array strategy used by short delays `@(3..mcd)`) produced no
    /// iterations and the delay line emitted silence.
    pub(super) fn compile_for_loop(
        &mut self,
        store: &FirStore,
        params: ForLoopParams<'_>,
    ) -> Result<(), CompileError> {
        // Init sub-block: the `DeclareVar` allocates and seeds the loop variable.
        self.begin_sub_block();
        self.compile_node(store, params.init)?;
        let init_block_id = self.end_sub_block();

        let desc = self.field_table.get(params.var).cloned().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: params.var.to_string(),
            }
        })?;

        // Body sub-block: body → `var += step` → condition → kCondBranch(loop back).
        self.begin_sub_block();
        self.compile_node(store, params.body)?;

        // var = var + step (step carries its sign, e.g. -1 for reverse).
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadInt,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        self.compile_node(store, params.step)?;
        self.current_block
            .push(FbcInstruction::new(FbcOpcode::AddInt));
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::StoreInt,
                0,
                R::default(),
                desc.offset,
                0,
            ));

        // Condition: continue while `is_reverse ? var > end : var < end`.
        // Stack convention: LHS on TOS → push `end` (RHS) first, then `var` (LHS).
        self.compile_node(store, params.end)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadInt,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        self.current_block
            .push(FbcInstruction::new(if params.is_reverse {
                FbcOpcode::GTInt
            } else {
                FbcOpcode::LTInt
            }));

        // Predict the next BlockId for the CondBranch loop-back.
        let next_id = BlockId::from_raw(self.arena.len() as u32);
        self.current_block.push(FbcInstruction::full(
            FbcOpcode::CondBranch,
            "",
            0,
            R::default(),
            0,
            0,
            Some(next_id),
            None,
        ));
        let loop_body_id = self.end_sub_block();
        debug_assert_eq!(loop_body_id.as_u32(), next_id.as_u32());

        // Emit kLoop in the parent block. vec_size = 1 (conservative).
        self.current_block.push(FbcInstruction::full(
            FbcOpcode::Loop,
            "",
            1,
            R::default(),
            0,
            0,
            Some(init_block_id),
            Some(loop_body_id),
        ));
        Ok(())
    }

    /// Compiles `SimpleForLoop(var, upper, body)` as a canonical counting loop.
    ///
    /// Forward loops implement `for (var = 0; var < upper; var = var + 1)`.
    /// Reverse loops implement `for (var = upper - 1; var >= 0; var = var - 1)`.
    pub(super) fn compile_simple_for_loop(
        &mut self,
        store: &FirStore,
        var: &str,
        upper: FirId,
        body: FirId,
        is_reverse: bool,
    ) -> Result<(), CompileError> {
        // Allocate loop variable if missing (simple pragmatic model: function-scoped slot).
        if !self.field_table.contains_key(var) {
            let offset = self.int_heap_offset;
            self.int_heap_offset += 1;
            self.field_table.insert(
                var.to_string(),
                MemoryDesc {
                    offset,
                    size: 1,
                    heap_type: HeapType::Int,
                },
            );
        }
        let desc =
            self.field_table
                .get(var)
                .cloned()
                .ok_or_else(|| CompileError::UndeclaredVariable {
                    name: var.to_string(),
                })?;

        // Init block.
        self.begin_sub_block();
        if is_reverse {
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::Int32Value,
                1,
                R::default(),
            ));
            self.compile_node(store, upper)?;
            self.current_block
                .push(FbcInstruction::new(FbcOpcode::SubInt));
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::StoreInt,
                    0,
                    R::default(),
                    desc.offset,
                    0,
                ));
        } else {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::StoreIntValue,
                    0,
                    R::default(),
                    desc.offset,
                    0,
                ));
        }
        let init_block_id = self.end_sub_block();

        // Body block: body; step; loop condition; cond-branch(loop back).
        self.begin_sub_block();
        self.compile_node(store, body)?;
        if is_reverse {
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::Int32Value,
                1,
                R::default(),
            ));
        }
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadInt,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        if !is_reverse {
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::Int32Value,
                1,
                R::default(),
            ));
        }
        self.current_block.push(FbcInstruction::new(if is_reverse {
            FbcOpcode::SubInt
        } else {
            FbcOpcode::AddInt
        }));
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::StoreInt,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        // Condition.
        // Stack convention: LHS on TOS → push upper (RHS) first, then var (LHS).
        if is_reverse {
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::Int32Value,
                0,
                R::default(),
            ));
        } else {
            self.compile_node(store, upper)?;
        }
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadInt,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        self.current_block.push(FbcInstruction::new(if is_reverse {
            FbcOpcode::GEInt
        } else {
            FbcOpcode::LTInt
        }));

        let next_id = BlockId::from_raw(self.arena.len() as u32);
        self.current_block.push(FbcInstruction::full(
            FbcOpcode::CondBranch,
            "",
            0,
            R::default(),
            0,
            0,
            Some(next_id),
            None,
        ));
        let loop_body_id = self.end_sub_block();
        debug_assert_eq!(loop_body_id.as_u32(), next_id.as_u32());

        self.current_block.push(FbcInstruction::full(
            FbcOpcode::Loop,
            "",
            1,
            R::default(),
            0,
            0,
            Some(init_block_id),
            Some(loop_body_id),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(FunCallInst*)` — compiles args in reverse order, then
    ///   emits the opcode from `gMathLibTable`.
    pub(super) fn compile_fun_call(
        &mut self,
        store: &FirStore,
        name: &str,
        args: &[FirId],
        typ: &FirType,
    ) -> Result<(), CompileError> {
        // Compile args in reverse order (stack discipline).
        for &arg in args.iter().rev() {
            self.compile_node(store, arg)?;
        }

        if matches!(name, "exp10f" | "exp10") && args.len() == 1 {
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::RealValue,
                0,
                R::from_f64(10.0),
            ));
            self.current_block
                .push(FbcInstruction::new(FbcOpcode::Powf));
            return Ok(());
        }

        if let Some(opcode) = math_lib_lookup(name) {
            self.current_block.push(FbcInstruction::new(opcode));
            return Ok(());
        }

        if !is_registered_foreign_function(name) {
            return Err(CompileError::UnknownMathFunction {
                name: name.to_string(),
            });
        }

        let ret = ForeignScalarType::from_fir_type(typ).ok_or_else(|| {
            CompileError::UnsupportedForeignFunctionSignature {
                name: name.to_string(),
                description: format!("unsupported return type {typ:?}"),
            }
        })?;
        let mut sig_args = Vec::with_capacity(args.len());
        for &arg in args {
            let arg_typ = store.value_type(arg).ok_or_else(|| {
                CompileError::UnsupportedForeignFunctionSignature {
                    name: name.to_string(),
                    description: format!(
                        "unsupported non-value argument node {:?}",
                        match_fir(store, arg)
                    ),
                }
            })?;
            let scalar = ForeignScalarType::from_fir_type(&arg_typ).ok_or_else(|| {
                CompileError::UnsupportedForeignFunctionSignature {
                    name: name.to_string(),
                    description: format!("unsupported argument type {arg_typ:?}"),
                }
            })?;
            sig_args.push(scalar);
        }

        if !is_supported_signature(ret, &sig_args) {
            return Err(CompileError::UnsupportedForeignFunctionSignature {
                name: name.to_string(),
                description: format!(
                    "ret={ret:?}, args={sig_args:?} are outside the interpreter foreign-call ABI"
                ),
            });
        }

        let signature = ForeignSignature {
            name: name.to_string(),
            ret,
            args: sig_args,
        };
        let opcode = match signature.ret {
            ForeignScalarType::Float32
            | ForeignScalarType::Float64
            | ForeignScalarType::FaustFloat => FbcOpcode::ForeignCallReal,
            ForeignScalarType::Int32 | ForeignScalarType::Bool => FbcOpcode::ForeignCallInt,
            ForeignScalarType::Void => FbcOpcode::ForeignCallVoid,
        };
        self.current_block
            .push(FbcInstruction::with_name(opcode, signature.encode()));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // UI
    // -----------------------------------------------------------------------
}
