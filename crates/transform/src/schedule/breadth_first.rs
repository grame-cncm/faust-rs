//! `SchedulingStrategy::BreadthFirst`: C++ `bfschedule` via `parallelize`
//! (`compiler/DirectedGraph/Schedule.hh`, `DirectedGraphAlgorythm.hh`).

use super::common::compute_heights;
use super::dag::ScheduleDag;

/// `h(v) = 0` if `deps(v)` is empty, else `1 + max(h(d))` over
/// `d in deps(v)`; final order is `(h(v) ascending, v ascending)`. The C++
/// `parallelize` buckets nodes by level and concatenates buckets in level
/// order, and each bucket is already in ascending node order (iterating
/// `g.nodes()`, itself `std::set`-ordered, and pushing into the level
/// vectors preserves that order); sorting all nodes by the `(h, key)` pair
/// produces the identical order in one pass.
pub(super) fn run<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Vec<D::Node> {
    let height = compute_heights(nodes, |n| dag.dependencies(n));
    let mut order: Vec<D::Node> = nodes.to_vec();
    order.sort_by_key(|&n| {
        (
            *height
                .get(&n)
                .expect("compute_heights covers every node in `nodes`"),
            n,
        )
    });
    order
}
