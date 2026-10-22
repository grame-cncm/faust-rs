//! Box DAGs whose expanded tree is exponential: the complexity memo and the
//! bounded printer must stay linear in the number of distinct nodes (C++
//! #1290, `porting/svg-shared-node-folding-port-plan-2026-10-22-en.md`).

use boxes::{
    BoxBuilder, BoxComplexity, BoxId, FloatSize, MAX_BOX_COMPLEXITY, box_complexity, box_pp,
    box_pp_bounded,
};
use tlib::TreeArena;

/// `f(0) = 1`, `f(1) = _`, `f(k) = f(k-1), f(k-2) : +`: each level shares
/// both previous ones, so the expanded tree grows like Fibonacci.
fn fibonacci_dag(arena: &mut TreeArena, k: usize) -> Vec<BoxId> {
    let mut b = BoxBuilder::new(arena);
    let mut levels = vec![b.int(1), b.wire()];
    for i in 2..=k {
        let pair = b.par(levels[i - 1], levels[i - 2]);
        let add = b.add();
        levels.push(b.seq(pair, add));
    }
    levels
}

/// The scoring rules of the nodes above, written again without a memo.
fn naive(arena: &TreeArena, b: BoxId) -> usize {
    match boxes::match_box(arena, b) {
        boxes::BoxMatch::Seq(x, y) | boxes::BoxMatch::Par(x, y) => {
            naive(arena, x) + naive(arena, y)
        }
        boxes::BoxMatch::Wire => 0,
        _ => 1,
    }
}

#[test]
fn the_memo_scores_as_the_unmemoized_definition() {
    let mut arena = TreeArena::new();
    let levels = fibonacci_dag(&mut arena, 20);
    let mut memo = BoxComplexity::new();
    for &level in &levels {
        assert_eq!(memo.of(&arena, level), naive(&arena, level));
        assert_eq!(box_complexity(&arena, level), naive(&arena, level));
    }
}

#[test]
fn complexity_is_linear_in_the_dag_and_saturates() {
    // f(90) expands to about 10^19 boxes: unreachable without the memo, and
    // past MAX_BOX_COMPLEXITY, where it stops
    let mut arena = TreeArena::new();
    let levels = fibonacci_dag(&mut arena, 90);
    assert_eq!(box_complexity(&arena, levels[90]), MAX_BOX_COMPLEXITY);
    assert!(box_complexity(&arena, levels[30]) < MAX_BOX_COMPLEXITY);
}

#[test]
fn a_bounded_print_is_bounded_and_otherwise_box_pp() {
    let mut arena = TreeArena::new();
    let levels = fibonacci_dag(&mut arena, 90);
    // small: the same text as box_pp
    let small = box_pp(&arena, levels[4], 0, FloatSize::Single).unwrap();
    assert_eq!(
        box_pp_bounded(&arena, levels[4], 0, FloatSize::Single, 1024).unwrap(),
        small
    );
    // huge: cut, and computed at once
    let text = box_pp_bounded(&arena, levels[90], 0, FloatSize::Single, 64).unwrap();
    assert!(text.ends_with(" ...") || text.contains("..."), "{text}");
    assert!(text.chars().count() <= 64 + " ...".len(), "{text}");
}
