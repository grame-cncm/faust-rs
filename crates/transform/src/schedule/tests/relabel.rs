//! Item 5: relabelling. Applies an order-preserving bijection to the
//! diamond's node ids (`f(x) = 10x`, so `0,1,2,3 -> 0,10,20,30`, preserving
//! every `Ord` relation) while *also* scrambling the fixture builder's
//! insertion order relative to `fixtures::diamond`. If a strategy secretly
//! depended on raw numeric spacing, on which concrete integers were used, or
//! on the order nodes were first inserted into `VecDag`'s adjacency table
//! (rather than purely on the `Ord`-ranked graph structure), the relabelled
//! schedule would stop being the pointwise image of the original one.

use super::fixtures::{ALL_STRATEGIES, VecDag, diamond};
use crate::schedule::schedule;

fn f(x: u32) -> u32 {
    x * 10
}

/// Same graph as `fixtures::diamond` after applying `f`, built in a
/// different call order (`1`'s edge before `3`'s, `2`'s edge last) so the
/// two constructions do not even visit nodes in the same sequence.
fn relabelled_diamond() -> VecDag {
    VecDag::new()
        .deps(f(1), &[f(0)])
        .deps(f(3), &[f(1), f(2)])
        .deps(f(2), &[f(0)])
}

#[test]
fn relabelling_commutes_with_scheduling_for_every_strategy() {
    let original = diamond();
    let relabelled = relabelled_diamond();

    for strategy in ALL_STRATEGIES {
        let original_order = schedule(strategy, &original).expect("diamond schedules");
        let relabelled_order =
            schedule(strategy, &relabelled).expect("relabelled diamond schedules");
        let expected: Vec<u32> = original_order.iter().copied().map(f).collect();
        assert_eq!(
            relabelled_order, expected,
            "{strategy:?}: relabelling must commute with scheduling"
        );
    }
}
