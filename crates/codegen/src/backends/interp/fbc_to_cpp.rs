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
//! | `If(b1, b2)` | conditional statement; `Return`-only branches are omitted |
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
//! # Control-flow and stack invariant
//!
//! FBC branch blocks are statement blocks for `If` and value-producing blocks
//! for `SelectReal`/`SelectInt`. The compiler snapshots the virtual real and
//! integer stacks before each branch and restores them at the join; only a
//! select's explicit merge temporary is allowed to cross that boundary. A
//! `Return` terminates an FBC block rather than the generated C++ method, so a
//! return-only `If` branch is intentionally not emitted. If only the false
//! branch has statements, the emitter inverts the condition instead of
//! producing an empty true branch.
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
        /// Opcode that requires the missing target.
        opcode: FbcOpcode,
        /// Containing block of the malformed instruction.
        block_id: BlockId,
        /// Program counter within `block_id`.
        pc: usize,
    },
    /// A `BlockId` referenced in the bytecode is out of range for the arena.
    InvalidBlockId {
        /// Invalid arena index.
        block_id: BlockId,
    },
    /// An opcode is not translatable in code-generation mode.
    ///
    /// Currently only `LoadSoundFieldInt` / `LoadSoundFieldReal` fall here,
    /// as sound-file support requires an external runtime object.
    Unsupported {
        /// Opcode with no self-contained C++ lowering.
        opcode: FbcOpcode,
        /// Containing block of the unsupported instruction.
        block_id: BlockId,
        /// Program counter within `block_id`.
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
    // ── Instruction dispatch ─────────────────────────────────────────────────

    /// Compiles one FBC instruction into its native C++ equivalent.
    ///
    /// This routes each opcode to one instruction-family module; the arms live
    /// in `fbc_to_cpp/{memory_io,casts,math_std_stack,math_std_value,math_ext,
    /// control}.rs`.
    ///
    /// The partition is checked by the compiler rather than by review: an
    /// opcode missing from every family leaves this match non-exhaustive, and
    /// an opcode claimed by two families makes an arm unreachable. Each family
    /// takes only the parameters it uses, so a signature says what that family
    /// cannot reach.
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

        match instr.opcode {
            Nop | ForeignCallReal | ForeignCallInt | ForeignCallVoid | RealValue | Int32Value
            | LoadReal | LoadInt | StoreReal | StoreInt | StoreRealValue | StoreIntValue
            | LoadIndexedReal | LoadIndexedInt | StoreIndexedReal | StoreIndexedInt
            | BlockStoreReal | BlockStoreInt | MoveReal | MoveInt | PairMoveReal | PairMoveInt
            | BlockPairMoveReal | BlockPairMoveInt | BlockShiftReal | BlockShiftInt | LoadInput
            | LoadOutput | StoreOutput | LoadSoundFieldInt | LoadSoundFieldReal => {
                self.compile_memory_io(out, t, instr, block_id, pc)
            }
            CastReal | CastInt | CastRealHeap | CastIntHeap | BitcastInt | BitcastReal => {
                self.compile_casts(out, t, instr)
            }
            AddReal | SubReal | MultReal | DivReal | RemReal | AddInt | SubInt | MultInt
            | DivInt | RemInt | LshInt | ARshInt | LRshInt | GTInt | LTInt | GEInt | LEInt
            | EQInt | NEInt | GTReal | LTReal | GEReal | LEReal | EQReal | NEReal | ANDInt
            | ORInt | XORInt | AddRealHeap | SubRealHeap | MultRealHeap | DivRealHeap
            | RemRealHeap | AddIntHeap | SubIntHeap | MultIntHeap | DivIntHeap | RemIntHeap
            | LshIntHeap | ARshIntHeap | LRshIntHeap | GTIntHeap | LTIntHeap | GEIntHeap
            | LEIntHeap | EQIntHeap | NEIntHeap | GTRealHeap | LTRealHeap | GERealHeap
            | LERealHeap | EQRealHeap | NERealHeap | ANDIntHeap | ORIntHeap | XORIntHeap
            | AddRealStack | SubRealStack | MultRealStack | DivRealStack | RemRealStack
            | AddIntStack | SubIntStack | MultIntStack | DivIntStack | RemIntStack
            | LshIntStack | ARshIntStack | LRshIntStack | GTIntStack | LTIntStack | GEIntStack
            | LEIntStack | EQIntStack | NEIntStack | GTRealStack | LTRealStack | GERealStack
            | LERealStack | EQRealStack | NERealStack | ANDIntStack | ORIntStack | XORIntStack => {
                self.compile_math_std_stack(out, t, instr)
            }
            AddRealStackValue | SubRealStackValue | MultRealStackValue | DivRealStackValue
            | RemRealStackValue | AddIntStackValue | SubIntStackValue | MultIntStackValue
            | DivIntStackValue | RemIntStackValue | LshIntStackValue | ARshIntStackValue
            | LRshIntStackValue | GTIntStackValue | LTIntStackValue | GEIntStackValue
            | LEIntStackValue | EQIntStackValue | NEIntStackValue | GTRealStackValue
            | LTRealStackValue | GERealStackValue | LERealStackValue | EQRealStackValue
            | NERealStackValue | ANDIntStackValue | ORIntStackValue | XORIntStackValue
            | AddRealValue | SubRealValue | MultRealValue | DivRealValue | RemRealValue
            | AddIntValue | SubIntValue | MultIntValue | DivIntValue | RemIntValue
            | LshIntValue | ARshIntValue | LRshIntValue | GTIntValue | LTIntValue | GEIntValue
            | LEIntValue | EQIntValue | NEIntValue | GTRealValue | LTRealValue | GERealValue
            | LERealValue | EQRealValue | NERealValue | ANDIntValue | ORIntValue | XORIntValue
            | SubRealValueInvert | SubIntValueInvert | DivRealValueInvert | DivIntValueInvert
            | RemRealValueInvert | RemIntValueInvert | LshIntValueInvert | ARshIntValueInvert
            | LRshIntValueInvert | GTIntValueInvert | LTIntValueInvert | GEIntValueInvert
            | LEIntValueInvert | GTRealValueInvert | LTRealValueInvert | GERealValueInvert
            | LERealValueInvert => self.compile_math_std_value(out, t, instr),
            Abs | Absf | Acosf | Acoshf | Asinf | Asinhf | Atanf | Atanhf | Ceilf | Cosf
            | Coshf | Expf | Floorf | Logf | Log10f | Rintf | Roundf | Sinf | Sinhf | Sqrtf
            | Tanf | Tanhf | Isnanf | Isinff | AbsHeap | AbsfHeap | AcosfHeap | AcoshfHeap
            | AsinfHeap | AsinhfHeap | AtanfHeap | AtanhfHeap | CeilfHeap | CosfHeap
            | CoshfHeap | ExpfHeap | FloorfHeap | LogfHeap | Log10fHeap | RintfHeap
            | RoundfHeap | SinfHeap | SinhfHeap | SqrtfHeap | TanfHeap | TanhfHeap | Atan2f
            | Fmodf | Powf | Max | Maxf | Min | Minf | Copysignf | Atan2fHeap | FmodfHeap
            | PowfHeap | MaxHeap | MaxfHeap | MinHeap | MinfHeap | Atan2fStack | FmodfStack
            | PowfStack | MaxStack | MaxfStack | MinStack | MinfStack | Atan2fStackValue
            | FmodfStackValue | PowfStackValue | MaxStackValue | MaxfStackValue | MinStackValue
            | MinfStackValue | Atan2fValue | FmodfValue | PowfValue | MaxValue | MaxfValue
            | MinValue | MinfValue | Atan2fValueInvert | FmodfValueInvert | PowfValueInvert => {
                self.compile_math_ext(out, t, instr)
            }
            Loop
            | CondBranch
            | If
            | SelectReal
            | SelectInt
            | Return
            | OpenVerticalBox
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
            | Declare => self.compile_control(arena, out, t, instr, block_id, pc),
        }
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

mod casts;
mod control;
mod math_ext;
mod math_std_stack;
mod math_std_value;
mod memory_io;

#[cfg(test)]
mod tests;
