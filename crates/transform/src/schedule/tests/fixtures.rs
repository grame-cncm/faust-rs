//! [`VecDag`]: the smallest possible [`ScheduleDag`] — nodes are `u32` keys,
//! dependencies an explicit adjacency table built fluently by the fixture
//! functions below. Deliberately backed by a `BTreeMap` (not a hash map): a
//! test fixture must not itself be the source of an insertion-order leak
//! that a broken algorithm could accidentally paper over.

use std::collections::BTreeMap;

use crate::schedule::ScheduleDag;

/// The four strategies, in the fixed order every "for every strategy" test
/// below iterates them.
pub(super) const ALL_STRATEGIES: [crate::schedule::SchedulingStrategy; 4] = [
    crate::schedule::SchedulingStrategy::DepthFirst,
    crate::schedule::SchedulingStrategy::BreadthFirst,
    crate::schedule::SchedulingStrategy::Special,
    crate::schedule::SchedulingStrategy::ReverseBreadthFirst,
];

#[derive(Clone, Debug, Default)]
pub(super) struct VecDag {
    /// node -> its dependency list, in the exact order given to `deps`.
    adjacency: BTreeMap<u32, Vec<u32>>,
}

impl VecDag {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Registers `node` with no dependencies (a no-op if already present).
    /// Needed so an isolated node — no outgoing *or* incoming edge — is
    /// still part of `nodes()`.
    pub(super) fn node(mut self, node: u32) -> Self {
        self.adjacency.entry(node).or_default();
        self
    }

    /// `consumer` gains `deps` as additional dependencies, appended in the
    /// given order; every id in `deps` is registered as a node (with no
    /// dependencies of its own unless a later `.deps(...)` call adds some).
    pub(super) fn deps(mut self, consumer: u32, deps: &[u32]) -> Self {
        self.adjacency
            .entry(consumer)
            .or_default()
            .extend_from_slice(deps);
        for &d in deps {
            self.adjacency.entry(d).or_default();
        }
        self
    }
}

impl ScheduleDag for VecDag {
    type Node = u32;

    fn nodes(&self) -> Vec<u32> {
        // `BTreeMap` keys iterate ascending: already the required order.
        self.adjacency.keys().copied().collect()
    }

    fn dependencies(&self, n: u32) -> Vec<u32> {
        self.adjacency.get(&n).cloned().unwrap_or_default()
    }
}

/// `0 <- 1 <- 2 <- 3`: a total order, no scheduling freedom at all. Every
/// strategy must agree on `[0, 1, 2, 3]`.
pub(super) fn chain() -> VecDag {
    VecDag::new().deps(1, &[0]).deps(2, &[1]).deps(3, &[2])
}

/// The task-specified diamond: `3 -> [1, 2]`, `1 -> [0]`, `2 -> [0]`. Matches
/// the Lean `diamondGraph` fixture
/// (`porting/vector-mode-scheduling-formal-spec.lean`), which proves both
/// `[0, 1, 2, 3]` and `[0, 2, 1, 3]` valid and `[1, 0, 2, 3]` invalid.
pub(super) fn diamond() -> VecDag {
    VecDag::new().deps(3, &[1, 2]).deps(1, &[0]).deps(2, &[0])
}

/// Asymmetric fork/join: `0` forks into two one-hop branches (`1`, `2`, both
/// depending only on `0`) plus an unrelated bystander leaf `3`; `4` joins
/// all three (`deps(4) = [1, 2, 3]`, in that order). Unlike a purely
/// "layered" shape, this has enough asymmetry that the four strategies
/// genuinely disagree (see `tests::exact_orders`), which is the point: a
/// fully symmetric fixture cannot distinguish `Special` from
/// `ReverseBreadthFirst`.
pub(super) fn asymmetric_fork_join() -> VecDag {
    VecDag::new()
        .deps(1, &[0])
        .deps(2, &[0])
        .node(3)
        .deps(4, &[1, 2, 3])
}

/// Two independent 2-node chains: `{0, 1}` (`1 -> 0`) and `{2, 3}` (`3 ->
/// 2`), sharing no edge.
pub(super) fn disconnected() -> VecDag {
    VecDag::new().deps(1, &[0]).deps(3, &[2])
}

/// A two-wide "ladder" of `layers` fully cross-connected layers: layer `0`
/// is `{0, 1}` with no dependencies; for `1 <= i < layers`, layer `i` is
/// `{2i, 2i+1}` and *both* of its nodes depend on *both* nodes of layer
/// `i - 1`. `2 * layers` nodes total.
///
/// `Special`'s literal, unmemoized recursion re-expands a shared dependency
/// once per consumer, so the duplicate-laden `raw(G)` list this shape
/// produces grows geometrically with `layers` (`special.rs` module docs):
/// with `M(0) = 1` and `M(i) = 1 + 2 * M(i - 1) = 2^(i+1) - 1`, each of the
/// two top-layer roots contributes `M(layers - 1)` entries, so
/// `len(raw(G)) = 2 * M(layers - 1) = 2^(layers + 1) - 2`.
pub(super) fn ladder(layers: u32) -> VecDag {
    let mut g = VecDag::new().node(0).node(1);
    for i in 1..layers {
        let (a0, b0) = (2 * (i - 1), 2 * (i - 1) + 1);
        let (a1, b1) = (2 * i, 2 * i + 1);
        g = g.deps(a1, &[a0, b0]).deps(b1, &[a0, b0]);
    }
    g
}
