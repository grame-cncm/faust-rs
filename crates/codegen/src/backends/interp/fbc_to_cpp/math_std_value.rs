//! `math_std_value` instruction family of the FBC to C++ generator.
//!
//! Standard arithmetic and comparison: immediate-value addressing
//!
//! Split out of `fbc_to_cpp.rs` on 2026-08-18, where these arms were one
//! region of a single 1449-line `compile_instr`. The arms are moved verbatim.
//! The parameter list is this family's actual needs, so what a family does
//! not touch is visible from its signature.

use super::*;

impl BlockComp {
    /// Compiles one math_std_value instruction into its native C++ equivalent.
    pub(super) fn compile_math_std_value<R: FbcReal>(
        &mut self,
        out: &mut String,
        t: usize,
        instr: &FbcInstruction<R>,
    ) -> Result<(), FbcCppError> {
        use FbcOpcode::*;

        let o1 = instr.offset1;
        let iv = instr.int_value;
        let rv = instr.real_value;
        match instr.opcode {
            // ════════════════════════════════════════════════════════════
            // Standard math: value OP stack  (pop1 stack + immediate → push1)
            // Each: v = pop_stack; push v OP imm
            // ════════════════════════════════════════════════════════════
            AddRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("{v} + {lit}"));
            }
            SubRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("{v} - {lit}"));
            }
            MultRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("{v} * {lit}"));
            }
            DivRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("{v} / {lit}"));
            }
            RemRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::remainder({v}, {lit})"));
            }
            AddIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} + {iv}"));
            }
            SubIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} - {iv}"));
            }
            MultIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} * {iv}"));
            }
            DivIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({iv} != 0 ? {v} / {iv} : 0)"));
            }
            RemIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({iv} != 0 ? {v} % {iv} : 0)"));
            }
            LshIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} << {iv}"));
            }
            ARshIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} >> {iv}"));
            }
            LRshIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(int)((unsigned){v} >> {iv})"));
            }
            GTIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} > {iv})"));
            }
            LTIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} < {iv})"));
            }
            GEIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} >= {iv})"));
            }
            LEIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} <= {iv})"));
            }
            EQIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} == {iv})"));
            }
            NEIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} != {iv})"));
            }
            GTRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("({v} > {lit})"));
            }
            LTRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("({v} < {lit})"));
            }
            GERealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("({v} >= {lit})"));
            }
            LERealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("({v} <= {lit})"));
            }
            EQRealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("({v} == {lit})"));
            }
            NERealStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("({v} != {lit})"));
            }
            ANDIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} & {iv}"));
            }
            ORIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} | {iv}"));
            }
            XORIntStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("{v} ^ {iv}"));
            }

            // ════════════════════════════════════════════════════════════
            // Standard math: value OP heap  → push1  (non-inverted)
            // heap[o1] OP immediate
            // ════════════════════════════════════════════════════════════
            AddRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("fVec[{o1}] + {lit}"));
            }
            SubRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("fVec[{o1}] - {lit}"));
            }
            MultRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("fVec[{o1}] * {lit}"));
            }
            DivRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("fVec[{o1}] / {lit}"));
            }
            RemRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::remainder(fVec[{o1}], {lit})"));
            }
            AddIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] + {iv}"));
            }
            SubIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] - {iv}"));
            }
            MultIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] * {iv}"));
            }
            DivIntValue => {
                self.push_i(out, t, &format!("({iv} != 0 ? iVec[{o1}] / {iv} : 0)"));
            }
            RemIntValue => {
                self.push_i(out, t, &format!("({iv} != 0 ? iVec[{o1}] % {iv} : 0)"));
            }
            LshIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] << {iv}"));
            }
            ARshIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] >> {iv}"));
            }
            LRshIntValue => {
                self.push_i(out, t, &format!("(int)((unsigned)iVec[{o1}] >> {iv})"));
            }
            GTIntValue => {
                self.push_i(out, t, &format!("(iVec[{o1}] > {iv})"));
            }
            LTIntValue => {
                self.push_i(out, t, &format!("(iVec[{o1}] < {iv})"));
            }
            GEIntValue => {
                self.push_i(out, t, &format!("(iVec[{o1}] >= {iv})"));
            }
            LEIntValue => {
                self.push_i(out, t, &format!("(iVec[{o1}] <= {iv})"));
            }
            EQIntValue => {
                self.push_i(out, t, &format!("(iVec[{o1}] == {iv})"));
            }
            NEIntValue => {
                self.push_i(out, t, &format!("(iVec[{o1}] != {iv})"));
            }
            GTRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] > {lit})"));
            }
            LTRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] < {lit})"));
            }
            GERealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] >= {lit})"));
            }
            LERealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] <= {lit})"));
            }
            EQRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] == {lit})"));
            }
            NERealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] != {lit})"));
            }
            ANDIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] & {iv}"));
            }
            ORIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] | {iv}"));
            }
            XORIntValue => {
                self.push_i(out, t, &format!("iVec[{o1}] ^ {iv}"));
            }

            // ════════════════════════════════════════════════════════════
            // Standard math: value OP heap — non-commutative inverted
            // Meaning: immediate OP heap[o1] (operands swapped vs above)
            // ════════════════════════════════════════════════════════════
            SubRealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("{lit} - fVec[{o1}]"));
            }
            SubIntValueInvert => {
                self.push_i(out, t, &format!("{iv} - iVec[{o1}]"));
            }
            DivRealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("{lit} / fVec[{o1}]"));
            }
            DivIntValueInvert => {
                self.push_i(
                    out,
                    t,
                    &format!("(iVec[{o1}] != 0 ? {iv} / iVec[{o1}] : 0)"),
                );
            }
            RemRealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::remainder({lit}, fVec[{o1}])"));
            }
            RemIntValueInvert => {
                self.push_i(
                    out,
                    t,
                    &format!("(iVec[{o1}] != 0 ? {iv} % iVec[{o1}] : 0)"),
                );
            }
            LshIntValueInvert => {
                self.push_i(out, t, &format!("iVec[{o1}] << {iv}"));
            }
            ARshIntValueInvert => {
                self.push_i(out, t, &format!("iVec[{o1}] >> {iv}"));
            }
            LRshIntValueInvert => {
                self.push_i(out, t, &format!("(int)((unsigned)iVec[{o1}] >> {iv})"));
            }
            GTIntValueInvert => {
                self.push_i(out, t, &format!("(iVec[{o1}] > {iv})"));
            }
            LTIntValueInvert => {
                self.push_i(out, t, &format!("(iVec[{o1}] < {iv})"));
            }
            GEIntValueInvert => {
                self.push_i(out, t, &format!("(iVec[{o1}] >= {iv})"));
            }
            LEIntValueInvert => {
                self.push_i(out, t, &format!("(iVec[{o1}] <= {iv})"));
            }
            GTRealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] > {lit})"));
            }
            LTRealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] < {lit})"));
            }
            GERealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] >= {lit})"));
            }
            LERealValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_i(out, t, &format!("(fVec[{o1}] <= {lit})"));
            }

            other => unreachable!("math_std_value dispatch received {other:?}"),
        }

        Ok(())
    }
}
