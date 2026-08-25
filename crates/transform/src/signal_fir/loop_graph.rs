//! Loop-DAG data model and deterministic loop ordering for `compute` emission.
//!
//! This module owns the [`LoopGraph`]: nodes are per-sample loops (each with
//! `pre`/`exec`/`post` phase statement lists), edges are backward
//! dependencies, and [`LoopGraph::topological_order`] serializes them
//! deterministically — loops are keyed by insertion-ordered [`LoopId`] and
//! every set is `LoopId`-ordered, unlike the C++ `sortGraph` whose
//! `std::set<Loop*>` is pointer-ordered and therefore non-deterministic
//! across runs.
//!
//! # Who uses what today
//! - **Scalar lowering:** `module::build_module` routes every per-sample
//!   slice through this graph — one node per non-empty slice, emitted as one
//!   plain sample loop each, in insertion order via `topological_order`.
//! - **Loop-separation criterion (diagnostic):** [`needs_separate_loop`]
//!   (the C++ `needSeparateLoop` port) is consumed by the `pv_slice`
//!   diagnostic surface and by its exhaustive in-file tests.
//!
//! This file once also carried an in-scalar `-vec` chunk-driver half
//! (chunk buffers, recursive-slice partition, split emission). It became
//! unreachable when the checked vector pipeline (`signal_fir::vector`) took
//! over every accepted `-vec` compile — a rejected one falls back to
//! *scalar* lowering — and was deleted on 2026-08-25; see the journal entry
//! and git history to recover it.
//!
//! Plan provenance: vectorization roadmap P6, vector doc V2
//! (`porting/vector-mode-analysis-port-plan-2026-06-10-en.md`); the
//! superseding checked pipeline is
//! `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`.

use std::collections::BTreeSet;

use fir::FirId;
use sigtype::Variability;

use crate::schedule::ScheduleDag;

/// Index of a loop node in a [`LoopGraph`].
///
/// Allocation order == insertion order, and every set/queue below is
/// `LoopId`-ordered, so the levelization and emission are deterministic — the
/// fix for the C++ pointer-ordered `lset` non-determinism.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct LoopId(pub(crate) u32);

/// One sample loop: three phase statement lists plus its backward
/// dependencies.
///
/// The three phases mirror the C++ `fPreCode` / `fExecCode` / `fPostCode`
/// printed around a loop: `pre` is per-loop setup, `exec` is the loop body
/// (`for i in 0..count`), `post` is per-loop teardown. The scalar path
/// populates only `exec` and leaves `pre`/`post` empty.
#[derive(Clone, Debug)]
pub(crate) struct LoopNode {
    /// Whether the sample `for` runs in reverse sample time (RAD/BRA).
    pub(crate) is_reverse: bool,
    /// Statements emitted **before** the loop (per-loop setup).
    pub(crate) pre: Vec<FirId>,
    /// Statements forming the loop body (`for i in 0..count`).
    pub(crate) exec: Vec<FirId>,
    /// Statements emitted **after** the loop (per-loop teardown).
    pub(crate) post: Vec<FirId>,
    /// Loops that must run before this one (this loop reads their output).
    pub(crate) deps: BTreeSet<LoopId>,
}

impl LoopNode {
    fn new(is_reverse: bool) -> Self {
        Self {
            is_reverse,
            pre: Vec::new(),
            exec: Vec::new(),
            post: Vec::new(),
            deps: BTreeSet::new(),
        }
    }
}

/// A DAG of sample loops. Nodes are stored in insertion order; edges are backward
/// dependencies (`a` depends on `b` ⇒ `b` is emitted before `a`).
#[derive(Clone, Debug, Default)]
pub(crate) struct LoopGraph {
    nodes: Vec<LoopNode>,
}

/// Error returned when the loop DAG has a cycle (which must never happen: a
/// backward dependency edge always points at an earlier-produced value).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct LoopCycle {
    /// The loops that remained unscheduled (participate in a cycle).
    pub(crate) unscheduled: Vec<LoopId>,
}

impl LoopGraph {
    /// Creates an empty graph.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Allocates a new loop node and returns its id.
    pub(crate) fn add_loop(&mut self, is_reverse: bool) -> LoopId {
        let id = LoopId(u32::try_from(self.nodes.len()).expect("loop count fits u32"));
        self.nodes.push(LoopNode::new(is_reverse));
        id
    }

    /// Number of loops. Test-only assertion helper.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph has no loops. Test-only assertion helper.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn index(id: LoopId) -> usize {
        id.0 as usize
    }

    /// Immutable access to a loop node.
    #[must_use]
    pub(crate) fn node(&self, id: LoopId) -> &LoopNode {
        &self.nodes[Self::index(id)]
    }

    /// Mutable access to a loop node (to push phase statements).
    pub(crate) fn node_mut(&mut self, id: LoopId) -> &mut LoopNode {
        &mut self.nodes[Self::index(id)]
    }

    /// Records that `from` must run after `to` (`from` reads `to`'s output).
    /// A self-edge is ignored; edges within one loop are not dependencies.
    ///
    /// Test-only today: the scalar path builds edgeless graphs (slices are
    /// already emitted in dependency-safe insertion order), and the tests use
    /// this to pin `topological_order`'s deterministic tie-breaking for the
    /// day a producer adds real edges.
    #[cfg(test)]
    pub(crate) fn add_dep(&mut self, from: LoopId, to: LoopId) {
        if from != to {
            self.nodes[Self::index(from)].deps.insert(to);
        }
    }

    /// Iterates loop ids in insertion order.
    pub(crate) fn ids(&self) -> impl Iterator<Item = LoopId> {
        (0..self.nodes.len()).map(|i| LoopId(i as u32))
    }

    /// Deterministic topological order (dependencies before dependents).
    ///
    /// Kahn's algorithm with a `LoopId`-ordered ready set: among loops whose
    /// dependencies are all satisfied, the lowest [`LoopId`] is emitted first,
    /// so independent loops keep their insertion order. This is the stable
    /// replacement for the C++ pointer-ordered `sortGraph`.
    pub(crate) fn topological_order(&self) -> Result<Vec<LoopId>, LoopCycle> {
        let n = self.nodes.len();
        // Outgoing "dependents" adjacency + in-degree = number of unmet deps.
        let mut indegree = vec![0usize; n];
        let mut dependents: Vec<BTreeSet<LoopId>> = vec![BTreeSet::new(); n];
        for (i, node) in self.nodes.iter().enumerate() {
            indegree[i] = node.deps.len();
            for &dep in &node.deps {
                dependents[Self::index(dep)].insert(LoopId(i as u32));
            }
        }
        // BTreeSet keeps the ready frontier LoopId-ordered.
        let mut ready: BTreeSet<LoopId> = (0..n)
            .filter(|&i| indegree[i] == 0)
            .map(|i| LoopId(i as u32))
            .collect();
        let mut order = Vec::with_capacity(n);
        while let Some(&next) = ready.iter().next() {
            ready.remove(&next);
            order.push(next);
            for &d in &dependents[Self::index(next)] {
                let di = Self::index(d);
                indegree[di] -= 1;
                if indegree[di] == 0 {
                    ready.insert(d);
                }
            }
        }
        if order.len() == n {
            Ok(order)
        } else {
            let scheduled: BTreeSet<LoopId> = order.iter().copied().collect();
            Err(LoopCycle {
                unscheduled: self.ids().filter(|id| !scheduled.contains(id)).collect(),
            })
        }
    }
}

/// [`crate::schedule::ScheduleDag`] adapter. `LoopGraph` is
/// `pub(crate)` behind a private `signal_fir::loop_graph` module path, so
/// this impl lives here rather than alongside the generic scheduler core:
/// `crate::schedule` cannot name `LoopGraph` at all, while every item in
/// `signal_fir::loop_graph` can already name both `LoopGraph` (this module)
/// and `ScheduleDag` (`pub`, reachable crate-wide) — the impl goes where
/// visibility allows, per the trait's own orphan-rule freedom (same crate on
/// either side). Nodes are already `LoopId`-ordered by allocation
/// (`add_loop` assigns ids `0, 1, 2, ...`) and deps are already a
/// `BTreeSet<LoopId>`, so both methods are simple, already-ordered
/// collections — no behavior of `LoopGraph` itself changes.
impl ScheduleDag for LoopGraph {
    type Node = LoopId;

    fn nodes(&self) -> Vec<Self::Node> {
        self.ids().collect()
    }

    fn dependencies(&self, n: Self::Node) -> Vec<Self::Node> {
        self.node(n).deps.iter().copied().collect()
    }
}

// ── Loop-separation criterion ────────────────────────────────────────────────
//
// A port of the C++ `needSeparateLoop` (`compile_vect.cpp:304-339`,
// `dag_instructions_compiler.cpp:370-393`; provenance: vector doc §2 table).
// This is the *decision*: given a sample signal's properties, does it get its
// own loop, and may that loop vectorize? The pv_slice diagnostic extracts the
// [`SignalLoopProps`] and consumes the [`LoopSeparation`] verdict; keeping the
// decision pure makes it exhaustively testable without the lowering machinery.

/// The `needSeparateLoop` queries for one signal, as computed by the lowering.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SignalLoopProps {
    /// Rate class. Only `Samp` signals live in the sample loop at all; `Konst`
    /// and `Block` ("slower than kSamp") are compiled once into control code.
    pub(crate) variability: Variability,
    /// Largest delay any reader applies to this signal (`getMaxDelay`). A
    /// non-zero value forces a dedicated loop with a delay-line buffer.
    pub(crate) max_delay: usize,
    /// This signal is a recursive-group projection (a back-edge carrier): it
    /// must be computed one sample at a time.
    pub(crate) is_recursive_proj: bool,
    /// This signal feeds ≥ 2 distinct consumers (`hasMultiOccurrences`): worth
    /// materializing once in a chunk buffer instead of recomputing.
    pub(crate) is_shared: bool,
    /// This signal is a `sigDelay` *read* — compiled where used, never split.
    pub(crate) is_delay_read: bool,
    /// This signal is "very simple" (a leaf: var / const / input) — free to
    /// duplicate, so never given a loop of its own.
    pub(crate) is_very_simple: bool,
}

/// Verdict for one sample-rate signal: whether it gets its own chunk loop, and
/// whether that loop may auto-vectorize.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoopSeparation {
    /// No dedicated loop: inline into the consumer's loop (or, for non-`Samp`
    /// signals, hoist to control code outside the chunk loop).
    Inline,
    /// A dedicated loop the C backend may auto-vectorize.
    SeparateVectorizable,
    /// A dedicated **serial** loop (recursive group — one sample after another).
    SeparateSerial,
}

/// Decides whether `props` requires its own chunk loop (provenance: vector
/// doc §2 table;
/// C++ `DAGInstructionsCompiler::needSeparateLoop`).
///
/// Precedence (first match wins):
/// 1. used delayed (`max_delay > 0`) -> **separate**;
/// 2. very-simple leaf or non-`Samp` rate -> **inline**;
/// 3. a `sigDelay` read -> **inline** at the use site;
/// 4. recursive projection -> **separate serial** loop;
/// 5. shared value -> **separate vectorizable** loop;
/// 6. otherwise -> **inline** into the consumer.
///
/// The first rule is semantic: even a simple or slow value needs a per-sample
/// producer when its history is read. The C++ predicate returns only a Boolean;
/// this adapted Rust result additionally keeps recursive projections serial.
#[must_use]
pub(crate) fn needs_separate_loop(props: &SignalLoopProps) -> LoopSeparation {
    if props.max_delay > 0 {
        return if props.is_recursive_proj {
            LoopSeparation::SeparateSerial
        } else {
            LoopSeparation::SeparateVectorizable
        };
    }
    if props.is_very_simple || props.variability != Variability::Samp {
        return LoopSeparation::Inline;
    }
    if props.is_delay_read {
        return LoopSeparation::Inline;
    }
    if props.is_recursive_proj {
        return LoopSeparation::SeparateSerial;
    }
    if props.is_shared {
        return LoopSeparation::SeparateVectorizable;
    }
    LoopSeparation::Inline
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample-rate, non-shared, non-delayed, non-recursive, non-trivial signal
    /// (the "otherwise" row) — the base other rows tweak one field from.
    fn base_props() -> SignalLoopProps {
        SignalLoopProps {
            variability: Variability::Samp,
            max_delay: 0,
            is_recursive_proj: false,
            is_shared: false,
            is_delay_read: false,
            is_very_simple: false,
        }
    }

    #[test]
    fn non_sample_rate_signals_without_delayed_use_are_inlined() {
        for v in [Variability::Konst, Variability::Block] {
            let p = SignalLoopProps {
                variability: v,
                is_shared: true,
                ..base_props()
            };
            assert_eq!(needs_separate_loop(&p), LoopSeparation::Inline);
        }
    }

    #[test]
    fn positive_max_delay_dominates_slow_simple_and_delay_read_rules() {
        for p in [
            SignalLoopProps {
                variability: Variability::Block,
                max_delay: 1,
                ..base_props()
            },
            SignalLoopProps {
                max_delay: 1,
                is_very_simple: true,
                ..base_props()
            },
            SignalLoopProps {
                max_delay: 1,
                is_delay_read: true,
                ..base_props()
            },
        ] {
            assert_eq!(
                needs_separate_loop(&p),
                LoopSeparation::SeparateVectorizable
            );
        }
    }

    #[test]
    fn delay_reads_without_delayed_use_are_inlined() {
        let p = SignalLoopProps {
            is_delay_read: true,
            is_shared: true,
            ..base_props()
        };
        assert_eq!(needs_separate_loop(&p), LoopSeparation::Inline);
    }

    #[test]
    fn separation_matches_the_exhaustive_lean_characterization() {
        let mut cases = 0;
        for max_delay in [0, 1] {
            for variability in [Variability::Konst, Variability::Block, Variability::Samp] {
                for is_very_simple in [false, true] {
                    for is_delay_read in [false, true] {
                        for is_recursive_proj in [false, true] {
                            for is_shared in [false, true] {
                                let props = SignalLoopProps {
                                    variability,
                                    max_delay,
                                    is_recursive_proj,
                                    is_shared,
                                    is_delay_read,
                                    is_very_simple,
                                };
                                let separates = max_delay > 0
                                    || (!is_very_simple
                                        && variability == Variability::Samp
                                        && !is_delay_read
                                        && (is_recursive_proj || is_shared));
                                let expected = if !separates {
                                    LoopSeparation::Inline
                                } else if is_recursive_proj {
                                    LoopSeparation::SeparateSerial
                                } else {
                                    LoopSeparation::SeparateVectorizable
                                };

                                assert_eq!(
                                    needs_separate_loop(&props),
                                    expected,
                                    "separateLoop mismatch for {props:?}"
                                );
                                assert_eq!(
                                    needs_separate_loop(&props) != LoopSeparation::Inline,
                                    separates,
                                    "Boolean separation mismatch for {props:?}"
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 96);
    }

    #[test]
    fn recursive_projection_gets_a_serial_loop() {
        let p = SignalLoopProps {
            is_recursive_proj: true,
            ..base_props()
        };
        assert_eq!(needs_separate_loop(&p), LoopSeparation::SeparateSerial);
        assert_eq!(
            needs_separate_loop(&p),
            LoopSeparation::SeparateSerial
        );
    }

    #[test]
    fn very_simple_leaves_are_inlined_even_if_shared() {
        let p = SignalLoopProps {
            is_very_simple: true,
            is_shared: true,
            ..base_props()
        };
        assert_eq!(needs_separate_loop(&p), LoopSeparation::Inline);
    }

    #[test]
    fn delayed_or_shared_expressions_get_a_vectorizable_loop() {
        let delayed = SignalLoopProps {
            max_delay: 1,
            ..base_props()
        };
        assert_eq!(
            needs_separate_loop(&delayed),
            LoopSeparation::SeparateVectorizable
        );
        assert_eq!(
            needs_separate_loop(&delayed),
            LoopSeparation::SeparateVectorizable
        );

        let shared = SignalLoopProps {
            is_shared: true,
            ..base_props()
        };
        assert_eq!(
            needs_separate_loop(&shared),
            LoopSeparation::SeparateVectorizable
        );
    }

    #[test]
    fn plain_sample_expression_is_inlined() {
        assert_eq!(needs_separate_loop(&base_props()), LoopSeparation::Inline);
        assert_eq!(base_props().variability, Variability::Samp);
    }

    #[test]
    fn empty_graph_orders_to_nothing() {
        let g = LoopGraph::new();
        assert!(g.is_empty());
        assert_eq!(g.topological_order().unwrap(), vec![]);
    }

    #[test]
    fn independent_loops_keep_insertion_order() {
        let mut g = LoopGraph::new();
        let a = g.add_loop(false);
        let b = g.add_loop(false);
        let c = g.add_loop(true);
        assert_eq!(g.len(), 3);
        // No edges → insertion order, deterministically.
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
        assert!(g.node(c).is_reverse);
    }

    #[test]
    fn dependencies_are_emitted_before_dependents() {
        // c depends on b, b depends on a → a, b, c regardless of alloc order.
        let mut g = LoopGraph::new();
        let a = g.add_loop(false);
        let b = g.add_loop(false);
        let c = g.add_loop(false);
        g.add_dep(c, b);
        g.add_dep(b, a);
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
    }

    #[test]
    fn ready_frontier_is_loop_id_ordered() {
        // a is a shared root feeding b and c; b and c are independent, so they
        // come out in LoopId order (b before c), deterministically.
        let mut g = LoopGraph::new();
        let a = g.add_loop(false);
        let b = g.add_loop(false);
        let c = g.add_loop(false);
        g.add_dep(b, a);
        g.add_dep(c, a);
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
    }

    #[test]
    fn self_edges_are_ignored() {
        let mut g = LoopGraph::new();
        let a = g.add_loop(false);
        g.add_dep(a, a);
        assert!(g.node(a).deps.is_empty());
        assert_eq!(g.topological_order().unwrap(), vec![a]);
    }

    #[test]
    fn a_cycle_is_reported() {
        let mut g = LoopGraph::new();
        let a = g.add_loop(false);
        let b = g.add_loop(false);
        g.add_dep(a, b);
        g.add_dep(b, a);
        let err = g.topological_order().unwrap_err();
        assert_eq!(err.unscheduled, vec![a, b]);
    }

    /// The generic scheduler core must agree with
    /// `LoopGraph`'s own `topological_order` on the same DAG: build one
    /// through the existing `add_loop`/`add_dep` API, run all four
    /// `crate::schedule` strategies over it, and check every result against
    /// the independent `verify_schedule` checker.
    #[test]
    fn schedule_dag_conformance_through_the_existing_api() {
        use crate::schedule::{SchedulingStrategy, schedule, verify_schedule};

        let mut g = LoopGraph::new();
        let a = g.add_loop(false);
        let b = g.add_loop(false);
        let c = g.add_loop(false);
        // c depends on b, b depends on a.
        g.add_dep(c, b);
        g.add_dep(b, a);

        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let order = schedule(strategy, &g).expect("acyclic loop graph schedules");
            assert!(
                verify_schedule(&g, &order).is_ok(),
                "{strategy:?}: {order:?} fails verify_schedule"
            );
        }
        // Every strategy agrees with `topological_order` on this simple
        // chain: only one valid order exists.
        assert_eq!(
            schedule(SchedulingStrategy::DepthFirst, &g).unwrap(),
            vec![a, b, c]
        );
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
    }

    #[test]
    fn phase_statements_and_deps_round_trip() {
        let mut store = fir::FirStore::new();
        let (s0, s1) = {
            let mut b = fir::FirBuilder::new(&mut store);
            (b.int32(0), b.int32(1))
        };
        let mut g = LoopGraph::new();
        let l = g.add_loop(false);
        g.node_mut(l).pre.push(s0);
        g.node_mut(l).exec.push(s1);
        assert_eq!(g.node(l).pre, vec![s0]);
        assert_eq!(g.node(l).exec, vec![s1]);
        assert!(g.node(l).post.is_empty());
    }
}
