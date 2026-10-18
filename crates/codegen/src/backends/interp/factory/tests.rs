use super::*;
use crate::backends::interp::bytecode::FbcBlock;
use crate::backends::interp::opcode::FbcOpcode;
use crate::backends::interp::{FbcInstruction, INTERP_FILE_VERSION};

use crate::backends::interp::test_support::trivial_block;

/// Helper: creates a factory with trivial (empty) blocks.
fn make_trivial_factory() -> FbcDspFactory<f32> {
    let mut arena = FbcBlockArena::new();
    let b1 = trivial_block(&mut arena);
    let b2 = trivial_block(&mut arena);
    let b3 = trivial_block(&mut arena);
    let b4 = trivial_block(&mut arena);
    let b5 = trivial_block(&mut arena);
    let b6 = trivial_block(&mut arena);

    FbcDspFactory::new(
        "test",
        "sha123",
        "-lang interp",
        INTERP_FILE_VERSION,
        1,
        1,
        16,
        16,
        0, // sr_offset
        1, // count_offset
        2, // iota_offset
        4, // opt_level
        arena,
        vec![FbcMetaInstruction::new("name", "test")],
        vec![],
        b1,
        b2,
        b3,
        b4,
        b5,
        b6,
    )
}

#[test]
fn factory_construction() {
    let factory = make_trivial_factory();
    assert_eq!(factory.name, "test");
    assert_eq!(factory.sha_key, "sha123");
    assert_eq!(factory.num_inputs, 1);
    assert_eq!(factory.num_outputs, 1);
    assert_eq!(factory.int_heap_size, 16);
    assert_eq!(factory.real_heap_size, 16);
    assert_eq!(factory.opt_level, 4);
    assert!(!factory.is_optimized());
    assert_eq!(factory.meta_block.len(), 1);
    assert_eq!(factory.meta_block[0].key, "name");
}

#[test]
fn factory_optimize_idempotent() {
    let mut factory = make_trivial_factory();
    assert!(!factory.is_optimized());

    factory.optimize();
    assert!(factory.is_optimized());

    // Second call is a no-op.
    factory.optimize();
    assert!(factory.is_optimized());
}

#[test]
fn factory_optimize_reduces_instructions() {
    // Build a block with LoadReal(0) + StoreReal(1) that should fuse to MoveReal.
    let mut arena = FbcBlockArena::<f32>::new();

    // Optimizable block.
    let mut block = FbcBlock::new();
    block.push(FbcInstruction::with_values_and_offsets(
        FbcOpcode::LoadReal,
        0,
        0.0,
        0,
        -1,
    ));
    block.push(FbcInstruction::with_values_and_offsets(
        FbcOpcode::StoreReal,
        0,
        0.0,
        1,
        -1,
    ));
    block.push(FbcInstruction::new(FbcOpcode::Return));
    let opt_block = arena.alloc(block);

    // Trivial blocks for the rest.
    let b2 = trivial_block(&mut arena);
    let b3 = trivial_block(&mut arena);
    let b4 = trivial_block(&mut arena);
    let b5 = trivial_block(&mut arena);
    let b6 = trivial_block(&mut arena);

    let mut factory = FbcDspFactory::new(
        "test",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        0,
        4,
        4,
        0,
        1,
        -1,
        4, // opt_level 4 includes level 2 (Move fusion)
        arena,
        vec![],
        vec![],
        opt_block,
        b2,
        b3,
        b4,
        b5,
        b6,
    );

    // Before optimization: 3 instructions (Load, Store, Return).
    assert_eq!(factory.arena.get(factory.static_init_block).len(), 3);

    factory.optimize();

    // After optimization: 2 instructions (MoveReal, Return).
    assert_eq!(factory.arena.get(factory.static_init_block).len(), 2);
    assert_eq!(
        factory.arena.get(factory.static_init_block).instructions[0].opcode,
        FbcOpcode::MoveReal
    );
}
