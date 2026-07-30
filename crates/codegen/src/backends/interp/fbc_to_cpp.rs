//! FBC → native C++ code generator.
//!
//! Translates a compiled [`FbcDspFactory<R>`] into a self-contained C++ header
//! containing a class that faithfully reproduces the interpreter's semantics
//! using native C++ code — **no** interpreter runtime dependency.
//!
//! # Overview
//!
//! The generator performs a single pass over each of the 6 code blocks,
//! maintaining a **virtual stack** of named C++ temporary variables
//! (`fRN` for reals, `iIN` for integers). Instructions are translated
//! one-by-one into C++ statements that declare and use those temporaries.
//!
//! | FBC instruction | Generated C++ |
//! |---|---|
//! | `Loop(init, body)` | `<init>; while(true){ <body>; }` |
//! | `CondBranch` | `if (!<cond>) { break; }` inside `while(true)` |
//! | `If(b1, b2)` | `if (<cond>) { <b1> } else { <b2> }` |
//! | `SelectReal/Int(b1, b2)` | pre-declared merge var + `if/else` |
//! | `Return` | end of block (no explicit `return` emitted) |
//!
//! # Memory layout
//!
//! The generated class owns:
//! - `int iVec[int_heap_size]` — integer heap
//! - `<REAL> fVec[real_heap_size]` — real heap
//! - `int fSampleRate` — sample rate shadow (`iVec[sr_offset]` alias)
//!
//! # Role in the Rust port
//! This path is an ahead-of-time backend over already compiled interpreter
//! bytecode. It is therefore useful for validating interpreter semantics and
//! producing native artifacts without depending on FIR/C++ backend parity.
//!
//! # Usage example
//!
//! ```rust,ignore
//! let factory = read_fbc(source)?;
//! let opts = FbcCppOptions::default();
//! let cpp = generate_cpp_from_fbc(&factory, &opts)?;
//! std::fs::write("my_dsp.h", cpp)?;
//! ```

use std::fmt::Write as _;

use super::bytecode::{
    BlockId, BlockStoreData, FbcBlockArena, FbcInstruction, FbcMetaInstruction, FbcUiInstruction,
};
use super::factory::FbcDspFactory;
use super::opcode::FbcOpcode;
use super::real::FbcReal;

// ── Public API ──────────────────────────────────────────────────────────────

/// Options for the FBC → native C++ code generator.
///
/// These options only affect the generated wrapper/header surface; they do not
/// alter interpreter semantics encoded in the source bytecode factory.
#[derive(Clone, Debug)]
pub struct FbcCppOptions {
    /// Class name override.
    ///
    /// When `None`, defaults to the factory name (sanitized to a valid C++
    /// identifier), matching Faust C++'s `mydsp` default. Falls back to
    /// `"FbcDsp"` if the factory name is empty.
    pub class_name: Option<String>,
    /// Whether to emit `#pragma once` at the top of the header. Default: `true`.
    pub pragma_once: bool,
    /// Optional C++ namespace to wrap the class in. Default: `None`.
    pub namespace: Option<String>,
}

impl Default for FbcCppOptions {
    /// Returns the default C++ wrapper-generation options.
    fn default() -> Self {
        Self {
            class_name: None,
            pragma_once: true,
            namespace: None,
        }
    }
}

/// Errors that can occur during FBC → native C++ code generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FbcCppError {
    /// An instruction references a branch (sub-block) that is absent.
    MissingBranchTarget {
        opcode: FbcOpcode,
        block_id: BlockId,
        pc: usize,
    },
    /// A `BlockId` referenced in the bytecode is out of range for the arena.
    InvalidBlockId { block_id: BlockId },
    /// An opcode is not translatable in code-generation mode.
    ///
    /// Currently only `LoadSoundFieldInt` / `LoadSoundFieldReal` fall here,
    /// as sound-file support requires an external runtime object.
    Unsupported {
        opcode: FbcOpcode,
        block_id: BlockId,
        pc: usize,
    },
}

impl std::fmt::Display for FbcCppError {
    /// Formats the code-generation error as a human-readable diagnostic.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBranchTarget {
                opcode,
                block_id,
                pc,
            } => {
                write!(
                    f,
                    "missing branch target for {opcode:?} at block {block_id:?} pc {pc}"
                )
            }
            Self::InvalidBlockId { block_id } => {
                write!(f, "invalid BlockId {block_id:?}")
            }
            Self::Unsupported {
                opcode,
                block_id,
                pc,
            } => {
                write!(
                    f,
                    "unsupported opcode {opcode:?} at block {block_id:?} pc {pc}"
                )
            }
        }
    }
}

impl std::error::Error for FbcCppError {}

/// Generates a self-contained native C++ header from a compiled
/// [`FbcDspFactory<R>`].
///
/// The class extends `dsp` from `faust/dsp/dsp.h` and implements the full
/// Faust DSP lifecycle without any interpreter runtime.
///
/// This is a semantic re-emission pass over FBC, not a pretty-printer for FIR:
/// if the produced C++ diverges from interpreter behavior, the bug is in this
/// lowering layer, not in earlier FIR backends.
///
/// # Errors
///
/// Returns [`FbcCppError`] if the bytecode contains unsupported opcodes
/// or invalid branch targets.
pub fn generate_cpp_from_fbc<R: FbcReal>(
    factory: &FbcDspFactory<R>,
    options: &FbcCppOptions,
) -> Result<String, FbcCppError> {
    CppGen::new(factory, options).generate()
}

// ── Internal: class-level generator ─────────────────────────────────────────

/// Class-level generator state shared across all emitted lifecycle methods.
///
/// Per-block temporary stacks/counters are intentionally delegated to
/// [`BlockComp`] so temporaries can be either isolated or shared depending on
/// the method being generated (`compute` shares one instance across both
/// interpreter compute blocks).
struct CppGen<'a, R: FbcReal> {
    factory: &'a FbcDspFactory<R>,
    options: &'a FbcCppOptions,
    class_name: String,
    real_ctype: &'static str,
}

impl<'a, R: FbcReal> CppGen<'a, R> {
    /// Creates a class-level generator from one factory/options pair.
    fn new(factory: &'a FbcDspFactory<R>, options: &'a FbcCppOptions) -> Self {
        let class_name = options
            .class_name
            .as_deref()
            .map(sanitize_cpp_ident)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let base = sanitize_cpp_ident(&factory.name);
                if base.is_empty() {
                    "FbcDsp".to_owned()
                } else {
                    base
                }
            });
        let real_ctype = if R::TYPE_NAME == "f32" {
            "float"
        } else {
            "double"
        };
        Self {
            factory,
            options,
            class_name,
            real_ctype,
        }
    }

    /// Generates the full self-contained C++ header for this FBC factory.
    fn generate(&self) -> Result<String, FbcCppError> {
        let mut out = String::new();
        let f = self.factory;

        // ── File header ──────────────────────────────────────────────────
        if self.options.pragma_once {
            writeln!(out, "#pragma once").unwrap();
        }
        writeln!(
            out,
            "// Auto-generated by faust-rs (FBC → native C++). DO NOT EDIT.\n\
             // Factory : {name}\n\
             // SHA key : {sha}\n\
             // Options : {opts}",
            name = f.name,
            sha = f.sha_key,
            opts = f.compile_options,
        )
        .unwrap();
        writeln!(
            out,
            "\n#include <algorithm>\n\
             #include <cmath>\n\
             #include <cstring>\n\
             #include <limits>\n\
             #include \"faust/dsp/dsp.h\"\n\
             #include \"faust/gui/UI.h\"\n\
             #include \"faust/gui/meta.h\"\n\
             \n\
             #ifndef FAUSTFLOAT\n\
             #define FAUSTFLOAT float\n\
             #endif"
        )
        .unwrap();

        // ── Namespace open ───────────────────────────────────────────────
        if let Some(ns) = &self.options.namespace {
            writeln!(out, "\nnamespace {ns} {{").unwrap();
        }

        // ── Class declaration ────────────────────────────────────────────
        let cls = &self.class_name;
        writeln!(out, "\nclass {cls} final : public dsp {{").unwrap();
        writeln!(out, "private:").unwrap();
        writeln!(out, "\tint fSampleRate;").unwrap();
        if f.int_heap_size > 0 {
            writeln!(out, "\tint iVec[{}];", f.int_heap_size).unwrap();
        }
        if f.real_heap_size > 0 {
            writeln!(out, "\t{} fVec[{}];", self.real_ctype, f.real_heap_size).unwrap();
        }
        writeln!(out, "\npublic:").unwrap();

        // ── Constructor ──────────────────────────────────────────────────
        writeln!(out, "\t{cls}() {{").unwrap();
        writeln!(out, "\t\tfSampleRate = 0;").unwrap();
        if f.int_heap_size > 0 {
            writeln!(out, "\t\tmemset(iVec, 0, sizeof(iVec));").unwrap();
        }
        if f.real_heap_size > 0 {
            writeln!(out, "\t\tmemset(fVec, 0, sizeof(fVec));").unwrap();
        }
        writeln!(out, "\t}}\n").unwrap();

        // ── getNumInputs / getNumOutputs / getSampleRate ─────────────────
        writeln!(
            out,
            "\tint getNumInputs() override {{ return {}; }}",
            f.num_inputs
        )
        .unwrap();
        writeln!(
            out,
            "\tint getNumOutputs() override {{ return {}; }}",
            f.num_outputs
        )
        .unwrap();
        writeln!(
            out,
            "\tint getSampleRate() override {{ return fSampleRate; }}\n"
        )
        .unwrap();

        // ── buildUserInterface ───────────────────────────────────────────
        writeln!(
            out,
            "\tvoid buildUserInterface(UI* ui_interface) override {{"
        )
        .unwrap();
        emit_ui_block(&mut out, &f.ui_block, self.real_ctype, 2);
        writeln!(out, "\t}}\n").unwrap();

        // ── metadata ────────────────────────────────────────────────────
        writeln!(out, "\tvoid metadata(Meta* m) override {{").unwrap();
        emit_meta_block(&mut out, &f.meta_block, 2);
        writeln!(out, "\t}}\n").unwrap();

        // ── classInit ───────────────────────────────────────────────────
        // Static/class-level initialization (sample-rate-independent tables).
        // Not declared virtual in dsp.h, so no 'override'.
        writeln!(out, "\tvoid classInit(int sample_rate) {{").unwrap();
        self.new_block_comp()
            .compile_block(&f.arena, &mut out, 2, f.static_init_block)?;
        writeln!(out, "\t}}\n").unwrap();

        // ── instanceConstants ────────────────────────────────────────────
        writeln!(out, "\tvoid instanceConstants(int sample_rate) override {{").unwrap();
        writeln!(out, "\t\tfSampleRate = sample_rate;").unwrap();
        if f.sr_offset >= 0 && f.sr_offset < f.int_heap_size {
            writeln!(out, "\t\tiVec[{}] = sample_rate;", f.sr_offset).unwrap();
        }
        self.new_block_comp()
            .compile_block(&f.arena, &mut out, 2, f.init_block)?;
        writeln!(out, "\t}}\n").unwrap();

        // ── instanceResetUserInterface ───────────────────────────────────
        writeln!(out, "\tvoid instanceResetUserInterface() override {{").unwrap();
        self.new_block_comp()
            .compile_block(&f.arena, &mut out, 2, f.reset_ui_block)?;
        writeln!(out, "\t}}\n").unwrap();

        // ── instanceClear ────────────────────────────────────────────────
        writeln!(out, "\tvoid instanceClear() override {{").unwrap();
        self.new_block_comp()
            .compile_block(&f.arena, &mut out, 2, f.clear_block)?;
        writeln!(out, "\t}}\n").unwrap();

        // ── instanceInit ─────────────────────────────────────────────────
        // Pure orchestrator — no inline code, matching the generated C++
        // backend: constants, UI reset, then clear. `classInit` belongs to
        // `init`, not `instanceInit`.
        writeln!(out, "\tvoid instanceInit(int sample_rate) override {{").unwrap();
        writeln!(out, "\t\tinstanceConstants(sample_rate);").unwrap();
        writeln!(out, "\t\tinstanceResetUserInterface();").unwrap();
        writeln!(out, "\t\tinstanceClear();").unwrap();
        writeln!(out, "\t}}\n").unwrap();

        // ── init ────────────────────────────────────────────────────────
        writeln!(out, "\tvoid init(int sample_rate) override {{").unwrap();
        writeln!(out, "\t\tclassInit(sample_rate);").unwrap();
        writeln!(out, "\t\tinstanceInit(sample_rate);").unwrap();
        writeln!(out, "\t}}\n").unwrap();

        // ── clone ───────────────────────────────────────────────────────
        writeln!(out, "\tdsp* clone() override {{ return new {cls}(); }}\n").unwrap();

        // ── compute ─────────────────────────────────────────────────────
        writeln!(
            out,
            "\tvoid compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) override {{"
        )
        .unwrap();
        writeln!(out, "\t\tif (count == 0) return;").unwrap();
        if f.count_offset >= 0 && f.count_offset < f.int_heap_size {
            writeln!(out, "\t\tiVec[{}] = count;", f.count_offset).unwrap();
        }
        // Both blocks share one BlockComp so temporaries are unique within compute().
        let mut comp = self.new_block_comp();

        writeln!(out, "\t\t// compute_block (control, runs once per buffer)").unwrap();
        comp.compile_block(&f.arena, &mut out, 2, f.compute_block)?;

        writeln!(out, "\t\t// compute_dsp_block (sample loop)").unwrap();
        comp.compile_block(&f.arena, &mut out, 2, f.compute_dsp_block)?;

        writeln!(out, "\t}}").unwrap();

        // ── Class end ────────────────────────────────────────────────────
        writeln!(out, "\n}};").unwrap();

        // ── Namespace close ──────────────────────────────────────────────
        if let Some(ns) = &self.options.namespace {
            writeln!(out, "\n}} // namespace {ns}").unwrap();
        }

        Ok(out)
    }

    /// Returns a fresh `BlockComp` for this generator's real type.
    fn new_block_comp(&self) -> BlockComp {
        BlockComp::new(self.real_ctype)
    }
}

// ── Internal: block-level compiler ──────────────────────────────────────────

/// Block-level compiler from linear FBC instructions to structured C++ code.
///
/// The compiler simulates the interpreter operand stacks with temporary C++
/// variable names. This keeps code generation close to bytecode semantics while
/// still emitting readable native code.
struct BlockComp {
    real_ctype: &'static str,
    /// Counter for real temporaries (fRN).
    rc: usize,
    /// Counter for int temporaries (iIN).
    ic: usize,
    /// Counter for static inline tables (kTab_N).
    tc: usize,
    /// Virtual real-value stack (C++ variable names).
    rstack: Vec<String>,
    /// Virtual int-value stack (C++ variable names).
    istack: Vec<String>,
}

impl BlockComp {
    /// Creates a fresh block compiler with empty virtual stacks and counters.
    fn new(real_ctype: &'static str) -> Self {
        Self {
            real_ctype,
            rc: 0,
            ic: 0,
            tc: 0,
            rstack: Vec::new(),
            istack: Vec::new(),
        }
    }

    // ── Stack helpers ────────────────────────────────────────────────────────

    /// Declares one REAL temporary, pushes it onto the virtual stack, and returns its name.
    fn push_r(&mut self, out: &mut String, t: usize, expr: &str) -> String {
        let name = format!("fR{}", self.rc);
        self.rc += 1;
        writeln!(out, "{}{} {} = {};", tab(t), self.real_ctype, name, expr).unwrap();
        self.rstack.push(name.clone());
        name
    }

    /// Declares one integer temporary, pushes it onto the virtual stack, and returns its name.
    fn push_i(&mut self, out: &mut String, t: usize, expr: &str) -> String {
        let name = format!("iI{}", self.ic);
        self.ic += 1;
        writeln!(out, "{}int {} = {};", tab(t), name, expr).unwrap();
        self.istack.push(name.clone());
        name
    }

    /// Pops one REAL temporary name, falling back to `0.0` on malformed bytecode.
    fn pop_r(&mut self) -> String {
        self.rstack.pop().unwrap_or_else(|| "0.0".to_owned())
    }

    /// Pops one integer temporary name, falling back to `0` on malformed bytecode.
    fn pop_i(&mut self) -> String {
        self.istack.pop().unwrap_or_else(|| "0".to_owned())
    }

    // ── Block compilation ────────────────────────────────────────────────────

    /// Compiles one linear FBC block into native C++ statements.
    fn compile_block<R: FbcReal>(
        &mut self,
        arena: &FbcBlockArena<R>,
        out: &mut String,
        t: usize,
        block_id: BlockId,
    ) -> Result<(), FbcCppError> {
        let block_len = arena
            .try_get(block_id)
            .map(|b| b.len())
            .ok_or(FbcCppError::InvalidBlockId { block_id })?;

        for pc in 0..block_len {
            // Clone instruction to avoid holding borrow on `arena` across
            // the recursive `compile_instr` call.
            let instr = arena.get(block_id).instructions[pc].clone();
            if instr.opcode == FbcOpcode::Return {
                break; // End of block; no C++ statement needed.
            }
            self.compile_instr(arena, out, t, &instr, block_id, pc)?;
        }
        Ok(())
    }

    // ── Instruction dispatch ─────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    /// Compiles one FBC instruction into its native C++ equivalent.
    fn compile_instr<R: FbcReal>(
        &mut self,
        arena: &FbcBlockArena<R>,
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

            // ════════════════════════════════════════════════════════════
            // Extended unary math (stack)
            // ════════════════════════════════════════════════════════════
            Abs => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::abs({v})"));
            }
            Absf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::fabs({v})"));
            }
            Acosf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::acos({v})"));
            }
            Acoshf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::acosh({v})"));
            }
            Asinf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::asin({v})"));
            }
            Asinhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::asinh({v})"));
            }
            Atanf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::atan({v})"));
            }
            Atanhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::atanh({v})"));
            }
            Ceilf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::ceil({v})"));
            }
            Cosf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::cos({v})"));
            }
            Coshf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::cosh({v})"));
            }
            Expf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::exp({v})"));
            }
            Floorf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::floor({v})"));
            }
            Logf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::log({v})"));
            }
            Log10f => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::log10({v})"));
            }
            Rintf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::rint({v})"));
            }
            Roundf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::round({v})"));
            }
            Sinf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::sin({v})"));
            }
            Sinhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::sinh({v})"));
            }
            Sqrtf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::sqrt({v})"));
            }
            Tanf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::tan({v})"));
            }
            Tanhf => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::tanh({v})"));
            }
            Isnanf => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("std::isnan({v})"));
            }
            Isinff => {
                let v = self.pop_r();
                self.push_i(out, t, &format!("std::isinf({v})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended unary math (heap → stack)
            // ════════════════════════════════════════════════════════════
            AbsHeap => {
                self.push_i(out, t, &format!("std::abs(iVec[{o1}])"));
            }
            AbsfHeap => {
                self.push_r(out, t, &format!("std::fabs(fVec[{o1}])"));
            }
            AcosfHeap => {
                self.push_r(out, t, &format!("std::acos(fVec[{o1}])"));
            }
            AcoshfHeap => {
                self.push_r(out, t, &format!("std::acosh(fVec[{o1}])"));
            }
            AsinfHeap => {
                self.push_r(out, t, &format!("std::asin(fVec[{o1}])"));
            }
            AsinhfHeap => {
                self.push_r(out, t, &format!("std::asinh(fVec[{o1}])"));
            }
            AtanfHeap => {
                self.push_r(out, t, &format!("std::atan(fVec[{o1}])"));
            }
            AtanhfHeap => {
                self.push_r(out, t, &format!("std::atanh(fVec[{o1}])"));
            }
            CeilfHeap => {
                self.push_r(out, t, &format!("std::ceil(fVec[{o1}])"));
            }
            CosfHeap => {
                self.push_r(out, t, &format!("std::cos(fVec[{o1}])"));
            }
            CoshfHeap => {
                self.push_r(out, t, &format!("std::cosh(fVec[{o1}])"));
            }
            ExpfHeap => {
                self.push_r(out, t, &format!("std::exp(fVec[{o1}])"));
            }
            FloorfHeap => {
                self.push_r(out, t, &format!("std::floor(fVec[{o1}])"));
            }
            LogfHeap => {
                self.push_r(out, t, &format!("std::log(fVec[{o1}])"));
            }
            Log10fHeap => {
                self.push_r(out, t, &format!("std::log10(fVec[{o1}])"));
            }
            RintfHeap => {
                self.push_r(out, t, &format!("std::rint(fVec[{o1}])"));
            }
            RoundfHeap => {
                self.push_r(out, t, &format!("std::round(fVec[{o1}])"));
            }
            SinfHeap => {
                self.push_r(out, t, &format!("std::sin(fVec[{o1}])"));
            }
            SinhfHeap => {
                self.push_r(out, t, &format!("std::sinh(fVec[{o1}])"));
            }
            SqrtfHeap => {
                self.push_r(out, t, &format!("std::sqrt(fVec[{o1}])"));
            }
            TanfHeap => {
                self.push_r(out, t, &format!("std::tan(fVec[{o1}])"));
            }
            TanhfHeap => {
                self.push_r(out, t, &format!("std::tanh(fVec[{o1}])"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (stack OP stack → push1)
            // ════════════════════════════════════════════════════════════
            Atan2f => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::atan2({v1}, {v2})"));
            }
            Fmodf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::fmod({v1}, {v2})"));
            }
            Powf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::pow({v1}, {v2})"));
            }
            Max => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("std::max({v1}, {v2})"));
            }
            Maxf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::max({v1}, {v2})"));
            }
            Min => {
                let v1 = self.pop_i();
                let v2 = self.pop_i();
                self.push_i(out, t, &format!("std::min({v1}, {v2})"));
            }
            Minf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::min({v1}, {v2})"));
            }
            Copysignf => {
                let v1 = self.pop_r();
                let v2 = self.pop_r();
                self.push_r(out, t, &format!("std::copysign({v1}, {v2})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (heap OP heap → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fHeap => {
                self.push_r(out, t, &format!("std::atan2(fVec[{o1}], fVec[{o2}])"));
            }
            FmodfHeap => {
                self.push_r(out, t, &format!("std::fmod(fVec[{o1}], fVec[{o2}])"));
            }
            PowfHeap => {
                self.push_r(out, t, &format!("std::pow(fVec[{o1}], fVec[{o2}])"));
            }
            MaxHeap => {
                self.push_i(out, t, &format!("std::max(iVec[{o1}], iVec[{o2}])"));
            }
            MaxfHeap => {
                self.push_r(out, t, &format!("std::max(fVec[{o1}], fVec[{o2}])"));
            }
            MinHeap => {
                self.push_i(out, t, &format!("std::min(iVec[{o1}], iVec[{o2}])"));
            }
            MinfHeap => {
                self.push_r(out, t, &format!("std::min(fVec[{o1}], fVec[{o2}])"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (heap OP stack → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::atan2(fVec[{o1}], {v})"));
            }
            FmodfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::fmod(fVec[{o1}], {v})"));
            }
            PowfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::pow(fVec[{o1}], {v})"));
            }
            MaxStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::max(iVec[{o1}], {v})"));
            }
            MaxfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::max(fVec[{o1}], {v})"));
            }
            MinStack => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::min(iVec[{o1}], {v})"));
            }
            MinfStack => {
                let v = self.pop_r();
                self.push_r(out, t, &format!("std::min(fVec[{o1}], {v})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (value OP stack → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::atan2({v}, {lit})"));
            }
            FmodfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::fmod({v}, {lit})"));
            }
            PowfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::pow({v}, {lit})"));
            }
            MaxStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::max({v}, {iv})"));
            }
            MaxfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::max({v}, {lit})"));
            }
            MinStackValue => {
                let v = self.pop_i();
                self.push_i(out, t, &format!("std::min({v}, {iv})"));
            }
            MinfStackValue => {
                let v = self.pop_r();
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::min({v}, {lit})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math (value OP heap → push1)
            // ════════════════════════════════════════════════════════════
            Atan2fValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::atan2(fVec[{o1}], {lit})"));
            }
            FmodfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::fmod(fVec[{o1}], {lit})"));
            }
            PowfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::pow(fVec[{o1}], {lit})"));
            }
            MaxValue => {
                self.push_i(out, t, &format!("std::max(iVec[{o1}], {iv})"));
            }
            MaxfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::max(fVec[{o1}], {lit})"));
            }
            MinValue => {
                self.push_i(out, t, &format!("std::min(iVec[{o1}], {iv})"));
            }
            MinfValue => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::min(fVec[{o1}], {lit})"));
            }

            // ════════════════════════════════════════════════════════════
            // Extended binary math: value OP heap — non-commutative inverted
            // ════════════════════════════════════════════════════════════
            Atan2fValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::atan2({lit}, fVec[{o1}])"));
            }
            FmodfValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::fmod({lit}, fVec[{o1}])"));
            }
            PowfValueInvert => {
                let lit = fmt_real_lit(rv, self.real_ctype);
                self.push_r(out, t, &format!("std::pow({lit}, fVec[{o1}])"));
            }

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

                writeln!(out, "{}if ({} != 0) {{", tab(t), cond).unwrap();
                self.compile_block(arena, out, t + 1, b1)?;
                self.rstack = saved_r.clone();
                self.istack = saved_i.clone();
                writeln!(out, "{}}} else {{", tab(t)).unwrap();
                self.compile_block(arena, out, t + 1, b2)?;
                self.rstack = saved_r;
                self.istack = saved_i;
                writeln!(out, "{}}}", tab(t)).unwrap();
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
        }

        Ok(())
    }

    // ── Binary op helpers ────────────────────────────────────────────────────

    /// Pop two reals, push `v1 OP v2`.
    fn bin_rr(&mut self, out: &mut String, t: usize, op: &str) {
        let v1 = self.pop_r();
        let v2 = self.pop_r();
        self.push_r(out, t, &format!("{v1} {op} {v2}"));
    }

    /// Pop two ints, push `v1 OP v2`.
    fn bin_ii(&mut self, out: &mut String, t: usize, op: &str) {
        let v1 = self.pop_i();
        let v2 = self.pop_i();
        self.push_i(out, t, &format!("{v1} {op} {v2}"));
    }

    /// Pop two ints, push `(int)(v1 OP v2)` — comparison → int.
    fn cmp_ii(&mut self, out: &mut String, t: usize, op: &str) {
        let v1 = self.pop_i();
        let v2 = self.pop_i();
        self.push_i(out, t, &format!("({v1} {op} {v2})"));
    }

    /// Pop two reals, push `(int)(v1 OP v2)` — comparison → int.
    fn cmp_rr(&mut self, out: &mut String, t: usize, op: &str) {
        let v1 = self.pop_r();
        let v2 = self.pop_r();
        self.push_i(out, t, &format!("({v1} {op} {v2})"));
    }
}

// ── UI block emitter ─────────────────────────────────────────────────────────

/// Emits the checked-in UI callback block as native C++ UI method calls.
fn emit_ui_block<R: FbcReal>(
    out: &mut String,
    ui: &[FbcUiInstruction<R>],
    real_ctype: &str,
    t: usize,
) {
    for instr in ui {
        match instr.opcode {
            FbcOpcode::OpenVerticalBox => {
                writeln!(
                    out,
                    "{}ui_interface->openVerticalBox(\"{}\");",
                    tab(t),
                    escape_str(&instr.label)
                )
                .unwrap();
            }
            FbcOpcode::OpenHorizontalBox => {
                writeln!(
                    out,
                    "{}ui_interface->openHorizontalBox(\"{}\");",
                    tab(t),
                    escape_str(&instr.label)
                )
                .unwrap();
            }
            FbcOpcode::OpenTabBox => {
                writeln!(
                    out,
                    "{}ui_interface->openTabBox(\"{}\");",
                    tab(t),
                    escape_str(&instr.label)
                )
                .unwrap();
            }
            FbcOpcode::CloseBox => {
                writeln!(out, "{}ui_interface->closeBox();", tab(t)).unwrap();
            }
            FbcOpcode::AddButton => {
                writeln!(
                    out,
                    "{}ui_interface->addButton(\"{}\", &fVec[{}]);",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset
                )
                .unwrap();
            }
            FbcOpcode::AddCheckButton => {
                writeln!(
                    out,
                    "{}ui_interface->addCheckButton(\"{}\", &fVec[{}]);",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset
                )
                .unwrap();
            }
            FbcOpcode::AddHorizontalSlider => {
                let (vinit, vmin, vmax, vstep) = (
                    fmt_real_lit(instr.init, real_ctype),
                    fmt_real_lit(instr.min, real_ctype),
                    fmt_real_lit(instr.max, real_ctype),
                    fmt_real_lit(instr.step, real_ctype),
                );
                writeln!(
                    out,
                    "{}ui_interface->addHorizontalSlider(\"{}\", &fVec[{}], ({rt}){vinit}, ({rt}){vmin}, ({rt}){vmax}, ({rt}){vstep});",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset,
                    rt = real_ctype,
                )
                .unwrap();
            }
            FbcOpcode::AddVerticalSlider => {
                let (vinit, vmin, vmax, vstep) = (
                    fmt_real_lit(instr.init, real_ctype),
                    fmt_real_lit(instr.min, real_ctype),
                    fmt_real_lit(instr.max, real_ctype),
                    fmt_real_lit(instr.step, real_ctype),
                );
                writeln!(
                    out,
                    "{}ui_interface->addVerticalSlider(\"{}\", &fVec[{}], ({rt}){vinit}, ({rt}){vmin}, ({rt}){vmax}, ({rt}){vstep});",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset,
                    rt = real_ctype,
                )
                .unwrap();
            }
            FbcOpcode::AddNumEntry => {
                let (vinit, vmin, vmax, vstep) = (
                    fmt_real_lit(instr.init, real_ctype),
                    fmt_real_lit(instr.min, real_ctype),
                    fmt_real_lit(instr.max, real_ctype),
                    fmt_real_lit(instr.step, real_ctype),
                );
                writeln!(
                    out,
                    "{}ui_interface->addNumEntry(\"{}\", &fVec[{}], ({rt}){vinit}, ({rt}){vmin}, ({rt}){vmax}, ({rt}){vstep});",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset,
                    rt = real_ctype,
                )
                .unwrap();
            }
            FbcOpcode::AddHorizontalBargraph => {
                let (vmin, vmax) = (
                    fmt_real_lit(instr.min, real_ctype),
                    fmt_real_lit(instr.max, real_ctype),
                );
                writeln!(
                    out,
                    "{}ui_interface->addHorizontalBargraph(\"{}\", &fVec[{}], ({rt}){vmin}, ({rt}){vmax});",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset,
                    rt = real_ctype,
                )
                .unwrap();
            }
            FbcOpcode::AddVerticalBargraph => {
                let (vmin, vmax) = (
                    fmt_real_lit(instr.min, real_ctype),
                    fmt_real_lit(instr.max, real_ctype),
                );
                writeln!(
                    out,
                    "{}ui_interface->addVerticalBargraph(\"{}\", &fVec[{}], ({rt}){vmin}, ({rt}){vmax});",
                    tab(t),
                    escape_str(&instr.label),
                    instr.offset,
                    rt = real_ctype,
                )
                .unwrap();
            }
            FbcOpcode::AddSoundfile => {
                writeln!(
                    out,
                    "{}// AddSoundfile(\"{}\") — sound-file support not generated.",
                    tab(t),
                    escape_str(&instr.label)
                )
                .unwrap();
            }
            FbcOpcode::Declare => {
                // offset == -1 means "no associated heap slot" (group-level
                // declare); emit nullptr to avoid out-of-bounds array access.
                let ptr = if instr.offset < 0 {
                    "nullptr".to_owned()
                } else {
                    format!("&fVec[{}]", instr.offset)
                };
                writeln!(
                    out,
                    "{}ui_interface->declare({}, \"{}\", \"{}\");",
                    tab(t),
                    ptr,
                    escape_str(&instr.key),
                    escape_str(&instr.value)
                )
                .unwrap();
            }
            _ => {}
        }
    }
}

// ── Meta block emitter ───────────────────────────────────────────────────────

/// Emits the metadata callback block as `Meta::declare` calls.
fn emit_meta_block(out: &mut String, meta: &[FbcMetaInstruction], t: usize) {
    for m in meta {
        writeln!(
            out,
            "{}m->declare(\"{}\", \"{}\");",
            tab(t),
            escape_str(&m.key),
            escape_str(&m.value)
        )
        .unwrap();
    }
}

// ── Utilities ────────────────────────────────────────────────────────────────

/// Returns `"\t".repeat(n)` (tabs for indentation).
fn tab(n: usize) -> String {
    "\t".repeat(n)
}

/// Formats a real-typed literal for C++ with appropriate suffix.
fn fmt_real_lit<R: FbcReal>(val: R, real_ctype: &str) -> String {
    let v64 = val.to_f64();
    if v64.is_nan() {
        if real_ctype == "float" {
            "std::numeric_limits<float>::quiet_NaN()".to_owned()
        } else {
            "std::numeric_limits<double>::quiet_NaN()".to_owned()
        }
    } else if v64.is_infinite() {
        let sign = if v64 > 0.0 { "" } else { "-" };
        if real_ctype == "float" {
            format!("{sign}std::numeric_limits<float>::infinity()")
        } else {
            format!("{sign}std::numeric_limits<double>::infinity()")
        }
    } else if real_ctype == "float" {
        // Use Rust's roundtrip display for f32 (adds enough digits), then
        // ensure a decimal point is present so the compiler never interprets
        // e.g. `0f` as an invalid octal constant or `1f` as an integer suffix.
        let s = format!("{val}");
        if s.contains('.') || s.contains('e') || s.contains('E') {
            format!("{s}f")
        } else {
            format!("{s}.0f")
        }
    } else {
        let s = format!("{val}");
        if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

/// Sanitizes a name into a valid C++ identifier.
///
/// Replaces leading digit with `_N`, and all non-alphanumeric/non-underscore
/// characters with `_`.
fn sanitize_cpp_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            if i == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// Escapes a string for use in C++ string literals.
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
