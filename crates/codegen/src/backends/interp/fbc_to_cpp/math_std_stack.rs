//! `math_std_stack` instruction family of the FBC to C++ generator.
//!
//! Standard arithmetic and comparison: stack/heap operand addressing
//!
//! Split out of `fbc_to_cpp.rs` on 2026-08-18, where these arms were one
//! region of a single 1449-line `compile_instr`. The arms are moved verbatim.
//! The parameter list is this family's actual needs, so what a family does
//! not touch is visible from its signature.

use super::*;

impl BlockComp {
    /// Compiles one math_std_stack instruction into its native C++ equivalent.
    pub(super) fn compile_math_std_stack<R: FbcReal>(
        &mut self,
        out: &mut String,
        t: usize,
        instr: &FbcInstruction<R>,
    ) -> Result<(), FbcCppError> {
        use FbcOpcode::*;

        let o1 = instr.offset1;
        let o2 = instr.offset2;
        match instr.opcode {
            // ════════════════════════════════════════════════════════════
            // Standard math: stack OP stack  (pop2 → push1)
            // ════════════════════════════════════════════════════════════
            AddReal => {
                self.bin_rr(out, t, "+");
            }
            SubReal => {
                self.bin_rr(out, t, "-");
            }
            MultReal => {
                self.bin_rr(out, t, "*");
            }
            DivReal => {
                self.bin_rr(out, t, "/");
            }
            RemReal => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::remainder({}, {})", v1, v2));
            }
            AddInt => {
                self.bin_ii(out, t, "+");
            }
            SubInt => {
                self.bin_ii(out, t, "-");
            }
            MultInt => {
                self.bin_ii(out, t, "*");
            }
            DivInt => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("({} != 0 ? {} / {} : 0)", v2, v1, v2));
            }
            RemInt => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("({} != 0 ? {} % {} : 0)", v2, v1, v2));
            }
            LshInt => {
                self.bin_ii(out, t, "<<");
            }
            ARshInt => {
                self.bin_ii(out, t, ">>");
            }
            LRshInt => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("(int)((unsigned){} >> {})", v1, v2));
            }
            GTInt => {
                self.cmp_ii(out, t, ">");
            }
            LTInt => {
                self.cmp_ii(out, t, "<");
            }
            GEInt => {
                self.cmp_ii(out, t, ">=");
            }
            LEInt => {
                self.cmp_ii(out, t, "<=");
            }
            EQInt => {
                self.cmp_ii(out, t, "==");
            }
            NEInt => {
                self.cmp_ii(out, t, "!=");
            }
            GTReal => {
                self.cmp_rr(out, t, ">");
            }
            LTReal => {
                self.cmp_rr(out, t, "<");
            }
            GEReal => {
                self.cmp_rr(out, t, ">=");
            }
            LEReal => {
                self.cmp_rr(out, t, "<=");
            }
            EQReal => {
                self.cmp_rr(out, t, "==");
            }
            NEReal => {
                self.cmp_rr(out, t, "!=");
            }
            ANDInt => {
                self.bin_ii(out, t, "&");
            }
            ORInt => {
                self.bin_ii(out, t, "|");
            }
            XORInt => {
                self.bin_ii(out, t, "^");
            }

            // ════════════════════════════════════════════════════════════
            // Standard math: heap OP heap  → push1
            // ════════════════════════════════════════════════════════════
            AddRealHeap => {
                self.push_r(out, t, &format!("fVec[{}] + fVec[{}]", o1, o2));
            }
            SubRealHeap => {
                self.push_r(out, t, &format!("fVec[{}] - fVec[{}]", o1, o2));
            }
            MultRealHeap => {
                self.push_r(out, t, &format!("fVec[{}] * fVec[{}]", o1, o2));
            }
            DivRealHeap => {
                self.push_r(out, t, &format!("fVec[{}] / fVec[{}]", o1, o2));
            }
            RemRealHeap => {
                self.push_r(
                    out,
                    t,
                    &format!("std::remainder(fVec[{}], fVec[{}])", o1, o2),
                );
            }
            AddIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] + iVec[{}]", o1, o2));
            }
            SubIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] - iVec[{}]", o1, o2));
            }
            MultIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] * iVec[{}]", o1, o2));
            }
            DivIntHeap => {
                self.push_i(
                    out,
                    t,
                    &format!("(iVec[{}] != 0 ? iVec[{}] / iVec[{}] : 0)", o2, o1, o2),
                );
            }
            RemIntHeap => {
                self.push_i(
                    out,
                    t,
                    &format!("(iVec[{}] != 0 ? iVec[{}] % iVec[{}] : 0)", o2, o1, o2),
                );
            }
            LshIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] << iVec[{}]", o1, o2));
            }
            ARshIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] >> iVec[{}]", o1, o2));
            }
            LRshIntHeap => {
                self.push_i(
                    out,
                    t,
                    &format!("(int)((unsigned)iVec[{}] >> iVec[{}])", o1, o2),
                );
            }
            GTIntHeap => {
                self.push_i(out, t, &format!("(iVec[{}] > iVec[{}])", o1, o2));
            }
            LTIntHeap => {
                self.push_i(out, t, &format!("(iVec[{}] < iVec[{}])", o1, o2));
            }
            GEIntHeap => {
                self.push_i(out, t, &format!("(iVec[{}] >= iVec[{}])", o1, o2));
            }
            LEIntHeap => {
                self.push_i(out, t, &format!("(iVec[{}] <= iVec[{}])", o1, o2));
            }
            EQIntHeap => {
                self.push_i(out, t, &format!("(iVec[{}] == iVec[{}])", o1, o2));
            }
            NEIntHeap => {
                self.push_i(out, t, &format!("(iVec[{}] != iVec[{}])", o1, o2));
            }
            GTRealHeap => {
                self.push_i(out, t, &format!("(fVec[{}] > fVec[{}])", o1, o2));
            }
            LTRealHeap => {
                self.push_i(out, t, &format!("(fVec[{}] < fVec[{}])", o1, o2));
            }
            GERealHeap => {
                self.push_i(out, t, &format!("(fVec[{}] >= fVec[{}])", o1, o2));
            }
            LERealHeap => {
                self.push_i(out, t, &format!("(fVec[{}] <= fVec[{}])", o1, o2));
            }
            EQRealHeap => {
                self.push_i(out, t, &format!("(fVec[{}] == fVec[{}])", o1, o2));
            }
            NERealHeap => {
                self.push_i(out, t, &format!("(fVec[{}] != fVec[{}])", o1, o2));
            }
            ANDIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] & iVec[{}]", o1, o2));
            }
            ORIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] | iVec[{}]", o1, o2));
            }
            XORIntHeap => {
                self.push_i(out, t, &format!("iVec[{}] ^ iVec[{}]", o1, o2));
            }

            // ════════════════════════════════════════════════════════════
            // Standard math: heap OP stack  (pop1 stack → push1)
            // Each: v = pop_stack; push heap[o1] OP v
            // ════════════════════════════════════════════════════════════
            AddRealStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("fVec[{}] + {}", o1, v));
            }
            SubRealStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("fVec[{}] - {}", o1, v));
            }
            MultRealStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("fVec[{}] * {}", o1, v));
            }
            DivRealStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("fVec[{}] / {}", o1, v));
            }
            RemRealStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::remainder(fVec[{}], {})", o1, v));
            }
            AddIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] + {}", o1, v));
            }
            SubIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] - {}", o1, v));
            }
            MultIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] * {}", o1, v));
            }
            DivIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} != 0 ? iVec[{o1}] / {v} : 0)"));
            }
            RemIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("({v} != 0 ? iVec[{o1}] % {v} : 0)"));
            }
            LshIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] << {}", o1, v));
            }
            ARshIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] >> {}", o1, v));
            }
            LRshIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(int)((unsigned)iVec[{}] >> {})", o1, v));
            }
            GTIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(iVec[{}] > {})", o1, v));
            }
            LTIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(iVec[{}] < {})", o1, v));
            }
            GEIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(iVec[{}] >= {})", o1, v));
            }
            LEIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(iVec[{}] <= {})", o1, v));
            }
            EQIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(iVec[{}] == {})", o1, v));
            }
            NEIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("(iVec[{}] != {})", o1, v));
            }
            GTRealStack => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(fVec[{}] > {})", o1, v));
            }
            LTRealStack => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(fVec[{}] < {})", o1, v));
            }
            GERealStack => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(fVec[{}] >= {})", o1, v));
            }
            LERealStack => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(fVec[{}] <= {})", o1, v));
            }
            EQRealStack => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(fVec[{}] == {})", o1, v));
            }
            NERealStack => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(fVec[{}] != {})", o1, v));
            }
            ANDIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] & {}", o1, v));
            }
            ORIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] | {}", o1, v));
            }
            XORIntStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("iVec[{}] ^ {}", o1, v));
            }

            other => unreachable!("math_std_stack dispatch received {other:?}"),
        }

        Ok(())
    }
}
