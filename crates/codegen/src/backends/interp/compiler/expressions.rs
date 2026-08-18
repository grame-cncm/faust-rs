//! `expressions` half of the FBC compiler.
//!
//! Pure value-producing instructions: arithmetic, negation, casts, bit reinterpretation, and select.
//!
//! Split out of `compiler.rs` on 2026-08-18, where all 54 methods sat in one
//! 1891-line `impl`. The method bodies are moved verbatim; only their
//! visibility widened from private to `pub(super)` so the sibling modules can
//! still reach them.

use super::*;

impl<R: FbcReal> FirToFbcCompiler<R> {
    /// # Source provenance (C++)
    /// - `visit(BinopInst*)` — compiles operands then emits int/real opcode.
    pub(super) fn compile_binop(
        &mut self,
        store: &FirStore,
        op: fir::FirBinOp,
        lhs: FirId,
        rhs: FirId,
    ) -> Result<(), CompileError> {
        // C++ compiles inst2 (rhs) first, then inst1 (lhs).
        // lhs ends up on TOS for the operator.
        self.compile_node(store, rhs)?;
        let real_t2 = self.current_block_top_is_real();
        self.compile_node(store, lhs)?;
        let real_t1 = self.current_block_top_is_real();

        let (int_op, real_op) = binop_to_fbc(op);
        let opcode = if real_t1 || real_t2 { real_op } else { int_op };
        self.current_block.push(FbcInstruction::new(opcode));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(NegInst*)` — multiplies by -1.
    pub(super) fn compile_neg(
        &mut self,
        store: &FirStore,
        value: FirId,
        typ: &FirType,
    ) -> Result<(), CompileError> {
        if is_int_type(typ) {
            // Push value, push -1, emit MultInt.
            self.compile_node(store, value)?;
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::Int32Value,
                -1,
                R::default(),
            ));
            self.current_block
                .push(FbcInstruction::new(FbcOpcode::MultInt));
        } else {
            // Push value, push -1.0, emit MultReal.
            self.compile_node(store, value)?;
            self.current_block.push(FbcInstruction::with_values(
                FbcOpcode::RealValue,
                0,
                R::from_f64(-1.0),
            ));
            self.current_block
                .push(FbcInstruction::new(FbcOpcode::MultReal));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Cast
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(CastInst*)` — emits `kCastInt` or `kCastReal` if type changes.
    pub(super) fn compile_cast(
        &mut self,
        store: &FirStore,
        typ: &FirType,
        value: FirId,
    ) -> Result<(), CompileError> {
        self.compile_node(store, value)?;
        let real_operand = self.current_block_top_is_real();

        if is_int_type(typ) {
            // Cast to int — only emit if operand is real.
            if real_operand {
                self.current_block
                    .push(FbcInstruction::new(FbcOpcode::CastInt));
            }
        } else {
            // Cast to real — only emit if operand is int.
            if !real_operand {
                self.current_block
                    .push(FbcInstruction::new(FbcOpcode::CastReal));
            }
        }
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(BitcastInst*)`.
    pub(super) fn compile_bitcast(
        &mut self,
        store: &FirStore,
        typ: &FirType,
        value: FirId,
    ) -> Result<(), CompileError> {
        self.compile_node(store, value)?;
        let opcode = if is_int_type(typ) {
            FbcOpcode::BitcastInt
        } else {
            FbcOpcode::BitcastReal
        };
        self.current_block.push(FbcInstruction::new(opcode));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Control flow
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(Select2Inst*)` — block-switching for select.
    pub(super) fn compile_select2(
        &mut self,
        store: &FirStore,
        cond: FirId,
        then_value: FirId,
        else_value: FirId,
    ) -> Result<(), CompileError> {
        // Compile condition into current block.
        self.compile_node(store, cond)?;

        // Compile 'then' in a new sub-block.
        self.begin_sub_block();
        self.compile_node(store, then_value)?;
        let is_real = self.current_block_top_is_real();
        let then_block_id = self.end_sub_block();

        // Compile 'else' in a new sub-block.
        self.begin_sub_block();
        self.compile_node(store, else_value)?;
        let else_block_id = self.end_sub_block();

        // Emit select instruction referencing both sub-blocks.
        let opcode = if is_real {
            FbcOpcode::SelectReal
        } else {
            FbcOpcode::SelectInt
        };
        self.current_block.push(FbcInstruction::full(
            opcode,
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
}
