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

use crate::backends::purity::is_obviously_side_effect_free_value;

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

impl<R: FbcReal> FirToFbcCompiler<R> {
    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Creates a new compiler with empty state.
    ///
    /// # Source provenance (C++)
    /// - `InterpreterInstVisitor()` constructor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena: FbcBlockArena::new(),
            current_block: FbcBlock::new(),
            saved_blocks: Vec::new(),
            real_heap_offset: 0,
            int_heap_offset: 0,
            field_table: HashMap::new(),
            ui_instructions: Vec::new(),
            soundfile_slots: HashMap::new(),
            num_soundfile_slots: 0,
        }
    }

    /// Returns the current real heap offset.
    #[must_use]
    pub fn real_heap_offset(&self) -> i32 {
        self.real_heap_offset
    }

    /// Returns the current int heap offset.
    #[must_use]
    pub fn int_heap_offset(&self) -> i32 {
        self.int_heap_offset
    }

    /// Returns whether the last emitted instruction leaves a real-valued
    /// result on the evaluation stack.
    fn current_block_top_is_real(&self) -> bool {
        self.current_block
            .instructions
            .last()
            .is_some_and(|instr| instr.opcode.is_real_type())
    }

    /// Returns a reference to the field table.
    #[must_use]
    pub fn field_table(&self) -> &HashMap<String, MemoryDesc> {
        &self.field_table
    }

    /// Compiles a single FIR node (and its transitive children) into
    /// FBC bytecode in the current block.
    ///
    /// Dispatch is intentionally exhaustive over [`FirMatch`]: unlike the C++
    /// visitor hierarchy, unsupported nodes are surfaced as typed Rust errors
    /// instead of falling through to `faustassert(false)`.
    pub fn compile_node(&mut self, store: &FirStore, id: FirId) -> Result<(), CompileError> {
        match match_fir(store, id) {
            // --- Values ---
            FirMatch::Int32 { value, .. } => self.compile_int32(value),
            FirMatch::Float32 { value, .. } => self.compile_float32(value),
            FirMatch::Float64 { value, .. } => self.compile_float64(value),
            FirMatch::Bool { value, .. } => self.compile_bool(value),

            // --- Variables ---
            FirMatch::LoadVar {
                ref name,
                access,
                ref typ,
            } => self.compile_load_var(store, name, access, typ),
            FirMatch::LoadTable {
                ref name,
                access,
                index,
                ref typ,
            } => self.compile_load_table(store, name, access, index, typ),
            FirMatch::LoadVarAddress { .. } => Err(CompileError::LoadVarAddressNotSupported),
            FirMatch::TeeVar {
                ref name,
                access,
                value,
                ref typ,
            } => self.compile_tee_var(store, name, access, value, typ),

            // --- Declarations ---
            FirMatch::DeclareVar {
                ref name,
                ref typ,
                access,
                init,
            } => self.compile_declare_var(store, name, typ, access, init),
            FirMatch::DeclareFun { .. } => Ok(()),
            FirMatch::DeclareStructType { .. } => Ok(()),

            // --- Storage ---
            FirMatch::StoreVar {
                ref name,
                access,
                value,
            } => self.compile_store_var(store, name, access, value),
            FirMatch::StoreTable {
                ref name,
                access,
                index,
                value,
            } => self.compile_store_table(store, name, access, index, value),
            FirMatch::ShiftArrayVar {
                ref name, delay, ..
            } => self.compile_shift_array(name, delay),
            FirMatch::Drop(inner) if is_obviously_side_effect_free_value(store, inner) => Ok(()),
            FirMatch::Drop(inner) => self.compile_node(store, inner),

            // --- Arithmetic ---
            FirMatch::BinOp { op, lhs, rhs, .. } => self.compile_binop(store, op, lhs, rhs),
            FirMatch::Neg { value, ref typ } => self.compile_neg(store, value, typ),

            // --- Cast ---
            FirMatch::Cast { ref typ, value } => self.compile_cast(store, typ, value),
            FirMatch::Bitcast { ref typ, value } => self.compile_bitcast(store, typ, value),

            // --- Control flow ---
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => self.compile_select2(store, cond, then_value, else_value),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => self.compile_if(store, cond, then_block, else_block),
            FirMatch::Switch {
                cond,
                ref cases,
                default,
            } => self.compile_switch(store, cond, cases, default),
            FirMatch::ForLoop {
                ref var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => self.compile_for_loop(
                store,
                ForLoopParams {
                    var,
                    init,
                    end,
                    step,
                    body,
                    is_reverse,
                },
            ),
            FirMatch::SimpleForLoop {
                ref var,
                upper,
                body,
                is_reverse,
                ..
            } => self.compile_simple_for_loop(store, var, upper, body, is_reverse),
            FirMatch::Block(ref stmts) => {
                let stmts = stmts.clone();
                self.compile_block(store, &stmts)
            }

            // --- Function calls ---
            FirMatch::FunCall {
                ref name,
                ref args,
                ref typ,
            } => {
                let name = name.clone();
                let args = args.clone();
                let typ = typ.clone();
                self.compile_fun_call(store, &name, &args, &typ)
            }

            // --- UI ---
            FirMatch::OpenBox { ref typ, ref label } => self.compile_open_box(typ, label),
            FirMatch::CloseBox => self.compile_close_box(),
            FirMatch::AddButton {
                ref typ,
                ref label,
                ref var,
            } => self.compile_add_button(typ, label, var),
            FirMatch::AddSlider {
                ref typ,
                ref label,
                ref var,
                init,
                lo,
                hi,
                step,
            } => self.compile_add_slider(typ, label, var, init, lo, hi, step),
            FirMatch::AddBargraph {
                ref typ,
                ref label,
                ref var,
                lo,
                hi,
            } => self.compile_add_bargraph(typ, label, var, lo, hi),
            FirMatch::AddSoundfile {
                ref label,
                ref url,
                ref var,
            } => self.compile_add_soundfile(label, url, var),

            // --- Soundfile access ---
            FirMatch::LoadSoundfileLength { ref var, part } => {
                let var = var.clone();
                self.compile_load_soundfile_length(store, &var, part)
            }
            FirMatch::LoadSoundfileRate { ref var, part } => {
                let var = var.clone();
                self.compile_load_soundfile_rate(store, &var, part)
            }
            FirMatch::LoadSoundfileBuffer {
                ref var,
                chan,
                part,
                idx,
                ..
            } => {
                let var = var.clone();
                self.compile_load_soundfile_buffer(store, &var, chan, part, idx)
            }

            FirMatch::AddMetaDeclare {
                ref var,
                ref key,
                ref value,
            } => self.compile_add_meta_declare(var, key, value),

            // --- No-ops ---
            FirMatch::NullStatement
            | FirMatch::Label(_)
            | FirMatch::Return(_)
            | FirMatch::Int32Array { .. }
            | FirMatch::Float32Array { .. }
            | FirMatch::Float64Array { .. } => Ok(()),

            // --- Unsupported ---
            other => Err(CompileError::UnsupportedNode {
                description: format!("{other:?}"),
            }),
        }
    }

    /// Finalizes compilation: seals the current block with `kReturn`,
    /// allocates it in the arena, and returns the result.
    ///
    /// Use this entrypoint only when the whole FIR program is intentionally
    /// compiled into one FBC block. Module-oriented interpreter generation uses
    /// [`Self::compile_fir_block`] and [`Self::into_parts`] instead.
    pub fn finalize(mut self) -> Result<FbcCompileResult<R>, CompileError> {
        self.current_block
            .push(FbcInstruction::new(FbcOpcode::Return));
        let entry_block = self.arena.alloc(self.current_block);
        Ok(FbcCompileResult {
            arena: self.arena,
            entry_block,
            int_heap_size: self.int_heap_offset,
            real_heap_size: self.real_heap_offset,
            field_table: self.field_table,
            ui_instructions: self.ui_instructions,
        })
    }

    /// Compiles a FIR block node as a new sub-block in the arena and returns
    /// its allocated [`BlockId`].
    ///
    /// If `block_id` does not decode as a [`FirMatch::Block`], an empty block
    /// (containing only `kReturn`) is emitted.
    ///
    /// This is the building block for [`super::generate_interp_module`] which compiles
    /// each named DSP section (init, compute, …) into a separate arena block.
    pub fn compile_fir_block(
        &mut self,
        store: &FirStore,
        block_id: FirId,
    ) -> Result<BlockId, CompileError> {
        let nodes = match match_fir(store, block_id) {
            FirMatch::Block(ids) => ids,
            _ => vec![],
        };
        self.begin_sub_block();
        for id in &nodes {
            self.compile_node(store, *id)?;
        }
        Ok(self.end_sub_block())
    }

    /// Compiles a list of FIR statements as a new sub-block in the arena.
    ///
    /// This is used by the interpreter backend to split a single FIR `compute`
    /// body into a control prefix block and a DSP loop block without inventing
    /// extra FIR declarations.
    pub fn compile_fir_stmt_list_block(
        &mut self,
        store: &FirStore,
        stmts: &[FirId],
    ) -> Result<BlockId, CompileError> {
        self.begin_sub_block();
        for &id in stmts {
            self.compile_node(store, id)?;
        }
        Ok(self.end_sub_block())
    }

    /// Pre-declares storage nodes from a FIR `Block` into the heap layout.
    ///
    /// This allocates entries in [`Self::field_table`] for top-level module
    /// storage (`dsp_struct`, `globals`) without emitting executable bytecode.
    /// The interpreter backend uses this to make struct/global fields visible
    /// before compiling function bodies that reference them.
    ///
    /// Only direct `DeclareVar` / `DeclareTable` items are accepted; other
    /// nodes are ignored so prototype-only `DeclareFun` entries in `globals`
    /// can coexist with storage declarations.
    pub fn predeclare_storage_block(
        &mut self,
        store: &FirStore,
        block_id: FirId,
    ) -> Result<(), CompileError> {
        let nodes = match match_fir(store, block_id) {
            FirMatch::Block(ids) => ids,
            _ => return Ok(()),
        };
        for id in nodes {
            match match_fir(store, id) {
                FirMatch::DeclareVar {
                    ref name, ref typ, ..
                } => self.predeclare_var_storage(name, typ),
                FirMatch::DeclareTable {
                    ref name,
                    ref elem_type,
                    ref values,
                    ..
                } => self.predeclare_table_storage(name, elem_type, values.len() as i32),
                FirMatch::DeclareFun { .. } | FirMatch::DeclareStructType { .. } => {}
                _ => {}
            }
        }
        Ok(())
    }

    /// Compiles bulk-initialization bytecode for file-scope `const static`
    /// tables declared in the `static_decls` FIR module block.
    ///
    /// In the C/C++ backends these tables are emitted as file-scope arrays with
    /// inline initialisers (`const static float fTbl[N] = {…}`) and require no
    /// runtime initialization. In the interpreter every value lives on the int
    /// or real heap, so the constant data must be written there before
    /// `compute()` runs.
    ///
    /// This method walks the `static_decls` block, allocates heap storage for
    /// each `DeclareTable` (idempotent — `predeclare_storage_block` may have
    /// already done it), and emits one `BlockStoreInt` or `BlockStoreReal`
    /// instruction per table that bulk-copies the constant element values.
    ///
    /// The returned block should be prepended to (or used as) the
    /// `staticInit` factory block so the data is in place before the first
    /// call to `compute()`.
    pub fn compile_static_decls_init_block(
        &mut self,
        store: &FirStore,
        block_id: FirId,
    ) -> Result<BlockId, CompileError> {
        let nodes = match match_fir(store, block_id) {
            FirMatch::Block(ids) => ids,
            _ => return Ok(self.alloc_empty_block()),
        };
        self.begin_sub_block();
        for id in nodes {
            if let FirMatch::DeclareTable {
                ref name,
                ref elem_type,
                ref values,
                ..
            } = match_fir(store, id)
            {
                if values.is_empty() {
                    continue;
                }
                // Ensure the table is registered in the heap layout.
                self.predeclare_table_storage(name, elem_type, values.len() as i32);
                let desc = self.field_table[name.as_str()].clone();
                if desc.heap_type == HeapType::Int {
                    let data: Vec<i32> = values
                        .iter()
                        .filter_map(|&v| {
                            if let FirMatch::Int32 { value, .. } = match_fir(store, v) {
                                Some(value)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if data.len() == values.len() {
                        let len = data.len() as i32;
                        self.current_block.push_block_store(
                            FbcInstruction::with_values_and_offsets(
                                FbcOpcode::BlockStoreInt,
                                0,
                                R::default(),
                                desc.offset,
                                len,
                            ),
                            BlockStoreData::Int(data),
                        );
                    }
                } else {
                    let data: Vec<R> = values
                        .iter()
                        .filter_map(|&v| match match_fir(store, v) {
                            FirMatch::Float32 { value, .. } => Some(R::from_f64(f64::from(value))),
                            FirMatch::Float64 { value, .. } => Some(R::from_f64(value)),
                            _ => None,
                        })
                        .collect();
                    if data.len() == values.len() {
                        let len = data.len() as i32;
                        self.current_block.push_block_store(
                            FbcInstruction::with_values_and_offsets(
                                FbcOpcode::BlockStoreReal,
                                0,
                                R::default(),
                                desc.offset,
                                len,
                            ),
                            BlockStoreData::Real(data),
                        );
                    }
                }
            }
        }
        Ok(self.end_sub_block())
    }

    /// Allocates an empty block (containing only `kReturn`) in the arena.
    ///
    /// Used by [`super::generate_interp_module`] to fill factory slots for DSP
    /// sections that are not present in the FIR module (e.g. `staticInit`
    /// when the legacy bridge is in use).
    pub fn alloc_empty_block(&mut self) -> BlockId {
        self.begin_sub_block();
        self.end_sub_block()
    }

    /// Destructs the compiler into its arena, heap sizes, UI instructions,
    /// and field table without sealing the outermost block.
    ///
    /// Call this after all function bodies have been compiled via
    /// [`Self::compile_fir_block`]. The outermost (current) block is expected to
    /// be empty at that point and is discarded on purpose: the section entry
    /// points live in the returned arena, not in `current_block`.
    pub fn into_parts(self) -> CompilerParts<R> {
        (
            self.arena,
            self.int_heap_offset,
            self.real_heap_offset,
            self.ui_instructions,
            self.field_table,
        )
    }

    // -----------------------------------------------------------------------
    // Block switching
    // -----------------------------------------------------------------------

    /// Saves the current block and starts building a new empty block.
    ///
    /// # Source provenance (C++)
    /// - The pattern `FBCBlockInstruction<REAL>* current = fCurrentBlock;
    ///   fCurrentBlock = new FBCBlockInstruction<REAL>();` in control-flow
    ///   visitors.
    fn begin_sub_block(&mut self) {
        let current = std::mem::take(&mut self.current_block);
        self.saved_blocks.push(current);
    }

    /// Seals the current block with `kReturn`, allocates it in the arena,
    /// and restores the previously saved block.
    ///
    /// Returns the [`BlockId`] of the newly allocated block.
    fn end_sub_block(&mut self) -> BlockId {
        self.current_block
            .push(FbcInstruction::new(FbcOpcode::Return));
        let finished = std::mem::replace(
            &mut self.current_block,
            self.saved_blocks
                .pop()
                .expect("unbalanced begin/end_sub_block"),
        );
        self.arena.alloc(finished)
    }

    // -----------------------------------------------------------------------
    // Values
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(Int32NumInst*)` — pushes integer onto int stack.
    fn compile_int32(&mut self, value: i32) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            value,
            R::default(),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(FloatNumInst*)` — pushes real onto real stack.
    fn compile_float32(&mut self, value: f32) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            R::from_f64(f64::from(value)),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(DoubleNumInst*)` — pushes real onto real stack.
    fn compile_float64(&mut self, value: f64) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            R::from_f64(value),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(BoolNumInst*)` — pushes 0 or 1 onto int stack.
    fn compile_bool(&mut self, value: bool) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            i32::from(value),
            R::default(),
        ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Variable access
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(LoadVarInst*)` — named address path.
    fn compile_load_var(
        &mut self,
        _store: &FirStore,
        name: &str,
        access: AccessType,
        _typ: &FirType,
    ) -> Result<(), CompileError> {
        if access == AccessType::FunArgs
            && name == "sample_rate"
            && !self.field_table.contains_key(name)
        {
            let desc = self
                .field_table
                .get("fSampleRate")
                .or_else(|| self.field_table.get("fSamplingFreq"))
                .or_else(|| self.field_table.get("fSamplingRate"))
                .cloned()
                .unwrap_or_else(|| {
                    let offset = self.int_heap_offset;
                    self.int_heap_offset += 1;
                    MemoryDesc {
                        offset,
                        size: 1,
                        heap_type: HeapType::Int,
                    }
                });
            self.field_table.insert(name.to_string(), desc);
        }
        if access == AccessType::FunArgs && name == "count" && !self.field_table.contains_key(name)
        {
            // Reserve a stable int heap slot for the runtime-set `count` pseudo argument.
            let offset = self.int_heap_offset;
            self.int_heap_offset += 1;
            self.field_table.insert(
                name.to_string(),
                MemoryDesc {
                    offset,
                    size: 1,
                    heap_type: HeapType::Int,
                },
            );
        }
        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::LoadInt
        } else {
            FbcOpcode::LoadReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(LoadVarInst*)` — indexed address path.
    fn compile_load_table(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        index: FirId,
        _typ: &FirType,
    ) -> Result<(), CompileError> {
        // Compile the index expression first (pushes onto int stack).
        self.compile_node(store, index)?;

        // Special handling for input channels.
        if let Some(channel) = parse_io_channel(name, "input") {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInput,
                    0,
                    R::default(),
                    channel,
                    0,
                ));
            return Ok(());
        }
        if let Some(channel) = parse_io_channel(name, "output") {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadOutput,
                    0,
                    R::default(),
                    channel,
                    0,
                ));
            return Ok(());
        }

        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::LoadIndexedInt
        } else {
            FbcOpcode::LoadIndexedReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                desc.size,
            ));
        Ok(())
    }

    /// Compile `TeeVar` — store to variable and leave value on stack.
    fn compile_tee_var(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        value: FirId,
        _typ: &FirType,
    ) -> Result<(), CompileError> {
        // Compile value, store it, then reload (store+load = tee).
        self.compile_node(store, value)?;
        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let (store_op, load_op) = if desc.heap_type == HeapType::Int {
            (FbcOpcode::StoreInt, FbcOpcode::LoadInt)
        } else {
            (FbcOpcode::StoreReal, FbcOpcode::LoadReal)
        };
        let offset = desc.offset;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                store_op,
                0,
                R::default(),
                offset,
                0,
            ));
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                load_op,
                0,
                R::default(),
                offset,
                0,
            ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Declarations
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(DeclareVarInst*)` — allocates heap slots and optionally
    ///   compiles the initializer.
    fn compile_declare_var(
        &mut self,
        store: &FirStore,
        name: &str,
        typ: &FirType,
        _access: AccessType,
        init: Option<FirId>,
    ) -> Result<(), CompileError> {
        // Skip input/output pseudo-variables.
        if name.starts_with("input") || name.starts_with("output") {
            return Ok(());
        }

        // Determine element type and array size.
        let (elem_type, array_size) = match typ {
            FirType::Array(elem, size) => (elem.as_ref(), *size as i32),
            _ => (typ, 1),
        };

        // Soundfile handles get a slot index, not a heap slot.
        if matches!(elem_type, FirType::Sound) {
            self.alloc_soundfile_slot(name);
            return Ok(());
        }

        self.alloc_storage_desc(name, elem_type, array_size);

        // Compile initializer if present.
        if let Some(init_id) = init {
            self.compile_init_store(store, name, typ, init_id)?;
        }
        Ok(())
    }

    /// Compiles the initializer for a `DeclareVar`.
    ///
    /// # Source provenance (C++)
    /// - `visitStore(inst->fAddress, inst->fValue, inst->fType)` called
    ///   from `visit(DeclareVarInst*)`.
    fn compile_init_store(
        &mut self,
        store: &FirStore,
        name: &str,
        typ: &FirType,
        init_id: FirId,
    ) -> Result<(), CompileError> {
        let desc = self.field_table[name].clone();

        // Array waveform store path.
        if let FirType::Array(_, _) = typ {
            match match_fir(store, init_id) {
                FirMatch::Int32Array { values, .. } => {
                    self.current_block.push_block_store(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::BlockStoreInt,
                            0,
                            R::default(),
                            desc.offset,
                            values.len() as i32,
                        ),
                        BlockStoreData::Int(values),
                    );
                    return Ok(());
                }
                FirMatch::Float32Array { values, .. } => {
                    let data: Vec<R> = values.iter().map(|v| R::from_f64(f64::from(*v))).collect();
                    self.current_block.push_block_store(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::BlockStoreReal,
                            0,
                            R::default(),
                            desc.offset,
                            data.len() as i32,
                        ),
                        BlockStoreData::Real(data),
                    );
                    return Ok(());
                }
                FirMatch::Float64Array { values, .. } => {
                    let data: Vec<R> = values.iter().map(|v| R::from_f64(*v)).collect();
                    self.current_block.push_block_store(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::BlockStoreReal,
                            0,
                            R::default(),
                            desc.offset,
                            data.len() as i32,
                        ),
                        BlockStoreData::Real(data),
                    );
                    return Ok(());
                }
                _ => {
                    // Fall through to scalar store.
                }
            }
        }

        // Scalar store path: compile value, then emit StoreInt/StoreReal.
        self.compile_node(store, init_id)?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::StoreInt
        } else {
            FbcOpcode::StoreReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        Ok(())
    }

    /// Reserves heap storage for a scalar/array variable declaration without
    /// compiling its initializer.
    fn predeclare_var_storage(&mut self, name: &str, typ: &FirType) {
        if name.starts_with("input") || name.starts_with("output") {
            return;
        }
        let (elem_type, array_size) = match typ {
            FirType::Array(elem, size) => (elem.as_ref(), *size as i32),
            _ => (typ, 1),
        };
        // Soundfile handles get a slot index, not a heap slot.
        if matches!(elem_type, FirType::Sound) {
            self.alloc_soundfile_slot(name);
            return;
        }
        let _ = self.alloc_storage_desc(name, elem_type, array_size);
    }

    /// Reserves heap storage for a table declaration without compiling values.
    fn predeclare_table_storage(&mut self, name: &str, elem_type: &FirType, size: i32) {
        if name.starts_with("input") || name.starts_with("output") {
            return;
        }
        let _ = self.alloc_storage_desc(name, elem_type, size.max(0));
    }

    /// Allocates (or reuses) a memory descriptor in the compiler heap layout.
    ///
    /// If the name already exists, the previous descriptor is preserved so
    /// repeated pre-declaration/compilation passes remain idempotent.
    fn alloc_storage_desc(
        &mut self,
        name: &str,
        elem_type: &FirType,
        array_size: i32,
    ) -> MemoryDesc {
        if let Some(existing) = self.field_table.get(name) {
            return existing.clone();
        }
        let heap_type = if is_int_type(elem_type) {
            HeapType::Int
        } else {
            HeapType::Real
        };
        let offset = if heap_type == HeapType::Int {
            let o = self.int_heap_offset;
            self.int_heap_offset += array_size;
            o
        } else {
            let o = self.real_heap_offset;
            self.real_heap_offset += array_size;
            o
        };
        let desc = MemoryDesc {
            offset,
            size: array_size,
            heap_type,
        };
        self.field_table.insert(name.to_string(), desc.clone());
        desc
    }

    // -----------------------------------------------------------------------
    // Storage
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(StoreVarInst*)` / `visitStore()` — named address path.
    fn compile_store_var(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        value: FirId,
    ) -> Result<(), CompileError> {
        // Compile value (pushes onto stack).
        self.compile_node(store, value)?;

        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::StoreInt
        } else {
            FbcOpcode::StoreReal
        };
        let offset = desc.offset;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                offset,
                0,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visitStore()` — indexed address path.
    fn compile_store_table(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        index: FirId,
        value: FirId,
    ) -> Result<(), CompileError> {
        // Compile value first, then index (matches C++ order).
        self.compile_node(store, value)?;
        self.compile_node(store, index)?;

        // Special handling for output channels.
        if let Some(channel) = parse_io_channel(name, "output") {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::StoreOutput,
                    0,
                    R::default(),
                    channel,
                    0,
                ));
            return Ok(());
        }

        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::StoreIndexedInt
        } else {
            FbcOpcode::StoreIndexedReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                desc.size,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(ShiftArrayVarInst*)`.
    fn compile_shift_array(&mut self, name: &str, delay: i32) -> Result<(), CompileError> {
        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::BlockShiftInt
        } else {
            FbcOpcode::BlockShiftReal
        };
        // C++: offset1 = tmp.fOffset + inst->fDelay, offset2 = tmp.fOffset
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset + delay,
                desc.offset,
            ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Arithmetic
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(BinopInst*)` — compiles operands then emits int/real opcode.
    fn compile_binop(
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
    fn compile_neg(
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
    fn compile_cast(
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
    fn compile_bitcast(
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
    fn compile_select2(
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

    /// # Source provenance (C++)
    /// - `visit(IfInst*)` — block-switching for if/else.
    fn compile_if(
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
    fn compile_switch(
        &mut self,
        store: &FirStore,
        cond: FirId,
        cases: &[(i64, FirId)],
        default: Option<FirId>,
    ) -> Result<(), CompileError> {
        self.compile_switch_cases(store, cond, cases, default)
    }

    /// Lowers one `switch` case list recursively as a right-nested `if` chain.
    fn compile_switch_cases(
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
    fn compile_for_loop(
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
    fn compile_simple_for_loop(
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
    /// - `visit(BlockInst*)` — iterates over block statements.
    fn compile_block(&mut self, store: &FirStore, stmts: &[FirId]) -> Result<(), CompileError> {
        for &stmt in stmts {
            self.compile_node(store, stmt)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Function calls
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(FunCallInst*)` — compiles args in reverse order, then
    ///   emits the opcode from `gMathLibTable`.
    fn compile_fun_call(
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

    /// # Source provenance (C++)
    /// - `visit(OpenboxInst*)`.
    fn compile_open_box(&mut self, typ: &UiBoxType, label: &str) -> Result<(), CompileError> {
        let opcode = match typ {
            UiBoxType::Vertical => FbcOpcode::OpenVerticalBox,
            UiBoxType::Horizontal => FbcOpcode::OpenHorizontalBox,
            UiBoxType::Tab => FbcOpcode::OpenTabBox,
        };
        self.ui_instructions
            .push(FbcUiInstruction::open_box(opcode, label));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(CloseboxInst*)`.
    fn compile_close_box(&mut self) -> Result<(), CompileError> {
        self.ui_instructions
            .push(FbcUiInstruction::new(FbcOpcode::CloseBox));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddButtonInst*)`.
    fn compile_add_button(
        &mut self,
        typ: &ButtonType,
        label: &str,
        var: &str,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            ButtonType::Button => FbcOpcode::AddButton,
            ButtonType::Checkbox => FbcOpcode::AddCheckButton,
        };
        let offset = self.get_field_offset(var);
        self.ui_instructions.push(FbcUiInstruction::widget(
            opcode,
            offset,
            label,
            R::default(),
            R::default(),
            R::default(),
            R::default(),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddSliderInst*)`.
    #[allow(clippy::too_many_arguments)]
    fn compile_add_slider(
        &mut self,
        typ: &SliderType,
        label: &str,
        var: &str,
        init: f64,
        lo: f64,
        hi: f64,
        step: f64,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            SliderType::Horizontal => FbcOpcode::AddHorizontalSlider,
            SliderType::Vertical => FbcOpcode::AddVerticalSlider,
            SliderType::NumEntry => FbcOpcode::AddNumEntry,
        };
        let offset = self.get_field_offset(var);
        self.ui_instructions.push(FbcUiInstruction::widget(
            opcode,
            offset,
            label,
            R::from_f64(init),
            R::from_f64(lo),
            R::from_f64(hi),
            R::from_f64(step),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddBargraphInst*)`.
    fn compile_add_bargraph(
        &mut self,
        typ: &BargraphType,
        label: &str,
        var: &str,
        lo: f64,
        hi: f64,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            BargraphType::Horizontal => FbcOpcode::AddHorizontalBargraph,
            BargraphType::Vertical => FbcOpcode::AddVerticalBargraph,
        };
        let offset = self.get_field_offset(var);
        self.ui_instructions.push(FbcUiInstruction::bargraph(
            opcode,
            offset,
            label,
            R::from_f64(lo),
            R::from_f64(hi),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddSoundfileInst*)`.
    fn compile_add_soundfile(
        &mut self,
        label: &str,
        url: &str,
        var: &str,
    ) -> Result<(), CompileError> {
        // Register (or look up) this variable's soundfile slot.
        let slot = self.alloc_soundfile_slot(var);
        let mut instr = FbcUiInstruction::new(FbcOpcode::AddSoundfile);
        instr.label = label.to_string();
        // Store URL in `key` field — mirrors how `dispatch_ui_*` passes it to the callback.
        instr.key = url.to_string();
        // Store slot index in `offset` so instances can populate the right soundfile slot.
        instr.offset = slot as i32;
        self.ui_instructions.push(instr);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Soundfile access
    // -----------------------------------------------------------------------

    /// Allocates (or reuses) a soundfile slot index for `name`.
    ///
    /// Soundfile variables (`fSoundN`) are tracked in a separate slot table
    /// rather than in the int/real heap, because they are runtime object
    /// references — not scalar values.
    fn alloc_soundfile_slot(&mut self, name: &str) -> usize {
        if let Some(&slot) = self.soundfile_slots.get(name) {
            return slot;
        }
        let slot = self.num_soundfile_slots;
        self.num_soundfile_slots += 1;
        self.soundfile_slots.insert(name.to_string(), slot);
        slot
    }

    /// Compiles `LoadSoundfileLength { var, part }` → `kLoadSoundFieldInt` (fLength).
    ///
    /// # Source provenance (C++)
    /// - `visit(LoadSoundfileInst*)` — `kInt32` / fLength case.
    fn compile_load_soundfile_length(
        &mut self,
        store: &FirStore,
        var: &str,
        part: FirId,
    ) -> Result<(), CompileError> {
        let slot = self.soundfile_slots.get(var).copied().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: var.to_string(),
            }
        })?;
        // Push part index onto int stack; executor pops it.
        self.compile_node(store, part)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadSoundFieldInt,
                0, // int_value = 0 → fLength field selector
                R::default(),
                slot as i32, // offset1 = soundfile slot index
                0,
            ));
        Ok(())
    }

    /// Compiles `LoadSoundfileRate { var, part }` → `kLoadSoundFieldInt` (fSR).
    ///
    /// # Source provenance (C++)
    /// - `visit(LoadSoundfileInst*)` — `kInt32` / fSR case.
    fn compile_load_soundfile_rate(
        &mut self,
        store: &FirStore,
        var: &str,
        part: FirId,
    ) -> Result<(), CompileError> {
        let slot = self.soundfile_slots.get(var).copied().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: var.to_string(),
            }
        })?;
        self.compile_node(store, part)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadSoundFieldInt,
                1, // int_value = 1 → fSR field selector
                R::default(),
                slot as i32,
                0,
            ));
        Ok(())
    }

    /// Compiles `LoadSoundfileBuffer { var, chan, part, idx }` → `kLoadSoundFieldReal`.
    ///
    /// # Source provenance (C++)
    /// - `visit(LoadSoundfileInst*)` — FAUSTFLOAT buffer case.
    ///
    /// Pushes `chan`, `part`, `idx` onto the int stack; the executor pops them
    /// in reverse order and computes `buffers[chan][offsets[part] + idx]`.
    fn compile_load_soundfile_buffer(
        &mut self,
        store: &FirStore,
        var: &str,
        chan: FirId,
        part: FirId,
        idx: FirId,
    ) -> Result<(), CompileError> {
        let slot = self.soundfile_slots.get(var).copied().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: var.to_string(),
            }
        })?;
        // Push chan, part, idx — executor pops in LIFO order: idx first, then part, then chan.
        self.compile_node(store, chan)?;
        self.compile_node(store, part)?;
        self.compile_node(store, idx)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadSoundFieldReal,
                0,
                R::default(),
                slot as i32, // offset1 = soundfile slot index
                0,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddMetaDeclareInst*)`.
    fn compile_add_meta_declare(
        &mut self,
        var: &str,
        key: &str,
        value: &str,
    ) -> Result<(), CompileError> {
        let offset = if var == "0" {
            -1
        } else {
            self.get_field_offset(var)
        };
        self.ui_instructions
            .push(FbcUiInstruction::declare(offset, key, value));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `getFieldOffset()`.
    fn get_field_offset(&self, name: &str) -> i32 {
        self.field_table.get(name).map_or(-1, |desc| desc.offset)
    }
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

#[cfg(test)]
mod tests;
