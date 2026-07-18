//! Item 3: `verify_schedule` rejection scenarios — a consumer scheduled
//! before its dependency, a duplicate, a missing node, an extra node, and a
//! malformed adapter whose `nodes()` itself repeats a node (the Lean
//! `coversB` `noDuplicatesB graph.nodes` clause).

use super::fixtures::diamond;
use crate::schedule::{ScheduleDag, VerifyError, verify_schedule};

#[test]
fn rejects_a_consumer_before_its_dependency() {
    let g = diamond();
    // 1 depends on 0, but is placed before it.
    let err = verify_schedule(&g, &[1, 0, 2, 3]).unwrap_err();
    assert_eq!(
        err,
        VerifyError::OutOfOrder {
            consumer: 1,
            dependency: 0
        }
    );
}

#[test]
fn rejects_a_duplicate() {
    let g = diamond();
    let err = verify_schedule(&g, &[0, 1, 1, 2, 3]).unwrap_err();
    assert_eq!(err, VerifyError::Duplicate { node: 1 });
}

#[test]
fn rejects_a_missing_node() {
    let g = diamond();
    // 2 is omitted entirely.
    let err = verify_schedule(&g, &[0, 1, 3]).unwrap_err();
    assert_eq!(err, VerifyError::Missing { node: 2 });
}

#[test]
fn rejects_an_extra_node() {
    let g = diamond();
    // 99 is not a node of the diamond.
    let err = verify_schedule(&g, &[0, 1, 2, 3, 99]).unwrap_err();
    assert_eq!(err, VerifyError::Extra { node: 99 });
}

#[test]
fn accepts_the_empty_schedule_of_the_empty_graph() {
    use super::fixtures::VecDag;
    let g = VecDag::new();
    assert!(verify_schedule(&g, &[]).is_ok());
}

/// A deliberately malformed [`ScheduleDag`] whose `nodes()` repeats a node —
/// impossible to build through `VecDag` (its `BTreeMap` keys are unique by
/// construction), so this dedicated double is the demonstrated rejecting
/// mutation for the `noDuplicatesB graph.nodes` clause: a checker without
/// one is not a trust boundary.
struct DuplicateNodesDag;

impl ScheduleDag for DuplicateNodesDag {
    type Node = u32;

    fn nodes(&self) -> Vec<u32> {
        // 1 appears twice; the edge list is otherwise a perfectly valid
        // 2-node chain, so only the graph-side duplicate check can reject.
        vec![0, 1, 1]
    }

    fn dependencies(&self, n: u32) -> Vec<u32> {
        if n == 1 { vec![0] } else { vec![] }
    }
}

#[test]
fn rejects_a_graph_whose_node_list_repeats_a_node() {
    let g = DuplicateNodesDag;
    // The candidate order is the graph's own deduplicated node set in a
    // dependency-respecting order — exactly the order that would slip
    // through a checker missing the `noDuplicatesB graph.nodes` clause.
    let err = verify_schedule(&g, &[0, 1]).unwrap_err();
    assert_eq!(err, VerifyError::DuplicateGraphNode { node: 1 });

    // Deterministic regardless of the candidate order: the graph-side check
    // runs first, so even an obviously wrong order reports the same error.
    let err = verify_schedule(&g, &[1, 0]).unwrap_err();
    assert_eq!(err, VerifyError::DuplicateGraphNode { node: 1 });
    let err = verify_schedule(&g, &[]).unwrap_err();
    assert_eq!(err, VerifyError::DuplicateGraphNode { node: 1 });
}
