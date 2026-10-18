use super::*;

/// Helper: build a block from a list of instructions and optimize it.
fn make_block(instrs: Vec<FbcInstruction<f64>>) -> FbcBlock<f64> {
    let mut block = FbcBlock::new();
    for i in instrs {
        block.push(i);
    }
    block
}

fn inst(op: FbcOpcode) -> FbcInstruction<f64> {
    FbcInstruction::new(op)
}

fn inst_off(op: FbcOpcode, o1: i32, o2: i32) -> FbcInstruction<f64> {
    FbcInstruction::with_values_and_offsets(op, 0, 0.0, o1, o2)
}

fn inst_int(val: i32) -> FbcInstruction<f64> {
    FbcInstruction::with_values(FbcOpcode::Int32Value, val, 0.0)
}

fn inst_real(val: f64) -> FbcInstruction<f64> {
    FbcInstruction::with_values(FbcOpcode::RealValue, 0, val)
}

// ── Level 1: LoadStore ────────────────────────────────────────────

#[test]
fn test_load_store_index_fold() {
    let block = make_block(vec![
        inst_int(5),
        inst_off(FbcOpcode::LoadIndexedReal, 10, 20),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_load_store);
    assert_eq!(result.len(), 2); // LoadReal + Return
    assert_eq!(result.instructions[0].opcode, FbcOpcode::LoadReal);
    assert_eq!(result.instructions[0].offset1, 15); // 5 + 10
}

#[test]
fn test_store_indexed_int_fold() {
    let block = make_block(vec![
        inst_int(3),
        inst_off(FbcOpcode::StoreIndexedInt, 7, 0),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_load_store);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::StoreInt);
    assert_eq!(result.instructions[0].offset1, 10); // 3 + 7
}

// ── Level 2: Move ─────────────────────────────────────────────────

#[test]
fn test_move_fusion() {
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadReal, 0, 0),
        inst_off(FbcOpcode::StoreReal, 1, 0),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_move);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::MoveReal);
    assert_eq!(result.instructions[0].offset1, 1); // destination
    assert_eq!(result.instructions[0].offset2, 0); // source
}

#[test]
fn test_value_store_fusion() {
    let block = make_block(vec![
        inst_real(3.0),
        inst_off(FbcOpcode::StoreReal, 5, 0),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_move);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::StoreRealValue);
    assert!((result.instructions[0].real_value - 3.0).abs() < 1e-10);
    assert_eq!(result.instructions[0].offset1, 5);
}

#[test]
fn test_int_value_store_fusion() {
    let block = make_block(vec![
        inst_int(42),
        inst_off(FbcOpcode::StoreInt, 8, 0),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_move);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::StoreIntValue);
    assert_eq!(result.instructions[0].int_value, 42);
    assert_eq!(result.instructions[0].offset1, 8);
}

// ── Level 4: PairMove ─────────────────────────────────────────────

#[test]
fn test_pair_move_real() {
    // Two MoveReal with offset1 = offset2+1, and i2.offset1 == i1.offset2
    let block = make_block(vec![
        inst_off(FbcOpcode::MoveReal, 11, 10), // offset1=11, offset2=10
        inst_off(FbcOpcode::MoveReal, 10, 9),  // offset1=10, offset2=9, 10==10 ✓
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_pair_move);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::PairMoveReal);
}

// ── Level 5: Cast ─────────────────────────────────────────────────

#[test]
fn test_cast_heap_fusion() {
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadInt, 5, 0),
        inst(FbcOpcode::CastReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_cast);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::CastRealHeap);
    assert_eq!(result.instructions[0].offset1, 5);
}

#[test]
fn test_cast_int_heap_fusion() {
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadReal, 7, 0),
        inst(FbcOpcode::CastInt),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_cast);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::CastIntHeap);
    assert_eq!(result.instructions[0].offset1, 7);
}

// ── Level 6: Math ─────────────────────────────────────────────────

#[test]
fn test_heap_math_fusion() {
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadReal, 0, 0),
        inst_off(FbcOpcode::LoadReal, 1, 0),
        inst(FbcOpcode::AddReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::AddRealHeap);
    assert_eq!(result.instructions[0].offset1, 1); // i2.offset1
    assert_eq!(result.instructions[0].offset2, 0); // i1.offset1
}

#[test]
fn test_stack_math_fusion() {
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadReal, 0, 0),
        inst(FbcOpcode::AddReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::AddRealStack);
    assert_eq!(result.instructions[0].offset1, 0);
}

#[test]
fn test_stack_value_fusion() {
    let block = make_block(vec![
        inst_real(2.5),
        inst(FbcOpcode::MultReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::MultRealStackValue);
    assert!((result.instructions[0].real_value - 2.5).abs() < 1e-10);
}

#[test]
fn test_value_commutative_fusion() {
    // LoadReal + RealValue + AddReal → AddRealValue
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadReal, 5, 0),
        inst_real(3.0),
        inst(FbcOpcode::AddReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::AddRealValue);
    assert_eq!(result.instructions[0].offset1, 5);
    assert!((result.instructions[0].real_value - 3.0).abs() < 1e-10);
}

#[test]
fn test_value_noncommutative_invert() {
    // RealValue + LoadReal + SubReal → SubRealValueInvert
    let block = make_block(vec![
        inst_real(7.0),
        inst_off(FbcOpcode::LoadReal, 3, 0),
        inst(FbcOpcode::SubReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::SubRealValueInvert);
}

#[test]
fn test_ext_unary_heap_fusion() {
    let block = make_block(vec![
        inst_off(FbcOpcode::LoadReal, 4, 0),
        inst(FbcOpcode::Sinf),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::SinfHeap);
    assert_eq!(result.instructions[0].offset1, 4);
}

#[test]
fn test_constant_fold_add() {
    let block = make_block(vec![
        inst_real(2.0),
        inst_real(3.0),
        inst(FbcOpcode::AddReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::RealValue);
    assert!((result.instructions[0].real_value - 5.0).abs() < 1e-10);
}

#[test]
fn test_constant_fold_int_mul() {
    let block = make_block(vec![
        inst_int(6),
        inst_int(7),
        inst(FbcOpcode::MultInt),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::Int32Value);
    assert_eq!(result.instructions[0].int_value, 42);
}

#[test]
fn test_constant_fold_identity_add_zero() {
    // RealValue(0.0) + LoadReal(5) + AddReal → LoadReal(5)
    let block = make_block(vec![
        inst_real(0.0),
        inst_off(FbcOpcode::LoadReal, 5, 0),
        inst(FbcOpcode::AddReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::LoadReal);
    assert_eq!(result.instructions[0].offset1, 5);
}

#[test]
fn test_constant_fold_annihilator_mul_zero() {
    // RealValue(0.0) + LoadReal(5) + MultReal → RealValue(0.0)
    let block = make_block(vec![
        inst_real(0.0),
        inst_off(FbcOpcode::LoadReal, 5, 0),
        inst(FbcOpcode::MultReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::RealValue);
    assert!((result.instructions[0].real_value - 0.0).abs() < 1e-10);
}

#[test]
fn test_cast_constant_fold() {
    // Int32Value(5) + CastReal → RealValue(5.0)
    let block = make_block(vec![
        inst_int(5),
        inst(FbcOpcode::CastReal),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::RealValue);
    assert!((result.instructions[0].real_value - 5.0).abs() < 1e-10);
}

#[test]
fn test_cast_real_to_int_constant() {
    // RealValue(3.7) + CastInt → Int32Value(3)
    let block = make_block(vec![
        inst_real(3.7),
        inst(FbcOpcode::CastInt),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::Int32Value);
    assert_eq!(result.instructions[0].int_value, 3);
}

#[test]
fn test_unary_constant_fold_sinf() {
    let block = make_block(vec![
        inst_real(0.0),
        inst(FbcOpcode::Sinf),
        inst(FbcOpcode::Return),
    ]);
    let result = optimize_until_fixpoint(block, rewrite_math);
    assert_eq!(result.len(), 2);
    assert_eq!(result.instructions[0].opcode, FbcOpcode::RealValue);
    assert!(result.instructions[0].real_value.abs() < 1e-10); // sin(0) = 0
}

// ── Recursive sub-block optimization ──────────────────────────────

#[test]
fn test_recursive_subblock() {
    let mut arena = FbcBlockArena::<f64>::new();

    // Create a sub-block with optimizable pattern
    let mut sub = FbcBlock::new();
    sub.push(inst_off(FbcOpcode::LoadReal, 0, 0));
    sub.push(inst_off(FbcOpcode::StoreReal, 1, 0));
    sub.push(inst(FbcOpcode::Return));
    let sub_id = arena.alloc(sub);

    // Create main block with an If that references the sub-block
    let mut main = FbcBlock::new();
    main.push(FbcInstruction::full(
        FbcOpcode::If,
        "",
        0,
        0.0,
        -1,
        -1,
        Some(sub_id),
        Some(sub_id),
    ));
    main.push(inst(FbcOpcode::Return));
    let main_id = arena.alloc(main);

    // Apply Level 2 (move optimization) recursively
    optimize_block(&mut arena, main_id, 2, 2);

    // Sub-block should now have MoveReal instead of LoadReal+StoreReal
    let optimized_sub = arena.get(sub_id);
    assert_eq!(optimized_sub.instructions[0].opcode, FbcOpcode::MoveReal);
}

// ── Integration: optimize_block multi-level ───────────────────────

#[test]
fn test_optimize_block_all_levels() {
    let mut arena = FbcBlockArena::<f64>::new();

    // Block: Int32Value(3) + LoadIndexedReal(10, 20) + StoreReal(5) + Return
    // L1: Int32Value(3) + LoadIndexedReal(10) → LoadReal(13)
    // L2: LoadReal(13) + StoreReal(5) → MoveReal(5, 13)
    let mut block = FbcBlock::new();
    block.push(inst_int(3));
    block.push(inst_off(FbcOpcode::LoadIndexedReal, 10, 20));
    block.push(inst_off(FbcOpcode::StoreReal, 5, 0));
    block.push(inst(FbcOpcode::Return));
    let id = arena.alloc(block);

    optimize_block(&mut arena, id, 1, 6);

    let result = arena.get(id);
    assert_eq!(result.len(), 2); // MoveReal + Return
    assert_eq!(result.instructions[0].opcode, FbcOpcode::MoveReal);
    assert_eq!(result.instructions[0].offset1, 5); // destination
    assert_eq!(result.instructions[0].offset2, 13); // source (3 + 10)
}

#[test]
fn test_block_store_payload_preserved_across_optimization() {
    let mut block = FbcBlock::new();
    let store = FbcInstruction::with_values_and_offsets(FbcOpcode::BlockStoreReal, 0, 0.0, 0, 3);
    block.push_block_store(
        store,
        super::super::bytecode::BlockStoreData::Real(vec![0.5, 0.6, 0.7]),
    );
    block.push(inst(FbcOpcode::Return));

    let result = optimize_until_fixpoint(block, rewrite_load_store);

    assert_eq!(result.instructions[0].opcode, FbcOpcode::BlockStoreReal);
    match &result.instructions[0].block_store {
        Some(super::super::bytecode::BlockStoreData::Real(values)) => {
            assert_eq!(values.as_slice(), &[0.5, 0.6, 0.7]);
        }
        _ => panic!("expected inline BlockStoreData::Real"),
    }
}

/// Folding `Maxf`/`Minf` gives what executing them gives, NaN included: the
/// executor follows C++ `std::max`/`std::min`, and the fold used Rust's
/// `f32::max`/`min`, which ignore a NaN. `v2` is the operand popped first.
#[test]
fn min_max_fold_matches_execution_on_nan() {
    let folded = |op| {
        fold_ext_binary_real::<f32>(1.0, f32::NAN, op)
            .expect("Maxf/Minf fold")
            .real_value
    };
    // NaN popped first: `std::max(NaN, 1)` and `std::min(NaN, 1)` are NaN
    // (Rust's `max`/`min` gave 1).
    assert!(folded(FbcOpcode::Maxf).is_nan());
    assert!(folded(FbcOpcode::Minf).is_nan());
    // NaN popped second: both are 1.
    let swapped = |op| {
        fold_ext_binary_real::<f32>(f32::NAN, 1.0, op)
            .expect("Maxf/Minf fold")
            .real_value
    };
    assert_eq!(swapped(FbcOpcode::Maxf), 1.0);
    assert_eq!(swapped(FbcOpcode::Minf), 1.0);
}
