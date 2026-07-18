//! Traversal helpers shared by two or more scheduling strategies: root
//! discovery (`DepthFirst`, `Special`) and the memoized height/level
//! computation (`BreadthFirst`, `ReverseBreadthFirst`), both ported as an
//! explicit-stack postorder so neither can overflow the call stack on a deep
//! DAG.

use ahash::{AHashMap, AHashSet};

use super::dag::ScheduleDag;

struct Frame<N> {
    node: N,
    deps: Vec<N>,
    next: usize,
}

/// C++ `roots(G)` (`DirectedGraphAlgorythm.hh`): nodes nobody depends on,
/// i.e. absent from every `dependencies(_)` list, in `nodes` order (already
/// ascending by the [`ScheduleDag`] contract).
pub(super) fn roots_of<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Vec<D::Node> {
    let mut has_consumer: AHashSet<D::Node> = AHashSet::new();
    for &n in nodes {
        has_consumer.extend(dag.dependencies(n));
    }
    nodes
        .iter()
        .copied()
        .filter(|n| !has_consumer.contains(n))
        .collect()
}

/// C++ `parallelize`'s memoized `level` closure
/// (`DirectedGraphAlgorythm.hh`), generalized over any child-edge function so
/// it serves both `BreadthFirst` (`deps_of = dependencies`, the level of the
/// original graph) and `ReverseBreadthFirst` (`deps_of` = the "users"
/// relation, the level of the reversed graph). `height(v) = 0` if
/// `deps_of(v)` is empty, else `1 + max(height(d))` over `d in deps_of(v)`.
///
/// Iterative postorder over an explicit stack, seeded from every node in
/// `nodes` (matching the C++ `for (n : g.nodes()) level(n)` outer loop, not
/// just the roots — `BreadthFirst`/`ReverseBreadthFirst` need a height for
/// every node, and a node with in-degree zero on `deps_of` is not
/// necessarily reachable by walking `deps_of` forward from a root of the
/// *other* relation). `height` is the memo: once a node is on the stack, it
/// is never pushed again, so every node is expanded at most once — the
/// explicit-stack analogue of the C++ `levelcache`.
pub(super) fn compute_heights<N, F>(nodes: &[N], deps_of: F) -> AHashMap<N, u64>
where
    N: Copy + Eq + core::hash::Hash,
    F: Fn(N) -> Vec<N>,
{
    let mut height: AHashMap<N, u64> = AHashMap::new();
    let mut queued: AHashSet<N> = AHashSet::new();

    for &seed in nodes {
        if height.contains_key(&seed) {
            continue;
        }
        let mut stack = vec![Frame {
            deps: deps_of(seed),
            node: seed,
            next: 0,
        }];
        queued.insert(seed);
        while let Some(frame) = stack.last_mut() {
            if frame.next < frame.deps.len() {
                let d = frame.deps[frame.next];
                frame.next += 1;
                if !height.contains_key(&d) && queued.insert(d) {
                    stack.push(Frame {
                        deps: deps_of(d),
                        node: d,
                        next: 0,
                    });
                }
            } else {
                // Every dependency of `frame.node` was either already
                // finished before this traversal started, or was pushed as
                // a child frame above us and — by stack discipline, and
                // because `ensure_schedulable` already ruled out cycles —
                // necessarily popped (and thus recorded in `height`) before
                // this frame can be popped.
                let h = frame
                    .deps
                    .iter()
                    .map(|d| {
                        *height
                            .get(d)
                            .expect("dependency height computed before its consumer (postorder)")
                    })
                    .max()
                    .map_or(0, |m| m + 1);
                height.insert(frame.node, h);
                stack.pop();
            }
        }
    }
    height
}
