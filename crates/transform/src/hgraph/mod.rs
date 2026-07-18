//! Hierarchical dependency graph and schedule (roadmap P1.2).
//!
//! # Source provenance (C++)
//! - `compiler/Dependencies/DependenciesGraph.cpp` (`dependenciesGraphs`,
//!   branch `master-dev-ocpp-od-fir-2-FIR19`, commit `8eebea429`)
//! - `compiler/Dependencies/DependenciesUtils.cpp` (`needSubGraph`,
//!   `isExternal`, `getSignalDependencies`)
//! - `compiler/Dependencies/DependenciesScheduling.hh` (`Hsched`)
//! - `compiler/Dependencies/DependenciesAudit.hh` (`auditHgraph`)
//!
//! # What this module computes
//! A **partition** of the reachable prepared signals into per-domain
//! dependency graphs, driven by the clock-environment inference of
//! [`crate::clk_env`]:
//!
//! - the top-level graph holds audio-rate (`nil`-domain) signals;
//! - each OD/US/DS wrapper node keys a **subgraph** holding the signals of
//!   its inner domain (the clock deliberately stays outside — it is a block
//!   *precondition*, not block content);
//! - a signal whose domain is a *strict ancestor* of the current traversal
//!   domain is **external**: it surfaces as an edge `wrapper → external` in
//!   the graph holding the wrapper ("the block needs this computed first"),
//!   and its own computation is placed in the graph of its own domain.
//!
//! [`schedule`] serializes each graph with the shared generic scheduler
//! (`crate::schedule`, vectorization port plan phase P1) under a caller-
//! selected [`crate::schedule::SchedulingStrategy`] — the same four `-ss`
//! strategies used everywhere else, on *immediate* edges only.
//!
//! # Immediate vs delayed dependencies
//! A `delay ≥ 1` dependency imposes no intra-tick ordering (the value is read
//! from state) but is still traversed so its defining computation lands in
//! the right domain. `Seq(od, y)` depends **only on `od`** — reading the held
//! `PermVar` is free once the block ran (plan §3.7).
//!
//! # Control graph (plan §4.6)
//! [`GraphKey::Control`] holds slower-than-sample (`Konst`/`Block`
//! [`sigtype::Variability`]) signals reached while traversing the **top**
//! domain — the global lifecycle/control section, run before `Top` exactly
//! like C++ `controls` (created lazily, on first use, not eagerly like
//! `Top`: see [`build_hgraph`]). A reference from `Top` to a `Control` value
//! is *not* recorded as a same-graph ordering edge: `Control` is an
//! unconditional precondition of every other graph, not a schedule-dependent
//! one (plan §4.2 "controls are compiled first as a fixed outer phase").
//!
//! Scope, deliberately narrower than full C++ parity: only *top-level*
//! Konst/Block signals are hoisted to the single global `Control` graph.
//! Konst/Block-variability signals reached while traversing a **wrapper's**
//! inner domain keep their existing per-wrapper placement unchanged — C++
//! has per-domain lifecycle sections too, and unifying those with the global
//! `Control` graph is deferred (open question, not silently assumed) rather
//! than guessed at here.
//!
//! # Adaptation status
//! - C++ keys graphs by `Tree` and attaches results as properties; Rust keys
//!   by [`GraphKey`] (`Control`, `Top`, or the wrapper `SigId`) and returns
//!   owned values.
//! - Node membership is partitioned (checked by [`audit_hgraph`], mirroring
//!   C++ `auditHgraph`, and run as a debug assertion by [`build_hgraph`]);
//!   edge *targets* may be foreign (externals owned by another graph).
//!   [`audit_control_variability`] additionally checks that `Control` never
//!   owns a `Samp`-variability signal.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use ahash::{AHashMap, AHashSet};
use propagate::{ClockDomainId, ClockDomainTable};
use signals::{SigId, SigMatch, match_sig};
use sigtype::{SigType, Variability};
use tlib::TreeArena;

use crate::clk_env::{ClkEnv, ClkEnvMap, is_ancestor_clk_env};
use crate::schedule::SchedulingStrategy;

/// Key of one per-domain dependency graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphKey {
    /// The global control/lifecycle graph: slower-than-sample (`Konst`/
    /// `Block`) signals reached at the top domain (plan §4.6). Declared
    /// first so the derived [`Ord`] places it before every other key,
    /// matching its role as an unconditional precondition.
    Control,
    /// The top-level (audio-rate) graph.
    Top,
    /// Subgraph keyed by its OD/US/DS wrapper node.
    Wrapper(SigId),
}

/// One directed dependency edge annotated with its temporal class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Dependency target (the signal that must be available first).
    pub to: SigId,
    /// `true` when the dependency crosses at least one sample of state
    /// (`delay ≥ 1`): traversed for placement, ignored for ordering.
    pub delayed: bool,
}

/// Small deterministic digraph: owned nodes in first-visit order, adjacency
/// per node. Edge targets may be foreign (signals owned by another graph).
#[derive(Clone, Debug)]
pub struct Digraph {
    nodes: Vec<SigId>,
    node_set: AHashSet<SigId>,
    edges: AHashMap<SigId, Vec<Edge>>,
}

impl Default for Digraph {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            node_set: AHashSet::new(),
            edges: AHashMap::new(),
        }
    }
}

impl Digraph {
    fn add_node(&mut self, sig: SigId) {
        if self.node_set.insert(sig) {
            self.nodes.push(sig);
        }
    }

    /// Adds `from → to`. Only `from` becomes an owned node; `to` may be
    /// foreign (an external precondition).
    fn add_edge(&mut self, from: SigId, to: SigId, delayed: bool) {
        self.add_node(from);
        let list = self.edges.entry(from).or_default();
        if !list.iter().any(|e| e.to == to && e.delayed == delayed) {
            list.push(Edge { to, delayed });
        }
    }

    /// Owned nodes in deterministic first-visit order.
    #[must_use]
    pub fn nodes(&self) -> &[SigId] {
        &self.nodes
    }

    /// Outgoing edges of one node.
    #[must_use]
    pub fn edges(&self, sig: SigId) -> &[Edge] {
        self.edges.get(&sig).map_or(&[], Vec::as_slice)
    }

    /// Whether `sig` is an owned node of this graph.
    #[must_use]
    pub fn contains(&self, sig: SigId) -> bool {
        self.node_set.contains(&sig)
    }

    /// Number of owned nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph owns no node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Hierarchical dependency graph: one digraph per clock-domain instance.
#[derive(Clone, Debug)]
pub struct Hgraph {
    /// Per-domain graphs in deterministic creation order (`Top` first, then
    /// wrappers in traversal order).
    graphs: Vec<(GraphKey, Digraph)>,
    index: AHashMap<GraphKey, usize>,
}

impl Default for Hgraph {
    fn default() -> Self {
        Self {
            graphs: Vec::new(),
            index: AHashMap::new(),
        }
    }
}

impl Hgraph {
    fn graph_mut(&mut self, key: GraphKey) -> &mut Digraph {
        if let Some(&slot) = self.index.get(&key) {
            return &mut self.graphs[slot].1;
        }
        self.index.insert(key, self.graphs.len());
        self.graphs.push((key, Digraph::default()));
        &mut self.graphs.last_mut().expect("just pushed").1
    }

    /// Returns one graph by key.
    #[must_use]
    pub fn graph(&self, key: GraphKey) -> Option<&Digraph> {
        self.index.get(&key).map(|&slot| &self.graphs[slot].1)
    }

    /// All graphs in deterministic creation order.
    #[must_use]
    pub fn graphs(&self) -> &[(GraphKey, Digraph)] {
        &self.graphs
    }
}

/// Errors raised while building or scheduling the hierarchical graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HgraphError {
    /// A reachable signal carries no inferred clock environment.
    MissingEnv { sig: SigId },
    /// The partition property is violated (a signal owned by two graphs).
    PartitionViolated { sig: SigId },
    /// An instantaneous (immediate-edge) cycle inside one domain: a
    /// causality error, exactly as in classic Faust.
    InstantaneousCycle { key: GraphKey, sig: SigId },
    /// Structural error while walking the prepared forest.
    Malformed { sig: SigId, detail: String },
    /// [`audit_control_variability`]: a `Samp`-variability signal is owned by
    /// [`GraphKey::Control`] (a per-sample value can never be part of the
    /// unconditional, once-per-lifecycle control precondition).
    ControlVariabilityViolated { sig: SigId },
}

impl fmt::Display for HgraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv { sig } => write!(
                f,
                "signal {} is reachable but has no inferred clock environment",
                sig.as_u32()
            ),
            Self::PartitionViolated { sig } => write!(
                f,
                "signal {} is owned by more than one dependency graph",
                sig.as_u32()
            ),
            Self::InstantaneousCycle { key, sig } => write!(
                f,
                "instantaneous cycle through signal {} in graph {key:?} (causality error)",
                sig.as_u32()
            ),
            Self::Malformed { sig, detail } => {
                write!(f, "malformed signal {}: {detail}", sig.as_u32())
            }
            Self::ControlVariabilityViolated { sig } => write!(
                f,
                "signal {} is Samp-variability but owned by the Control graph",
                sig.as_u32()
            ),
        }
    }
}

impl std::error::Error for HgraphError {}

/// `needSubGraph(sig)` ⇔ the node is an OD/US/DS wrapper.
#[must_use]
pub fn needs_subgraph(arena: &TreeArena, sig: SigId) -> bool {
    matches!(
        match_sig(arena, sig),
        SigMatch::OnDemand(_) | SigMatch::Upsampling(_) | SigMatch::Downsampling(_)
    )
}

/// Whether any signal reachable from `outputs` is an OD/US/DS wrapper.
///
/// Pure structural scan — needs no domain table or clock-environment map, so
/// it is safe to run before either exists. A caller with no
/// `ClockDomainTable` at all must check this *before* attempting
/// [`build_hgraph`] with an empty table: `clk_env::annotate` cannot resolve
/// a real wrapper's clock relationship without real domain data, and will
/// report a structurally confusing `ClockedViolation`-family error instead
/// of the specific, intentional "clocked node reached without a domain
/// table" rejection the caller almost certainly wants (`signal_fir`'s
/// `FRS-SFIR-0007`).
pub fn contains_wrapper(arena: &TreeArena, outputs: &[SigId]) -> Result<bool, HgraphError> {
    let mut visited: AHashSet<SigId> = AHashSet::new();
    let mut stack: Vec<SigId> = outputs.to_vec();
    while let Some(sig) = stack.pop() {
        if !visited.insert(sig) {
            continue;
        }
        if needs_subgraph(arena, sig) {
            return Ok(true);
        }
        if let Some(children) = arena.children(sig) {
            stack.extend(children.iter().copied());
        }
    }
    Ok(false)
}

/// `isExternal(cur_env, sig)` ⇔ the signal's domain is a *strict* ancestor of
/// the current traversal domain: computed elsewhere, visible here.
#[must_use]
pub fn is_external(
    domains: &ClockDomainTable,
    envs: &ClkEnvMap,
    cur_env: ClkEnv,
    sig: SigId,
) -> bool {
    let Some(sig_env) = envs.env(sig) else {
        return false;
    };
    sig_env != cur_env && is_ancestor_clk_env(domains, sig_env, cur_env)
}

/// `getSignalDependencies`: immediate/delayed dependency split (plan §3.7).
///
/// Returns the dependency targets of `sig` in deterministic child order. The
/// opaque clock-env child of `Clocked` is never a dependency.
pub fn get_signal_dependencies(
    analysis: &crate::signal_fir::vector_analysis::SignalAnalysisContext<'_>,
    sig: SigId,
) -> Result<Vec<Edge>, HgraphError> {
    crate::signal_fir::vector_analysis::signal_dependencies(analysis, sig)
        .map_err(analysis_error_to_hgraph)
        .map(|dependencies| {
            dependencies
                .scheduling()
                .iter()
                .map(|dependency| Edge {
                    to: dependency.to,
                    delayed: matches!(
                        dependency.kind,
                        crate::signal_fir::vector_analysis::DepKind::Delayed { .. }
                    ),
                })
                .collect()
        })
}

fn analysis_error_to_hgraph(
    error: crate::signal_fir::vector_analysis::AnalysisError,
) -> HgraphError {
    match error {
        crate::signal_fir::vector_analysis::AnalysisError::Malformed { sig, detail } => {
            HgraphError::Malformed { sig, detail }
        }
        other => HgraphError::Malformed {
            sig: match &other {
                crate::signal_fir::vector_analysis::AnalysisError::MissingType { sig }
                | crate::signal_fir::vector_analysis::AnalysisError::MissingClock { sig }
                | crate::signal_fir::vector_analysis::AnalysisError::InvalidRecursiveProjection {
                    sig,
                    ..
                }
                | crate::signal_fir::vector_analysis::AnalysisError::InvalidDelayInterval {
                    sig,
                    ..
                } => *sig,
                crate::signal_fir::vector_analysis::AnalysisError::Malformed { .. } => {
                    unreachable!("handled above")
                }
            },
            detail: other.to_string(),
        },
    }
}

struct Builder<'a> {
    arena: &'a TreeArena,
    analysis: crate::signal_fir::vector_analysis::SignalAnalysisContext<'a>,
    domains: &'a ClockDomainTable,
    envs: &'a ClkEnvMap,
    /// Variability source for the `Control` redirect (plan §4.6). A signal
    /// missing from this map is conservatively treated as `Samp` (stays
    /// wherever clock-domain routing would already place it).
    sig_types: &'a HashMap<SigId, SigType>,
    hgraph: Hgraph,
    /// Which graph owns each domain's signals. `None → Top`; a wrapper's
    /// inner domain maps to its subgraph once the wrapper is visited.
    domain_key: AHashMap<ClkEnv, GraphKey>,
    /// Global visited set: with the env dispatch, guarantees the partition
    /// property (plan §4.2).
    visited: AHashSet<SigId>,
}

impl<'a> Builder<'a> {
    fn env_of(&self, sig: SigId) -> Result<ClkEnv, HgraphError> {
        self.envs.env(sig).ok_or(HgraphError::MissingEnv { sig })
    }

    /// Graph owning signals of `env`. Signals of a clocked domain are only
    /// reachable through their wrapper (`Seq(od, permvar)` depends on `od`
    /// alone), so the wrapper — which registers the mapping — is always
    /// visited first.
    fn key_for(&self, env: ClkEnv) -> GraphKey {
        self.domain_key.get(&env).copied().unwrap_or(GraphKey::Top)
    }

    /// Redirects a `Top`-routed, non-`Samp`-variability ordinary signal to
    /// [`GraphKey::Control`] (plan §4.6). Never redirects out of a wrapper
    /// subgraph — see the module docs' "Control graph" section for why that
    /// is deliberately out of scope here.
    fn effective_key(&self, base: GraphKey, sig: SigId) -> GraphKey {
        if base == GraphKey::Top {
            let variability = self
                .sig_types
                .get(&sig)
                .map_or(Variability::Samp, SigType::variability);
            if variability != Variability::Samp {
                return GraphKey::Control;
            }
        }
        base
    }

    fn visit(&mut self, sig: SigId) -> Result<(), HgraphError> {
        if !self.visited.insert(sig) {
            return Ok(());
        }
        let env = self.env_of(sig)?;

        if needs_subgraph(self.arena, sig) {
            // The wrapper node itself belongs to the outer graph (R_CD gives
            // it the parent env); its contents populate the subgraph.
            let outer_key = self.key_for(env);
            self.hgraph.graph_mut(outer_key).add_node(sig);
            let sub_key = GraphKey::Wrapper(sig);
            let _ = self.hgraph.graph_mut(sub_key);
            let inner_env = wrapper_inner_env(self.arena, sig);
            self.domain_key.insert(inner_env, sub_key);

            let projection =
                crate::signal_fir::vector_analysis::signal_dependencies(&self.analysis, sig)
                    .map_err(analysis_error_to_hgraph)?;
            let Some(clock_edge) = projection.scheduling().iter().find(|edge| {
                matches!(
                    edge.kind,
                    crate::signal_fir::vector_analysis::DepKind::ClockBoundary
                )
            }) else {
                return Ok(());
            };
            // The clock stays outside: block precondition in the outer graph.
            self.hgraph
                .graph_mut(outer_key)
                .add_edge(sig, clock_edge.to, false);
            self.visit(clock_edge.to)?;
            for edge in projection.scheduling().iter().filter(|edge| {
                !matches!(
                    edge.kind,
                    crate::signal_fir::vector_analysis::DepKind::ClockBoundary
                )
            }) {
                self.visit(edge.to)?;
            }
            return Ok(());
        }

        let base_key = self.key_for(env);
        let own_key = self.effective_key(base_key, sig);
        self.hgraph.graph_mut(own_key).add_node(sig);

        for edge in get_signal_dependencies(&self.analysis, sig)? {
            // Wrapper deps carry the parent env; ordinary deps their own.
            self.visit(edge.to)?;
            let dep_env = self.env_of(edge.to)?;
            if dep_env == env {
                let dep_key = self.effective_key(base_key, edge.to);
                if own_key != GraphKey::Control && dep_key == GraphKey::Control {
                    // Control is an unconditional precondition of every other
                    // graph, not a schedule-dependent same-graph ordering
                    // edge (module docs, "Control graph").
                    continue;
                }
                // Same-domain: ordering edge (immediate) or placement-only
                // edge (delayed) inside the same graph.
                self.hgraph
                    .graph_mut(own_key)
                    .add_edge(sig, edge.to, edge.delayed);
            } else if is_ancestor_clk_env(self.domains, dep_env, env) {
                // External (strict-ancestor) dependency: no intra-graph
                // ordering edge; surface the block precondition on the
                // enclosing wrapper in the wrapper's own graph.
                if let GraphKey::Wrapper(w) = own_key {
                    let wrapper_env = self.env_of(w)?;
                    let wrapper_key = self.key_for(wrapper_env);
                    self.hgraph
                        .graph_mut(wrapper_key)
                        .add_edge(w, edge.to, false);
                }
            } else {
                // Deeper/sibling deps only occur through wrapper nodes; a
                // well-clocked forest cannot reach here (inference would have
                // rejected it). Record the edge to stay conservative.
                self.hgraph
                    .graph_mut(own_key)
                    .add_edge(sig, edge.to, edge.delayed);
            }
        }
        Ok(())
    }
}

/// Decodes the inner domain of one wrapper from its first payload child.
fn wrapper_inner_env(arena: &TreeArena, sig: SigId) -> ClkEnv {
    let children = match match_sig(arena, sig) {
        SigMatch::OnDemand(c) | SigMatch::Upsampling(c) | SigMatch::Downsampling(c) => c,
        _ => return None,
    };
    let &first = children.first()?;
    let SigMatch::Clocked(env, _) = match_sig(arena, first) else {
        return None;
    };
    match match_sig(arena, env) {
        SigMatch::ClockEnvToken(id) => Some(ClockDomainId::from_u32(id)),
        _ => None,
    }
}

/// Builds the hierarchical dependency graph from the prepared outputs.
///
/// `sig_types` drives the [`GraphKey::Control`] redirect (plan §4.6): pass
/// the prepared forest's full `SigType` map (e.g.
/// `PreparedSignals::sig_types_map`) so control/lifecycle signals separate
/// from the top graph. An empty map is safe — every signal is then
/// conservatively treated as `Samp` and routing degrades to the pre-P3
/// behavior (no `Control` graph populated).
pub fn build_hgraph(
    arena: &TreeArena,
    domains: &ClockDomainTable,
    envs: &ClkEnvMap,
    outputs: &[SigId],
    sig_types: &HashMap<SigId, SigType>,
) -> Result<Hgraph, HgraphError> {
    let analysis =
        crate::signal_fir::vector_analysis::SignalAnalysisContext::new(arena, sig_types, outputs)
            .map_err(analysis_error_to_hgraph)?;
    let mut builder = Builder {
        arena,
        analysis,
        domains,
        envs,
        sig_types,
        hgraph: Hgraph::default(),
        domain_key: AHashMap::from_iter([(None, GraphKey::Top)]),
        visited: AHashSet::new(),
    };
    // Materialize the top graph first for deterministic ordering, matching
    // pre-P3 behavior. `Control` is deliberately *not* eagerly materialized
    // here: unlike `Top`, most programs redirect nothing to it, and an
    // always-present (possibly empty) `Control` entry would silently change
    // `Hgraph::graphs().len()` for every caller — including ones with no
    // Konst/Block top-level signal at all. `Builder::graph_mut` creates it
    // lazily on the first redirect, so `graphs()` only gains an entry when
    // `Control` genuinely owns something.
    let _ = builder.hgraph.graph_mut(GraphKey::Top);
    for &out in outputs {
        builder.visit(out)?;
    }
    let hgraph = builder.hgraph;
    debug_assert!(
        audit_hgraph(&hgraph).is_ok(),
        "hgraph partition property violated: {:?}",
        audit_hgraph(&hgraph)
    );
    debug_assert!(
        audit_control_variability(&hgraph, sig_types).is_ok(),
        "hgraph control-variability property violated: {:?}",
        audit_control_variability(&hgraph, sig_types)
    );
    Ok(hgraph)
}

/// C++ `auditHgraph`: every signal is *owned* by exactly one graph.
pub fn audit_hgraph(hgraph: &Hgraph) -> Result<(), HgraphError> {
    let mut seen: AHashSet<SigId> = AHashSet::new();
    for (_, graph) in hgraph.graphs() {
        for &sig in graph.nodes() {
            if !seen.insert(sig) {
                return Err(HgraphError::PartitionViolated { sig });
            }
        }
    }
    Ok(())
}

/// Extended audit (plan §4.6): [`GraphKey::Control`] never owns a
/// `Samp`-variability signal. A signal absent from `sig_types` cannot
/// violate this (the builder's redirect only ever *adds to* `Control` when
/// it positively knows a non-`Samp` variability; a missing entry keeps a
/// signal at its clock-domain-routed graph).
pub fn audit_control_variability(
    hgraph: &Hgraph,
    sig_types: &HashMap<SigId, SigType>,
) -> Result<(), HgraphError> {
    let Some(control) = hgraph.graph(GraphKey::Control) else {
        return Ok(());
    };
    for &sig in control.nodes() {
        if sig_types
            .get(&sig)
            .is_some_and(|ty| ty.variability() == Variability::Samp)
        {
            return Err(HgraphError::ControlVariabilityViolated { sig });
        }
    }
    Ok(())
}

/// One serialized per-graph schedule.
#[derive(Clone, Debug, Default)]
pub struct Hsched {
    /// `(key, schedule)` pairs in the same deterministic order as
    /// [`Hgraph::graphs`]. Each schedule lists the graph's owned signals in
    /// dependencies-first order on immediate edges.
    pub schedules: Vec<(GraphKey, Vec<SigId>)>,
}

impl Hsched {
    /// Returns the schedule of one graph.
    #[must_use]
    pub fn schedule(&self, key: GraphKey) -> Option<&[SigId]> {
        self.schedules
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, s)| s.as_slice())
    }
}

/// Serializes each per-domain graph independently under `strategy`, using
/// the shared generic scheduler (`crate::schedule`, plan phase P1) through
/// the [`Digraph`] [`crate::schedule::ScheduleDag`] adapter — the same four
/// `-ss` strategies as everywhere else, replacing this module's former
/// hand-rolled depth-first walk (C++ `dfschedule` was the only strategy
/// available here before P1 existed).
///
/// Instantaneous in-graph cycles are causality errors, reported per graph.
/// [`crate::schedule::ScheduleError::SelfEdge`] and
/// [`crate::schedule::ScheduleError::Cycle`] both fold into
/// [`HgraphError::InstantaneousCycle`] (a self-edge is a length-1 cycle);
/// `sig` is the cycle's first stable-sorted member for a multi-node cycle,
/// matching this module's existing single-node error shape rather than
/// widening it to carry the whole cycle.
pub fn schedule(hgraph: &Hgraph, strategy: SchedulingStrategy) -> Result<Hsched, HgraphError> {
    let mut out = Hsched::default();
    for (key, graph) in hgraph.graphs() {
        let order = crate::schedule::schedule(strategy, graph).map_err(|err| match err {
            crate::schedule::ScheduleError::SelfEdge { node } => HgraphError::InstantaneousCycle {
                key: *key,
                sig: node,
            },
            crate::schedule::ScheduleError::Cycle { remaining } => {
                HgraphError::InstantaneousCycle {
                    key: *key,
                    sig: remaining[0],
                }
            }
        })?;
        out.schedules.push((*key, order));
    }
    Ok(out)
}

/// Adds strategy-independent ordering edges between conflicting compute-time
/// effects.
///
/// The baseline is the deterministic depth-first linear extension of each
/// graph's data dependencies. Conflicting nodes that are not already ordered
/// are chained in that baseline order. Every public scheduling strategy then
/// observes the same effect constraints and may reorder only commuting work.
///
/// This is the scalar counterpart of vector-plan effect orientation. The
/// supplied facts come from the canonical signal-level analysis rather than
/// from generated FIR statements.
pub fn orient_effect_conflicts(
    hgraph: &mut Hgraph,
    effects: &crate::signal_fir::vector_analysis::ScalarSchedulingEffects,
) -> Result<(), HgraphError> {
    use crate::signal_fir::vector_analysis::{EffectAtom, ForeignPurity};

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum EffectOrderKey {
        State(crate::signal_fir::vector_analysis::StateResource),
        Table(u32),
        Ui(u32),
        Output(u32),
    }

    fn add_baseline_edge(
        hgraph: &mut Hgraph,
        graph_index: usize,
        position: &AHashMap<SigId, usize>,
        left: SigId,
        right: SigId,
    ) {
        if left == right {
            return;
        }
        let graph = &hgraph.graphs[graph_index].1;
        if dependency_reachable(graph, left, right) || dependency_reachable(graph, right, left) {
            return;
        }
        let (consumer, dependency) = if position[&left] < position[&right] {
            (right, left)
        } else {
            (left, right)
        };
        hgraph.graphs[graph_index]
            .1
            .add_edge(consumer, dependency, false);
    }

    for graph_index in 0..hgraph.graphs.len() {
        let graph = &hgraph.graphs[graph_index].1;
        let baseline =
            crate::schedule::schedule(SchedulingStrategy::DepthFirst, graph).map_err(|err| {
                match err {
                    crate::schedule::ScheduleError::SelfEdge { node } => {
                        HgraphError::InstantaneousCycle {
                            key: hgraph.graphs[graph_index].0,
                            sig: node,
                        }
                    }
                    crate::schedule::ScheduleError::Cycle { remaining } => {
                        HgraphError::InstantaneousCycle {
                            key: hgraph.graphs[graph_index].0,
                            sig: remaining[0],
                        }
                    }
                }
            })?;
        let position = baseline
            .iter()
            .enumerate()
            .map(|(position, &sig)| (sig, position))
            .collect::<AHashMap<_, _>>();
        let nodes = graph
            .nodes()
            .iter()
            .copied()
            .filter(|&sig| !effects.direct_effects(sig).is_empty())
            .collect::<Vec<_>>();

        // The former all-pairs scan was quadratic in the number of stateful
        // nodes, even when every node touched a different resource. Grouping
        // by resource retains the baseline order of each conflict class with
        // adjacent edges only, which is its transitive reduction. Read/read
        // pairs in one group become ordered too; that is conservative and does
        // not change the observable state value they read.
        let mut groups = BTreeMap::<EffectOrderKey, Vec<SigId>>::new();
        let mut foreign_barriers = Vec::new();
        for &sig in &nodes {
            for effect in effects.direct_effects(sig) {
                let key = match effect {
                    EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource) => {
                        Some(EffectOrderKey::State(resource.clone()))
                    }
                    EffectAtom::ReadTable(table) | EffectAtom::WriteTable(table) => {
                        Some(EffectOrderKey::Table(*table))
                    }
                    EffectAtom::WriteUi(control) => Some(EffectOrderKey::Ui(*control)),
                    EffectAtom::WriteOutput(output) => Some(EffectOrderKey::Output(*output)),
                    EffectAtom::Foreign {
                        purity: ForeignPurity::Impure | ForeignPurity::Unknown,
                        ..
                    } => {
                        foreign_barriers.push(sig);
                        None
                    }
                    EffectAtom::Foreign { .. } => None,
                };
                if let Some(key) = key {
                    groups.entry(key).or_default().push(sig);
                }
            }
        }

        for group in groups.values_mut() {
            group.sort_unstable_by_key(|sig| position[sig]);
            group.dedup();
            for pair in group.windows(2) {
                add_baseline_edge(hgraph, graph_index, &position, pair[0], pair[1]);
            }
        }
        foreign_barriers.sort_unstable_by_key(|sig| position[sig]);
        foreign_barriers.dedup();
        for foreign in foreign_barriers {
            for &node in &nodes {
                add_baseline_edge(hgraph, graph_index, &position, foreign, node);
            }
        }
    }
    Ok(())
}

fn dependency_reachable(graph: &Digraph, from: SigId, to: SigId) -> bool {
    let mut stack = vec![from];
    let mut visited = AHashSet::new();
    while let Some(sig) = stack.pop() {
        if !visited.insert(sig) {
            continue;
        }
        for edge in graph.edges(sig) {
            if edge.delayed || !graph.contains(edge.to) {
                continue;
            }
            if edge.to == to {
                return true;
            }
            stack.push(edge.to);
        }
    }
    false
}

#[cfg(test)]
mod tests;
