//! FBC bytecode instruction and block types.
//!
//! # Source provenance (C++)
//! - `compiler/generator/interpreter/interpreter_bytecode.hh`
//!   (`FBCBasicInstruction<REAL>`, `FBCBlockInstruction<REAL>`,
//!   `FIRBlockStoreRealInstruction<REAL>`, `FIRBlockStoreIntInstruction<REAL>`,
//!   `FIRUserInterfaceInstruction<REAL>`, `FIRMetaInstruction`)
//!
//! # Design notes
//! - FBC instructions are stored in flat `Vec`s, not in `TreeArena`, because
//!   FBC is a linear instruction stream optimized for sequential execution
//!   (see porting plan §3.1).
//! - C++ raw pointers to `FBCBlockInstruction<REAL>` are replaced by [`BlockId`]
//!   indices into [`FbcBlockArena`].
//! - C++ `template <class REAL>` is replaced by the [`FbcReal`] trait bound.
//!
//! # API mapping status
//! - `FBCBasicInstruction<REAL>` → [`FbcInstruction<R>`]: adapted (fields kept,
//!   pointers replaced by `BlockId`).
//! - `FBCBlockInstruction<REAL>` → [`FbcBlock<R>`]: adapted.
//! - `FIRBlockStoreRealInstruction<REAL>` → [`BlockStoreData::Real`]: merged.
//! - `FIRBlockStoreIntInstruction<REAL>` → [`BlockStoreData::Int`]: merged.
//! - `FIRUserInterfaceInstruction<REAL>` → [`FbcUiInstruction<R>`]: adapted.
//! - `FIRMetaInstruction` → [`FbcMetaInstruction`]: 1:1.

use super::opcode::FbcOpcode;
use super::real::FbcReal;

/// Index into [`FbcBlockArena`].
///
/// Replaces C++ raw `FBCBlockInstruction<REAL>*` pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(u32);

impl BlockId {
    /// Returns the raw index value.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }

    /// Creates a `BlockId` from a raw index.
    ///
    /// Used by the compiler to predict the next arena allocation index
    /// for `CondBranch` loop-back references.
    ///
    /// # Safety note
    /// This is `pub(crate)` because it creates an ID that may not yet
    /// exist in the arena — the caller must allocate the block immediately
    /// after.
    #[must_use]
    pub(crate) fn from_raw(index: u32) -> Self {
        Self(index)
    }
}

/// A single FBC instruction.
///
/// # Source provenance (C++)
/// - `FBCBasicInstruction<REAL>` in `interpreter_bytecode.hh`
///
/// Generic over `R` (the REAL type: `f32` or `f64`).
///
/// # Field semantics
/// - `opcode`: the instruction type.
/// - `name`: variable/field name for UI and memory instructions (rare).
/// - `int_value`: integer immediate (e.g., constant value, loop bound).
/// - `real_value`: real immediate (e.g., constant value).
/// - `offset1`, `offset2`: heap offsets for memory operations.
/// - `branch1`: branch 1 (if-true / loop-init block).
/// - `branch2`: branch 2 (if-false / loop-body block).
///
/// Branch interpretation is opcode-specific: `If` and `Select*` use
/// `branch1`/`branch2` as true/false blocks, while `Loop` uses them as its
/// initialization/body blocks. Branch targets are arena indices rather than
/// nested blocks, so a serializer and an optimizer can preserve sharing and
/// loop back-edges without cloning instruction streams.
#[derive(Clone, Debug)]
pub struct FbcInstruction<R: FbcReal> {
    /// Operation code selecting the instruction semantics.
    pub opcode: FbcOpcode,
    /// Optional symbolic payload (variable name, UI label, etc.).
    pub name: String,
    /// Integer immediate payload.
    pub int_value: i32,
    /// Real immediate payload.
    pub real_value: R,
    /// First heap offset / aux integer slot.
    pub offset1: i32,
    /// Second heap offset / aux integer slot.
    pub offset2: i32,
    /// Optional inline payload for `BlockStoreReal` / `BlockStoreInt`.
    ///
    /// This keeps block-store table ownership attached to the instruction
    /// itself, mirroring C++ specialized instruction objects.
    pub block_store: Option<BlockStoreData<R>>,
    /// Optional first branch target.
    pub branch1: Option<BlockId>,
    /// Optional second branch target.
    pub branch2: Option<BlockId>,
}

impl<R: FbcReal> FbcInstruction<R> {
    /// Creates a new instruction with only an opcode (all other fields default).
    ///
    /// Corresponds to `FBCBasicInstruction(Opcode opcode)` in C++.
    #[must_use]
    pub fn new(opcode: FbcOpcode) -> Self {
        Self {
            opcode,
            name: String::new(),
            int_value: 0,
            real_value: R::default(),
            offset1: -1,
            offset2: -1,
            block_store: None,
            branch1: None,
            branch2: None,
        }
    }

    /// Creates a new instruction with opcode and immediates.
    ///
    /// Corresponds to `FBCBasicInstruction(Opcode, int, REAL)` in C++.
    #[must_use]
    pub fn with_values(opcode: FbcOpcode, int_value: i32, real_value: R) -> Self {
        Self {
            opcode,
            name: String::new(),
            int_value,
            real_value,
            offset1: -1,
            offset2: -1,
            block_store: None,
            branch1: None,
            branch2: None,
        }
    }

    /// Creates a new instruction with opcode, immediates, and offsets.
    ///
    /// Corresponds to `FBCBasicInstruction(Opcode, int, REAL, int, int)` in C++.
    #[must_use]
    pub fn with_values_and_offsets(
        opcode: FbcOpcode,
        int_value: i32,
        real_value: R,
        offset1: i32,
        offset2: i32,
    ) -> Self {
        Self {
            opcode,
            name: String::new(),
            int_value,
            real_value,
            offset1,
            offset2,
            block_store: None,
            branch1: None,
            branch2: None,
        }
    }

    /// Creates a new instruction with a name.
    ///
    /// Corresponds to `FBCBasicInstruction(Opcode, string)` in C++.
    #[must_use]
    pub fn with_name(opcode: FbcOpcode, name: impl Into<String>) -> Self {
        Self {
            opcode,
            name: name.into(),
            int_value: 0,
            real_value: R::default(),
            offset1: -1,
            offset2: -1,
            block_store: None,
            branch1: None,
            branch2: None,
        }
    }

    /// Creates a fully specified instruction.
    ///
    /// Corresponds to the most general `FBCBasicInstruction` constructor in C++.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn full(
        opcode: FbcOpcode,
        name: impl Into<String>,
        int_value: i32,
        real_value: R,
        offset1: i32,
        offset2: i32,
        branch1: Option<BlockId>,
        branch2: Option<BlockId>,
    ) -> Self {
        Self {
            opcode,
            name: name.into(),
            int_value,
            real_value,
            offset1,
            offset2,
            block_store: None,
            branch1,
            branch2,
        }
    }

    /// Returns branch1 unless this is a `CondBranch` instruction.
    ///
    /// # Source provenance (C++)
    /// - `FBCBasicInstruction::getBranch1()` — returns `nullptr` for `kCondBranch`
    ///   because `fBranch1` is the loop-back pointer (owned by the block, not the
    ///   instruction).
    #[must_use]
    pub fn get_branch1(&self) -> Option<BlockId> {
        if self.opcode == FbcOpcode::CondBranch {
            None
        } else {
            self.branch1
        }
    }

    /// Returns branch2.
    #[must_use]
    pub fn get_branch2(&self) -> Option<BlockId> {
        self.branch2
    }
}

/// Optional bulk-store data attached to `BlockStoreReal` / `BlockStoreInt`
/// instructions.
///
/// # Source provenance (C++)
/// - `FIRBlockStoreRealInstruction<REAL>` and `FIRBlockStoreIntInstruction<REAL>`
///   in `interpreter_bytecode.hh`.
///
/// In C++ these are subclasses carrying a `fNumTable` vector. In Rust we model
/// them as an auxiliary enum stored inline on the instruction.
#[derive(Clone, Debug)]
pub enum BlockStoreData<R: FbcReal> {
    /// Bulk real payload for `BlockStoreReal`.
    Real(Vec<R>),
    /// Bulk integer payload for `BlockStoreInt`.
    Int(Vec<i32>),
}

/// A block of FBC instructions (linear sequence ending with `Return`).
///
/// # Source provenance (C++)
/// - `FBCBlockInstruction<REAL>` in `interpreter_bytecode.hh`
///
/// # Invariant
/// The last instruction in a well-formed block must have opcode
/// [`FbcOpcode::Return`].
#[derive(Clone, Debug)]
pub struct FbcBlock<R: FbcReal> {
    /// Linear bytecode sequence for this block.
    pub instructions: Vec<FbcInstruction<R>>,
}

impl<R: FbcReal> FbcBlock<R> {
    /// Creates an empty block.
    #[must_use]
    pub fn new() -> Self {
        Self {
            instructions: Vec::new(),
        }
    }

    /// Appends an instruction to the block.
    pub fn push(&mut self, instr: FbcInstruction<R>) {
        self.instructions.push(instr);
    }

    /// Appends a block-store instruction with associated data table.
    pub fn push_block_store(&mut self, mut instr: FbcInstruction<R>, data: BlockStoreData<R>) {
        instr.block_store = Some(data);
        self.instructions.push(instr);
    }

    /// Returns `true` if the block ends with a `Return` instruction.
    ///
    /// # Source provenance (C++)
    /// - `FBCBlockInstruction::check()` in `interpreter_bytecode.hh`
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        self.instructions
            .last()
            .is_some_and(|instr| instr.opcode == FbcOpcode::Return)
    }

    /// Returns `true` if the last instruction operates on real values.
    ///
    /// # Source provenance (C++)
    /// - `FBCBlockInstruction::isRealInst()` in `interpreter_bytecode.hh`
    #[must_use]
    pub fn is_real_inst(&self) -> bool {
        self.instructions
            .last()
            .is_some_and(|instr| instr.opcode.is_real_type())
    }

    /// Returns the number of instructions in the block.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instructions.len()
    }

    /// Returns `true` if the block is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instructions.is_empty()
    }
}

impl<R: FbcReal> Default for FbcBlock<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// Arena-like storage for all blocks in a DSP factory.
///
/// Blocks reference each other via [`BlockId`] indices, replacing C++ raw
/// `FBCBlockInstruction<REAL>*` pointers. This provides safe, index-based
/// ownership without `unsafe` pointer arithmetic.
///
/// # Source provenance (C++)
/// - Implicit in `interpreter_dsp_aux.hh` (blocks are owned by the factory
///   and referenced by pointer).
///
/// Allocation order matters for a few compiler-side patterns such as predicted
/// loop-back targets, so the arena should be treated as append-only once block
/// ids have escaped to instructions or factories.
#[derive(Clone, Debug)]
pub struct FbcBlockArena<R: FbcReal> {
    blocks: Vec<FbcBlock<R>>,
}

impl<R: FbcReal> FbcBlockArena<R> {
    /// Creates an empty block arena.
    #[must_use]
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    /// Allocates a block in the arena and returns its [`BlockId`].
    pub fn alloc(&mut self, block: FbcBlock<R>) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(block);
        id
    }

    /// Returns a reference to the block at the given ID.
    ///
    /// # Panics
    /// Panics if `id` is out of range.
    #[must_use]
    pub fn get(&self, id: BlockId) -> &FbcBlock<R> {
        &self.blocks[id.0 as usize]
    }

    /// Returns a reference to the block at the given ID, or `None` if out of range.
    #[must_use]
    pub fn try_get(&self, id: BlockId) -> Option<&FbcBlock<R>> {
        self.blocks.get(id.0 as usize)
    }

    /// Returns a mutable reference to the block at the given ID.
    ///
    /// # Panics
    /// Panics if `id` is out of range.
    pub fn get_mut(&mut self, id: BlockId) -> &mut FbcBlock<R> {
        &mut self.blocks[id.0 as usize]
    }

    /// Returns a mutable reference to the block at the given ID, or `None` if out of range.
    pub fn try_get_mut(&mut self, id: BlockId) -> Option<&mut FbcBlock<R>> {
        self.blocks.get_mut(id.0 as usize)
    }

    /// Returns the number of blocks in the arena.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Returns `true` if the arena contains no blocks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl<R: FbcReal> Default for FbcBlockArena<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// A UI instruction in the FBC bytecode.
///
/// # Source provenance (C++)
/// - `FIRUserInterfaceInstruction<REAL>` in `interpreter_bytecode.hh`
///
/// # API mapping status
/// - adapted: C++ overloaded constructors are replaced by a single struct
///   with `Option` / default fields.
///
/// The opcode determines which subset of fields is meaningful:
/// - `label` for boxes/widgets,
/// - `offset` for widget/bargraph zone bindings,
/// - `key`/`value` for `Declare`,
/// - `init/min/max/step` for ranged controls.
#[derive(Clone, Debug)]
pub struct FbcUiInstruction<R: FbcReal> {
    /// UI opcode selecting the widget/box operation.
    pub opcode: FbcOpcode,
    /// Bound zone offset when the instruction targets one heap slot.
    pub offset: i32,
    /// Primary label payload.
    pub label: String,
    /// Metadata key payload for `Declare`.
    pub key: String,
    /// Metadata value or URL payload.
    pub value: String,
    /// Initial control value.
    pub init: R,
    /// Minimum control value.
    pub min: R,
    /// Maximum control value.
    pub max: R,
    /// Control step size.
    pub step: R,
}

impl<R: FbcReal> FbcUiInstruction<R> {
    /// Creates a UI instruction with all fields defaulted.
    #[must_use]
    pub fn new(opcode: FbcOpcode) -> Self {
        Self {
            opcode,
            offset: -1,
            label: String::new(),
            key: String::new(),
            value: String::new(),
            init: R::default(),
            min: R::default(),
            max: R::default(),
            step: R::default(),
        }
    }

    /// Creates a box-open UI instruction (vertical/horizontal/tab).
    #[must_use]
    pub fn open_box(opcode: FbcOpcode, label: impl Into<String>) -> Self {
        Self {
            opcode,
            offset: -1,
            label: label.into(),
            key: String::new(),
            value: String::new(),
            init: R::default(),
            min: R::default(),
            max: R::default(),
            step: R::default(),
        }
    }

    /// Creates a widget UI instruction (button/slider/etc.) with range.
    #[must_use]
    pub fn widget(
        opcode: FbcOpcode,
        offset: i32,
        label: impl Into<String>,
        init: R,
        min: R,
        max: R,
        step: R,
    ) -> Self {
        Self {
            opcode,
            offset,
            label: label.into(),
            key: String::new(),
            value: String::new(),
            init,
            min,
            max,
            step,
        }
    }

    /// Creates a bargraph UI instruction with range.
    #[must_use]
    pub fn bargraph(
        opcode: FbcOpcode,
        offset: i32,
        label: impl Into<String>,
        min: R,
        max: R,
    ) -> Self {
        Self {
            opcode,
            offset,
            label: label.into(),
            key: String::new(),
            value: String::new(),
            init: R::default(),
            min,
            max,
            step: R::default(),
        }
    }

    /// Creates a metadata declare UI instruction.
    #[must_use]
    pub fn declare(offset: i32, key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            opcode: FbcOpcode::Declare,
            offset,
            label: String::new(),
            key: key.into(),
            value: value.into(),
            init: R::default(),
            min: R::default(),
            max: R::default(),
            step: R::default(),
        }
    }
}

/// A metadata key-value pair.
///
/// # Source provenance (C++)
/// - `FIRMetaInstruction` in `interpreter_bytecode.hh`
///
/// Kept separate from [`FbcUiInstruction`] because metadata is replayed through
/// the factory metadata callback, not through the UI builder stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FbcMetaInstruction {
    pub key: String,
    pub value: String,
}

impl FbcMetaInstruction {
    /// Creates a new metadata instruction.
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[cfg(test)]
mod tests;
