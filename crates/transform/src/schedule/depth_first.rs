//! `SchedulingStrategy::DepthFirst`: C++ `dfschedule`
//! (`compiler/DirectedGraph/Schedule.hh`).

use ahash::AHashSet;

use super::common::roots_of;
use super::dag::ScheduleDag;

struct Frame<N> {
    node: N,
    deps: Vec<N>,
    next: usize,
}

/// Postorder visit of `roots(G)` in stable order, recursively visiting
/// `deps(v)`, appending `v` once every dependency has been appended. Ported
/// as an explicit stack (the C++ original is a recursive lambda, `dfvisit`)
/// so a graph deep enough to overflow the C++ call stack cannot overflow the
/// Rust one either; `visited` mirrors the C++ `std::set<N> V` dedup guard —
/// a node visited once through one root/branch is never revisited through
/// another (`ensure_schedulable` already ruled out cycles, so "already
/// visited" only ever means "already fully scheduled").
pub(super) fn run<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Vec<D::Node> {
    let roots = roots_of(dag, nodes);
    let mut visited: AHashSet<D::Node> = AHashSet::new();
    let mut order = Vec::with_capacity(nodes.len());

    for &root in &roots {
        if !visited.insert(root) {
            continue;
        }
        let mut stack = vec![Frame {
            deps: dag.dependencies(root),
            node: root,
            next: 0,
        }];
        while let Some(frame) = stack.last_mut() {
            if frame.next < frame.deps.len() {
                let dep = frame.deps[frame.next];
                frame.next += 1;
                if visited.insert(dep) {
                    stack.push(Frame {
                        deps: dag.dependencies(dep),
                        node: dep,
                        next: 0,
                    });
                }
            } else {
                order.push(frame.node);
                stack.pop();
            }
        }
    }
    order
}
