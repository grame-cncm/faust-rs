// The nested `if let` pattern is idiomatic for this optimizer: outer `if` checks
// opcode patterns, inner `if let` unwraps the Option from offset arithmetic.
#![allow(clippy::collapsible_if)]

//! FBC bytecode optimizer — peephole rewriting of instruction sequences.
//!
//! # Source provenance (C++)
//! - `compiler/generator/interpreter/interpreter_optimizer.hh`
//!   (`FBCInstructionOptimizer<REAL>` and its 12 subclasses)
//!
//! # Design notes
//! - The C++ class hierarchy is replaced by free functions that each implement
//!   a rewrite rule returning a `RewriteResult`.
//! - Six optimization levels are applied sequentially, each as a peephole pass
//!   with a fixed-point loop (repeat until the block stops shrinking).
//! - Sub-blocks referenced by control-flow instructions (`If`, `SelectReal`,
//!   `SelectInt`, `Loop`, `CondBranch`) are optimized recursively.
//! - Opcode offset arithmetic (see [`super::opcode`]) enables O(1) translation
//!   between addressing-mode variants.
//!
//! # API mapping status
//! - `FBCInstructionOptimizer::optimizeBlock()` → [`optimize_block`]: 1:1.
//! - `FBCInstructionLoadStoreOptimizer` → `rewrite_load_store` (Level 1).
//! - `FBCInstructionMoveOptimizer` → `rewrite_move` (Level 2).
//! - `FBCInstructionBlockMoveOptimizer` → `rewrite_block_move` (Level 3).
//! - `FBCInstructionPairMoveOptimizer` → `rewrite_pair_move` (Level 4).
//! - `FBCInstructionCastOptimizer` → `rewrite_cast` (Level 5).
//! - `FBCInstructionMathOptimizer` → `rewrite_math` (Level 6).
//! - `FBCInstructionMathSpecializer` → integrated into `rewrite_math`.
//! - `FBCInstructionCastSpecializer` → integrated into `rewrite_math`.

use super::bytecode::{BlockId, FbcBlock, FbcBlockArena, FbcInstruction};
use super::opcode::FbcOpcode;
use super::real::FbcReal;

/// Maximum optimization level supported.
///
/// Levels are cumulative: requesting level `N` enables all rewrite families
/// from `1..=N`.
pub const MAX_OPT_LEVEL: u8 = 6;

// ═══════════════════════════════════════════════════════════════════════════
// Rewrite framework
// ═══════════════════════════════════════════════════════════════════════════

/// Result of a single rewrite attempt at a cursor position.
///
/// Rewrites are purely local peephole decisions: they can emit one fused
/// instruction replacing `advance` source instructions, or request a verbatim
/// copy of the next `advance` instructions.
enum RewriteResult<R: FbcReal> {
    /// Replace `advance` instructions with the given instruction.
    Emit(FbcInstruction<R>, usize),
    /// No rewrite — copy `advance` instructions as-is.
    Copy(usize),
}

/// Apply a rewrite function across an entire block, producing a new block.
///
/// The rewriter inspects instructions starting at `cursor` and returns either
/// a fused instruction (replacing N) or a copy signal.
///
/// The output block preserves instruction order modulo the requested local
/// rewrites; no global control-flow restructuring happens here.
fn apply_rewriter<R: FbcReal>(
    block: &FbcBlock<R>,
    rewrite: impl Fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>,
) -> FbcBlock<R> {
    let instrs = &block.instructions;
    let mut result = FbcBlock::new();
    let mut cursor = 0;

    while cursor < instrs.len() {
        match rewrite(instrs, cursor) {
            RewriteResult::Emit(inst, advance) => {
                result.push(inst);
                cursor += advance;
            }
            RewriteResult::Copy(advance) => {
                for i in 0..advance {
                    let src_idx = cursor + i;
                    result.push(instrs[src_idx].clone());
                }
                cursor += advance;
            }
        }
    }

    result
}

/// Repeat a rewrite pass until the block stops shrinking (fixed-point).
///
/// The shrink-based stop criterion matches the historical interpreter optimizer:
/// current rewrites only justify another pass when they reduce instruction
/// count and may expose a new adjacent peephole opportunity.
fn optimize_until_fixpoint<R: FbcReal>(
    mut block: FbcBlock<R>,
    rewrite: impl Fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>,
) -> FbcBlock<R> {
    loop {
        let old_size = block.len();
        block = apply_rewriter(&block, &rewrite);
        if block.len() >= old_size {
            break;
        }
    }
    block
}

/// Recursively optimize a block and all its sub-blocks.
///
/// This is the equivalent of C++ `optimize_aux`: it traverses the block,
/// recursively optimizing sub-blocks of control-flow instructions, then
/// applies the rewrite pass on the current block.
///
/// Only bytecode sub-block references are traversed here. Factory-level block
/// slots are optimized by the caller (`FbcDspFactory::optimize`).
fn optimize_recursive<R: FbcReal>(
    arena: &mut FbcBlockArena<R>,
    block_id: BlockId,
    rewrite: &(impl Fn(&[FbcInstruction<R>], usize) -> RewriteResult<R> + Copy),
) {
    // First pass: recurse into sub-blocks of control-flow instructions.
    let block = arena.get(block_id);
    let len = block.instructions.len();

    // Collect sub-block IDs that need recursive optimization.
    let mut sub_blocks: Vec<BlockId> = Vec::new();
    for i in 0..len {
        let inst = &arena.get(block_id).instructions[i];
        match inst.opcode {
            FbcOpcode::Loop => {
                // branch2 = loop body (optimize), branch1 = init (no optimization per C++)
                if let Some(b2) = inst.branch2 {
                    sub_blocks.push(b2);
                }
            }
            op if op.is_choice() => {
                if let Some(b1) = inst.branch1 {
                    sub_blocks.push(b1);
                }
                if let Some(b2) = inst.branch2 {
                    sub_blocks.push(b2);
                }
            }
            _ => {}
        }
    }

    // Recursively optimize sub-blocks.
    for sub_id in sub_blocks {
        optimize_recursive(arena, sub_id, rewrite);
    }

    // Second pass: apply the rewrite on this block's instructions.
    let block = arena.get(block_id).clone();
    let optimized = optimize_until_fixpoint(block, rewrite);
    *arena.get_mut(block_id) = optimized;
}

// ═══════════════════════════════════════════════════════════════════════════
// Level 1: LoadStore — index folding
// ═══════════════════════════════════════════════════════════════════════════

/// Level 1: Fold `Int32Value(idx) + Load/StoreIndexed*` into simple `Load/Store*`.
fn rewrite_load_store<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> RewriteResult<R> {
    if cursor + 1 < instrs.len() {
        let i1 = &instrs[cursor];
        let i2 = &instrs[cursor + 1];

        if i1.opcode == FbcOpcode::Int32Value {
            let new_offset = i1.int_value + i2.offset1;
            match i2.opcode {
                FbcOpcode::LoadIndexedReal => {
                    return RewriteResult::Emit(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::LoadReal,
                            0,
                            R::default(),
                            new_offset,
                            0,
                        ),
                        2,
                    );
                }
                FbcOpcode::LoadIndexedInt => {
                    return RewriteResult::Emit(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::LoadInt,
                            0,
                            R::default(),
                            new_offset,
                            0,
                        ),
                        2,
                    );
                }
                FbcOpcode::StoreIndexedReal => {
                    return RewriteResult::Emit(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::StoreReal,
                            0,
                            R::default(),
                            new_offset,
                            0,
                        ),
                        2,
                    );
                }
                FbcOpcode::StoreIndexedInt => {
                    return RewriteResult::Emit(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::StoreInt,
                            0,
                            R::default(),
                            new_offset,
                            0,
                        ),
                        2,
                    );
                }
                _ => {}
            }
        }
    }
    RewriteResult::Copy(1)
}

// ═══════════════════════════════════════════════════════════════════════════
// Level 2: Move — load/store fusion
// ═══════════════════════════════════════════════════════════════════════════

/// Level 2: Fuse `Load*/Value* + Store*` into `Move*`/`Store*Value`.
fn rewrite_move<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> RewriteResult<R> {
    if cursor + 1 < instrs.len() {
        let i1 = &instrs[cursor];
        let i2 = &instrs[cursor + 1];

        // LoadReal + StoreReal → MoveReal (note: C++ reverses offsets)
        if i1.opcode == FbcOpcode::LoadReal && i2.opcode == FbcOpcode::StoreReal {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::MoveReal,
                    0,
                    R::default(),
                    i2.offset1, // destination
                    i1.offset1, // source
                ),
                2,
            );
        }
        // LoadInt + StoreInt → MoveInt
        if i1.opcode == FbcOpcode::LoadInt && i2.opcode == FbcOpcode::StoreInt {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::MoveInt,
                    0,
                    R::default(),
                    i2.offset1,
                    i1.offset1,
                ),
                2,
            );
        }
        // RealValue + StoreReal → StoreRealValue
        if i1.opcode == FbcOpcode::RealValue && i2.opcode == FbcOpcode::StoreReal {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::StoreRealValue,
                    0,
                    i1.real_value,
                    i2.offset1,
                    0,
                ),
                2,
            );
        }
        // Int32Value + StoreInt → StoreIntValue
        if i1.opcode == FbcOpcode::Int32Value && i2.opcode == FbcOpcode::StoreInt {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::StoreIntValue,
                    i1.int_value,
                    R::default(),
                    i2.offset1,
                    0,
                ),
                2,
            );
        }
    }
    RewriteResult::Copy(1)
}

// ═══════════════════════════════════════════════════════════════════════════
// Level 3: BlockMove — consecutive moves fusion
// ═══════════════════════════════════════════════════════════════════════════

/// Level 3: Fuse long runs of sequential `MoveReal` into `BlockPairMoveReal`.
fn rewrite_block_move<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> RewriteResult<R> {
    let inst = &instrs[cursor];
    if inst.opcode != FbcOpcode::MoveReal {
        return RewriteResult::Copy(1);
    }

    // Scan for a run of MoveReal where offset1 == offset2 + 1 and
    // consecutive entries step by 2.
    let mut begin_move: i32 = -1;
    let mut end_move: i32 = -1;
    let mut last_offset: i32 = -1;
    let mut count = 0;
    let mut pos = cursor;

    while pos < instrs.len() {
        let cur = &instrs[pos];
        if cur.opcode != FbcOpcode::MoveReal || cur.opcode == FbcOpcode::Return {
            break;
        }
        if cur.offset1 == cur.offset2 + 1 && (last_offset == -1 || cur.offset1 == last_offset + 2) {
            if begin_move == -1 {
                begin_move = cur.offset2;
            }
            last_offset = cur.offset1;
            end_move = cur.offset1;
            count += 1;
            pos += 1;
        } else {
            break;
        }
    }

    if begin_move != -1 && end_move != -1 && (end_move - begin_move) > 4 {
        return RewriteResult::Emit(
            FbcInstruction::with_values_and_offsets(
                FbcOpcode::BlockPairMoveReal,
                0,
                R::default(),
                begin_move,
                end_move,
            ),
            count,
        );
    }

    RewriteResult::Copy(1)
}

// ═══════════════════════════════════════════════════════════════════════════
// Level 4: PairMove — two-move fusion
// ═══════════════════════════════════════════════════════════════════════════

/// Level 4: Fuse two adjacent `MoveReal`/`MoveInt` into `PairMove*`.
fn rewrite_pair_move<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> RewriteResult<R> {
    if cursor + 1 < instrs.len() {
        let i1 = &instrs[cursor];
        let i2 = &instrs[cursor + 1];

        // MoveReal pair: both must be offset1 == offset2 + 1, and
        // i2.offset1 == i1.offset2
        if i1.opcode == FbcOpcode::MoveReal
            && i2.opcode == FbcOpcode::MoveReal
            && i1.offset1 == i1.offset2 + 1
            && i2.offset1 == i2.offset2 + 1
            && i2.offset1 == i1.offset2
        {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::PairMoveReal,
                    0,
                    R::default(),
                    i1.offset1,
                    i2.offset1,
                ),
                2,
            );
        }

        // MoveInt pair
        if i1.opcode == FbcOpcode::MoveInt
            && i2.opcode == FbcOpcode::MoveInt
            && i1.offset1 == i1.offset2 + 1
            && i2.offset1 == i2.offset2 + 1
            && i2.offset1 == i1.offset2
        {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::PairMoveInt,
                    0,
                    R::default(),
                    i1.offset1,
                    i2.offset1,
                ),
                2,
            );
        }
    }
    RewriteResult::Copy(1)
}

// ═══════════════════════════════════════════════════════════════════════════
// Level 5: Cast — load+cast fusion
// ═══════════════════════════════════════════════════════════════════════════

/// Level 5: Fuse `LoadInt + CastReal` → `CastRealHeap`, etc.
fn rewrite_cast<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> RewriteResult<R> {
    if cursor + 1 < instrs.len() {
        let i1 = &instrs[cursor];
        let i2 = &instrs[cursor + 1];

        if i1.opcode == FbcOpcode::LoadInt && i2.opcode == FbcOpcode::CastReal {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::CastRealHeap,
                    0,
                    R::default(),
                    i1.offset1,
                    0,
                ),
                2,
            );
        }
        if i1.opcode == FbcOpcode::LoadReal && i2.opcode == FbcOpcode::CastInt {
            return RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    FbcOpcode::CastIntHeap,
                    0,
                    R::default(),
                    i1.offset1,
                    0,
                ),
                2,
            );
        }
    }
    RewriteResult::Copy(1)
}

// ═══════════════════════════════════════════════════════════════════════════
// Level 6: Math — the big one
// ═══════════════════════════════════════════════════════════════════════════

/// Level 6: Fuse load/value + math operations, constant fold, and cast specialize.
///
/// This combines the C++ `FBCInstructionMathOptimizer`, `FBCInstructionMathSpecializer`,
/// and `FBCInstructionCastSpecializer` into a single rewrite pass.
fn rewrite_math<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> RewriteResult<R> {
    // Try 3-instruction patterns first, then 2-instruction patterns.
    if let Some(result) = try_math_3(instrs, cursor) {
        return result;
    }
    if let Some(result) = try_math_2(instrs, cursor) {
        return result;
    }
    RewriteResult::Copy(1)
}

/// Try to match and rewrite a 3-instruction pattern at the cursor.
fn try_math_3<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> Option<RewriteResult<R>> {
    if cursor + 2 >= instrs.len() {
        return None;
    }
    let i1 = &instrs[cursor];
    let i2 = &instrs[cursor + 1];
    let i3 = &instrs[cursor + 2];

    // ── Constant folding (MathSpecializer) ────────────────────────────

    // RealValue + RealValue + math → RealValue (constant fold)
    if i1.opcode == FbcOpcode::RealValue && i2.opcode == FbcOpcode::RealValue && i3.opcode.is_math()
    {
        if let Some(inst) = fold_binary_real::<R>(i1.real_value, i2.real_value, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // Int32Value + Int32Value + math → Int32Value (constant fold)
    if i1.opcode == FbcOpcode::Int32Value
        && i2.opcode == FbcOpcode::Int32Value
        && i3.opcode.is_math()
    {
        if let Some(inst) = fold_binary_int::<R>(i1.int_value, i2.int_value, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // RealValue + RealValue + extended binary math → RealValue
    if i1.opcode == FbcOpcode::RealValue
        && i2.opcode == FbcOpcode::RealValue
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(inst) = fold_ext_binary_real::<R>(i1.real_value, i2.real_value, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // Int32Value + Int32Value + extended binary math → Int32Value
    if i1.opcode == FbcOpcode::Int32Value
        && i2.opcode == FbcOpcode::Int32Value
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(inst) = fold_ext_binary_int::<R>(i1.int_value, i2.int_value, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // ── Identity/annihilator patterns (MathSpecializer) ───────────────

    // RealValue + LoadReal + math → identity/annihilator
    if i1.opcode == FbcOpcode::RealValue && i2.opcode == FbcOpcode::LoadReal && i3.opcode.is_math()
    {
        // Note: i1 is TOS (pushed first = deeper), i2 is second push = closer to TOS
        // In C++ convention: inst1=deeper, inst2=closer to top
        // rewriteBinaryRealMath2: inst1=RealValue, inst2=LoadReal
        if let Some(inst) = identity_real_value_load::<R>(i1.real_value, i2.offset1, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // LoadReal + RealValue + math → identity/annihilator
    if i1.opcode == FbcOpcode::LoadReal && i2.opcode == FbcOpcode::RealValue && i3.opcode.is_math()
    {
        if let Some(inst) = identity_load_real_value::<R>(i1.offset1, i2.real_value, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // Int32Value + LoadInt + math → identity/annihilator
    if i1.opcode == FbcOpcode::Int32Value && i2.opcode == FbcOpcode::LoadInt && i3.opcode.is_math()
    {
        if let Some(inst) = identity_int_value_load::<R>(i1.int_value, i2.offset1, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // LoadInt + Int32Value + math → identity/annihilator
    if i1.opcode == FbcOpcode::LoadInt && i2.opcode == FbcOpcode::Int32Value && i3.opcode.is_math()
    {
        if let Some(inst) = identity_load_int_value::<R>(i1.offset1, i2.int_value, i3.opcode) {
            return Some(RewriteResult::Emit(inst, 3));
        }
    }

    // ── Heap fusion (3 instructions) ──────────────────────────────────

    // LoadReal + LoadReal + math → HeapMath
    if i1.opcode == FbcOpcode::LoadReal && i2.opcode == FbcOpcode::LoadReal && i3.opcode.is_math() {
        if let Some(heap_op) = i3.opcode.to_heap() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    heap_op,
                    0,
                    R::default(),
                    i2.offset1,
                    i1.offset1,
                ),
                3,
            ));
        }
    }

    // LoadInt + LoadInt + math → HeapMath
    if i1.opcode == FbcOpcode::LoadInt && i2.opcode == FbcOpcode::LoadInt && i3.opcode.is_math() {
        if let Some(heap_op) = i3.opcode.to_heap() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    heap_op,
                    0,
                    R::default(),
                    i2.offset1,
                    i1.offset1,
                ),
                3,
            ));
        }
    }

    // LoadReal + LoadReal + extended binary math → HeapMath
    if i1.opcode == FbcOpcode::LoadReal
        && i2.opcode == FbcOpcode::LoadReal
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(heap_op) = i3.opcode.to_heap() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    heap_op,
                    0,
                    R::default(),
                    i2.offset1,
                    i1.offset1,
                ),
                3,
            ));
        }
    }

    // LoadInt + LoadInt + extended binary math → HeapMath
    if i1.opcode == FbcOpcode::LoadInt
        && i2.opcode == FbcOpcode::LoadInt
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(heap_op) = i3.opcode.to_heap() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    heap_op,
                    0,
                    R::default(),
                    i2.offset1,
                    i1.offset1,
                ),
                3,
            ));
        }
    }

    // ── Value fusion (3 instructions) ─────────────────────────────────

    // LoadReal + RealValue + math → Value
    if i1.opcode == FbcOpcode::LoadReal && i2.opcode == FbcOpcode::RealValue && i3.opcode.is_math()
    {
        if let Some(value_op) = i3.opcode.to_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(value_op, 0, i2.real_value, i1.offset1, 0),
                3,
            ));
        }
    }

    // RealValue + LoadReal + math → ValueInvert
    if i1.opcode == FbcOpcode::RealValue && i2.opcode == FbcOpcode::LoadReal && i3.opcode.is_math()
    {
        if let Some(invert_op) = i3.opcode.to_value_invert() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(invert_op, 0, i1.real_value, i2.offset1, 0),
                3,
            ));
        }
    }

    // LoadInt + Int32Value + math → Value
    if i1.opcode == FbcOpcode::LoadInt && i2.opcode == FbcOpcode::Int32Value && i3.opcode.is_math()
    {
        if let Some(value_op) = i3.opcode.to_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    value_op,
                    i2.int_value,
                    R::default(),
                    i1.offset1,
                    0,
                ),
                3,
            ));
        }
    }

    // Int32Value + LoadInt + math → ValueInvert
    if i1.opcode == FbcOpcode::Int32Value && i2.opcode == FbcOpcode::LoadInt && i3.opcode.is_math()
    {
        if let Some(invert_op) = i3.opcode.to_value_invert() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    invert_op,
                    i1.int_value,
                    R::default(),
                    i2.offset1,
                    0,
                ),
                3,
            ));
        }
    }

    // ── Extended binary value fusion ──────────────────────────────────

    // LoadReal + RealValue + ext binary → Value
    if i1.opcode == FbcOpcode::LoadReal
        && i2.opcode == FbcOpcode::RealValue
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(value_op) = i3.opcode.to_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(value_op, 0, i2.real_value, i1.offset1, 0),
                3,
            ));
        }
    }

    // RealValue + LoadReal + ext binary → ValueInvert
    if i1.opcode == FbcOpcode::RealValue
        && i2.opcode == FbcOpcode::LoadReal
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(invert_op) = i3.opcode.to_value_invert() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(invert_op, 0, i1.real_value, i2.offset1, 0),
                3,
            ));
        }
    }

    // LoadInt + Int32Value + ext binary → Value
    if i1.opcode == FbcOpcode::LoadInt
        && i2.opcode == FbcOpcode::Int32Value
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(value_op) = i3.opcode.to_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    value_op,
                    i2.int_value,
                    R::default(),
                    i1.offset1,
                    0,
                ),
                3,
            ));
        }
    }

    // Int32Value + LoadInt + ext binary → ValueInvert
    if i1.opcode == FbcOpcode::Int32Value
        && i2.opcode == FbcOpcode::LoadInt
        && i3.opcode.is_extended_binary_math()
    {
        if let Some(invert_op) = i3.opcode.to_value_invert() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(
                    invert_op,
                    i1.int_value,
                    R::default(),
                    i2.offset1,
                    0,
                ),
                3,
            ));
        }
    }

    None
}

/// Try to match and rewrite a 2-instruction pattern at the cursor.
fn try_math_2<R: FbcReal>(instrs: &[FbcInstruction<R>], cursor: usize) -> Option<RewriteResult<R>> {
    if cursor + 1 >= instrs.len() {
        return None;
    }
    let i1 = &instrs[cursor];
    let i2 = &instrs[cursor + 1];

    // ── Cast specializer ──────────────────────────────────────────────

    // Int32Value + CastReal → RealValue
    if i1.opcode == FbcOpcode::Int32Value && i2.opcode == FbcOpcode::CastReal {
        return Some(RewriteResult::Emit(
            FbcInstruction::with_values(FbcOpcode::RealValue, 0, R::from_i32(i1.int_value)),
            2,
        ));
    }

    // RealValue + CastInt → Int32Value
    if i1.opcode == FbcOpcode::RealValue && i2.opcode == FbcOpcode::CastInt {
        return Some(RewriteResult::Emit(
            FbcInstruction::with_values(
                FbcOpcode::Int32Value,
                i1.real_value.to_i32(),
                R::default(),
            ),
            2,
        ));
    }

    // ── Unary math constant fold ──────────────────────────────────────

    // RealValue + extended unary → RealValue
    if i1.opcode == FbcOpcode::RealValue && i2.opcode.is_extended_unary_math() {
        if let Some(inst) = fold_unary_real::<R>(i1.real_value, i2.opcode) {
            return Some(RewriteResult::Emit(inst, 2));
        }
    }

    // Int32Value + Abs → Int32Value
    if i1.opcode == FbcOpcode::Int32Value && i2.opcode == FbcOpcode::Abs {
        return Some(RewriteResult::Emit(
            FbcInstruction::with_values(FbcOpcode::Int32Value, i1.int_value.abs(), R::default()),
            2,
        ));
    }

    // ── Stack fusion (2 instructions) ─────────────────────────────────

    // LoadReal/LoadInt + math → Stack version
    if (i1.opcode == FbcOpcode::LoadReal || i1.opcode == FbcOpcode::LoadInt) && i2.opcode.is_math()
    {
        if let Some(stack_op) = i2.opcode.to_stack() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(stack_op, 0, R::default(), i1.offset1, 0),
                2,
            ));
        }
    }

    // RealValue + math → StackValue version
    if i1.opcode == FbcOpcode::RealValue && i2.opcode.is_math() {
        if let Some(sv_op) = i2.opcode.to_stack_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values(sv_op, 0, i1.real_value),
                2,
            ));
        }
    }

    // Int32Value + math → StackValue version
    if i1.opcode == FbcOpcode::Int32Value && i2.opcode.is_math() {
        if let Some(sv_op) = i2.opcode.to_stack_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values(sv_op, i1.int_value, R::default()),
                2,
            ));
        }
    }

    // ── Extended binary stack fusion ──────────────────────────────────

    // LoadReal/LoadInt + ext binary → Stack version
    if (i1.opcode == FbcOpcode::LoadReal || i1.opcode == FbcOpcode::LoadInt)
        && i2.opcode.is_extended_binary_math()
    {
        if let Some(stack_op) = i2.opcode.to_stack() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(stack_op, 0, R::default(), i1.offset1, 0),
                2,
            ));
        }
    }

    // RealValue + ext binary → StackValue version
    if i1.opcode == FbcOpcode::RealValue && i2.opcode.is_extended_binary_math() {
        if let Some(sv_op) = i2.opcode.to_stack_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values(sv_op, 0, i1.real_value),
                2,
            ));
        }
    }

    // Int32Value + ext binary → StackValue version
    if i1.opcode == FbcOpcode::Int32Value && i2.opcode.is_extended_binary_math() {
        if let Some(sv_op) = i2.opcode.to_stack_value() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values(sv_op, i1.int_value, R::default()),
                2,
            ));
        }
    }

    // ── Extended unary heap fusion ────────────────────────────────────

    // LoadReal + extended unary → Heap version
    if i1.opcode == FbcOpcode::LoadReal && i2.opcode.is_extended_unary_math() {
        if let Some(heap_op) = i2.opcode.to_heap() {
            return Some(RewriteResult::Emit(
                FbcInstruction::with_values_and_offsets(heap_op, 0, R::default(), i1.offset1, 0),
                2,
            ));
        }
    }

    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Constant folding helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Fold `RealValue(v2) OP RealValue(v1)` (note: v1 is TOS in C++ convention).
fn fold_binary_real<R: FbcReal>(v1: R, v2: R, op: FbcOpcode) -> Option<FbcInstruction<R>> {
    // C++ convention: inst2 OP inst1 (inst1 = first pushed = deeper, inst2 = TOS)
    // So the operation is v2 OP v1
    match op {
        FbcOpcode::AddReal => Some(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            v2 + v1,
        )),
        FbcOpcode::SubReal => Some(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            v2 - v1,
        )),
        FbcOpcode::MultReal => Some(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            v2 * v1,
        )),
        FbcOpcode::DivReal => Some(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            v2 / v1,
        )),
        FbcOpcode::RemReal => Some(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            v2.fbc_remainder(v1),
        )),
        FbcOpcode::GTReal => Some(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            if v2 > v1 { 1 } else { 0 },
            R::default(),
        )),
        FbcOpcode::LTReal => Some(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            if v2 < v1 { 1 } else { 0 },
            R::default(),
        )),
        FbcOpcode::GEReal => Some(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            if v2 >= v1 { 1 } else { 0 },
            R::default(),
        )),
        FbcOpcode::LEReal => Some(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            if v2 <= v1 { 1 } else { 0 },
            R::default(),
        )),
        FbcOpcode::EQReal => Some(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            if v2 == v1 { 1 } else { 0 },
            R::default(),
        )),
        FbcOpcode::NEReal => Some(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            if v2 != v1 { 1 } else { 0 },
            R::default(),
        )),
        _ => None,
    }
}

/// Fold `Int32Value(v2) OP Int32Value(v1)`.
fn fold_binary_int<R: FbcReal>(v1: i32, v2: i32, op: FbcOpcode) -> Option<FbcInstruction<R>> {
    let i = |val: i32| FbcInstruction::with_values(FbcOpcode::Int32Value, val, R::default());
    match op {
        FbcOpcode::AddInt => Some(i(v2.wrapping_add(v1))),
        FbcOpcode::SubInt => Some(i(v2.wrapping_sub(v1))),
        FbcOpcode::MultInt => Some(i(v2.wrapping_mul(v1))),
        FbcOpcode::DivInt => {
            if v1 != 0 {
                Some(i(v2.wrapping_div(v1)))
            } else {
                None
            }
        }
        FbcOpcode::RemInt => {
            if v1 != 0 {
                Some(i(v2.wrapping_rem(v1)))
            } else {
                None
            }
        }
        FbcOpcode::LshInt => Some(i(v2.wrapping_shl(v1 as u32))),
        FbcOpcode::ARshInt => Some(i(v2.wrapping_shr(v1 as u32))),
        FbcOpcode::GTInt => Some(i(if v2 > v1 { 1 } else { 0 })),
        FbcOpcode::LTInt => Some(i(if v2 < v1 { 1 } else { 0 })),
        FbcOpcode::GEInt => Some(i(if v2 >= v1 { 1 } else { 0 })),
        FbcOpcode::LEInt => Some(i(if v2 <= v1 { 1 } else { 0 })),
        FbcOpcode::EQInt => Some(i(if v2 == v1 { 1 } else { 0 })),
        FbcOpcode::NEInt => Some(i(if v2 != v1 { 1 } else { 0 })),
        FbcOpcode::ANDInt => Some(i(v2 & v1)),
        FbcOpcode::ORInt => Some(i(v2 | v1)),
        FbcOpcode::XORInt => Some(i(v2 ^ v1)),
        _ => None,
    }
}

/// Fold extended binary real: `RealValue(v2) EXT_OP RealValue(v1)`.
fn fold_ext_binary_real<R: FbcReal>(v1: R, v2: R, op: FbcOpcode) -> Option<FbcInstruction<R>> {
    let r = |val: R| FbcInstruction::with_values(FbcOpcode::RealValue, 0, val);
    match op {
        FbcOpcode::Atan2f => Some(r(v2.fbc_atan2(v1))),
        FbcOpcode::Fmodf => Some(r(v2.fbc_fmod(v1))),
        FbcOpcode::Powf => Some(r(v2.fbc_pow(v1))),
        // `v2` is the operand the executor pops first: folding must give what
        // executing gives, including on a NaN, so both use C++ `std::max`/`std::min`.
        FbcOpcode::Maxf => Some(r(crate::backends::interp::real::cpp_max(v2, v1))),
        FbcOpcode::Minf => Some(r(crate::backends::interp::real::cpp_min(v2, v1))),
        _ => None,
    }
}

/// Fold extended binary int: `Int32Value(v2) EXT_OP Int32Value(v1)`.
fn fold_ext_binary_int<R: FbcReal>(v1: i32, v2: i32, op: FbcOpcode) -> Option<FbcInstruction<R>> {
    let i = |val: i32| FbcInstruction::with_values(FbcOpcode::Int32Value, val, R::default());
    match op {
        FbcOpcode::Max => Some(i(v2.max(v1))),
        FbcOpcode::Min => Some(i(v2.min(v1))),
        _ => None,
    }
}

/// Fold unary real: `RealValue(v) UNARY_OP → RealValue(f(v))`.
fn fold_unary_real<R: FbcReal>(v: R, op: FbcOpcode) -> Option<FbcInstruction<R>> {
    let r = |val: R| FbcInstruction::with_values(FbcOpcode::RealValue, 0, val);
    match op {
        FbcOpcode::Absf => Some(r(v.fbc_absf())),
        FbcOpcode::Acosf => Some(r(v.fbc_acos())),
        FbcOpcode::Acoshf => Some(r(v.fbc_acosh())),
        FbcOpcode::Asinf => Some(r(v.fbc_asin())),
        FbcOpcode::Asinhf => Some(r(v.fbc_asinh())),
        FbcOpcode::Atanf => Some(r(v.fbc_atan())),
        FbcOpcode::Atanhf => Some(r(v.fbc_atanh())),
        FbcOpcode::Ceilf => Some(r(v.fbc_ceil())),
        FbcOpcode::Cosf => Some(r(v.fbc_cos())),
        FbcOpcode::Coshf => Some(r(v.fbc_cosh())),
        FbcOpcode::Expf => Some(r(v.fbc_exp())),
        FbcOpcode::Floorf => Some(r(v.fbc_floor())),
        FbcOpcode::Logf => Some(r(v.fbc_log())),
        FbcOpcode::Log10f => Some(r(v.fbc_log10())),
        FbcOpcode::Rintf => Some(r(v.fbc_rint())),
        FbcOpcode::Roundf => Some(r(v.fbc_round())),
        FbcOpcode::Sinf => Some(r(v.fbc_sin())),
        FbcOpcode::Sinhf => Some(r(v.fbc_sinh())),
        FbcOpcode::Sqrtf => Some(r(v.fbc_sqrt())),
        FbcOpcode::Tanf => Some(r(v.fbc_tan())),
        FbcOpcode::Tanhf => Some(r(v.fbc_tanh())),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Identity / annihilator helpers
// ═══════════════════════════════════════════════════════════════════════════

/// `RealValue(val) + LoadReal(off) + OP` — identity/annihilator patterns.
/// C++ rewriteBinaryRealMath2: val on stack bottom, LoadReal on top.
fn identity_real_value_load<R: FbcReal>(
    val: R,
    load_offset: i32,
    op: FbcOpcode,
) -> Option<FbcInstruction<R>> {
    let zero = R::from_i32(0);
    let one = R::from_i32(1);
    match op {
        FbcOpcode::AddReal | FbcOpcode::SubReal => {
            if val == zero {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadReal,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        FbcOpcode::MultReal => {
            if val == one {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadReal,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else if val == zero {
                Some(FbcInstruction::with_values(FbcOpcode::RealValue, 0, zero))
            } else {
                None
            }
        }
        FbcOpcode::DivReal => {
            if val == one {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadReal,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `LoadReal(off) + RealValue(val) + OP` — identity/annihilator patterns.
/// C++ rewriteBinaryRealMath3: LoadReal on stack bottom, val on top.
/// Sub and div not rewritten here (non-commutative).
fn identity_load_real_value<R: FbcReal>(
    load_offset: i32,
    val: R,
    op: FbcOpcode,
) -> Option<FbcInstruction<R>> {
    let zero = R::from_i32(0);
    let one = R::from_i32(1);
    match op {
        FbcOpcode::AddReal => {
            if val == zero {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadReal,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        FbcOpcode::MultReal => {
            if val == one {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadReal,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else if val == zero {
                Some(FbcInstruction::with_values(FbcOpcode::RealValue, 0, zero))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `Int32Value(val) + LoadInt(off) + OP` — identity/annihilator patterns.
fn identity_int_value_load<R: FbcReal>(
    val: i32,
    load_offset: i32,
    op: FbcOpcode,
) -> Option<FbcInstruction<R>> {
    match op {
        FbcOpcode::AddInt | FbcOpcode::SubInt => {
            if val == 0 {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInt,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        FbcOpcode::MultInt => {
            if val == 1 {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInt,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else if val == 0 {
                Some(FbcInstruction::with_values(
                    FbcOpcode::Int32Value,
                    0,
                    R::default(),
                ))
            } else {
                None
            }
        }
        FbcOpcode::DivInt => {
            if val == 1 {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInt,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `LoadInt(off) + Int32Value(val) + OP` — identity/annihilator patterns.
fn identity_load_int_value<R: FbcReal>(
    load_offset: i32,
    val: i32,
    op: FbcOpcode,
) -> Option<FbcInstruction<R>> {
    match op {
        FbcOpcode::AddInt => {
            if val == 0 {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInt,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        FbcOpcode::MultInt => {
            if val == 1 {
                Some(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInt,
                    0,
                    R::default(),
                    load_offset,
                    0,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════════════════════

/// Optimize an FBC block in-place using the specified optimization level range.
///
/// Applies levels `min_level..=max_level` sequentially. Each level runs a
/// peephole rewrite pass with fixed-point iteration, recursing into sub-blocks
/// referenced by control-flow instructions.
///
/// # Source provenance (C++)
/// - `FBCInstructionOptimizer::optimizeBlock()` in `interpreter_optimizer.hh`
///
/// # Panics
/// Panics if `min_level` or `max_level` is 0 or exceeds [`MAX_OPT_LEVEL`].
pub fn optimize_block<R: FbcReal>(
    arena: &mut FbcBlockArena<R>,
    block_id: BlockId,
    min_level: u8,
    max_level: u8,
) {
    assert!(min_level >= 1 && max_level <= MAX_OPT_LEVEL);

    if min_level <= 1 && 1 <= max_level {
        optimize_recursive(
            arena,
            block_id,
            &(rewrite_load_store::<R> as fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>),
        );
    }
    if min_level <= 2 && 2 <= max_level {
        optimize_recursive(
            arena,
            block_id,
            &(rewrite_move::<R> as fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>),
        );
    }
    if min_level <= 3 && 3 <= max_level {
        optimize_recursive(
            arena,
            block_id,
            &(rewrite_block_move::<R> as fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>),
        );
    }
    if min_level <= 4 && 4 <= max_level {
        optimize_recursive(
            arena,
            block_id,
            &(rewrite_pair_move::<R> as fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>),
        );
    }
    if min_level <= 5 && 5 <= max_level {
        optimize_recursive(
            arena,
            block_id,
            &(rewrite_cast::<R> as fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>),
        );
    }
    if min_level <= 6 && 6 <= max_level {
        optimize_recursive(
            arena,
            block_id,
            &(rewrite_math::<R> as fn(&[FbcInstruction<R>], usize) -> RewriteResult<R>),
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests;
