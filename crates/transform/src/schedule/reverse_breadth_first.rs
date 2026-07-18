//! `SchedulingStrategy::ReverseBreadthFirst`: C++ `rbschedule`
//! (`compiler/DirectedGraph/Schedule.hh`).

use ahash::AHashMap;

use super::common::compute_heights;
use super::dag::ScheduleDag;

/// `r(v) = 0` if `v` has no users (nothing depends on it), else
/// `1 + max(r(u))` over `users(v) = { u | v in dependencies(u) }`; order by
/// `(r ascending, key ascending)`, then reverse the whole sequence. C++
/// computes this as `parallelize(reverse(G))` followed by `S.reverse()`;
/// `successors` below is that same reversed adjacency (`v`'s "users"),
/// built once up front instead of materializing a whole reversed graph.
pub(super) fn run<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Vec<D::Node> {
    let mut successors: AHashMap<D::Node, Vec<D::Node>> = AHashMap::new();
    for &n in nodes {
        for d in dag.dependencies(n) {
            successors.entry(d).or_default().push(n);
        }
    }
    let r = compute_heights(nodes, |n| successors.get(&n).cloned().unwrap_or_default());
    let mut order: Vec<D::Node> = nodes.to_vec();
    order.sort_by_key(|&n| {
        (
            *r.get(&n)
                .expect("compute_heights covers every node in `nodes`"),
            n,
        )
    });
    order.reverse();
    order
}
