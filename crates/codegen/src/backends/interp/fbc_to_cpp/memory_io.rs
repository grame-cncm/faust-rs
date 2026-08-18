//! `memory_io` instruction family of the FBC to C++ generator.
//!
//! Nop, constants, heap and indexed memory, bulk moves, audio I/O, sound fields
//!
//! Split out of `fbc_to_cpp.rs` on 2026-08-18, where these arms were one
//! region of a single 1449-line `compile_instr`. The arms are moved verbatim.
//! The parameter list is this family's actual needs, so what a family does
//! not touch is visible from its signature.

use super::*;

impl BlockComp {
    /// Compiles one memory_io instruction into its native C++ equivalent.
    pub(super) fn compile_memory_io<R: FbcReal>(
        &mut self,
        out: &mut String,
        t: usize,
        instr: &FbcInstruction<R>,
        block_id: BlockId,
        pc: usize,
    ) -> Result<(), FbcCppError> {
        use FbcOpcode::*;

        let o1 = instr.offset1;
        let o2 = instr.offset2;
        let iv = instr.int_value;
        let rv = instr.real_value;
        match instr.opcode {
            // ── Nop ──────────────────────────────────────────────────────
            Nop => {}
            ForeignCallReal | ForeignCallInt | ForeignCallVoid => {
                return Err(FbcCppError::Unsupported {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                });
            }

            // ── Constants ────────────────────────────────────────────────
            RealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &lit);
            }
            Int32Value => {
                self.push_i(out, t, &iv.to_string());
            }

            // ── Memory: simple load ───────────────────────────────────────
            LoadReal => {
                self.push_r(out, t, &format!("fVec[{}]", o1));
            }
            LoadInt => {
                self.push_i(out, t, &format!("iVec[{}]", o1));
            }

            // ── Memory: simple store ──────────────────────────────────────
            StoreReal => {
                let v = self.pop_r();
                writeln!(out, "{}fVec[{}] = {};", tab(t), o1, v).unwrap();
            }
            StoreInt => {
                let v = self.pop_i();
                writeln!(out, "{}iVec[{}] = {};", tab(t), o1, v).unwrap();
            }

            // ── Memory: store immediate ───────────────────────────────────
            StoreRealValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                writeln!(out, "{}fVec[{}] = {};", tab(t), o1, lit).unwrap();
            }
            StoreIntValue => {
                writeln!(out, "{}iVec[{}] = {};", tab(t), o1, iv).unwrap();
            }

            // ── Memory: indexed load ──────────────────────────────────────
            LoadIndexedReal => {
                let idx = self.pop_i();
                self.push_r(out, t, &format!("fVec[{} + {}]", o1, idx));
            }
            LoadIndexedInt => {
                let idx = self.pop_i();
                self.push_i(out, t, &format!("iVec[{} + {}]", o1, idx));
            }

            // ── Memory: indexed store ─────────────────────────────────────
            StoreIndexedReal => {
                let idx = self.pop_i();
                let val = self.pop_r();
                writeln!(out, "{}fVec[{} + {}] = {};", tab(t), o1, idx, val).unwrap();
            }
            StoreIndexedInt => {
                let idx = self.pop_i();
                let val = self.pop_i();
                writeln!(out, "{}iVec[{} + {}] = {};", tab(t), o1, idx, val).unwrap();
            }

            // ── Memory: bulk store ────────────────────────────────────────
            BlockStoreReal => {
                if let Some(BlockStoreData::Real(table)) = &instr.block_store {
                    let count = o2 as usize;
                    let tname = format!("kTab_{}", self.tc);
                    self.tc += 1;
                    write!(
                        out,
                        "{}{{ static const {} {}[] = {{",
                        tab(t),
                        self.real_ctype,
                        tname
                    )
                    .unwrap();
                    for (i, &v) in table[..count.min(table.len())].iter().enumerate() {
                        if i > 0 {
                            write!(out, ",").unwrap();
                        }
                        write!(out, "{}", fmt_real_lit(v, self.real_ctype)).unwrap();
                    }
                    writeln!(out, "}};").unwrap();
                    writeln!(
                        out,
                        "{}  for (int kI = 0; kI < {}; kI++) fVec[{} + kI] = {}[kI]; }}",
                        tab(t),
                        count,
                        o1,
                        tname
                    )
                    .unwrap();
                }
            }
            BlockStoreInt => {
                if let Some(BlockStoreData::Int(table)) = &instr.block_store {
                    let count = o2 as usize;
                    let tname = format!("kTab_{}", self.tc);
                    self.tc += 1;
                    write!(out, "{}{{ static const int {}[] = {{", tab(t), tname).unwrap();
                    for (i, &v) in table[..count.min(table.len())].iter().enumerate() {
                        if i > 0 {
                            write!(out, ",").unwrap();
                        }
                        write!(out, "{v}").unwrap();
                    }
                    writeln!(out, "}};").unwrap();
                    writeln!(
                        out,
                        "{}  for (int kI = 0; kI < {}; kI++) iVec[{} + kI] = {}[kI]; }}",
                        tab(t),
                        count,
                        o1,
                        tname
                    )
                    .unwrap();
                }
            }

            // ── Memory: move (heap-to-heap) ───────────────────────────────
            MoveReal => {
                writeln!(out, "{}fVec[{}] = fVec[{}];", tab(t), o1, o2).unwrap();
            }
            MoveInt => {
                writeln!(out, "{}iVec[{}] = iVec[{}];", tab(t), o1, o2).unwrap();
            }
            PairMoveReal => {
                writeln!(out, "{}fVec[{}] = fVec[{}];", tab(t), o1, o1 - 1).unwrap();
                writeln!(out, "{}fVec[{}] = fVec[{}];", tab(t), o2, o2 - 1).unwrap();
            }
            PairMoveInt => {
                writeln!(out, "{}iVec[{}] = iVec[{}];", tab(t), o1, o1 - 1).unwrap();
                writeln!(out, "{}iVec[{}] = iVec[{}];", tab(t), o2, o2 - 1).unwrap();
            }
            BlockPairMoveReal => {
                writeln!(
                    out,
                    "{}for (int kI = {}; kI < {}; kI += 2) fVec[kI + 1] = fVec[kI];",
                    tab(t),
                    o1,
                    o2
                )
                .unwrap();
            }
            BlockPairMoveInt => {
                writeln!(
                    out,
                    "{}for (int kI = {}; kI < {}; kI += 2) iVec[kI + 1] = iVec[kI];",
                    tab(t),
                    o1,
                    o2
                )
                .unwrap();
            }
            BlockShiftReal => {
                writeln!(
                    out,
                    "{}for (int kI = {}; kI > {}; kI--) fVec[kI] = fVec[kI - 1];",
                    tab(t),
                    o1,
                    o2
                )
                .unwrap();
            }
            BlockShiftInt => {
                writeln!(
                    out,
                    "{}for (int kI = {}; kI > {}; kI--) iVec[kI] = iVec[kI - 1];",
                    tab(t),
                    o1,
                    o2
                )
                .unwrap();
            }

            // ── I/O ───────────────────────────────────────────────────────
            LoadInput => {
                let idx = self.pop_i();
                self.push_r(
                    out,
                    t,
                    &format!("({})inputs[{}][{}]", self.real_ctype, o1, idx),
                );
            }
            LoadOutput => {
                let idx = self.pop_i();
                self.push_r(
                    out,
                    t,
                    &format!("({})outputs[{}][{}]", self.real_ctype, o1, idx),
                );
            }
            StoreOutput => {
                let idx = self.pop_i();
                let val = self.pop_r();
                writeln!(
                    out,
                    "{}outputs[{}][{}] = (FAUSTFLOAT){};",
                    tab(t),
                    o1,
                    idx,
                    val
                )
                .unwrap();
            }

            // ── Sound fields (unsupported) ────────────────────────────────
            LoadSoundFieldInt | LoadSoundFieldReal => {
                return Err(FbcCppError::Unsupported {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                });
            }

            other => unreachable!("memory_io dispatch received {other:?}"),
        }

        Ok(())
    }
}
