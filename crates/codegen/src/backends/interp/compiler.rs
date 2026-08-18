//! FIR → FBC bytecode compiler.
//!
//! # Source provenance (C++)
//! - `compiler/generator/interpreter/interpreter_instructions.hh`
//!   (`InterpreterInstVisitor<REAL>`)
//!
//! # Design notes
//! - The C++ visitor/`accept()` pattern is replaced by exhaustive `match`
//!   dispatch on [`FirMatch`] variants obtained from [`match_fir`].
//! - Block switching (save/restore `fCurrentBlock`) is replaced by a
//!   `saved_blocks` stack using [`std::mem::replace`].
//! - The compiler owns the [`FbcBlockArena`]; `finalize` moves it into
//!   the result.
//!
//! # API mapping status
//! - `InterpreterInstVisitor<REAL>` → [`FirToFbcCompiler<R>`]: adapted.
//! - `gMathLibTable` → [`math_lib_lookup`]: const fn match.
//! - `gBinOpTable` → [`binop_to_fbc`]: const fn match.
//! - `fFieldTable` → [`FirToFbcCompiler::field_table`]: `HashMap<String, MemoryDesc>`.

use std::collections::HashMap;
use std::fmt;

use fir::{
    AccessType, BargraphType, ButtonType, FirId, FirMatch, FirStore, FirType, SliderType,
    UiBoxType, match_fir,
};

use super::bytecode::{
    BlockId, BlockStoreData, FbcBlock, FbcBlockArena, FbcInstruction, FbcUiInstruction,
};
use super::foreign::{
    ForeignScalarType, ForeignSignature, is_registered_foreign_function, is_supported_signature,
};
use super::opcode::FbcOpcode;
use super::real::FbcReal;

// ---------------------------------------------------------------------------
// Helper types
// ---------------------------------------------------------------------------

/// Return type of [`FirToFbcCompiler::into_parts`].
///
/// This mirrors the pieces that the surrounding module-level emitter needs to
/// assemble an interpreter factory: finalized blocks, heap sizes, collected UI
/// side effects, and the stable variable-to-heap layout.
pub type CompilerParts<R> = (
    FbcBlockArena<R>,
    i32,
    i32,
    Vec<FbcUiInstruction<R>>,
    HashMap<String, MemoryDesc>,
);

/// Which heap a variable is allocated in.
///
/// The interpreter uses two separate heaps for cache-locality and to avoid
/// type-punning: integer counters/indices live apart from floating-point
/// filter state and delay memory.
///
/// # Source provenance (C++)
/// - `Typed::VarType` (only `kInt32` vs everything-else distinction matters
///   for the interpreter backend).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapType {
    /// Integer heap (`int_heap`): loop counters, indices, control booleans.
    Int,
    /// Real heap (`real_heap`): filter state, delay lines, audio accumulation.
    Real,
}

/// Describes a variable's location in the interpreter's dual heaps.
///
/// # Source provenance (C++)
/// - `MemoryDesc` in `struct_manager.hh` (simplified: only the fields
///   used by `InterpreterInstVisitor`).
#[derive(Clone, Debug)]
pub struct MemoryDesc {
    /// Heap offset (index into `int_heap` or `real_heap`).
    pub offset: i32,
    /// Element count (not byte count): 1 for scalars, >1 for arrays.
    ///
    /// Used only for heap allocation sizing during `DeclareVar` compilation.
    /// Indexed access uses `offset` from the field table directly; there is no
    /// runtime stride calculation.
    pub size: i32,
    /// Whether this variable lives in the int heap or the real heap.
    pub heap_type: HeapType,
}

#[derive(Clone, Copy, Debug)]
struct ForLoopParams<'a> {
    var: &'a str,
    init: FirId,
    end: FirId,
    step: FirId,
    body: FirId,
    is_reverse: bool,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during FIR → FBC compilation.
///
/// # Source provenance (C++)
/// - `faustassert(false)` and `throw faustexception(...)` in
///   `interpreter_instructions.hh`.
#[derive(Clone, Debug)]
pub enum CompileError {
    /// A variable was used but never declared.
    UndeclaredVariable { name: String },
    /// A math function call references an unknown function.
    UnknownMathFunction { name: String },
    /// A foreign function signature cannot be represented by the interpreter.
    UnsupportedForeignFunctionSignature { name: String, description: String },
    /// A FIR node kind is not supported by the interpreter backend.
    UnsupportedNode { description: String },
    /// `LoadVarAddress` is not supported (mirrors `faustassert(false)` in C++).
    LoadVarAddressNotSupported,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndeclaredVariable { name } => {
                write!(f, "undeclared variable: {name}")
            }
            Self::UnknownMathFunction { name } => {
                write!(f, "unknown math function: {name}")
            }
            Self::UnsupportedForeignFunctionSignature { name, description } => {
                write!(
                    f,
                    "unsupported foreign function signature for {name}: {description}"
                )
            }
            Self::UnsupportedNode { description } => {
                write!(f, "unsupported FIR node: {description}")
            }
            Self::LoadVarAddressNotSupported => {
                write!(
                    f,
                    "LoadVarAddress is not supported by the interpreter backend"
                )
            }
        }
    }
}

impl std::error::Error for CompileError {}

// ---------------------------------------------------------------------------
// Compilation result
// ---------------------------------------------------------------------------

/// Result of a successful FIR → FBC compilation.
///
/// The interpreter backend has two usage modes:
/// - [`FirToFbcCompiler::finalize`] for a single entry block.
/// - [`FirToFbcCompiler::into_parts`] when the caller compiles several named FIR
///   sections into separate arena blocks and assembles the final factory
///   metadata outside this file.
///
/// This owned bundle is the single-entry variant returned by
/// [`FirToFbcCompiler::finalize`].
pub struct FbcCompileResult<R: FbcReal> {
    /// The block arena containing all compiled blocks.
    pub arena: FbcBlockArena<R>,
    /// The top-level block (entry point).
    pub entry_block: BlockId,
    /// Total int heap slots allocated.
    pub int_heap_size: i32,
    /// Total real heap slots allocated.
    pub real_heap_size: i32,
    /// Variable-to-heap-slot mapping.
    pub field_table: HashMap<String, MemoryDesc>,
    /// Collected UI instructions.
    pub ui_instructions: Vec<FbcUiInstruction<R>>,
}

// ---------------------------------------------------------------------------
// Compiler struct
// ---------------------------------------------------------------------------

/// FIR → FBC bytecode compiler.
///
/// # Source provenance (C++)
/// - `InterpreterInstVisitor<REAL>` in `interpreter_instructions.hh`
///
/// Translates FIR nodes (stored in a [`FirStore`]) into FBC bytecode
/// blocks stored in an [`FbcBlockArena`].
///
/// The compiler owns temporary block-switching state and the growing dual-heap
/// layout while lowering one or more FIR functions into bytecode.
pub struct FirToFbcCompiler<R: FbcReal> {
    /// Block arena — all compiled blocks live here.
    arena: FbcBlockArena<R>,
    /// Block currently being compiled into.
    current_block: FbcBlock<R>,
    /// Stack of saved parent blocks for the block-switching pattern.
    saved_blocks: Vec<FbcBlock<R>>,
    /// Current allocation pointer in the real heap.
    real_heap_offset: i32,
    /// Current allocation pointer in the int heap.
    int_heap_offset: i32,
    /// Maps variable names to their heap locations.
    field_table: HashMap<String, MemoryDesc>,
    /// UI instructions collected during compilation.
    ui_instructions: Vec<FbcUiInstruction<R>>,
    /// Maps soundfile variable names to their executor slot indices.
    soundfile_slots: HashMap<String, usize>,
    /// Number of soundfile slots allocated so far.
    num_soundfile_slots: usize,
}

impl<R: FbcReal> Default for FirToFbcCompiler<R> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Static lookup tables (free functions)
// ---------------------------------------------------------------------------

/// Maps a FIR binary operation to the corresponding FBC opcodes.
///
/// # Source provenance (C++)
/// - `gBinOpTable[opcode]->fInterpIntInst32` / `fInterpFloatInst`.
///
/// Returns `(int_opcode, real_opcode)`.
#[must_use]
pub const fn binop_to_fbc(op: fir::FirBinOp) -> (FbcOpcode, FbcOpcode) {
    use fir::FirBinOp;
    match op {
        FirBinOp::Add => (FbcOpcode::AddInt, FbcOpcode::AddReal),
        FirBinOp::Sub => (FbcOpcode::SubInt, FbcOpcode::SubReal),
        FirBinOp::Mul => (FbcOpcode::MultInt, FbcOpcode::MultReal),
        FirBinOp::Div => (FbcOpcode::DivInt, FbcOpcode::DivReal),
        FirBinOp::Rem => (FbcOpcode::RemInt, FbcOpcode::RemReal),
        FirBinOp::And => (FbcOpcode::ANDInt, FbcOpcode::ANDInt),
        FirBinOp::Or => (FbcOpcode::ORInt, FbcOpcode::ORInt),
        FirBinOp::Xor => (FbcOpcode::XORInt, FbcOpcode::XORInt),
        FirBinOp::Lsh => (FbcOpcode::LshInt, FbcOpcode::LshInt),
        FirBinOp::ARsh => (FbcOpcode::ARshInt, FbcOpcode::ARshInt),
        FirBinOp::LRsh => (FbcOpcode::LRshInt, FbcOpcode::LRshInt),
        FirBinOp::Eq => (FbcOpcode::EQInt, FbcOpcode::EQReal),
        FirBinOp::Ne => (FbcOpcode::NEInt, FbcOpcode::NEReal),
        FirBinOp::Lt => (FbcOpcode::LTInt, FbcOpcode::LTReal),
        FirBinOp::Le => (FbcOpcode::LEInt, FbcOpcode::LEReal),
        FirBinOp::Gt => (FbcOpcode::GTInt, FbcOpcode::GTReal),
        FirBinOp::Ge => (FbcOpcode::GEInt, FbcOpcode::GEReal),
    }
}

/// Maps a math function name to its FBC opcode.
///
/// # Source provenance (C++)
/// - `InterpreterInstVisitor::initMathTable()` in `interpreter_instructions.hh`.
///
/// Handles both float-suffix (`sinf`) and double (bare `sin`) forms.
///
/// Note on `min`/`max` aliases:
/// - `fmin`/`fmax` (and `fminf`/`fmaxf`) are the standard C math spellings and
///   are the primary names used by the current FIR fast-lane/tests.
/// - `min_f`/`min_` and `max_f`/`max_` are kept for compatibility with older
///   or alternate FIR producers. They appear to be legacy aliases and may be
///   removable after a dedicated compatibility audit.
#[must_use]
pub fn math_lib_lookup(name: &str) -> Option<FbcOpcode> {
    match name {
        // Integer
        "abs" => Some(FbcOpcode::Abs),
        "min_i" => Some(FbcOpcode::Min),
        "max_i" => Some(FbcOpcode::Max),
        // Float and double
        "fabsf" | "fabs" => Some(FbcOpcode::Absf),
        "acosf" | "acos" => Some(FbcOpcode::Acosf),
        "asinf" | "asin" => Some(FbcOpcode::Asinf),
        "atanf" | "atan" => Some(FbcOpcode::Atanf),
        "atan2f" | "atan2" => Some(FbcOpcode::Atan2f),
        "ceilf" | "ceil" => Some(FbcOpcode::Ceilf),
        "cosf" | "cos" => Some(FbcOpcode::Cosf),
        "expf" | "exp" => Some(FbcOpcode::Expf),
        "floorf" | "floor" => Some(FbcOpcode::Floorf),
        "fmodf" | "fmod" => Some(FbcOpcode::Fmodf),
        "logf" | "log" => Some(FbcOpcode::Logf),
        "log10f" | "log10" => Some(FbcOpcode::Log10f),
        // Legacy aliases (`min_f`/`min_`, `max_f`/`max_`) are preserved for
        // compatibility; prefer standard C names `fmin`/`fmax`.
        "min_f" | "min_" | "fminf" | "fmin" => Some(FbcOpcode::Minf),
        "max_f" | "max_" | "fmaxf" | "fmax" => Some(FbcOpcode::Maxf),
        "powf" | "pow" => Some(FbcOpcode::Powf),
        "remainderf" | "remainder" => Some(FbcOpcode::RemReal),
        "rintf" | "rint" => Some(FbcOpcode::Rintf),
        "roundf" | "round" => Some(FbcOpcode::Roundf),
        "sinf" | "sin" => Some(FbcOpcode::Sinf),
        "sqrtf" | "sqrt" => Some(FbcOpcode::Sqrtf),
        "tanf" | "tan" => Some(FbcOpcode::Tanf),
        // Hyperbolic
        "acoshf" | "acosh" => Some(FbcOpcode::Acoshf),
        "asinhf" | "asinh" => Some(FbcOpcode::Asinhf),
        "atanhf" | "atanh" => Some(FbcOpcode::Atanhf),
        "coshf" | "cosh" => Some(FbcOpcode::Coshf),
        "sinhf" | "sinh" => Some(FbcOpcode::Sinhf),
        "tanhf" | "tanh" => Some(FbcOpcode::Tanhf),
        // Special
        "isnanf" | "isnan" => Some(FbcOpcode::Isnanf),
        "isinff" | "isinf" => Some(FbcOpcode::Isinff),
        "copysignf" | "copysign" => Some(FbcOpcode::Copysignf),
        _ => None,
    }
}

/// Extracts the channel number from `"input0"`, `"output1"`, etc.
fn parse_io_channel(name: &str, prefix: &str) -> Option<i32> {
    name.strip_prefix(prefix)
        .and_then(|suffix| suffix.parse::<i32>().ok())
}

/// Returns `true` if the FIR type maps to the int heap.
fn is_int_type(typ: &FirType) -> bool {
    matches!(typ, FirType::Int32 | FirType::Int64 | FirType::Bool)
}

// ===========================================================================
// Tests
// ===========================================================================

mod blocks;
mod control;
mod expressions;
mod lifecycle;
mod storage;
mod ui_soundfile;

#[cfg(test)]
mod tests;
