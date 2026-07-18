//! [`ScheduleDag`] adapter for [`crate::hgraph::Digraph`].
//!
//! Only immediate edges (`Edge::delayed == false`) order the schedule — a
//! delayed dependency is a same-tick *placement* hint, not a same-tick
//! *ordering* edge (`crate::hgraph` module docs). Edge targets outside the
//! graph's own owned node set (`Digraph`'s docs: "Edge targets may be
//! foreign") are also dropped, keeping the [`ScheduleDag`] contract that
//! every returned dependency is itself a node of `nodes()`.
//!
//! `LoopGraph`'s adapter lives in `signal_fir::loop_graph` instead of here:
//! `LoopGraph` is `pub(crate)` behind a private `signal_fir::loop_graph`
//! module path, so this crate-external file cannot name it. Placing the impl
//! is not a visibility choice about `ScheduleDag` (`pub`, reachable
//! anywhere in the crate) — only about where `LoopGraph` itself can be
//! named, which is `signal_fir` and its descendants.

use signals::SigId;

use super::dag::ScheduleDag;
use crate::hgraph::Digraph;

impl ScheduleDag for Digraph {
    type Node = SigId;

    fn nodes(&self) -> Vec<Self::Node> {
        // `Digraph::nodes` is first-visit (insertion) order, not
        // necessarily ascending by `SigId`; sort to satisfy the
        // `ScheduleDag` contract.
        let mut nodes: Vec<SigId> = Digraph::nodes(self).to_vec();
        nodes.sort();
        nodes
    }

    fn dependencies(&self, n: Self::Node) -> Vec<Self::Node> {
        // Preserve `Digraph`'s own edge order (child-visit order from the
        // signal tree) rather than forcing an ascending sort: the
        // `ScheduleDag` contract only requires `dependencies` to be stable,
        // not ascending.
        Digraph::edges(self, n)
            .iter()
            .filter(|e| !e.delayed && self.contains(e.to))
            .map(|e| e.to)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use propagate::ClockDomainTable;
    use signals::{BinOp, SigBuilder};
    use tlib::TreeArena;

    use super::*;
    use crate::clk_env::annotate;
    use crate::hgraph::{GraphKey, build_hgraph};
    use crate::schedule::{SchedulingStrategy, schedule, verify_schedule};

    /// Builds `Digraph`s only through the existing public pipeline
    /// (`clk_env::annotate` + `hgraph::build_hgraph`): `Digraph` itself
    /// exposes no public mutator, by design (plan §4.2 — the partition
    /// property is a builder invariant, not something callers may violate).
    /// A flat program with no `ondemand`/`upsampling`/`downsampling` wrapper
    /// stays entirely in the `Top` graph, giving a plain immediate-edge DAG:
    /// `c = a * b` (`c` depends on `a`, `b`), `d = c + a` (`d` depends on
    /// `c`, `a`).
    fn top_digraph() -> (crate::hgraph::Digraph, signals::SigId) {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let a = b.input(0);
        let c = b.binop(BinOp::Mul, a, a);
        let d = b.binop(BinOp::Add, c, a);

        let domains = ClockDomainTable::new();
        let envs = annotate(&arena, &domains, &[d]).expect("flat program is well-clocked");
        let hgraph =
            build_hgraph(&arena, &domains, &envs, &[d], &HashMap::new()).expect("hgraph builds");
        let top = hgraph
            .graph(GraphKey::Top)
            .expect("flat program stays in the top graph")
            .clone();
        (top, d)
    }

    #[test]
    fn digraph_conformance_through_the_public_api() {
        let (g, _d) = top_digraph();

        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let order = schedule(strategy, &g).expect("acyclic graph schedules");
            assert!(
                verify_schedule(&g, &order).is_ok(),
                "{strategy:?}: {order:?} fails verify_schedule"
            );
        }
    }

    #[test]
    fn a_delayed_edge_does_not_constrain_the_order() {
        // `Delay1(x)` reads `x` through a delayed (state-read) edge only: a
        // placement hint, not a same-tick ordering constraint.
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let out = b.delay1(x);

        let domains = ClockDomainTable::new();
        let envs = annotate(&arena, &domains, &[out]).expect("flat program is well-clocked");
        let hgraph =
            build_hgraph(&arena, &domains, &envs, &[out], &HashMap::new()).expect("hgraph builds");
        let g = hgraph
            .graph(GraphKey::Top)
            .expect("flat program stays in the top graph");

        let deps = <Digraph as ScheduleDag>::dependencies(g, out);
        assert!(
            deps.is_empty(),
            "a delayed edge must not appear as a ScheduleDag dependency: {deps:?}"
        );

        // Both orderings are therefore legal: `out` never needs to follow
        // `x` in this DAG (the delayed read is placement-only).
        assert!(verify_schedule(g, &[out, x]).is_ok());
        assert!(verify_schedule(g, &[x, out]).is_ok());
    }
}
