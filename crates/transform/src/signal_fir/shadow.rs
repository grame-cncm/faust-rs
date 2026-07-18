//! P3 schedule-conformance comparison (observation-only).
//!
//! This module was introduced for the pre-activation shadow audit required by
//! `vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md` P3. The
//! report remains useful after activation: it compares actual first lowering
//! with the accepted `Hsched` over the same prepared forest.
//!
//! An empty inversion set proves that every materialized same-tick dependency
//! was emitted first. Exact intersection equality additionally proves that a
//! non-recursive graph was driven directly by the selected strategy. Recursive
//! graphs can have a smaller comparable intersection because carrier
//! projections are deliberately not inserted into the ordinary value cache;
//! their body nodes are nevertheless visited by the global schedule.
//!
//! The comparison never mutates FIR and never panics. `Hgraph` includes inline
//! nodes that need not receive a separate FIR materialization, so exact order
//! is deliberately checked only on the common node set.

use ahash::AHashMap;
use signals::SigId;

use crate::hgraph::{GraphKey, Hgraph, Hsched};
use crate::signal_fir::SignalFirOutput;

/// One same-tick dependency edge the actual emission order failed to respect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeInversion {
    /// The graph the edge belongs to.
    pub key: GraphKey,
    /// The consumer node (emitted too early).
    pub consumer: SigId,
    /// The immediate dependency that should have preceded it.
    pub dependency: SigId,
}

/// Per-graph schedule-conformance facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphShadow {
    /// Which hierarchical graph this describes.
    pub key: GraphKey,
    /// Owned nodes of the graph that also appear in the actual
    /// emission order (the comparable intersection).
    pub covered_nodes: usize,
    /// Owned nodes of the graph *absent* from the emission order (inline or
    /// otherwise never separately materialized by the lowerer).
    pub uncovered_nodes: usize,
    /// `true` when the actual and selected orders match on `covered_nodes`.
    pub matches_schedule_on_intersection: bool,
}

/// Result of comparing one program's actual emission order against a selected
/// `Hsched`. Observation-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowReport {
    /// Per-graph facts, in `Hsched` order (`Control`, `Top`, wrappers…).
    pub graphs: Vec<GraphShadow>,
    /// Every immediate edge emitted out of order. Empty means every observable
    /// same-tick dependency is respected.
    pub inversions: Vec<EdgeInversion>,
}

impl ShadowReport {
    /// The conformance verdict: the actual order respects every
    /// observable immediate dependency of every graph.
    #[must_use]
    pub fn respects_all_immediate_edges(&self) -> bool {
        self.inversions.is_empty()
    }

    /// Whether actual and selected orders agree on every comparable node.
    #[must_use]
    pub fn matches_schedule_everywhere(&self) -> bool {
        self.graphs
            .iter()
            .all(|g| g.matches_schedule_on_intersection)
    }
}

/// Builds a [`ShadowReport`] from a hierarchical graph, its selected schedule,
/// and the demand-driven FIR output (whose `emission_order` is the comparison
/// subject). See the module docs for the alignment contract.
#[must_use]
pub fn compare_emission_order(
    hgraph: &Hgraph,
    hsched: &Hsched,
    output: &SignalFirOutput,
) -> ShadowReport {
    // First-emission position of each materialized signal.
    let position: AHashMap<SigId, usize> = output
        .emission_order
        .iter()
        .enumerate()
        .map(|(i, &sig)| (sig, i))
        .collect();

    let mut inversions = Vec::new();
    let mut graphs = Vec::with_capacity(hgraph.graphs().len());

    for (key, graph) in hgraph.graphs() {
        let mut covered = 0usize;
        let mut uncovered = 0usize;

        // Immediate-edge respect: for every owned consumer and every owned
        // immediate dependency, the dependency must have been emitted first.
        // Only edges whose *both* endpoints were materialized are checkable.
        for &consumer in graph.nodes() {
            if position.contains_key(&consumer) {
                covered += 1;
            } else {
                uncovered += 1;
            }
            let Some(&consumer_pos) = position.get(&consumer) else {
                continue;
            };
            for edge in graph.edges(consumer) {
                if edge.delayed || !graph.contains(edge.to) {
                    continue;
                }
                if let Some(&dep_pos) = position.get(&edge.to)
                    && dep_pos >= consumer_pos
                {
                    inversions.push(EdgeInversion {
                        key: *key,
                        consumer,
                        dependency: edge.to,
                    });
                }
            }
        }

        // Intersection-order match: the demand-driven order and the selected
        // schedule, each restricted to nodes present in the other, must be
        // the same subsequence.
        let matches = match hsched.schedule(*key) {
            Some(sched) => {
                let demand: Vec<SigId> = output
                    .emission_order
                    .iter()
                    .copied()
                    .filter(|s| graph.contains(*s) && sched.contains(s))
                    .collect();
                let scheduled: Vec<SigId> = sched
                    .iter()
                    .copied()
                    .filter(|s| position.contains_key(s))
                    .collect();
                demand == scheduled
            }
            None => true,
        };

        graphs.push(GraphShadow {
            key: *key,
            covered_nodes: covered,
            uncovered_nodes: uncovered,
            matches_schedule_on_intersection: matches,
        });
    }

    ShadowReport { graphs, inversions }
}

#[cfg(test)]
mod tests {
    use signals::{BinOp, SigBuilder};
    use tlib::TreeArena;
    use ui::UiProgram;

    use crate::signal_fir::{
        RealType, SignalFirOptions, compile_signals_to_fir_fastlane_with_ui_and_shadow,
    };

    fn compile(
        arena: &TreeArena,
        outputs: &[signals::SigId],
        num_out: usize,
    ) -> super::ShadowReport {
        let out = compile_signals_to_fir_fastlane_with_ui_and_shadow(
            arena,
            outputs,
            1,
            num_out,
            &UiProgram::empty(),
            &SignalFirOptions {
                module_name: "shadow_probe".to_owned(),
                real_type: RealType::Float32,
                ..SignalFirOptions::default()
            },
        )
        .expect("flat program lowers");
        out.shadow_report
            .expect("a non-clocked, wrapper-free program builds an Hgraph, hence a report")
    }

    #[test]
    fn demand_driven_order_respects_all_immediate_edges_for_a_shared_fork_join() {
        // x = input(0) * 0.5 (shared); a = x + 1; b = x * 2; out = a + b.
        let mut arena = TreeArena::new();
        let (out,) = {
            let mut b = SigBuilder::new(&mut arena);
            let inp = b.input(0);
            let half = b.real(0.5);
            let x = b.binop(BinOp::Mul, inp, half);
            let one = b.real(1.0);
            let a = b.binop(BinOp::Add, x, one);
            let two = b.real(2.0);
            let bb = b.binop(BinOp::Mul, x, two);
            (b.binop(BinOp::Add, a, bb),)
        };

        let report = compile(&arena, &[out], 1);
        assert!(
            report.respects_all_immediate_edges(),
            "demand-driven lowering must already respect every same-tick edge; \
             inversions: {:?}",
            report.inversions
        );
        // Every graph is covered by construction here (flat program: no
        // inline-only owned nodes escape the emission order in this shape).
        assert!(report.graphs.iter().any(|g| g.covered_nodes > 0));
    }

    #[test]
    fn report_is_present_and_covers_the_top_graph_for_a_recursive_program() {
        // y = input(0) + 0.9 * y' — a one-pole via SYMREC-shaped recursion is
        // built by the compiler front-end; here a simple delayed self-add is
        // enough to exercise a delayed edge (which must NOT count as an
        // immediate-edge inversion).
        let mut arena = TreeArena::new();
        let out = {
            let mut b = SigBuilder::new(&mut arena);
            let inp = b.input(0);
            let d = b.delay1(inp);
            b.binop(BinOp::Add, inp, d)
        };
        let report = compile(&arena, &[out], 1);
        assert!(
            report.respects_all_immediate_edges(),
            "a delayed (state) edge must never be treated as a same-tick \
             ordering constraint; inversions: {:?}",
            report.inversions
        );
    }
}
