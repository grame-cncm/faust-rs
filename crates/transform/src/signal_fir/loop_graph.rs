//! Loop graph for vector mode (`-vec`) — roadmap P6, vector doc V2.
//!
//! Scalar mode compiles the whole per-sample block into one `for i in 0..count`
//! loop. Vector mode restructures it into an **outer chunk loop** of `vec_size`
//! samples containing a **DAG of small inner loops** — one per recursive group,
//! per delayed-or-shared signal, etc. — so the C compiler can auto-vectorize the
//! non-recursive ones (SIMD), while recursive computations stay in serial loops.
//!
//! This module owns the loop-DAG **data model** and its **deterministic
//! levelization** (a port of the C++ `sortGraph`, whose `std::set<Loop*>` is
//! pointer-ordered and therefore non-deterministic across runs — here loops are
//! keyed by insertion-ordered [`LoopId`], so emission order is stable). Two
//! later slices consume it:
//!
//! - **V3–V4** populate it from the signal lowering (a current-loop stack
//!   mirroring the C++ `openLoop`/`closeLoop`, the `needSeparateLoop` criterion,
//!   cross-loop chunk buffers, and vector delay-line layouts);
//! - **V5** emits it (each [`LoopNode`] becomes a chunk `for` with its
//!   pre/exec/post phases; levels drive `// Section : n` grouping).
//!
//! Nothing here is wired into scalar codegen yet, so it cannot affect existing
//! output; the `dead_code` allowance is removed when V3 starts populating it.
#![allow(dead_code)]

use std::collections::BTreeSet;

use ahash::AHashMap;
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};
use signals::SigId;
use sigtype::Variability;

use crate::schedule::ScheduleDag;

/// Index of a loop node in a [`LoopGraph`].
///
/// Allocation order == insertion order, and every set/queue below is
/// `LoopId`-ordered, so the levelization and emission are deterministic — the
/// fix for the C++ pointer-ordered `lset` non-determinism.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct LoopId(pub(crate) u32);

/// Whether a chunk loop may be auto-vectorized, and why not when it may not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoopKind {
    /// Non-recursive: the inner `for` is a candidate for auto-vectorization.
    Vectorizable,
    /// A recursive group (`maxDelay > 0` back-edge / recursive projection):
    /// must run serially, one sample after another.
    Recursive,
    /// A clocked-domain block (`ondemand`/`upsampling`/`downsampling`): a serial
    /// **scalar island** (vector doc §6, rule D1). Its externals are chunk
    /// buffers; its inner-domain state stays scalar.
    Island,
}

impl LoopKind {
    /// Whether the C backend may auto-vectorize this loop's inner body.
    #[must_use]
    pub(crate) fn is_vectorizable(self) -> bool {
        matches!(self, Self::Vectorizable)
    }
}

/// One chunk loop: three phase statement lists plus its backward dependencies.
///
/// The three phases mirror the C++ `fPreCode` / `fExecCode` / `fPostCode`
/// printed around the per-chunk `for`: `pre` is the head-copy / index setup,
/// `exec` is the chunk body (`for i in 0..count`), `post` is the tail-copy /
/// index save. Scalar-equivalent loops leave `pre`/`post` empty.
#[derive(Clone, Debug)]
pub(crate) struct LoopNode {
    /// Vectorizable / recursive / island classification.
    pub(crate) kind: LoopKind,
    /// Whether the chunk `for` runs in reverse sample time (RAD/BRA).
    pub(crate) is_reverse: bool,
    /// Statements emitted **before** the chunk `for` (per-chunk setup / head copy).
    pub(crate) pre: Vec<FirId>,
    /// Statements forming the chunk `for` body (`for i in 0..count`).
    pub(crate) exec: Vec<FirId>,
    /// Statements emitted **after** the chunk `for` (tail copy / index save).
    pub(crate) post: Vec<FirId>,
    /// Loops that must run before this one (this loop reads their chunk buffers).
    pub(crate) deps: BTreeSet<LoopId>,
}

impl LoopNode {
    fn new(kind: LoopKind, is_reverse: bool) -> Self {
        Self {
            kind,
            is_reverse,
            pre: Vec::new(),
            exec: Vec::new(),
            post: Vec::new(),
            deps: BTreeSet::new(),
        }
    }
}

/// A DAG of chunk loops. Nodes are stored in insertion order; edges are backward
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
    pub(crate) fn add_loop(&mut self, kind: LoopKind, is_reverse: bool) -> LoopId {
        let id = LoopId(u32::try_from(self.nodes.len()).expect("loop count fits u32"));
        self.nodes.push(LoopNode::new(kind, is_reverse));
        id
    }

    /// Number of loops.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph has no loops.
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

/// [`crate::schedule::ScheduleDag`] adapter (roadmap P1). `LoopGraph` is
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

// ── Loop-separation criterion (V3) ──────────────────────────────────────────
//
// A port of the C++ `needSeparateLoop` (`compile_vect.cpp:304-339`,
// `dag_instructions_compiler.cpp:370-393`; the table is in the vector doc §2).
// This is the *decision*: given a sample signal's properties, does it get its
// own chunk loop, and may that loop vectorize? The lowering (V4) extracts the
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

impl LoopSeparation {
    /// The [`LoopKind`] a *separated* verdict maps to (`None` for `Inline`).
    #[must_use]
    pub(crate) fn loop_kind(self) -> Option<LoopKind> {
        match self {
            Self::Inline => None,
            Self::SeparateVectorizable => Some(LoopKind::Vectorizable),
            Self::SeparateSerial => Some(LoopKind::Recursive),
        }
    }
}

/// Decides whether `props` requires its own chunk loop (vector doc §2 table;
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

// ── Loop-carried classification (V5b, separation foundation) ─────────────────
//
// Actually *splitting* a slice's recursive core out from its vectorizable
// pre/post parts (with cross-loop chunk buffers) requires loop-aware lowering:
// the fused sample body (`fRecCur = fRec + …; out = f(fRecCur); fRec = fRecCur`)
// is a single loop-carried chain by the time it reaches the FIR, so it cannot be
// partitioned after the fact. What *is* recoverable from the FIR is whether a
// slice carries persistent (cross-sample) state at all — the classification that
// decides `Recursive` vs `Vectorizable`, and whether chunking it can pay off.

/// Whether a sample-loop slice writes persistent (cross-sample) DSP state — a
/// `Struct`-access `StoreVar`/`StoreTable` (a recursion carrier `fRec*`, a delay
/// line, an `fIOTA`, …). Such a slice has a loop-carried dependency, so its inner
/// loop cannot be auto-vectorized as one block: it is `LoopKind::Recursive`. A
/// slice that writes no state is `LoopKind::Vectorizable`.
#[must_use]
pub(crate) fn slice_has_persistent_state(store: &FirStore, statements: &[FirId]) -> bool {
    statements
        .iter()
        .any(|&s| node_writes_struct_state(store, s))
}

fn node_writes_struct_state(store: &FirStore, node: FirId) -> bool {
    match match_fir(store, node) {
        FirMatch::StoreVar {
            access: AccessType::Struct,
            ..
        }
        | FirMatch::StoreTable {
            access: AccessType::Struct,
            ..
        } => true,
        // Recurse into structural bodies (guarded clocked blocks, loops, blocks).
        FirMatch::Block(body) => body.iter().any(|&s| node_writes_struct_state(store, s)),
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => {
            node_writes_struct_state(store, then_block)
                || else_block.is_some_and(|e| node_writes_struct_state(store, e))
        }
        FirMatch::Control { stmt, .. } => node_writes_struct_state(store, stmt),
        FirMatch::SimpleForLoop { body, .. }
        | FirMatch::ForLoop { body, .. }
        | FirMatch::IteratorForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. } => node_writes_struct_state(store, body),
        _ => false,
    }
}

// ── loop_env — signal-level loop assignment (vector doc S-A) ─────────────────
//
// The loop analog of `clk_env::annotate`: a memoized DFS over the sample-signal
// DAG that assigns each signal to a [`LoopId`] via [`needs_separate_loop`] and
// records the loop dependency edges into a [`LoopGraph`] *shape* (no statements
// yet — those are routed in a later slice). Kept pure — signal properties come in
// through a caller-supplied closure, exactly as [`needs_separate_loop`] takes its
// props — so the assignment algorithm is unit-testable without the lowering's
// delay/sharing/variability analyses. Cycles (recursive back-edges) terminate on
// the memo: a revisit only records the dependency edge.

/// Result of [`assign_loops`]: which loop each visited signal lives in, plus the
/// populated (statement-free) loop graph with its dependency edges.
#[derive(Debug)]
pub(crate) struct LoopAssignment {
    /// `SigId → LoopId`. A separated signal maps to its own loop; an inlined one
    /// maps to its consumer's loop.
    pub(crate) map: AHashMap<SigId, LoopId>,
    /// The loop graph (nodes = loops, edges = "reads the output of").
    pub(crate) graph: LoopGraph,
    /// The root loop every output starts in.
    pub(crate) root: LoopId,
}

impl LoopAssignment {
    /// The loop a signal was assigned to, if it was visited.
    #[must_use]
    pub(crate) fn loop_of(&self, sig: SigId) -> Option<LoopId> {
        self.map.get(&sig).copied()
    }
}

/// The **sample-value operands** of a signal — the edges [`assign_loops`] should
/// follow. `match_sig` already decodes the op-code / control ids out of the enum,
/// so this returns only the `SigId` value fields, never the raw arena children
/// (which include the op-code atom and constant indices — following those, since
/// `int(0)` is hash-consed, would fabricate a spurious cross-loop edge).
///
/// The decoded rules are shared with Hgraph and PV through
/// [`super::vector_analysis::signal_dependencies`]. Malformed list payloads and
/// unsupported legacy recursion are reported instead of becoming silent leaves.
pub(crate) fn signal_value_children(
    analysis: &super::vector_analysis::SignalAnalysisContext<'_>,
    sig: SigId,
) -> Result<Vec<SigId>, super::vector_analysis::AnalysisError> {
    super::vector_analysis::signal_dependencies(analysis, sig).map(|dependencies| {
        dependencies
            .scheduling()
            .iter()
            .map(|dependency| dependency.to)
            .collect()
    })
}

/// Assigns every sample signal reachable from `outputs` to a loop.
///
/// `children(sig)` yields the signal's **sample-value operands** — the edges to
/// follow. Non-value children (an op-code atom, a constant delay/index, a
/// clock-env token) must be excluded: a shared constant node would otherwise
/// fabricate a spurious cross-loop edge (and a cycle). `props(sig)` supplies the
/// [`SignalLoopProps`] the [`needs_separate_loop`] verdict is computed from. Both
/// are caller-supplied so the assignment stays a pure, testable graph algorithm
/// decoupled from the signal-specific value-child extraction (wired later).
///
/// A signal that needs a separate loop opens a new [`LoopNode`] (serial for a
/// recursive projection, vectorizable otherwise) and the enclosing loop gains a
/// dependency edge on it; the rest inline into their consumer's loop. Cycles
/// (recursive back-edges) terminate on the memo — a revisit only records an edge.
pub(crate) fn assign_loops(
    outputs: &[SigId],
    mut children: impl FnMut(SigId) -> Vec<SigId>,
    mut props: impl FnMut(SigId) -> SignalLoopProps,
) -> LoopAssignment {
    let mut graph = LoopGraph::new();
    // Every output starts in the top-level sample loop.
    let root = graph.add_loop(LoopKind::Vectorizable, false);
    let mut map = AHashMap::new();
    for &out in outputs {
        assign_one(&mut graph, &mut map, &mut children, &mut props, out, root);
    }
    LoopAssignment { map, graph, root }
}

fn assign_one(
    graph: &mut LoopGraph,
    map: &mut AHashMap<SigId, LoopId>,
    children: &mut impl FnMut(SigId) -> Vec<SigId>,
    props: &mut impl FnMut(SigId) -> SignalLoopProps,
    sig: SigId,
    current: LoopId,
) {
    if let Some(&loop_s) = map.get(&sig) {
        // Already placed; the current loop reads it → a cross-loop edge (a
        // self-edge, i.e. same loop, is ignored by `add_dep`).
        graph.add_dep(current, loop_s);
        return;
    }
    let child_loop = match needs_separate_loop(&props(sig)).loop_kind() {
        Some(kind) => {
            let l = graph.add_loop(kind, false);
            graph.add_dep(current, l);
            map.insert(sig, l);
            l
        }
        None => {
            map.insert(sig, current);
            current
        }
    };
    for child in children(sig) {
        assign_one(graph, map, children, props, child, child_loop);
    }
}

// ── Cross-loop chunk buffers (vector doc §4, S-C) ────────────────────────────
//
// A sample value produced in one loop and consumed in another is materialized in
// a `vec_size`-element array, indexed by the **chunk-local** `i0 - vindex` so the
// producing store and the consuming load address the same slot within the chunk.
// This keeps V5's "global `i0`, no I/O rebasing" bit-exactness. The mechanism is
// pure FIR building; S-D wires it into the split emission.

/// A cross-loop chunk buffer `<elem> vbufN[vec_size]`.
#[derive(Clone, Debug)]
pub(crate) struct ChunkBuffer {
    name: String,
    elem: FirType,
    vec_size: u32,
}

impl ChunkBuffer {
    /// A fresh buffer with the deterministic name `vbuf<index>`.
    #[must_use]
    pub(crate) fn new(index: u32, elem: FirType, vec_size: u32) -> Self {
        Self {
            name: format!("vbuf{index}"),
            elem,
            vec_size,
        }
    }

    /// The buffer's variable name.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// `<elem> vbufN[vec_size];` — the stack-array declaration (loop pre-phase).
    pub(crate) fn declare(&self, store: &mut FirStore) -> FirId {
        let ty = FirType::Array(Box::new(self.elem.clone()), self.vec_size as usize);
        FirBuilder::new(store).declare_var(self.name.clone(), ty, AccessType::Stack, None)
    }

    /// The chunk-local index `i0 - vindex` (Int32).
    fn chunk_index(store: &mut FirStore) -> FirId {
        let mut b = FirBuilder::new(store);
        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let vindex = b.load_var("vindex", AccessType::Loop, FirType::Int32);
        b.binop(FirBinOp::Sub, i0, vindex, FirType::Int32)
    }

    /// `vbufN[i0 - vindex] = value;` — emitted in the producing loop's `exec`.
    pub(crate) fn store(&self, store: &mut FirStore, value: FirId) -> FirId {
        let idx = Self::chunk_index(store);
        FirBuilder::new(store).store_table(self.name.clone(), AccessType::Stack, idx, value)
    }

    /// `vbufN[i0 - vindex]` — read in the consuming loop's `exec`.
    pub(crate) fn load(&self, store: &mut FirStore) -> FirId {
        let idx = Self::chunk_index(store);
        FirBuilder::new(store).load_table(
            self.name.clone(),
            AccessType::Stack,
            idx,
            self.elem.clone(),
        )
    }
}

// ── Recursive-slice partition (vector doc S-D, "pure tail") ──────────────────
//
// A recursive slice reaches the FIR as a fused loop-carried chain:
//
//     fRecCur = fRec + 2*input0[i0];   // serial: reads state fRec
//     output0[i0] = 0.5 * fRecCur;     // state-free tail: reads fRecCur
//     fRec = fRecCur;                   // serial: writes state fRec
//
// [`partition_recursive_body`] splits it into a **serial core** (everything that
// touches state, plus every temp such a statement needs) and a **vectorizable
// tail** (the state-free statements), connected by chunk buffers on the boundary
// temps (`fRecCur`). Running the serial core over the whole chunk first (buffering
// the boundary temps) then the tail is bit-exact: state evolves exactly as in the
// fused loop, and the tail reads the same per-sample values back.
//
// The split is valid because the fixpoint below closes the serial set under
// "producers of temps a serial statement reads": no serial statement can then read
// a tail-produced temp, so reordering the tail after the whole serial core cannot
// change any serial result. If the body has no state-free statement (nothing to
// hoist) or any unsupported statement shape, the analysis returns `None` and
// emission falls back to the single fused loop (still bit-exact).

/// A recursive slice split into a serial core and a vectorizable tail.
#[derive(Debug)]
pub(crate) struct RecursivePartition {
    /// Statements that must run serially (touch state, or feed something that does).
    pub(crate) serial: Vec<FirId>,
    /// State-free statements that can run in a vectorizable loop.
    pub(crate) vectorizable: Vec<FirId>,
    /// Serial-produced temps read by the tail — each becomes a chunk buffer
    /// `(name, element type)`, in deterministic producer order.
    pub(crate) boundary: Vec<(String, FirType)>,
}

/// Collects every storage reference occurring in a FIR value tree.
///
/// The recursive-slice split must be conservative: a missed nested read can make
/// the partition hoist a tail past the serial state update that it still
/// observes. Keep this traversal symmetric with [`rewrite_var_loads`].
fn collect_var_loads(store: &FirStore, node: FirId, out: &mut Vec<(String, AccessType)>) -> bool {
    match match_fir(store, node) {
        FirMatch::Int32 { .. }
        | FirMatch::Int64 { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. }
        | FirMatch::Bool { .. }
        | FirMatch::Quad { .. }
        | FirMatch::FixedPoint { .. }
        | FirMatch::Int32Array { .. }
        | FirMatch::Float32Array { .. }
        | FirMatch::Float64Array { .. }
        | FirMatch::QuadArray { .. }
        | FirMatch::FixedPointArray { .. }
        | FirMatch::NullValue { .. }
        | FirMatch::NewDsp { .. } => true,
        FirMatch::LoadVar { name, access, .. } | FirMatch::LoadVarAddress { name, access, .. } => {
            out.push((name, access));
            true
        }
        FirMatch::ValueArray { values, .. } => {
            for v in values {
                if !collect_var_loads(store, v, out) {
                    return false;
                }
            }
            true
        }
        FirMatch::BinOp { lhs, rhs, .. } => {
            collect_var_loads(store, lhs, out) && collect_var_loads(store, rhs, out)
        }
        FirMatch::Neg { value, .. }
        | FirMatch::Cast { value, .. }
        | FirMatch::Bitcast { value, .. } => collect_var_loads(store, value, out),
        FirMatch::TeeVar {
            name,
            access,
            value,
            ..
        } => {
            // `TeeVar` is a value expression with a side effect; treating the
            // target as a reference makes `Struct` tees stay in the serial core.
            out.push((name, access));
            collect_var_loads(store, value, out)
        }
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            collect_var_loads(store, cond, out)
                && collect_var_loads(store, then_value, out)
                && collect_var_loads(store, else_value, out)
        }
        FirMatch::FunCall { args, .. } => {
            for a in args {
                if !collect_var_loads(store, a, out) {
                    return false;
                }
            }
            true
        }
        FirMatch::LoadSoundfileLength { part, .. } | FirMatch::LoadSoundfileRate { part, .. } => {
            collect_var_loads(store, part, out)
        }
        FirMatch::LoadSoundfileBuffer {
            chan, part, idx, ..
        } => {
            collect_var_loads(store, chan, out)
                && collect_var_loads(store, part, out)
                && collect_var_loads(store, idx, out)
        }
        FirMatch::LoadTable {
            name,
            access,
            index,
            ..
        } => {
            out.push((name, access));
            collect_var_loads(store, index, out)
        }
        // Statement/control/UI/module nodes are not value trees accepted by the
        // split analysis. Falling back keeps the fused serial loop.
        _ => false,
    }
}

/// A list of `(variable name, access)` references (reads or writes).
type VarRefs = Vec<(String, AccessType)>;

/// A statement's written and read vars, or `None` for a kind the split does not
/// handle (nested `If`/loops/blocks — e.g. a clocked island — which force the
/// single-loop fallback).
fn stmt_reads_writes(store: &FirStore, stmt: FirId) -> Option<(VarRefs, VarRefs)> {
    let mut reads = Vec::new();
    let writes = match match_fir(store, stmt) {
        FirMatch::DeclareVar {
            name, access, init, ..
        } => {
            if let Some(init) = init
                && !collect_var_loads(store, init, &mut reads)
            {
                return None;
            }
            vec![(name, access)]
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            if !collect_var_loads(store, value, &mut reads) {
                return None;
            }
            vec![(name, access)]
        }
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => {
            if !collect_var_loads(store, index, &mut reads)
                || !collect_var_loads(store, value, &mut reads)
            {
                return None;
            }
            vec![(name, access)]
        }
        _ => return None,
    };
    Some((writes, reads))
}

/// Partitions a **flat** recursive sample-loop body into a serial core and a
/// vectorizable tail (vector doc §5 S-D). Returns `None` — "keep the single fused
/// loop" — when the body has an unsupported statement shape or no state-free
/// statement worth hoisting.
#[must_use]
pub(crate) fn partition_recursive_body(
    store: &FirStore,
    exec: &[FirId],
) -> Option<RecursivePartition> {
    let n = exec.len();
    // Per-statement (writes, reads); bail on any unsupported statement kind.
    let mut rw = Vec::with_capacity(n);
    for &s in exec {
        rw.push(stmt_reads_writes(store, s)?);
    }

    // Stack temp name -> index of the statement that declares/stores it.
    let mut producer: AHashMap<String, usize> = AHashMap::new();
    for (i, (writes, _)) in rw.iter().enumerate() {
        for (name, access) in writes {
            if *access == AccessType::Stack {
                producer.insert(name.clone(), i);
            }
        }
    }

    // Seed serial with every statement that reads or writes Struct state.
    let mut serial = vec![false; n];
    for (i, (writes, reads)) in rw.iter().enumerate() {
        if writes
            .iter()
            .chain(reads.iter())
            .any(|(_, a)| *a == AccessType::Struct)
        {
            serial[i] = true;
        }
    }
    // Fixpoint: the producer of any Stack temp a serial statement reads is serial.
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            if !serial[i] {
                continue;
            }
            for (name, access) in &rw[i].1 {
                if *access == AccessType::Stack
                    && let Some(&p) = producer.get(name)
                    && !serial[p]
                {
                    serial[p] = true;
                    changed = true;
                }
            }
        }
    }

    let vec_idx: Vec<usize> = (0..n).filter(|&i| !serial[i]).collect();
    if vec_idx.is_empty() {
        return None; // nothing state-free to hoist — no vectorization benefit
    }

    // Boundary temps: Stack temps produced in the serial core and read by the
    // tail, gathered in producer order for a deterministic buffer numbering.
    let mut boundary_idx: Vec<usize> = Vec::new();
    let mut is_boundary = vec![false; n];
    for &i in &vec_idx {
        for (name, access) in &rw[i].1 {
            if *access == AccessType::Stack
                && let Some(&p) = producer.get(name)
                && serial[p]
                && !is_boundary[p]
            {
                is_boundary[p] = true;
                boundary_idx.push(p);
            }
        }
    }
    boundary_idx.sort_unstable();

    let mut boundary = Vec::with_capacity(boundary_idx.len());
    for &p in &boundary_idx {
        // Only a `DeclareVar` temp carries a concrete element type to buffer.
        let FirMatch::DeclareVar { name, typ, .. } = match_fir(store, exec[p]) else {
            return None;
        };
        boundary.push((name, typ));
    }

    let serial_stmts: Vec<FirId> = (0..n).filter(|&i| serial[i]).map(|i| exec[i]).collect();
    let vectorizable: Vec<FirId> = vec_idx.iter().map(|&i| exec[i]).collect();

    Some(RecursivePartition {
        serial: serial_stmts,
        vectorizable,
        boundary,
    })
}

/// Rebuilds a value tree (or supported statement), replacing every `LoadVar`
/// whose name is a key of `repl` with `repl[name]` (a chunk-buffer load).
/// Returns `node` unchanged when nothing matched, preserving interned identity
/// on subtrees that do not touch a boundary temp.
///
/// Returns `None` when a boundary value is used in a shape that cannot be
/// represented as a scalar chunk-buffer load (for example taking its address).
#[must_use]
pub(crate) fn rewrite_var_loads(
    store: &mut FirStore,
    node: FirId,
    repl: &AHashMap<String, FirId>,
) -> Option<FirId> {
    match match_fir(store, node) {
        FirMatch::LoadVar { name, .. } => Some(repl.get(&name).copied().unwrap_or(node)),
        FirMatch::LoadVarAddress { name, .. } => {
            if repl.contains_key(&name) {
                None
            } else {
                Some(node)
            }
        }
        FirMatch::Int32 { .. }
        | FirMatch::Int64 { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. }
        | FirMatch::Bool { .. }
        | FirMatch::Quad { .. }
        | FirMatch::FixedPoint { .. }
        | FirMatch::Int32Array { .. }
        | FirMatch::Float32Array { .. }
        | FirMatch::Float64Array { .. }
        | FirMatch::QuadArray { .. }
        | FirMatch::FixedPointArray { .. }
        | FirMatch::NullValue { .. }
        | FirMatch::NewDsp { .. } => Some(node),
        FirMatch::ValueArray { values, typ } => {
            let new_values: Option<Vec<FirId>> = values
                .iter()
                .map(|&v| rewrite_var_loads(store, v, repl))
                .collect();
            let new_values = new_values?;
            if new_values == values {
                Some(node)
            } else {
                Some(FirBuilder::new(store).value_array(&new_values, typ))
            }
        }
        FirMatch::BinOp { op, lhs, rhs, typ } => {
            let l = rewrite_var_loads(store, lhs, repl)?;
            let r = rewrite_var_loads(store, rhs, repl)?;
            if l == lhs && r == rhs {
                Some(node)
            } else {
                Some(FirBuilder::new(store).binop(op, l, r, typ))
            }
        }
        FirMatch::Neg { value, typ } => {
            let v = rewrite_var_loads(store, value, repl)?;
            if v == value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).neg(v, typ))
            }
        }
        FirMatch::Cast { typ, value } => {
            let v = rewrite_var_loads(store, value, repl)?;
            if v == value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).cast(typ, v))
            }
        }
        FirMatch::Bitcast { typ, value } => {
            let v = rewrite_var_loads(store, value, repl)?;
            if v == value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).bitcast(typ, v))
            }
        }
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            typ,
        } => {
            let c = rewrite_var_loads(store, cond, repl)?;
            let t = rewrite_var_loads(store, then_value, repl)?;
            let e = rewrite_var_loads(store, else_value, repl)?;
            if c == cond && t == then_value && e == else_value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).select2(c, t, e, typ))
            }
        }
        FirMatch::FunCall { name, args, typ } => {
            let new_args: Option<Vec<FirId>> = args
                .iter()
                .map(|&a| rewrite_var_loads(store, a, repl))
                .collect();
            let new_args = new_args?;
            if new_args == args {
                Some(node)
            } else {
                Some(FirBuilder::new(store).fun_call(name, &new_args, typ))
            }
        }
        FirMatch::LoadTable {
            name,
            access,
            index,
            typ,
        } => {
            if repl.contains_key(&name) {
                return None;
            }
            let idx = rewrite_var_loads(store, index, repl)?;
            if idx == index {
                Some(node)
            } else {
                Some(FirBuilder::new(store).load_table(name, access, idx, typ))
            }
        }
        FirMatch::LoadSoundfileLength { var, part } => {
            let new_part = rewrite_var_loads(store, part, repl)?;
            if new_part == part {
                Some(node)
            } else {
                Some(FirBuilder::new(store).load_soundfile_length(var, new_part))
            }
        }
        FirMatch::LoadSoundfileRate { var, part } => {
            let new_part = rewrite_var_loads(store, part, repl)?;
            if new_part == part {
                Some(node)
            } else {
                Some(FirBuilder::new(store).load_soundfile_rate(var, new_part))
            }
        }
        FirMatch::LoadSoundfileBuffer {
            var,
            chan,
            part,
            idx,
            typ,
        } => {
            let new_chan = rewrite_var_loads(store, chan, repl)?;
            let new_part = rewrite_var_loads(store, part, repl)?;
            let new_idx = rewrite_var_loads(store, idx, repl)?;
            if new_chan == chan && new_part == part && new_idx == idx {
                Some(node)
            } else {
                Some(
                    FirBuilder::new(store)
                        .load_soundfile_buffer(var, new_chan, new_part, new_idx, typ),
                )
            }
        }
        FirMatch::TeeVar {
            name,
            access,
            value,
            typ,
        } => {
            if repl.contains_key(&name) {
                return None;
            }
            let v = rewrite_var_loads(store, value, repl)?;
            if v == value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).tee_var(name, access, v, typ))
            }
        }
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => {
            if repl.contains_key(&name) {
                return None;
            }
            let idx = rewrite_var_loads(store, index, repl)?;
            let v = rewrite_var_loads(store, value, repl)?;
            if idx == index && v == value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).store_table(name, access, idx, v))
            }
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            if repl.contains_key(&name) {
                return None;
            }
            let v = rewrite_var_loads(store, value, repl)?;
            if v == value {
                Some(node)
            } else {
                Some(FirBuilder::new(store).store_var(name, access, v))
            }
        }
        FirMatch::DeclareVar {
            name,
            typ,
            access,
            init: Some(init),
        } => {
            if repl.contains_key(&name) {
                return None;
            }
            let ni = rewrite_var_loads(store, init, repl)?;
            if ni == init {
                Some(node)
            } else {
                Some(FirBuilder::new(store).declare_var(name, typ, access, Some(ni)))
            }
        }
        FirMatch::DeclareVar { .. } => Some(node),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use tlib::TreeArena;

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
            needs_separate_loop(&p).loop_kind(),
            Some(LoopKind::Recursive)
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
            needs_separate_loop(&delayed).loop_kind(),
            Some(LoopKind::Vectorizable)
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
        let a = g.add_loop(LoopKind::Vectorizable, false);
        let b = g.add_loop(LoopKind::Recursive, false);
        let c = g.add_loop(LoopKind::Island, true);
        assert_eq!(g.len(), 3);
        // No edges → insertion order, deterministically.
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
        assert!(g.node(a).kind.is_vectorizable());
        assert!(!g.node(b).kind.is_vectorizable());
        assert!(g.node(c).is_reverse);
    }

    #[test]
    fn dependencies_are_emitted_before_dependents() {
        // c depends on b, b depends on a → a, b, c regardless of alloc order.
        let mut g = LoopGraph::new();
        let a = g.add_loop(LoopKind::Vectorizable, false);
        let b = g.add_loop(LoopKind::Vectorizable, false);
        let c = g.add_loop(LoopKind::Vectorizable, false);
        g.add_dep(c, b);
        g.add_dep(b, a);
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
    }

    #[test]
    fn ready_frontier_is_loop_id_ordered() {
        // a is a shared root feeding b and c; b and c are independent, so they
        // come out in LoopId order (b before c), deterministically.
        let mut g = LoopGraph::new();
        let a = g.add_loop(LoopKind::Vectorizable, false);
        let b = g.add_loop(LoopKind::Vectorizable, false);
        let c = g.add_loop(LoopKind::Vectorizable, false);
        g.add_dep(b, a);
        g.add_dep(c, a);
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
    }

    #[test]
    fn self_edges_are_ignored() {
        let mut g = LoopGraph::new();
        let a = g.add_loop(LoopKind::Recursive, false);
        g.add_dep(a, a);
        assert!(g.node(a).deps.is_empty());
        assert_eq!(g.topological_order().unwrap(), vec![a]);
    }

    #[test]
    fn a_cycle_is_reported() {
        let mut g = LoopGraph::new();
        let a = g.add_loop(LoopKind::Vectorizable, false);
        let b = g.add_loop(LoopKind::Vectorizable, false);
        g.add_dep(a, b);
        g.add_dep(b, a);
        let err = g.topological_order().unwrap_err();
        assert_eq!(err.unscheduled, vec![a, b]);
    }

    /// The generic scheduler core (roadmap P1) must agree with
    /// `LoopGraph`'s own `topological_order` on the same DAG: build one
    /// through the existing `add_loop`/`add_dep` API, run all four
    /// `crate::schedule` strategies over it, and check every result against
    /// the independent `verify_schedule` checker.
    #[test]
    fn schedule_dag_conformance_through_the_existing_api() {
        use crate::schedule::{SchedulingStrategy, schedule, verify_schedule};

        let mut g = LoopGraph::new();
        let a = g.add_loop(LoopKind::Vectorizable, false);
        let b = g.add_loop(LoopKind::Vectorizable, false);
        let c = g.add_loop(LoopKind::Recursive, false);
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

    // ── loop_env assignment (S-A) ──
    use signals::SigBuilder;

    /// Three distinct signal ids to wire a mock value-child graph with.
    fn three_sigs() -> (TreeArena, SigId, SigId, SigId) {
        let mut arena = TreeArena::new();
        let a = SigBuilder::new(&mut arena).input(0);
        let b = SigBuilder::new(&mut arena).input(1);
        let c = SigBuilder::new(&mut arena).input(2);
        (arena, a, b, c)
    }

    #[test]
    fn all_inline_signals_share_the_root_loop() {
        // out reads a; both inline (base props → Inline).
        let (_arena, a, _b, out) = three_sigs();
        let asn = assign_loops(
            &[out],
            |sig| if sig == out { vec![a] } else { vec![] },
            |_| base_props(),
        );
        assert_eq!(asn.graph.len(), 1, "only the root loop is created");
        assert_eq!(asn.loop_of(out), Some(asn.root));
        assert_eq!(asn.loop_of(a), Some(asn.root));
    }

    #[test]
    fn a_shared_signal_opens_its_own_loop_with_an_edge() {
        // out reads `shared`; `shared` reads `a`; `shared` is marked shared.
        let (_arena, a, shared, out) = three_sigs();
        let asn = assign_loops(
            &[out],
            |sig| {
                if sig == out {
                    vec![shared]
                } else if sig == shared {
                    vec![a]
                } else {
                    vec![]
                }
            },
            |sig| {
                if sig == shared {
                    SignalLoopProps {
                        is_shared: true,
                        ..base_props()
                    }
                } else {
                    base_props()
                }
            },
        );
        assert_eq!(asn.graph.len(), 2, "root + the shared signal's loop");
        assert_eq!(asn.loop_of(out), Some(asn.root), "out inlines into root");
        let shared_loop = asn.loop_of(shared).expect("shared signal is placed");
        assert_ne!(shared_loop, asn.root);
        assert!(
            asn.graph.node(asn.root).deps.contains(&shared_loop),
            "root must depend on the shared loop"
        );
    }

    #[test]
    fn recursive_projection_opens_a_serial_loop() {
        let (_arena, _a, _b, out) = three_sigs();
        let asn = assign_loops(
            &[out],
            |_| vec![],
            |_| SignalLoopProps {
                is_recursive_proj: true,
                ..base_props()
            },
        );
        let out_loop = asn.loop_of(out).expect("output is placed");
        assert_ne!(out_loop, asn.root);
        assert_eq!(asn.graph.node(out_loop).kind, LoopKind::Recursive);
        assert!(asn.graph.node(asn.root).deps.contains(&out_loop));
    }

    #[test]
    fn separated_chain_topologically_orders_dependencies_first() {
        // out(root) reads mid(own loop) which reads leaf(own loop).
        let (_arena, leaf, mid, out) = three_sigs();
        let asn = assign_loops(
            &[out],
            |sig| {
                if sig == out {
                    vec![mid]
                } else if sig == mid {
                    vec![leaf]
                } else {
                    vec![]
                }
            },
            |sig| {
                if sig == mid || sig == leaf {
                    SignalLoopProps {
                        is_shared: true,
                        ..base_props()
                    }
                } else {
                    base_props()
                }
            },
        );
        let mid_loop = asn.loop_of(mid).unwrap();
        let leaf_loop = asn.loop_of(leaf).unwrap();
        assert!(asn.graph.node(asn.root).deps.contains(&mid_loop));
        assert!(asn.graph.node(mid_loop).deps.contains(&leaf_loop));
        let order = asn.graph.topological_order().unwrap();
        let pos = |l: LoopId| order.iter().position(|&x| x == l).unwrap();
        assert!(pos(leaf_loop) < pos(mid_loop), "leaf loop emits before mid");
        assert!(pos(mid_loop) < pos(asn.root), "mid loop emits before root");
    }

    #[test]
    fn value_children_drive_a_real_signal_graph_without_the_op_atom_cycle() {
        // out = (in0 + in1) * sin(in0).
        let mut arena = TreeArena::new();
        let in0 = SigBuilder::new(&mut arena).input(0);
        let in1 = SigBuilder::new(&mut arena).input(1);
        let sum = SigBuilder::new(&mut arena).add(in0, in1);
        let s = SigBuilder::new(&mut arena).sin(in0);
        let out = SigBuilder::new(&mut arena).mul(sum, s);
        let sig_types = sigtype::TypeAnnotator::new(&arena, &ui::UiProgram::empty())
            .annotate(&[out])
            .unwrap();
        let analysis =
            super::super::vector_analysis::SignalAnalysisContext::new(&arena, &sig_types, &[out])
                .unwrap();

        // Only value operands — no op-code atom, no input index.
        assert_eq!(signal_value_children(&analysis, out).unwrap(), vec![sum, s]);
        assert_eq!(
            signal_value_children(&analysis, sum).unwrap(),
            vec![in0, in1]
        );
        assert_eq!(signal_value_children(&analysis, s).unwrap(), vec![in0]);
        assert!(signal_value_children(&analysis, in0).unwrap().is_empty());

        // All-inline on the real graph: one loop, and — the S-A bug — no cycle.
        let asn = assign_loops(
            &[out],
            |sig| signal_value_children(&analysis, sig).unwrap(),
            |_| base_props(),
        );
        assert_eq!(asn.graph.len(), 1);
        assert!(asn.graph.topological_order().is_ok());

        // in0 is genuinely shared (read by both `sum` and `s`) → its own loop,
        // still acyclic.
        let asn = assign_loops(
            &[out],
            |sig| signal_value_children(&analysis, sig).unwrap(),
            |sig| {
                if sig == in0 {
                    SignalLoopProps {
                        is_shared: true,
                        ..base_props()
                    }
                } else {
                    base_props()
                }
            },
        );
        let in0_loop = asn.loop_of(in0).expect("in0 is placed");
        assert_ne!(in0_loop, asn.root);
        assert!(
            asn.graph.topological_order().is_ok(),
            "sharing must not create a cycle"
        );
    }

    /// `index` must be the chunk-local `i0 - vindex`.
    fn assert_chunk_index(store: &FirStore, index: FirId) {
        let FirMatch::BinOp {
            op: FirBinOp::Sub,
            lhs,
            rhs,
            ..
        } = match_fir(store, index)
        else {
            panic!("chunk index must be a subtraction");
        };
        assert!(
            matches!(match_fir(store, lhs), FirMatch::LoadVar { ref name, access: AccessType::Loop, .. } if name == "i0")
        );
        assert!(
            matches!(match_fir(store, rhs), FirMatch::LoadVar { ref name, access: AccessType::Loop, .. } if name == "vindex")
        );
    }

    #[test]
    fn chunk_buffer_declare_store_load() {
        let mut store = FirStore::new();
        let buf = ChunkBuffer::new(0, FirType::Float32, 32);
        assert_eq!(buf.name(), "vbuf0");

        // declare: `float vbuf0[32];` — a stack array, uninitialized.
        let decl = buf.declare(&mut store);
        let FirMatch::DeclareVar {
            name,
            typ,
            access,
            init,
        } = match_fir(&store, decl)
        else {
            panic!("declare must be a DeclareVar");
        };
        assert_eq!(name, "vbuf0");
        assert_eq!(access, AccessType::Stack);
        assert!(init.is_none());
        assert_eq!(typ, FirType::Array(Box::new(FirType::Float32), 32));

        // store: `vbuf0[i0 - vindex] = value;`
        let value = FirBuilder::new(&mut store).float32(1.5);
        let st = buf.store(&mut store, value);
        let FirMatch::StoreTable {
            name,
            access,
            index,
            value: stored,
        } = match_fir(&store, st)
        else {
            panic!("store must be a StoreTable");
        };
        assert_eq!(name, "vbuf0");
        assert_eq!(access, AccessType::Stack);
        assert_eq!(stored, value);
        assert_chunk_index(&store, index);

        // load: `vbuf0[i0 - vindex]`
        let ld = buf.load(&mut store);
        let FirMatch::LoadTable {
            name,
            access,
            index,
            typ,
        } = match_fir(&store, ld)
        else {
            panic!("load must be a LoadTable");
        };
        assert_eq!(name, "vbuf0");
        assert_eq!(access, AccessType::Stack);
        assert_eq!(typ, FirType::Float32);
        assert_chunk_index(&store, index);
    }

    /// Builds the fused body of `process = (_ : + ~ _) * 0.5`:
    /// `[fRecCur = fRec + 1; output0[i0] = 0.5*fRecCur; fRec = fRecCur]`.
    /// (`+ 1` stands in for the input term — irrelevant to the partition.)
    fn simple_recursive_body(store: &mut FirStore) -> (Vec<FirId>, FirId, FirId, FirId) {
        let mut b = FirBuilder::new(store);
        let frec = b.load_var("fRec", AccessType::Struct, FirType::Float32);
        let one = b.float32(1.0);
        let sum = b.binop(FirBinOp::Add, frec, one, FirType::Float32);
        let decl = b.declare_var("fRecCur", FirType::Float32, AccessType::Stack, Some(sum));

        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let cur = b.load_var("fRecCur", AccessType::Stack, FirType::Float32);
        let half = b.float32(0.5);
        let scaled = b.binop(FirBinOp::Mul, half, cur, FirType::Float32);
        let out = b.store_table("output0", AccessType::Stack, i0, scaled);

        let cur2 = b.load_var("fRecCur", AccessType::Stack, FirType::Float32);
        let store_state = b.store_var("fRec", AccessType::Struct, cur2);

        (vec![decl, out, store_state], decl, out, store_state)
    }

    #[test]
    fn partition_hoists_state_free_tail() {
        let mut store = FirStore::new();
        let (exec, decl, out, store_state) = simple_recursive_body(&mut store);

        let part = partition_recursive_body(&store, &exec).expect("splittable");
        // Serial = recursion compute + state write-back (original order).
        assert_eq!(part.serial, vec![decl, store_state]);
        // Vectorizable tail = the output scaling.
        assert_eq!(part.vectorizable, vec![out]);
        // The recursion carrier crosses the boundary and is buffered.
        assert_eq!(
            part.boundary,
            vec![("fRecCur".to_string(), FirType::Float32)]
        );
    }

    #[test]
    fn partition_declines_fully_recursive_body() {
        // No state-free statement: `[fRecCur = fRec + 1; fRec = fRecCur]`.
        let mut store = FirStore::new();
        let (exec, decl, _out, store_state) = simple_recursive_body(&mut store);
        let recursive_only = vec![exec[0], exec[2]];
        assert_eq!(recursive_only, vec![decl, store_state]);
        assert!(partition_recursive_body(&store, &recursive_only).is_none());
    }

    #[test]
    fn partition_declines_tail_that_reads_struct_table_state() {
        // APF-like shape: the output expression reads `fRec[0]`/`fRec[2]`
        // directly.  Hoisting that output after the serial core would observe
        // the chunk-final table state instead of the per-sample state.
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);

        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let zero = b.int32(0);
        let one_i = b.int32(1);
        let two = b.int32(2);
        let rec1 = b.load_table("fRec", AccessType::Struct, one_i, FirType::Float32);
        let temp = b.declare_var("fTemp", FirType::Float32, AccessType::Stack, Some(rec1));

        let cur = b.load_table("fRec", AccessType::Struct, zero, FirType::Float32);
        let prev2 = b.load_table("fRec", AccessType::Struct, two, FirType::Float32);
        let sum = b.binop(FirBinOp::Add, cur, prev2, FirType::Float32);
        let out = b.store_table("output0", AccessType::Stack, i0, sum);

        let temp_load = b.load_var("fTemp", AccessType::Stack, FirType::Float32);
        let store_state = b.store_table("fRec", AccessType::Struct, zero, temp_load);

        assert!(partition_recursive_body(&store, &[temp, out, store_state]).is_none());
    }

    #[test]
    fn partition_declines_unsupported_statement() {
        // A bare value node is not a statement kind the split handles → fallback.
        let mut store = FirStore::new();
        let stray = FirBuilder::new(&mut store).float32(2.0);
        assert!(partition_recursive_body(&store, &[stray]).is_none());
    }

    #[test]
    fn rewrite_var_loads_redirects_boundary_temp_to_buffer() {
        let mut store = FirStore::new();
        let (_exec, _decl, out, _store_state) = simple_recursive_body(&mut store);

        // Redirect reads of `fRecCur` to the chunk-buffer load `vbuf0[i0-vindex]`.
        let buf = ChunkBuffer::new(0, FirType::Float32, 32);
        let load = buf.load(&mut store);
        let mut repl = AHashMap::new();
        repl.insert("fRecCur".to_string(), load);

        let rewritten =
            rewrite_var_loads(&mut store, out, &repl).expect("output tail is rewritable");
        assert_ne!(rewritten, out, "the output write must be rebuilt");

        // The tail is now `output0[i0] = 0.5 * vbuf0[i0-vindex]`.
        let FirMatch::StoreTable {
            name, value, index, ..
        } = match_fir(&store, rewritten)
        else {
            panic!("still an output store");
        };
        assert_eq!(name, "output0");
        // The output is still written at the global sample index i0 (unchanged) —
        // only the *value*'s boundary-temp read is redirected to the buffer.
        assert!(matches!(
            match_fir(&store, index),
            FirMatch::LoadVar { ref name, access: AccessType::Loop, .. } if name == "i0"
        ));
        let FirMatch::BinOp { rhs, .. } = match_fir(&store, value) else {
            panic!("value is 0.5 * <load>");
        };
        // rhs is the buffer load, not a bare fRecCur LoadVar.
        assert!(matches!(
            match_fir(&store, rhs),
            FirMatch::LoadTable { ref name, .. } if name == "vbuf0"
        ));
    }

    #[test]
    fn rewrite_var_loads_descends_into_soundfile_buffer_index() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);

        let rec = b.load_var("fRec", AccessType::Struct, FirType::Int32);
        let idx_decl = b.declare_var("iRecCur", FirType::Int32, AccessType::Stack, Some(rec));
        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let zero = b.int32(0);
        let idx = b.load_var("iRecCur", AccessType::Stack, FirType::Int32);
        let sample = b.load_soundfile_buffer("fSound0", zero, zero, idx, FirType::Float32);
        let out = b.store_table("output0", AccessType::Stack, i0, sample);
        let idx2 = b.load_var("iRecCur", AccessType::Stack, FirType::Int32);
        let store_state = b.store_var("fRec", AccessType::Struct, idx2);

        let part = partition_recursive_body(&store, &[idx_decl, out, store_state])
            .expect("soundfile index tail is splittable through a boundary buffer");
        assert_eq!(part.vectorizable, vec![out]);
        assert_eq!(part.boundary, vec![("iRecCur".to_string(), FirType::Int32)]);

        let buf = ChunkBuffer::new(0, FirType::Int32, 32);
        let load = buf.load(&mut store);
        let mut repl = AHashMap::new();
        repl.insert("iRecCur".to_string(), load);
        let rewritten =
            rewrite_var_loads(&mut store, out, &repl).expect("soundfile tail is rewritable");

        let FirMatch::StoreTable { value, .. } = match_fir(&store, rewritten) else {
            panic!("still an output store");
        };
        let FirMatch::LoadSoundfileBuffer { idx, .. } = match_fir(&store, value) else {
            panic!("output value is still a soundfile buffer load");
        };
        assert!(matches!(
            match_fir(&store, idx),
            FirMatch::LoadTable { ref name, .. } if name == "vbuf0"
        ));
    }

    #[test]
    fn persistent_state_detection() {
        let mut store = fir::FirStore::new();
        // Stateless: out = 0.5 * in.  (no Struct store)
        let stateless = {
            let mut b = fir::FirBuilder::new(&mut store);
            let half = b.float32(0.5);
            let inp = b.load_var("in", AccessType::Stack, fir::FirType::Float32);
            let prod = b.binop(fir::FirBinOp::Mul, half, inp, fir::FirType::Float32);
            b.store_var("out", AccessType::Stack, prod)
        };
        assert!(!slice_has_persistent_state(&store, &[stateless]));

        // Stateful: fRec = fRec + 1  (a Struct store = cross-sample carrier).
        let stateful = {
            let mut b = fir::FirBuilder::new(&mut store);
            let rec = b.load_var("fRec", AccessType::Struct, fir::FirType::Float32);
            let one = b.float32(1.0);
            let sum = b.binop(fir::FirBinOp::Add, rec, one, fir::FirType::Float32);
            b.store_var("fRec", AccessType::Struct, sum)
        };
        assert!(slice_has_persistent_state(&store, &[stateful]));

        // Nested inside a guarded block (clocked-domain shape) is still detected.
        let guarded = {
            let mut b = fir::FirBuilder::new(&mut store);
            let body = b.block(&[stateful]);
            let cond = b.int32(1);
            b.if_(cond, body, None)
        };
        assert!(slice_has_persistent_state(&store, &[guarded]));
    }

    #[test]
    fn phase_statements_and_deps_round_trip() {
        let mut store = fir::FirStore::new();
        let (s0, s1) = {
            let mut b = fir::FirBuilder::new(&mut store);
            (b.int32(0), b.int32(1))
        };
        let mut g = LoopGraph::new();
        let l = g.add_loop(LoopKind::Vectorizable, false);
        g.node_mut(l).pre.push(s0);
        g.node_mut(l).exec.push(s1);
        assert_eq!(g.node(l).pre, vec![s0]);
        assert_eq!(g.node(l).exec, vec![s1]);
        assert!(g.node(l).post.is_empty());
    }
}
