//! `lifecycle` half of the FBC compiler.
//!
//! Construction, heap-offset accounting, block allocation, and teardown into the finished factory.
//!
//! Split out of `compiler.rs` on 2026-08-18, where all 54 methods sat in one
//! 1891-line `impl`. The method bodies are moved verbatim; only their
//! visibility widened from private to `pub(super)` so the sibling modules can
//! still reach them.

use super::*;

impl<R: FbcReal> FirToFbcCompiler<R> {
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
    pub(super) fn current_block_top_is_real(&self) -> bool {
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
    pub(super) fn begin_sub_block(&mut self) {
        let current = std::mem::take(&mut self.current_block);
        self.saved_blocks.push(current);
    }

    /// Seals the current block with `kReturn`, allocates it in the arena,
    /// and restores the previously saved block.
    ///
    /// Returns the [`BlockId`] of the newly allocated block.
    pub(super) fn end_sub_block(&mut self) -> BlockId {
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
    /// - `getFieldOffset()`.
    pub(super) fn get_field_offset(&self, name: &str) -> i32 {
        self.field_table.get(name).map_or(-1, |desc| desc.offset)
    }
}
