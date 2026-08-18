//! `casts` instruction family of the FBC to C++ generator.
//!
//! Numeric casts and bit-level reinterpretation
//!
//! Split out of `fbc_to_cpp.rs` on 2026-08-18, where these arms were one
//! region of a single 1449-line `compile_instr`. The arms are moved verbatim.
//! The parameter list is this family's actual needs, so what a family does
//! not touch is visible from its signature.

use super::*;

impl BlockComp {
    /// Compiles one casts instruction into its native C++ equivalent.
    pub(super) fn compile_casts<R: FbcReal>(
        &mut self,
        out: &mut String,
        t: usize,
        instr: &FbcInstruction<R>,
    ) -> Result<(), FbcCppError> {
        use FbcOpcode::*;

        let o1 = instr.offset1;
        match instr.opcode {
            // ── Cast / Bitcast ────────────────────────────────────────────
            CastReal => {
                let v = self.pop_i();
                self.push_r(out, t, &format!("({}){}", self.real_ctype, v));
            }
            CastInt => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("(int){}", v));
            }
            CastRealHeap => {
                self.push_r(out, t, &format!("({})iVec[{}]", self.real_ctype, o1));
            }
            CastIntHeap => {
                self.push_i(out, t, &format!("(int)fVec[{}]", o1));
            }
            BitcastInt => {
                let v = self.pop_r();
                // Reinterpret float bits as int32.
                self.push_i(
                    out,
                    t,
                    &format!(
                        "([]({} x){{ int r; memcpy(&r, &x, sizeof(int)); return r; }})({})",
                        self.real_ctype, v
                    ),
                );
            }
            BitcastReal => {
                let v = self.pop_i();
                // Reinterpret int32 bits as float.
                self.push_r(
                    out,
                    t,
                    &format!(
                        "([]( int x){{ {} r; memcpy(&r, &x, sizeof({})); return r; }})({})",
                        self.real_ctype, self.real_ctype, v
                    ),
                );
            }

            other => unreachable!("casts dispatch received {other:?}"),
        }

        Ok(())
    }
}
