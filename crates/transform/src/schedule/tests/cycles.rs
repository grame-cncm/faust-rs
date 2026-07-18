//! Item 6: self-edge and cycle diagnostics. All four strategies share one
//! pre-flight check (`validate::ensure_schedulable`), so every strategy must
//! report the identical typed error for the identical malformed graph —
//! this file asserts that uniformity directly, not just "some" strategy.

use super::fixtures::{ALL_STRATEGIES, VecDag};
use crate::schedule::{ScheduleError, schedule};

#[test]
fn self_edge_is_rejected_by_every_strategy() {
    let g = VecDag::new().deps(0, &[0]);
    for strategy in ALL_STRATEGIES {
        let err = schedule(strategy, &g).unwrap_err();
        assert_eq!(err, ScheduleError::SelfEdge { node: 0 }, "{strategy:?}");
    }
}

#[test]
fn self_edge_is_reported_before_an_unrelated_cycle_elsewhere() {
    // node 0 has a self-edge; nodes 1 and 2 form an unrelated 2-cycle.
    // `nodes()` ascending is [0,1,2], so the self-edge on 0 must win.
    let g = VecDag::new().deps(0, &[0]).deps(1, &[2]).deps(2, &[1]);
    for strategy in ALL_STRATEGIES {
        let err = schedule(strategy, &g).unwrap_err();
        assert_eq!(err, ScheduleError::SelfEdge { node: 0 }, "{strategy:?}");
    }
}

#[test]
fn two_cycle_is_rejected_by_every_strategy_with_both_members_remaining() {
    let g = VecDag::new().deps(0, &[1]).deps(1, &[0]);
    for strategy in ALL_STRATEGIES {
        let err = schedule(strategy, &g).unwrap_err();
        assert_eq!(
            err,
            ScheduleError::Cycle {
                remaining: vec![0, 1]
            },
            "{strategy:?}"
        );
    }
}

#[test]
fn three_cycle_is_rejected_by_every_strategy_with_all_members_remaining() {
    let g = VecDag::new().deps(0, &[1]).deps(1, &[2]).deps(2, &[0]);
    for strategy in ALL_STRATEGIES {
        let err = schedule(strategy, &g).unwrap_err();
        assert_eq!(
            err,
            ScheduleError::Cycle {
                remaining: vec![0, 1, 2]
            },
            "{strategy:?}"
        );
    }
}

#[test]
fn a_consumer_of_a_cycle_is_also_unschedulable() {
    // 2 depends on 1, which is inside the {0,1} cycle: 2 can never be peeled
    // either, so `remaining` must include it too.
    let g = VecDag::new().deps(0, &[1]).deps(1, &[0]).deps(2, &[1]);
    for strategy in ALL_STRATEGIES {
        let err = schedule(strategy, &g).unwrap_err();
        assert_eq!(
            err,
            ScheduleError::Cycle {
                remaining: vec![0, 1, 2]
            },
            "{strategy:?}"
        );
    }
}

#[test]
fn a_cycle_does_not_prevent_scheduling_an_unrelated_component() {
    // {0,1} cycle; independent, acyclic node 2. Still an overall error (the
    // graph as a whole is not a DAG), but `remaining` must not blame 2.
    let g = VecDag::new().deps(0, &[1]).deps(1, &[0]).node(2);
    for strategy in ALL_STRATEGIES {
        let err = schedule(strategy, &g).unwrap_err();
        assert_eq!(
            err,
            ScheduleError::Cycle {
                remaining: vec![0, 1]
            },
            "{strategy:?}"
        );
    }
}
