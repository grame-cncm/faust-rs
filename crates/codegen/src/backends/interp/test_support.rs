//! Builders shared by the interpreter's unit tests.

use crate::backends::interp::FbcInstruction;
use crate::backends::interp::bytecode::{BlockId, FbcBlock, FbcBlockArena};
use crate::backends::interp::opcode::FbcOpcode;

/// Allocates a block holding only a `Return` instruction.
///
/// Was written out identically in the factory, instance and FBC-to-C++ tests.
pub(crate) fn trivial_block(arena: &mut FbcBlockArena<f32>) -> BlockId {
    let mut block = FbcBlock::new();
    block.push(FbcInstruction::new(FbcOpcode::Return));
    arena.alloc(block)
}
