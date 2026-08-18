//! `control` instruction family of the FBC to C++ generator.
//!
//! Control flow, selects, and the UI opcodes that never appear in code blocks
//!
//! Split out of `fbc_to_cpp.rs` on 2026-08-18, where these arms were one
//! region of a single 1449-line `compile_instr`. The arms are moved verbatim.
//! The parameter list is this family's actual needs, so what a family does
//! not touch is visible from its signature.

use super::*;

impl BlockComp {
    /// Compiles one control instruction into its native C++ equivalent.
    pub(super) fn compile_control<R: FbcReal>(
        &mut self,
        arena: &FbcBlockArena<R>,
        out: &mut String,
        t: usize,
        instr: &FbcInstruction<R>,
        block_id: BlockId,
        pc: usize,
    ) -> Result<(), FbcCppError> {
        use FbcOpcode::*;

        match instr.opcode {
            // ════════════════════════════════════════════════════════════
            // Control flow
            // ════════════════════════════════════════════════════════════
            Loop => {
                let init_id = instr.branch1.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;
                let body_id = instr.branch2.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;
                // Compile init block (no-return, just heap writes).
                // No outer `{…}` needed: the shared ic/rc counters guarantee
                // unique variable names across init and body blocks.
                self.compile_block(arena, out, t, init_id)?;
                writeln!(out, "{}while (true) {{", tab(t)).unwrap();
                self.compile_block(arena, out, t + 1, body_id)?;
                writeln!(out, "{}}}", tab(t)).unwrap();
            }

            CondBranch => {
                // Loop exit: CondBranch is always inside a while(true) body.
                let cond = self.pop_i();
                writeln!(out, "{}if (!{}) {{ break; }}", tab(t), cond).unwrap();
            }

            If => {
                let cond = self.pop_i();
                let b1 = instr.branch1.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;
                let b2 = instr.branch2.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;

                // Save stack state; branches should not affect the computation
                // stack net (they may push/pop internally, ending in balance).
                let saved_r = self.rstack.clone();
                let saved_i = self.istack.clone();

                // Render branches independently so a Return-only FBC block does
                // not become an empty C++ `else {}` block. C++ does not need the
                // branch at all when it has no emitted statements.
                let mut then_out = String::new();
                self.compile_block(arena, &mut then_out, t + 1, b1)?;
                self.rstack = saved_r.clone();
                self.istack = saved_i.clone();

                let mut else_out = String::new();
                self.compile_block(arena, &mut else_out, t + 1, b2)?;
                self.rstack = saved_r;
                self.istack = saved_i;

                match (then_out.is_empty(), else_out.is_empty()) {
                    (false, false) => {
                        writeln!(out, "{}if ({} != 0) {{", tab(t), cond).unwrap();
                        out.push_str(&then_out);
                        writeln!(out, "{}}} else {{", tab(t)).unwrap();
                        out.push_str(&else_out);
                        writeln!(out, "{}}}", tab(t)).unwrap();
                    }
                    (false, true) => {
                        writeln!(out, "{}if ({} != 0) {{", tab(t), cond).unwrap();
                        out.push_str(&then_out);
                        writeln!(out, "{}}}", tab(t)).unwrap();
                    }
                    (true, false) => {
                        writeln!(out, "{}if ({} == 0) {{", tab(t), cond).unwrap();
                        out.push_str(&else_out);
                        writeln!(out, "{}}}", tab(t)).unwrap();
                    }
                    (true, true) => {}
                }
            }

            SelectReal => {
                let cond = self.pop_i();
                let b1 = instr.branch1.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;
                let b2 = instr.branch2.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;

                // Pre-declare merge variable.
                let merge = format!("fR{}", self.rc);
                self.rc += 1;
                writeln!(
                    out,
                    "{}{} {} = {}(0);",
                    tab(t),
                    self.real_ctype,
                    merge,
                    self.real_ctype
                )
                .unwrap();

                let saved_r = self.rstack.clone();
                let saved_i = self.istack.clone();

                writeln!(out, "{}if ({} != 0) {{", tab(t), cond).unwrap();
                self.compile_block(arena, out, t + 1, b1)?;
                if self.rstack.len() > saved_r.len() {
                    let bval = self.rstack.pop().unwrap();
                    writeln!(out, "{}\t{} = {};", tab(t), merge, bval).unwrap();
                }
                self.rstack = saved_r.clone();
                self.istack = saved_i.clone();

                writeln!(out, "{}}} else {{", tab(t)).unwrap();
                self.compile_block(arena, out, t + 1, b2)?;
                if self.rstack.len() > saved_r.len() {
                    let bval = self.rstack.pop().unwrap();
                    writeln!(out, "{}\t{} = {};", tab(t), merge, bval).unwrap();
                }
                self.rstack = saved_r;
                self.istack = saved_i;
                writeln!(out, "{}}}", tab(t)).unwrap();

                // The merged value is now on the real stack.
                self.rstack.push(merge);
            }

            SelectInt => {
                let cond = self.pop_i();
                let b1 = instr.branch1.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;
                let b2 = instr.branch2.ok_or(FbcCppError::MissingBranchTarget {
                    opcode: instr.opcode,
                    block_id,
                    pc,
                })?;

                let merge = format!("iI{}", self.ic);
                self.ic += 1;
                writeln!(out, "{}int {} = 0;", tab(t), merge).unwrap();

                let saved_r = self.rstack.clone();
                let saved_i = self.istack.clone();

                writeln!(out, "{}if ({} != 0) {{", tab(t), cond).unwrap();
                self.compile_block(arena, out, t + 1, b1)?;
                if self.istack.len() > saved_i.len() {
                    let bval = self.istack.pop().unwrap();
                    writeln!(out, "{}\t{} = {};", tab(t), merge, bval).unwrap();
                }
                self.rstack = saved_r.clone();
                self.istack = saved_i.clone();

                writeln!(out, "{}}} else {{", tab(t)).unwrap();
                self.compile_block(arena, out, t + 1, b2)?;
                if self.istack.len() > saved_i.len() {
                    let bval = self.istack.pop().unwrap();
                    writeln!(out, "{}\t{} = {};", tab(t), merge, bval).unwrap();
                }
                self.rstack = saved_r;
                self.istack = saved_i;
                writeln!(out, "{}}}", tab(t)).unwrap();

                self.istack.push(merge);
            }

            Return => {
                // Already handled as loop break in compile_block; should not reach here.
            }

            // ── UI opcodes: not valid inside a code block ─────────────────
            OpenVerticalBox
            | OpenHorizontalBox
            | OpenTabBox
            | CloseBox
            | AddButton
            | AddCheckButton
            | AddHorizontalSlider
            | AddVerticalSlider
            | AddNumEntry
            | AddSoundfile
            | AddHorizontalBargraph
            | AddVerticalBargraph
            | Declare => {
                // UI instructions appear in ui_block, not in code blocks.
                // Silently skip if encountered here.
            }
            other => unreachable!("control dispatch received {other:?}"),
        }

        Ok(())
    }
}
