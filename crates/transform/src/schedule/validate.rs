//! Pre-flight validation shared by all four scheduling strategies (plan
//! §4.1, §5.4). The C++ originals (`compiler/DirectedGraph/Schedule.hh`)
//! assume a DAG and never check; this runs once, independently of the
//! selected strategy, so a malformed graph is rejected identically
//! regardless of which of the four algorithms was requested — and so a
//! genuine cycle/self-edge test can assert the same error for every
//! [`crate::schedule::SchedulingStrategy`].

use std::collections::VecDeque;

use ahash::{AHashMap, AHashSet};

use super::dag::ScheduleDag;
use super::error::ScheduleError;

/// Rejects a self-edge or a longer cycle before any strategy runs; on
/// success, returns `dag.nodes()` so the caller reuses this one query
/// instead of re-asking the adapter.
///
/// A self-edge is reported for the first offending node in `nodes()` order.
/// A longer cycle reports every node Kahn peeling could not remove — the
/// cycle's own members plus every node that transitively depends on one —
/// stable-sorted ascending.
pub(super) fn ensure_schedulable<D: ScheduleDag>(
    dag: &D,
) -> Result<Vec<D::Node>, ScheduleError<D::Node>> {
    let nodes = dag.nodes();

    for &n in &nodes {
        if dag.dependencies(n).into_iter().any(|d| d == n) {
            return Err(ScheduleError::SelfEdge { node: n });
        }
    }

    // Kahn peeling on the "must be scheduled before" relation: a node is
    // ready once every one of its (deduplicated) dependencies has already
    // been peeled. Nodes peeling can never reach are exactly the
    // unschedulable set: cycle members plus everything that transitively
    // consumes them.
    let mut pending: AHashMap<D::Node, AHashSet<D::Node>> = AHashMap::new();
    let mut successors: AHashMap<D::Node, Vec<D::Node>> = AHashMap::new();
    for &n in &nodes {
        let deps: AHashSet<D::Node> = dag.dependencies(n).into_iter().collect();
        for &d in &deps {
            successors.entry(d).or_default().push(n);
        }
        pending.insert(n, deps);
    }

    let mut ready: VecDeque<D::Node> = nodes
        .iter()
        .copied()
        .filter(|n| pending.get(n).is_some_and(|deps| deps.is_empty()))
        .collect();
    let mut removed: AHashSet<D::Node> = AHashSet::new();
    while let Some(n) = ready.pop_front() {
        if !removed.insert(n) {
            continue;
        }
        if let Some(succs) = successors.get(&n) {
            for &s in succs {
                if removed.contains(&s) {
                    continue;
                }
                if let Some(set) = pending.get_mut(&s) {
                    set.remove(&n);
                    if set.is_empty() {
                        ready.push_back(s);
                    }
                }
            }
        }
    }

    if removed.len() == nodes.len() {
        Ok(nodes)
    } else {
        let mut remaining: Vec<D::Node> = nodes
            .iter()
            .copied()
            .filter(|n| !removed.contains(n))
            .collect();
        remaining.sort();
        Err(ScheduleError::Cycle { remaining })
    }
}
