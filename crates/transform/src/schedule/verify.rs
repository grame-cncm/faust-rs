//! Independent postcondition checker: [`verify_schedule`]. Mirrors the Lean
//! `verifySchedule` / `coversB` / `respectsDependenciesB`
//! (`porting/vector-mode-scheduling-formal-spec.lean`) — **including
//! `coversB`'s `noDuplicatesB graph.nodes` clause**: the graph's own node
//! list must be duplicate-free, not just the candidate order. The Lean spec
//! added that clause deliberately (see its comment above `coversB`): without
//! it, a graph whose node list contained duplicates would be "covered" by
//! its deduplicated order, and this checker would be strictly weaker than
//! the mathematical coverage predicate (and than the R1 "unique nodes"
//! requirement of `porting/lean-rust-certified-porting-plan-2026-07-11-en.md`).
//! Never re-runs a scheduling algorithm — it only checks the candidate order
//! directly against `dag.nodes()` / `dag.dependencies()`.

use ahash::{AHashMap, AHashSet};

use super::dag::ScheduleDag;
use super::error::VerifyError;

/// `dag.nodes()` is duplicate-free, `order` is a duplicate-free permutation
/// of it, and every dependency precedes its consumer (S-Sound + S-Complete,
/// plan §5.4; Lean `coversB` + `respectsDependenciesB`).
///
/// # Errors
/// [`VerifyError`] describing the first violation found: a graph node list
/// that repeats a node (malformed adapter — checked first, so the error is
/// deterministic regardless of the candidate order), a node repeated in
/// `order`, a node in `order` that is not a node of `dag`, a node of `dag`
/// missing from `order`, or a dependency that does not strictly precede its
/// consumer.
pub fn verify_schedule<D: ScheduleDag>(
    dag: &D,
    order: &[D::Node],
) -> Result<(), VerifyError<D::Node>> {
    let nodes = dag.nodes();

    // Lean `coversB`'s `noDuplicatesB graph.nodes`: the graph side of the
    // coverage bijection must itself be duplicate-free. Checked before any
    // order-side check.
    let mut node_set: AHashSet<D::Node> = AHashSet::with_capacity(nodes.len());
    for &n in &nodes {
        if !node_set.insert(n) {
            return Err(VerifyError::DuplicateGraphNode { node: n });
        }
    }

    let mut seen: AHashSet<D::Node> = AHashSet::new();
    for &n in order {
        if !seen.insert(n) {
            return Err(VerifyError::Duplicate { node: n });
        }
        if !node_set.contains(&n) {
            return Err(VerifyError::Extra { node: n });
        }
    }
    for &n in &nodes {
        if !seen.contains(&n) {
            return Err(VerifyError::Missing { node: n });
        }
    }

    // `order` is now known to be a duplicate-free permutation of `nodes`, so
    // every node queried below is guaranteed a position.
    let position: AHashMap<D::Node, usize> =
        order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    for &consumer in &nodes {
        let consumer_pos = position[&consumer];
        for dependency in dag.dependencies(consumer) {
            let precedes = position
                .get(&dependency)
                .is_some_and(|&dep_pos| dep_pos < consumer_pos);
            if !precedes {
                return Err(VerifyError::OutOfOrder {
                    consumer,
                    dependency,
                });
            }
        }
    }
    Ok(())
}
