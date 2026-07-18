//! Item 1/2: literal, by-hand-derived snapshots of each strategy on each
//! fixture, each cross-checked with [`verify_schedule`] (item 2: every
//! produced order must itself be accepted by the independent checker).
//!
//! Every derivation below follows the plan §5.4 pseudocode directly:
//! `roots(G)` = nodes nobody depends on; `DepthFirst` = postorder from
//! `roots(G)`; `BreadthFirst` = sort by `(height, key)` with
//! `height(v) = 1 + max(height(dep))`; `Special` = interleave-and-reverse-
//! dedup of the duplicate root-to-leaf lists; `ReverseBreadthFirst` =
//! `BreadthFirst` on the "users" (reverse) relation, then the whole sequence
//! reversed.

use super::fixtures::{ALL_STRATEGIES, asymmetric_fork_join, chain, diamond, disconnected, ladder};
use crate::schedule::{SchedulingStrategy, schedule, verify_schedule};

fn assert_every_produced_order_verifies(g: &impl crate::schedule::ScheduleDag) {
    for strategy in ALL_STRATEGIES {
        let order = schedule(strategy, g).expect("acyclic fixture schedules");
        assert!(
            verify_schedule(g, &order).is_ok(),
            "{strategy:?} produced an order verify_schedule rejects: {order:?}"
        );
    }
}

#[test]
fn chain_is_a_total_order_for_every_strategy() {
    // 0 <- 1 <- 2 <- 3: no scheduling freedom, so every strategy must agree.
    let g = chain();
    assert_every_produced_order_verifies(&g);
    for strategy in ALL_STRATEGIES {
        assert_eq!(
            schedule(strategy, &g).unwrap(),
            vec![0, 1, 2, 3],
            "{strategy:?}"
        );
    }
}

#[test]
fn diamond_depth_first_matches_the_task_specified_order() {
    // DFS from root 3: dfvisit(3) descends dependencies(3) = [1, 2] first,
    // fully finishing 1's subtree ([0] then 1) before touching 2 (whose only
    // dependency, 0, is already visited and skipped) — postorder [0,1,2,3].
    let g = diamond();
    assert_eq!(
        schedule(SchedulingStrategy::DepthFirst, &g).unwrap(),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn diamond_matches_the_lean_diamond_graph_fixture() {
    // porting/vector-mode-scheduling-formal-spec.lean `diamondGraph`: both
    // [0,1,2,3] and [0,2,1,3] are proved `ValidSchedule`; [1,0,2,3] is
    // proved invalid (consumer 1 precedes its dependency 0). This is a
    // cross-check against that independently authored fixture, not just
    // this port's own algorithm.
    let g = diamond();
    assert!(verify_schedule(&g, &[0, 1, 2, 3]).is_ok());
    assert!(verify_schedule(&g, &[0, 2, 1, 3]).is_ok());
    assert!(verify_schedule(&g, &[1, 0, 2, 3]).is_err());
}

#[test]
fn diamond_all_four_strategies() {
    let g = diamond();
    assert_every_produced_order_verifies(&g);

    // DepthFirst, BreadthFirst: h(0)=0, h(1)=h(2)=1, h(3)=2 — the ascending
    // key tie-break at height 1 gives [1, 2] either way, so BFS coincides
    // with the DFS postorder here.
    assert_eq!(
        schedule(SchedulingStrategy::DepthFirst, &g).unwrap(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(
        schedule(SchedulingStrategy::BreadthFirst, &g).unwrap(),
        vec![0, 1, 2, 3]
    );

    // Special: rec(0)=[0]; rec(1)=[1,0]; rec(2)=[2,0];
    // rec(3) = [3] ++ interleave(rec(1), rec(2))
    //        = [3] ++ interleave([1,0],[2,0]) = [3,1,2,0,0].
    // raw(G) = rec(3) (only root). reverse = [0,0,2,1,3]; first-occurrence
    // scan keeps 0, 2, 1, 3 in that order.
    assert_eq!(
        schedule(SchedulingStrategy::Special, &g).unwrap(),
        vec![0, 2, 1, 3]
    );

    // ReverseBreadthFirst: users(0)={1,2}, users(1)={3}, users(2)={3},
    // users(3)={}. r(3)=0, r(1)=r(2)=1, r(0)=2. Sorted by (r,key):
    // [3,1,2,0]; reversed: [0,2,1,3].
    assert_eq!(
        schedule(SchedulingStrategy::ReverseBreadthFirst, &g).unwrap(),
        vec![0, 2, 1, 3]
    );
}

#[test]
fn asymmetric_fork_join_all_four_strategies_disagree() {
    // deps: 1->[0], 2->[0], 3->[] (bystander leaf), 4->[1,2,3].
    let g = asymmetric_fork_join();
    assert_every_produced_order_verifies(&g);

    // DepthFirst: roots=[4]; dfvisit(4) descends [1,2,3] in order, so 1's
    // subtree (0,1) finishes, then 2's subtree (0 already visited, so just
    // 2), then leaf 3, then 4.
    assert_eq!(
        schedule(SchedulingStrategy::DepthFirst, &g).unwrap(),
        vec![0, 1, 2, 3, 4]
    );

    // BreadthFirst: h(0)=0, h(3)=0, h(1)=h(2)=1, h(4)=2. Height-0 bucket
    // ascending key: [0,3]; height-1: [1,2]; height-2: [4].
    assert_eq!(
        schedule(SchedulingStrategy::BreadthFirst, &g).unwrap(),
        vec![0, 3, 1, 2, 4]
    );

    // Special: rec(0)=[0]; rec(3)=[3]; rec(1)=[1,0]; rec(2)=[2,0];
    // rec(4) = [4] ++ fold(interleave, [], [rec(1), rec(2), rec(3)])
    //   step1: interleave([], [1,0]) = [1,0]
    //   step2: interleave([1,0], [2,0]) = [1,2,0,0]
    //   step3: interleave([1,2,0,0], [3]) = [1,3,2,0,0]
    //   => rec(4) = [4,1,3,2,0,0].
    // raw(G) = rec(4) (only root). reverse = [0,0,2,3,1,4]; first-occurrence
    // scan keeps 0, 2, 3, 1, 4.
    assert_eq!(
        schedule(SchedulingStrategy::Special, &g).unwrap(),
        vec![0, 2, 3, 1, 4]
    );

    // ReverseBreadthFirst: users(0)={1,2}, users(1)=users(2)=users(3)={4},
    // users(4)={}. r(4)=0, r(1)=r(2)=r(3)=1, r(0)=2. Sorted by (r,key):
    // [4,1,2,3,0]; reversed: [0,3,2,1,4].
    assert_eq!(
        schedule(SchedulingStrategy::ReverseBreadthFirst, &g).unwrap(),
        vec![0, 3, 2, 1, 4]
    );
}

#[test]
fn two_disconnected_components_all_four_strategies() {
    // {0,1} (1->0) and {2,3} (3->2), no edge between them. roots=[1,3].
    let g = disconnected();
    assert_every_produced_order_verifies(&g);

    // DepthFirst: root 1 finishes its whole component (0,1) before root 3's
    // component starts (2,3).
    assert_eq!(
        schedule(SchedulingStrategy::DepthFirst, &g).unwrap(),
        vec![0, 1, 2, 3]
    );

    // BreadthFirst: h(0)=h(2)=0, h(1)=h(3)=1. Height buckets, ascending key:
    // [0,2] then [1,3].
    assert_eq!(
        schedule(SchedulingStrategy::BreadthFirst, &g).unwrap(),
        vec![0, 2, 1, 3]
    );

    // Special: rec(0)=[0]; rec(2)=[2]; rec(1)=[1,0]; rec(3)=[3,2].
    // raw(G) = fold(interleave, [], [rec(1), rec(3)]) (roots ascending
    // [1,3]) = interleave([1,0],[3,2]) = [1,3,0,2]. reverse = [2,0,3,1];
    // first-occurrence scan keeps 2, 0, 3, 1 — every node is already
    // distinct, so the scan keeps the whole reversed list.
    assert_eq!(
        schedule(SchedulingStrategy::Special, &g).unwrap(),
        vec![2, 0, 3, 1]
    );

    // ReverseBreadthFirst: users(0)={1}, users(2)={3}, users(1)=users(3)={}.
    // r(1)=r(3)=0, r(0)=r(2)=1. Sorted by (r,key): [1,3,0,2]; reversed:
    // [2,0,3,1]. (Coincides with `Special` here: this fixture's two
    // components are structurally identical, leaving no asymmetry for the
    // two strategies to disagree on — `asymmetric_fork_join` above is the
    // fixture that tells them apart.)
    assert_eq!(
        schedule(SchedulingStrategy::ReverseBreadthFirst, &g).unwrap(),
        vec![2, 0, 3, 1]
    );
}

#[test]
fn path_heavy_shared_dag_all_four_strategies() {
    // ladder(3): layer0={0,1} (no deps); layer1={2,3}, both depending on
    // [0,1]; layer2={4,5}, both depending on [2,3]. roots=[4,5].
    let g = ladder(3);
    assert_every_produced_order_verifies(&g);

    // DepthFirst: root 4 finishes {0,1,2} then {3} (0,1 already visited),
    // then root 5's dependencies {2,3} are already visited, so only 5 is
    // appended.
    assert_eq!(
        schedule(SchedulingStrategy::DepthFirst, &g).unwrap(),
        vec![0, 1, 2, 3, 4, 5]
    );

    // BreadthFirst: h(0)=h(1)=0, h(2)=h(3)=1, h(4)=h(5)=2 — three clean
    // levels, ascending key within each.
    assert_eq!(
        schedule(SchedulingStrategy::BreadthFirst, &g).unwrap(),
        vec![0, 1, 2, 3, 4, 5]
    );

    // Special: rec(0)=[0]; rec(1)=[1];
    // rec(2) = [2] ++ interleave(rec(0), rec(1)) = [2,0,1];
    // rec(3) = [3,0,1] (same shape as rec(2));
    // rec(4) = [4] ++ interleave(rec(2), rec(3))
    //        = [4] ++ interleave([2,0,1],[3,0,1]) = [4,2,3,0,0,1,1];
    // rec(5) = [5,2,3,0,0,1,1] (same shape as rec(4)).
    // raw(G) = interleave(rec(4), rec(5))
    //        = [4,5,2,2,3,3,0,0,0,0,1,1,1,1].
    // reverse = [1,1,1,1,0,0,0,0,3,3,2,2,5,4]; first-occurrence scan keeps
    // 1, 0, 3, 2, 5, 4.
    assert_eq!(
        schedule(SchedulingStrategy::Special, &g).unwrap(),
        vec![1, 0, 3, 2, 5, 4]
    );

    // ReverseBreadthFirst: users(0)=users(1)={2,3}; users(2)=users(3)={4,5};
    // users(4)=users(5)={}. r(4)=r(5)=0, r(2)=r(3)=1, r(0)=r(1)=2. Sorted by
    // (r,key): [4,5,2,3,0,1]; reversed: [1,0,3,2,5,4]. (Coincides with
    // `Special` here — this fixture is fully symmetric between its two
    // rails at every layer; see `asymmetric_fork_join` for a fixture that
    // separates the two strategies.)
    assert_eq!(
        schedule(SchedulingStrategy::ReverseBreadthFirst, &g).unwrap(),
        vec![1, 0, 3, 2, 5, 4]
    );
}
