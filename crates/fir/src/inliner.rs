//! FIR function inliner scaffolding (Milestones 1–4 + Phase E iteration).
//!
//! # Scope
//! This module currently implements:
//! - function indexing from a FIR `Module`,
//! - call graph extraction,
//! - SCC detection,
//! - simple callee size metrics,
//! - candidate selection decisions (legality/profitability pre-checks).
//! - hygienic FIR subtree cloning with local-variable renaming,
//! - callee argument materialization and `kFunArgs` substitution,
//! - one-pass callsite rewriting for canonical value-returning helper bodies,
//! - iterative fixpoint driver with SCC-based deterministic function order.
//!
//! Current rewrite support is intentionally conservative: only a subset of
//! statement/value shapes are recursively rewritten for nested callsites, and
//! only canonical callee bodies ending with `Return(Some(v))` are inlined.
//!
//! # Source provenance (C++)
//! - `compiler/generator/fir_to_fir.cpp` (`FunctionInliner`, `FunctionCallInliner`)
//! - `compiler/generator/fir_to_fir.hh`
//!
//! # Public API mapping status
//! - `adapted`: the C++ code exposes inlining as visitor-side rewriting helpers.
//!   Rust starts with a module-level analysis API to make legality/profitability
//!   decisions explicit and testable before implementing rewriting.
//!
//! # Current policy
//! The pass intentionally prefers deterministic, conservative rewrites over
//! aggressive size reduction: reserved DSP API entry points, recursive SCCs,
//! and non-canonical helper bodies are excluded unless explicitly supported.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{
    AccessType, FirBuilder, FirId, FirMatch, FirMathOp, FirStore, FirType, NamedType, SliderRange,
    match_fir,
};

const RESERVED_DSP_API_FUNCTIONS: &[&str] = &[
    "classInit",
    "instanceInit",
    "init",
    "getSampleRate",
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "buildUserInterface",
    "compute",
    "metadata",
];

/// Analysis-time configuration for FIR function inlining candidate selection.
///
/// These options are used by [`analyze_fir_inliner`] to classify module
/// functions as "eligible" or "skipped" candidates without rewriting code yet.
/// They also define the public policy knobs that later rewrite phases are
/// expected to respect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirInlineOptions {
    /// Master switch for candidate selection.
    ///
    /// When `false`, all functions are analyzed but marked non-eligible with a
    /// `Disabled` reason.
    pub enabled: bool,
    /// Inline only functions explicitly flagged `is_inline`.
    pub inline_marked_only: bool,
    /// Maximum allowed unique FIR nodes reachable from a callee body.
    ///
    /// Functions whose body exceeds this threshold are marked as skipped with
    /// `TooLarge`.
    pub max_callee_nodes: usize,
    /// Reserved for later rewrite phase (depth-limited inlining).
    ///
    /// The analysis pass records the option for future compatibility but does
    /// not consume it yet.
    pub max_inline_depth: usize,
    /// Reserved for later rewrite phase (caller expansion budget).
    ///
    /// The analysis pass records the option for future compatibility but does
    /// not consume it yet.
    pub max_expansion_factor: usize,
    /// Whether recursive SCCs may be considered eligible.
    ///
    /// Default is `false`; recursive/self-recursive functions are skipped.
    pub allow_recursive: bool,
    /// Reserved for later rewrite phase integration with checker-driven
    /// validation after each transformed function.
    pub verify_after_each_function: bool,
}

impl Default for FirInlineOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            inline_marked_only: false,
            max_callee_nodes: 64,
            max_inline_depth: 8,
            max_expansion_factor: 8,
            allow_recursive: false,
            verify_after_each_function: false,
        }
    }
}

/// Stable analysis error for malformed FIR module inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirInlineAnalysisError {
    /// Root node is not `FirMatch::Module`.
    RootNotModule,
    /// One module section is not a `Block`.
    InvalidModuleSection { section: &'static str, node: FirId },
    /// Duplicate function names make call graph resolution ambiguous.
    DuplicateFunctionName {
        name: String,
        first: FirId,
        second: FirId,
    },
}

impl std::fmt::Display for FirInlineAnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotModule => write!(f, "FIR inliner analysis requires a Module root"),
            Self::InvalidModuleSection { section, node } => write!(
                f,
                "FIR inliner analysis expected '{section}' to be a Block (node={})",
                node.as_u32()
            ),
            Self::DuplicateFunctionName {
                name,
                first,
                second,
            } => write!(
                f,
                "duplicate FIR function name '{name}' (nodes {} and {})",
                first.as_u32(),
                second.as_u32()
            ),
        }
    }
}

impl std::error::Error for FirInlineAnalysisError {}

/// Location of a function declaration inside a FIR module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FirFunctionSection {
    /// Function declared in `Module.globals` (often extern prototypes).
    Globals,
    /// Function declared in `Module.functions`.
    Functions,
}

/// Per-function summary extracted during module analysis.
///
/// This is the stable analysis record used for candidate decisions and tests.
/// It intentionally separates raw module facts (body exists, direct callees,
/// reserved API name) from the final eligible/skipped decision.
#[derive(Clone, Debug, PartialEq)]
pub struct FirFunctionSummary {
    /// Function name.
    pub name: String,
    /// FIR node id of the `DeclareFun`.
    pub decl_id: FirId,
    /// Origin module section.
    pub section: FirFunctionSection,
    /// Parameter list copied from `DeclareFun`.
    pub params: Vec<NamedType>,
    /// Function has a body and is therefore rewriteable in principle.
    pub has_body: bool,
    /// Original `DeclareFun.is_inline` flag.
    pub is_inline: bool,
    /// Number of unique FIR nodes reachable from the body (0 for prototypes).
    pub body_node_count: usize,
    /// Unique direct callees referenced by `FunCall` nodes in the body.
    pub direct_callees: BTreeSet<String>,
    /// SCC index in [`FirInlineAnalysis::sccs`].
    pub scc_index: usize,
    /// `true` for self-recursive or mutually recursive SCC members.
    pub is_recursive_scc: bool,
    /// `true` when the name matches a reserved DSP API function.
    pub is_reserved_api: bool,
}

/// Why a function was excluded from inlining candidates in analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirInlineSkipReason {
    /// Global pass is disabled in options.
    Disabled,
    /// `inline_marked_only=true` and `DeclareFun.is_inline=false`.
    NotMarkedInline,
    /// Function has no body (`DeclareFun` prototype / extern declaration).
    PrototypeOnly,
    /// Default policy excludes DSP API entry points from inlining.
    ReservedApi,
    /// Function belongs to a recursive SCC and recursion is disabled.
    RecursiveScc,
    /// Body node count exceeded `max_callee_nodes`.
    TooLarge { body_nodes: usize, max: usize },
}

/// Candidate decision produced for each analyzed function.
///
/// Decisions are recorded for every function, not just eligible callees, so
/// tests and journaling can explain why a candidate was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirInlineCandidateDecision {
    /// Function name the decision applies to.
    pub function: String,
    /// Whether the function is eligible as an inlining callee in this phase.
    pub eligible: bool,
    /// Explanatory skip reasons (empty iff `eligible=true`).
    pub reasons: Vec<FirInlineSkipReason>,
}

/// One strongly-connected component in the function call graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirInlineScc {
    /// Stable index used by [`FirFunctionSummary::scc_index`].
    pub index: usize,
    /// Functions in deterministic order.
    pub functions: Vec<String>,
    /// `true` if the SCC is recursive (`len>1` or self-edge).
    pub is_recursive: bool,
}

/// Result of FIR inliner analysis and conservative rewrite preparation.
///
/// This structure is designed to be stable enough for differential tests and
/// journaling: it exposes the call graph, SCCs, summaries, and final candidate
/// decisions in deterministic maps/vectors.
#[derive(Clone, Debug, PartialEq)]
pub struct FirInlineAnalysis {
    /// Options used to compute candidate decisions.
    pub options: FirInlineOptions,
    /// Per-function summaries keyed by function name.
    pub functions: BTreeMap<String, FirFunctionSummary>,
    /// Direct call graph adjacency list (callee names filtered to known functions).
    pub call_graph: BTreeMap<String, BTreeSet<String>>,
    /// SCC decomposition of the call graph.
    pub sccs: Vec<FirInlineScc>,
    /// Callee candidate decisions keyed by function name.
    pub candidate_decisions: BTreeMap<String, FirInlineCandidateDecision>,
}

impl FirInlineAnalysis {
    /// Returns `true` when `function` is an eligible inlining callee.
    #[must_use]
    pub fn is_callee_candidate(&self, function: &str) -> bool {
        self.candidate_decisions
            .get(function)
            .map(|d| d.eligible)
            .unwrap_or(false)
    }
}

/// Raw function facts gathered before metrics, SCCs, and policy decisions.
///
/// This is kept separate from [`FirFunctionSummary`] so analysis can first
/// collect module facts, then derive deterministic summaries and decisions.
#[derive(Clone, Debug)]
struct RawFunctionInfo {
    decl_id: FirId,
    section: FirFunctionSection,
    params: Vec<NamedType>,
    body: Option<FirId>,
    is_inline: bool,
}

/// Minimal body metrics used by candidate selection.
///
/// These metrics deliberately stay cheap and structural: they are computed in a
/// single traversal and avoid any semantic modeling beyond direct `FunCall`
/// collection.
#[derive(Default)]
struct BodyMetrics {
    node_count: usize,
    direct_callees: BTreeSet<String>,
}

/// Analyze a FIR module for future function-inlining transformations.
///
/// This function performs **no rewrites**. It builds a function index and call
/// graph, computes SCCs, collects body-size metrics, and classifies callee
/// candidates using [`FirInlineOptions`].
///
/// Keeping analysis separate from rewriting is deliberate: parity-sensitive
/// ports can inspect and test legality/profitability decisions before any code
/// transformation is applied.
///
/// # Errors
/// Returns [`FirInlineAnalysisError`] when the input is not a valid FIR module
/// shape for analysis (non-`Module` root, non-block sections, duplicate
/// function names).
pub fn analyze_fir_inliner(
    store: &FirStore,
    module: FirId,
    options: &FirInlineOptions,
) -> Result<FirInlineAnalysis, FirInlineAnalysisError> {
    let FirMatch::Module {
        globals, functions, ..
    } = match_fir(store, module)
    else {
        return Err(FirInlineAnalysisError::RootNotModule);
    };

    let mut raw_functions: BTreeMap<String, RawFunctionInfo> = BTreeMap::new();
    collect_functions_from_section(
        store,
        globals,
        FirFunctionSection::Globals,
        &mut raw_functions,
    )?;
    collect_functions_from_section(
        store,
        functions,
        FirFunctionSection::Functions,
        &mut raw_functions,
    )?;

    let mut call_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut body_metrics: BTreeMap<String, BodyMetrics> = BTreeMap::new();
    for (name, info) in &raw_functions {
        let metrics = if let Some(body) = info.body {
            collect_body_metrics(store, body)
        } else {
            BodyMetrics::default()
        };
        let known_callees = metrics
            .direct_callees
            .iter()
            .filter(|callee| raw_functions.contains_key(callee.as_str()))
            .cloned()
            .collect::<BTreeSet<_>>();
        call_graph.insert(name.clone(), known_callees);
        body_metrics.insert(name.clone(), metrics);
    }

    let (sccs, scc_index_by_name) = tarjan_sccs(&call_graph);

    let mut functions = BTreeMap::new();
    let mut candidate_decisions = BTreeMap::new();
    for (name, info) in &raw_functions {
        let metrics = body_metrics
            .get(name)
            .expect("metrics collected for all funcs");
        let scc_index = *scc_index_by_name
            .get(name)
            .expect("scc index computed for all funcs");
        let scc = &sccs[scc_index];
        let summary = FirFunctionSummary {
            name: name.clone(),
            decl_id: info.decl_id,
            section: info.section,
            params: info.params.clone(),
            has_body: info.body.is_some(),
            is_inline: info.is_inline,
            body_node_count: metrics.node_count,
            direct_callees: metrics.direct_callees.clone(),
            scc_index,
            is_recursive_scc: scc.is_recursive,
            is_reserved_api: RESERVED_DSP_API_FUNCTIONS.contains(&name.as_str()),
        };
        let decision = decide_callee_candidate(&summary, options);
        candidate_decisions.insert(name.clone(), decision);
        functions.insert(name.clone(), summary);
    }

    Ok(FirInlineAnalysis {
        options: options.clone(),
        functions,
        call_graph,
        sccs,
        candidate_decisions,
    })
}

/// Collects all `DeclareFun` nodes from a module section (`globals`/`functions`).
fn collect_functions_from_section(
    store: &FirStore,
    section_id: FirId,
    section: FirFunctionSection,
    out: &mut BTreeMap<String, RawFunctionInfo>,
) -> Result<(), FirInlineAnalysisError> {
    let section_name = match section {
        FirFunctionSection::Globals => "globals",
        FirFunctionSection::Functions => "functions",
    };
    let FirMatch::Block(items) = match_fir(store, section_id) else {
        return Err(FirInlineAnalysisError::InvalidModuleSection {
            section: section_name,
            node: section_id,
        });
    };

    for item in items {
        let FirMatch::DeclareFun {
            name,
            args,
            body,
            is_inline,
            ..
        } = match_fir(store, item)
        else {
            continue;
        };

        if let Some(prev) = out.get(&name) {
            return Err(FirInlineAnalysisError::DuplicateFunctionName {
                name,
                first: prev.decl_id,
                second: item,
            });
        }

        out.insert(
            name,
            RawFunctionInfo {
                decl_id: item,
                section,
                params: args,
                body,
                is_inline,
            },
        );
    }
    Ok(())
}

/// Traverses a function body and extracts a size metric + direct `FunCall` names.
fn collect_body_metrics(store: &FirStore, root: FirId) -> BodyMetrics {
    let mut metrics = BodyMetrics::default();
    let mut stack = vec![root];
    let mut seen = HashSet::new();

    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        metrics.node_count += 1;
        let node = match_fir(store, id);
        if let FirMatch::FunCall { name, .. } = &node {
            metrics.direct_callees.insert(name.clone());
        }
        stack.extend(child_ids(&node));
    }

    metrics
}

/// Counts unique FIR nodes reachable from `root` (module-size baseline/expansion budget).
fn count_unique_nodes(store: &FirStore, root: FirId) -> usize {
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let node = match_fir(store, id);
        stack.extend(child_ids(&node));
    }
    seen.len()
}

/// Returns child FIR ids for recursive traversal.
///
/// This is intentionally local to the inliner module so analysis remains
/// independent from dump helpers in `lib.rs`.
fn child_ids(node: &FirMatch) -> Vec<FirId> {
    match node {
        FirMatch::Unknown
        | FirMatch::Int32 { .. }
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
        | FirMatch::LoadVar { .. }
        | FirMatch::LoadVarAddress { .. }
        | FirMatch::NullValue { .. }
        | FirMatch::NewDsp { .. }
        | FirMatch::DeclareStructType { .. }
        | FirMatch::DeclareBufferIterators { .. }
        | FirMatch::ShiftArrayVar { .. }
        | FirMatch::NullStatement
        | FirMatch::OpenBox { .. }
        | FirMatch::CloseBox
        | FirMatch::AddButton { .. }
        | FirMatch::AddSlider { .. }
        | FirMatch::AddBargraph { .. }
        | FirMatch::AddSoundfile { .. }
        | FirMatch::AddMetaDeclare { .. }
        | FirMatch::Label(_) => Vec::new(),
        FirMatch::LoadSoundfileLength { part, .. } | FirMatch::LoadSoundfileRate { part, .. } => {
            vec![*part]
        }
        FirMatch::LoadSoundfileBuffer {
            chan, part, idx, ..
        } => vec![*chan, *part, *idx],
        FirMatch::ValueArray { values, .. }
        | FirMatch::FunCall { args: values, .. }
        | FirMatch::DeclareTable { values, .. }
        | FirMatch::Block(values) => values.clone(),
        FirMatch::LoadTable { index, .. }
        | FirMatch::TeeVar { value: index, .. }
        | FirMatch::Neg { value: index, .. }
        | FirMatch::Cast { value: index, .. }
        | FirMatch::Bitcast { value: index, .. }
        | FirMatch::StoreVar { value: index, .. }
        | FirMatch::SimpleForLoop { upper: index, .. }
        | FirMatch::Drop(index) => vec![*index],
        FirMatch::BinOp { lhs, rhs, .. } => vec![*lhs, *rhs],
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => vec![*cond, *then_value, *else_value],
        FirMatch::DeclareVar { init, .. } => init.iter().copied().collect(),
        FirMatch::DeclareFun { body: Some(b), .. } => vec![*b],
        FirMatch::DeclareFun { body: None, .. } => vec![],
        FirMatch::StoreTable { index, value, .. } => vec![*index, *value],
        FirMatch::Return(value) => value.iter().copied().collect(),
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let mut out = vec![*cond, *then_block];
            out.extend(else_block.iter().copied());
            out
        }
        FirMatch::Control { cond, stmt } => vec![*cond, *stmt],
        FirMatch::ForLoop {
            init,
            end,
            step,
            body,
            ..
        } => vec![*init, *end, *step, *body],
        FirMatch::IteratorForLoop { body, .. } => vec![*body],
        FirMatch::WhileLoop { cond, body } => vec![*cond, *body],
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            let mut out = vec![*cond];
            out.extend(cases.iter().map(|(_, block)| *block));
            out.extend(default.iter().copied());
            out
        }
        FirMatch::Module {
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
            ..
        } => vec![
            *dsp_struct,
            *globals,
            *functions,
            *static_decls,
            *sub_modules,
        ],
        FirMatch::SubModule {
            dsp_struct,
            static_decls,
            globals,
            functions,
            sub_modules,
            ..
        } => vec![
            *dsp_struct,
            *static_decls,
            *globals,
            *functions,
            *sub_modules,
        ],
    }
}

/// Computes a deterministic SCC decomposition of the function call graph.
///
/// The graph is keyed by function name and edges should only target known keys.
/// SCCs are returned in a stable order derived from sorted node iteration.
fn tarjan_sccs(
    graph: &BTreeMap<String, BTreeSet<String>>,
) -> (Vec<FirInlineScc>, BTreeMap<String, usize>) {
    /// Working set for Tarjan SCC discovery.
    struct TarjanState {
        index: usize,
        stack: Vec<String>,
        on_stack: HashSet<String>,
        index_map: HashMap<String, usize>,
        lowlink_map: HashMap<String, usize>,
        components: Vec<Vec<String>>,
    }

    /// Standard Tarjan DFS step for one graph node.
    ///
    /// The implementation operates on function names instead of numeric indices
    /// to keep the resulting SCCs deterministic and directly testable.
    fn strong_connect(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        st: &mut TarjanState,
    ) {
        let node_s = node.to_string();
        st.index_map.insert(node_s.clone(), st.index);
        st.lowlink_map.insert(node_s.clone(), st.index);
        st.index += 1;
        st.stack.push(node_s.clone());
        st.on_stack.insert(node_s.clone());

        if let Some(neighbors) = graph.get(node) {
            for next in neighbors {
                if !st.index_map.contains_key(next) {
                    strong_connect(next, graph, st);
                    let low_n = *st.lowlink_map.get(node).expect("node lowlink set");
                    let low_next = *st.lowlink_map.get(next).expect("next lowlink set");
                    st.lowlink_map.insert(node_s.clone(), low_n.min(low_next));
                } else if st.on_stack.contains(next) {
                    let low_n = *st.lowlink_map.get(node).expect("node lowlink set");
                    let idx_next = *st.index_map.get(next).expect("next index set");
                    st.lowlink_map.insert(node_s.clone(), low_n.min(idx_next));
                }
            }
        }

        let low = *st.lowlink_map.get(node).expect("node lowlink set");
        let idx = *st.index_map.get(node).expect("node index set");
        if low == idx {
            let mut component = Vec::new();
            loop {
                let w = st.stack.pop().expect("stack contains SCC members");
                st.on_stack.remove(&w);
                let done = w == node;
                component.push(w);
                if done {
                    break;
                }
            }
            component.sort();
            st.components.push(component);
        }
    }

    let mut st = TarjanState {
        index: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        index_map: HashMap::new(),
        lowlink_map: HashMap::new(),
        components: Vec::new(),
    };
    for node in graph.keys() {
        if !st.index_map.contains_key(node) {
            strong_connect(node, graph, &mut st);
        }
    }

    st.components.sort_by(|a, b| a[0].cmp(&b[0]));

    let mut sccs = Vec::with_capacity(st.components.len());
    let mut scc_index_by_name = BTreeMap::new();
    for (idx, functions) in st.components.into_iter().enumerate() {
        let is_recursive = if functions.len() > 1 {
            true
        } else {
            let f = &functions[0];
            graph.get(f).is_some_and(|edges| edges.contains(f))
        };
        for name in &functions {
            scc_index_by_name.insert(name.clone(), idx);
        }
        sccs.push(FirInlineScc {
            index: idx,
            functions,
            is_recursive,
        });
    }

    (sccs, scc_index_by_name)
}

/// Returns a deterministic function rewrite order: callees first (reverse topo of SCC DAG).
///
/// Edges are caller -> callee. Reverse topological order therefore visits acyclic
/// leaf callees before their callers, which maximizes progress across fixpoint
/// iterations. Functions within the same SCC use the deterministic order stored
/// in [`FirInlineAnalysis::sccs`].
fn function_rewrite_order_by_scc(analysis: &FirInlineAnalysis) -> Vec<String> {
    let scc_count = analysis.sccs.len();
    let mut succs: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); scc_count];
    let mut indegree = vec![0usize; scc_count];

    for (caller, callees) in &analysis.call_graph {
        let Some(caller_summary) = analysis.functions.get(caller) else {
            continue;
        };
        let caller_scc = caller_summary.scc_index;
        for callee in callees {
            let Some(callee_summary) = analysis.functions.get(callee) else {
                continue;
            };
            let callee_scc = callee_summary.scc_index;
            if caller_scc != callee_scc && succs[caller_scc].insert(callee_scc) {
                indegree[callee_scc] += 1;
            }
        }
    }

    let mut ready = BTreeSet::new();
    for (idx, deg) in indegree.iter().copied().enumerate() {
        if deg == 0 {
            ready.insert(idx);
        }
    }

    let mut topo = Vec::with_capacity(scc_count);
    while let Some(next) = ready.iter().next().copied() {
        ready.remove(&next);
        topo.push(next);
        for succ in succs[next].iter().copied() {
            indegree[succ] -= 1;
            if indegree[succ] == 0 {
                ready.insert(succ);
            }
        }
    }
    if topo.len() != scc_count {
        // SCC DAG should be acyclic by construction; fall back deterministically.
        topo = (0..scc_count).collect();
    }

    let mut order = Vec::new();
    for scc_idx in topo.into_iter().rev() {
        order.extend(analysis.sccs[scc_idx].functions.iter().cloned());
    }
    order
}

/// Applies the current (conservative) callee-eligibility policy.
fn decide_callee_candidate(
    summary: &FirFunctionSummary,
    options: &FirInlineOptions,
) -> FirInlineCandidateDecision {
    let mut reasons = Vec::new();

    if !options.enabled {
        reasons.push(FirInlineSkipReason::Disabled);
    }
    if !summary.has_body {
        reasons.push(FirInlineSkipReason::PrototypeOnly);
    }
    if summary.is_reserved_api {
        reasons.push(FirInlineSkipReason::ReservedApi);
    }
    if summary.is_recursive_scc && !options.allow_recursive {
        reasons.push(FirInlineSkipReason::RecursiveScc);
    }
    if options.inline_marked_only && !summary.is_inline {
        reasons.push(FirInlineSkipReason::NotMarkedInline);
    }
    if summary.has_body && summary.body_node_count > options.max_callee_nodes {
        reasons.push(FirInlineSkipReason::TooLarge {
            body_nodes: summary.body_node_count,
            max: options.max_callee_nodes,
        });
    }

    FirInlineCandidateDecision {
        function: summary.name.clone(),
        eligible: reasons.is_empty(),
        reasons,
    }
}

/// Options controlling hygienic FIR subtree cloning for future inlining.
///
/// This is the Milestone-2 rename engine used to clone callee bodies into a
/// destination store while avoiding local variable name capture/collisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirHygienicCloneOptions {
    /// Prefix used for generated fresh local names.
    ///
    /// Generated names are of the form `<prefix><counter>_<original>`.
    pub local_prefix: String,
}

impl Default for FirHygienicCloneOptions {
    fn default() -> Self {
        Self {
            local_prefix: "__fir_inl".to_string(),
        }
    }
}

/// Reusable freshness state for hygienic cloning across multiple callsites.
///
/// Reusing one state instance across repeated clones guarantees distinct fresh
/// local names across all those clones, which is required for future inlining
/// of the same callee multiple times into one caller block.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FirHygienicCloneState {
    /// Clone options (notably fresh-name prefix).
    pub options: FirHygienicCloneOptions,
    /// Next fresh local id.
    pub next_local_id: usize,
}

/// Kind of local binding that was renamed during hygienic cloning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirLocalRenameKind {
    /// Local `DeclareVar` (`kStack` or `kLoop`).
    DeclareVar,
    /// Local `DeclareTable` (`kStack` or `kLoop`).
    DeclareTable,
    /// `ForLoop.var` / `SimpleForLoop.var` loop control variable.
    LoopVar,
    /// `IteratorForLoop.iterators[*]`.
    IteratorVar,
    /// `DeclareBufferIterators` generated locals.
    BufferIterator,
}

/// One recorded local rename performed by the hygienic clone engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirLocalRename {
    /// Source FIR node that introduced the binding.
    pub origin_node: FirId,
    /// Original local symbol name.
    pub original: String,
    /// Fresh cloned symbol name.
    pub renamed: String,
    /// Access class of the renamed local.
    pub access: AccessType,
    /// Syntactic origin category.
    pub kind: FirLocalRenameKind,
}

/// Result of a hygienic subtree clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirHygienicCloneResult {
    /// Root node id in the destination store.
    pub root: FirId,
    /// Local symbol renames performed during the clone.
    pub local_renames: Vec<FirLocalRename>,
}

/// Errors returned by the hygienic clone engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirHygienicCloneError {
    /// Source node could not be decoded by `match_fir`.
    UnknownNode(FirId),
}

impl std::fmt::Display for FirHygienicCloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownNode(id) => write!(
                f,
                "hygienic FIR clone cannot clone unknown node {}",
                id.as_u32()
            ),
        }
    }
}

impl std::error::Error for FirHygienicCloneError {}

/// One formal-parameter materialization generated during inline preparation.
#[derive(Clone, Debug, PartialEq)]
pub struct FirMaterializedArgBinding {
    /// Formal parameter name from the callee signature.
    pub param_name: String,
    /// Fresh local stack variable storing the actual argument value.
    pub temp_name: String,
    /// Parameter type copied from the callee signature.
    pub typ: FirType,
    /// `DeclareVar(kStack, init=actual_arg)` statement node in destination FIR.
    pub declare_stmt: FirId,
}

/// Result of Milestone-3 callee-body preparation (args materialized + params substituted).
#[derive(Clone, Debug, PartialEq)]
pub struct FirPreparedInlineBody {
    /// Materialization statements to emit before the cloned callee body.
    ///
    /// Evaluation order is left-to-right in the original actual-argument order.
    pub arg_materialization_stmts: Vec<FirId>,
    /// Hygienically cloned callee body with `kFunArgs` references substituted to stack temps.
    pub body: FirId,
    /// Per-parameter temp bindings created during materialization.
    pub param_bindings: Vec<FirMaterializedArgBinding>,
    /// Local renames performed while cloning the callee body.
    pub local_renames: Vec<FirLocalRename>,
}

/// Errors returned by Milestone-3 callee inline preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirInlinePrepareError {
    /// `callee_decl` is not a `DeclareFun`.
    CalleeNotFunction(FirId),
    /// Callee is a prototype (`body=None`) and cannot be prepared for inlining.
    CalleeHasNoBody { name: String, node: FirId },
    /// Number of actual arguments does not match formal parameters.
    ArgCountMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
    /// Error while cloning/materializing source FIR.
    Clone(FirHygienicCloneError),
}

impl std::fmt::Display for FirInlinePrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CalleeNotFunction(id) => write!(
                f,
                "inline preparation expects a DeclareFun callee, got node {}",
                id.as_u32()
            ),
            Self::CalleeHasNoBody { name, node } => write!(
                f,
                "callee '{name}' has no body and cannot be prepared for inlining (node={})",
                node.as_u32()
            ),
            Self::ArgCountMismatch {
                name,
                expected,
                got,
            } => write!(f, "callee '{name}' expects {expected} args, got {got}"),
            Self::Clone(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for FirInlinePrepareError {}

impl From<FirHygienicCloneError> for FirInlinePrepareError {
    fn from(value: FirHygienicCloneError) -> Self {
        Self::Clone(value)
    }
}

/// One-pass callsite inlining statistics for [`inline_fir_module_once`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirInlineRewriteStats {
    /// Number of `FunCall` nodes visited while rewriting function bodies.
    pub callsites_seen: usize,
    /// Number of callsites actually inlined.
    pub callsites_inlined: usize,
    /// Calls skipped because the callee is not an eligible candidate.
    pub callsites_skipped_non_candidate: usize,
    /// Calls skipped because the callee body shape is not yet supported for
    /// value extraction/splicing (for example non-canonical returns).
    pub callsites_skipped_unsupported_shape: usize,
    /// Calls skipped because the callee name is unknown in the analyzed module.
    pub callsites_skipped_unknown_callee: usize,
}

/// Why [`inline_fir_module`] stopped iterating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirInlineFixpointStopReason {
    /// Last pass produced no new inlined callsites.
    Fixpoint,
    /// Reached `FirInlineOptions.max_inline_depth` iterations.
    MaxIterations,
    /// Output module exceeded the configured expansion budget.
    ExpansionBudget,
}

/// Aggregate statistics for iterative module inlining (`Phase E`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirInlineFixpointStats {
    /// Number of rewrite passes executed.
    pub iterations: usize,
    /// Per-pass one-pass rewrite statistics in execution order.
    pub pass_stats: Vec<FirInlineRewriteStats>,
    /// Sum of all per-pass `callsites_seen`.
    pub total_callsites_seen: usize,
    /// Sum of all per-pass `callsites_inlined`.
    pub total_callsites_inlined: usize,
    /// Number of passes with `callsites_inlined > 0`.
    pub passes_with_progress: usize,
    /// Unique-node count of the input module used as the expansion baseline.
    pub baseline_module_nodes: usize,
    /// Final unique-node count of the returned module.
    pub final_module_nodes: usize,
    /// Budget threshold used for expansion checks.
    pub expansion_limit_nodes: usize,
    /// Why iteration stopped.
    pub stop_reason: FirInlineFixpointStopReason,
}

/// Errors returned by the one-pass FIR module inliner rewrite.
#[derive(Debug)]
pub enum FirInlineRewriteError {
    /// Analysis stage failed (invalid module shape, duplicate functions, ...).
    Analysis(FirInlineAnalysisError),
    /// Hygienic cloning failed on an unsupported/unknown node.
    Clone(FirHygienicCloneError),
    /// Callee-body preparation failed during a callsite inline attempt.
    Prepare(FirInlinePrepareError),
}

impl std::fmt::Display for FirInlineRewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Analysis(err) => err.fmt(f),
            Self::Clone(err) => err.fmt(f),
            Self::Prepare(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for FirInlineRewriteError {}

impl From<FirInlineAnalysisError> for FirInlineRewriteError {
    fn from(value: FirInlineAnalysisError) -> Self {
        Self::Analysis(value)
    }
}

impl From<FirHygienicCloneError> for FirInlineRewriteError {
    fn from(value: FirHygienicCloneError) -> Self {
        Self::Clone(value)
    }
}

impl From<FirInlinePrepareError> for FirInlineRewriteError {
    fn from(value: FirInlinePrepareError) -> Self {
        Self::Prepare(value)
    }
}

/// Iteratively inline FIR callsites until a fixpoint or budget is reached.
///
/// Phase E wraps [`inline_fir_module_once`] in a deterministic fixpoint driver:
/// - reruns one-pass inlining on the rewritten module,
/// - stops when a pass performs no inlining,
/// - or when `max_inline_depth`/expansion budget limits are reached.
///
/// Functions are still analyzed per pass, so candidate decisions and SCCs are
/// recomputed after each transformation step.
///
/// # Errors
/// Returns [`FirInlineRewriteError`] if any pass fails analysis or rewriting.
pub fn inline_fir_module(
    src_store: &FirStore,
    module: FirId,
    options: &FirInlineOptions,
) -> Result<(FirStore, FirId, FirInlineFixpointStats), FirInlineRewriteError> {
    let baseline_nodes = count_unique_nodes(src_store, module);
    let expansion_factor = options.max_expansion_factor.max(1);
    let expansion_limit_nodes = baseline_nodes.saturating_mul(expansion_factor);
    let max_iterations = options.max_inline_depth.max(1);

    let mut current_store: Option<FirStore> = None;
    let mut current_module = module;
    let mut pass_stats = Vec::new();
    let mut total_callsites_seen = 0usize;
    let mut total_callsites_inlined = 0usize;
    let mut passes_with_progress = 0usize;
    let mut final_nodes = baseline_nodes;
    let mut stop_reason = FirInlineFixpointStopReason::MaxIterations;

    for _iter in 0..max_iterations {
        let (next_store, next_module, stats) = if let Some(store) = current_store.as_ref() {
            inline_fir_module_once(store, current_module, options)?
        } else {
            inline_fir_module_once(src_store, current_module, options)?
        };

        total_callsites_seen += stats.callsites_seen;
        total_callsites_inlined += stats.callsites_inlined;
        if stats.callsites_inlined > 0 {
            passes_with_progress += 1;
        }
        final_nodes = count_unique_nodes(&next_store, next_module);
        let had_progress = stats.callsites_inlined > 0;
        pass_stats.push(stats);
        current_module = next_module;
        current_store = Some(next_store);

        if !had_progress {
            stop_reason = FirInlineFixpointStopReason::Fixpoint;
            break;
        }
        if final_nodes > expansion_limit_nodes {
            stop_reason = FirInlineFixpointStopReason::ExpansionBudget;
            break;
        }
    }

    let out_store = current_store.expect("at least one iteration is always executed");
    let out_module = current_module;
    let stats = FirInlineFixpointStats {
        iterations: pass_stats.len(),
        pass_stats,
        total_callsites_seen,
        total_callsites_inlined,
        passes_with_progress,
        baseline_module_nodes: baseline_nodes,
        final_module_nodes: final_nodes,
        expansion_limit_nodes,
        stop_reason,
    };
    Ok((out_store, out_module, stats))
}

/// Hygienically clones a FIR subtree into `dst_store` using a fresh local-state.
///
/// This convenience wrapper is suitable for one-off clones. For repeated clones
/// into the same destination (future inlining of the same callee in multiple
/// callsites), prefer [`clone_fir_hygienic_with_state`] so fresh local names
/// remain unique across clones.
///
/// # Errors
/// Returns [`FirHygienicCloneError`] if the source subtree contains an unknown
/// FIR node.
pub fn clone_fir_hygienic(
    src_store: &FirStore,
    root: FirId,
    dst_store: &mut FirStore,
) -> Result<FirHygienicCloneResult, FirHygienicCloneError> {
    let mut state = FirHygienicCloneState::default();
    clone_fir_hygienic_with_state(src_store, root, dst_store, &mut state)
}

/// Hygienically clones a FIR subtree into `dst_store` using caller-provided freshness state.
///
/// Local symbols declared in the cloned subtree (`kStack`, `kLoop`, loop
/// iterators, and buffer iterators) are renamed to fresh names and all matching
/// references are rewritten consistently.
///
/// The clone is scope-aware:
/// - `Block` introduces a new lexical frame,
/// - `If`/`Switch` branches are cloned in isolated branch frames,
/// - loop constructs introduce loop frames so iterator/loop-variable names do
///   not leak outside the cloned loop.
///
/// # Errors
/// Returns [`FirHygienicCloneError`] if the source subtree contains an unknown
/// FIR node.
pub fn clone_fir_hygienic_with_state(
    src_store: &FirStore,
    root: FirId,
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
) -> Result<FirHygienicCloneResult, FirHygienicCloneError> {
    let mut cloner = HygienicCloner::new(src_store, dst_store, state);
    cloner.push_scope();
    let root = cloner.clone_node(root)?;
    cloner.pop_scope();
    Ok(FirHygienicCloneResult {
        root,
        local_renames: cloner.local_renames,
    })
}

/// Clones one sub-module body into the same store with explicit `kFunArgs` and
/// `kStruct` substitutions, for [`crate::subcontainer::flatten_sub_modules`].
///
/// Unlike [`prepare_callee_body_for_inlining`], no argument is materialized
/// into a temp: the caller has already decided what each parameter maps to,
/// because a flattened `fill` writes directly into the caller's own table
/// rather than through a copied pointer. Local `kStack`/`kLoop` names are
/// still renamed hygienically, which is what keeps the sub-module's loop
/// counters from colliding with the enclosing function's.
///
/// # Errors
/// Returns [`FirHygienicCloneError`] when the body contains a node the clone
/// engine does not handle.
pub(crate) fn flatten_clone_body(
    store: &mut FirStore,
    body: FirId,
    state: &mut FirHygienicCloneState,
    fun_arg_subst: &HashMap<String, (String, AccessType)>,
    struct_subst: &HashMap<String, (String, AccessType)>,
) -> Result<FirId, FirHygienicCloneError> {
    // Source and destination are the same store, which the cloner cannot
    // borrow twice. Clone into a scratch store, restore the original, then
    // re-intern the result into it.
    //
    // Writing the clone directly into the emptied `store` and then restoring
    // the source over it is the obvious-looking version and is wrong: it
    // discards every node just written, leaving the returned id dangling. The
    // FIR matcher decodes such an id as `Unknown`, which most consumers ignore
    // rather than reject, so the damage surfaces far from here.
    let src = std::mem::take(store);
    let mut scratch = FirStore::new();
    let mut cloner = HygienicCloner::new(&src, &mut scratch, state);
    cloner.fun_arg_subst = fun_arg_subst.clone();
    cloner.struct_subst = struct_subst.clone();
    cloner.push_scope();
    let result = cloner.clone_node(body);
    cloner.pop_scope();
    let cloned = result?;
    *store = src;
    Ok(store.import_from(&scratch, cloned))
}

/// Prepares a callee body for future inlining by materializing actual arguments and
/// substituting `kFunArgs` references to fresh stack temporaries.
///
/// This function implements the Milestone-3 "parameter materialization +
/// substitution" stage without rewriting a caller `FunCall` yet.
///
/// Current policy is intentionally conservative:
/// - **all** actual arguments are materialized into fresh `kStack` temporaries
///   (left-to-right evaluation order),
/// - callee body is then hygienically cloned with every matching `kFunArgs`
///   access rewritten to the corresponding temp as `kStack`.
///
/// The returned [`FirPreparedInlineBody`] can later be spliced into a caller
/// block by the callsite inliner implementation (Milestone 4+).
///
/// # Errors
/// Returns [`FirInlinePrepareError`] when `callee_decl` is not a function,
/// the callee has no body, the argument count mismatches, or cloning fails.
pub fn prepare_callee_body_for_inlining(
    src_store: &FirStore,
    callee_decl: FirId,
    actual_args: &[FirId],
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
) -> Result<FirPreparedInlineBody, FirInlinePrepareError> {
    let FirMatch::DeclareFun {
        name, args, body, ..
    } = match_fir(src_store, callee_decl)
    else {
        return Err(FirInlinePrepareError::CalleeNotFunction(callee_decl));
    };
    let Some(body_id) = body else {
        return Err(FirInlinePrepareError::CalleeHasNoBody {
            name,
            node: callee_decl,
        });
    };
    if args.len() != actual_args.len() {
        return Err(FirInlinePrepareError::ArgCountMismatch {
            name,
            expected: args.len(),
            got: actual_args.len(),
        });
    }

    let mut arg_materialization_stmts = Vec::with_capacity(actual_args.len());
    let mut param_bindings = Vec::with_capacity(actual_args.len());
    let mut subst = HashMap::<String, String>::new();

    for (param, actual_arg) in args.iter().zip(actual_args.iter().copied()) {
        let mut arg_cloner = HygienicCloner::new(src_store, dst_store, state);
        arg_cloner.push_scope();
        let actual_cloned = arg_cloner.clone_node(actual_arg)?;
        arg_cloner.pop_scope();

        let temp_name = state_fresh_local_name(state, &format!("arg_{}", param.name));
        let decl = {
            let mut b = FirBuilder::new(dst_store);
            b.declare_var(
                temp_name.clone(),
                param.typ.clone(),
                AccessType::Stack,
                Some(actual_cloned),
            )
        };
        subst.insert(param.name.clone(), temp_name.clone());
        arg_materialization_stmts.push(decl);
        param_bindings.push(FirMaterializedArgBinding {
            param_name: param.name.clone(),
            temp_name,
            typ: param.typ.clone(),
            declare_stmt: decl,
        });
    }
    prepare_callee_body_for_inlining_with_cloned_args(
        src_store,
        callee_decl,
        body_id,
        dst_store,
        state,
        arg_materialization_stmts,
        param_bindings,
        subst,
    )
}

fn state_fresh_local_name(state: &mut FirHygienicCloneState, original: &str) -> String {
    let id = state.next_local_id;
    state.next_local_id += 1;
    format!("{}{}_{}", state.options.local_prefix, id, original)
}

#[allow(clippy::too_many_arguments)]
fn prepare_callee_body_for_inlining_with_cloned_args(
    src_store: &FirStore,
    _callee_decl: FirId,
    body_id: FirId,
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
    arg_materialization_stmts: Vec<FirId>,
    param_bindings: Vec<FirMaterializedArgBinding>,
    subst: HashMap<String, String>,
) -> Result<FirPreparedInlineBody, FirInlinePrepareError> {
    let mut cloner = HygienicCloner::new(src_store, dst_store, state);
    cloner.fun_arg_subst = subst
        .into_iter()
        .map(|(param, temp)| (param, (temp, AccessType::Stack)))
        .collect();
    cloner.push_scope();
    let body = cloner.clone_node(body_id)?;
    cloner.pop_scope();

    Ok(FirPreparedInlineBody {
        arg_materialization_stmts,
        body,
        param_bindings,
        local_renames: cloner.local_renames,
    })
}

fn prepare_callee_body_for_inlining_from_cloned_args(
    src_store: &FirStore,
    callee_decl: FirId,
    actual_args_in_dst: &[FirId],
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
) -> Result<FirPreparedInlineBody, FirInlinePrepareError> {
    let FirMatch::DeclareFun {
        name, args, body, ..
    } = match_fir(src_store, callee_decl)
    else {
        return Err(FirInlinePrepareError::CalleeNotFunction(callee_decl));
    };
    let Some(body_id) = body else {
        return Err(FirInlinePrepareError::CalleeHasNoBody {
            name,
            node: callee_decl,
        });
    };
    if args.len() != actual_args_in_dst.len() {
        return Err(FirInlinePrepareError::ArgCountMismatch {
            name,
            expected: args.len(),
            got: actual_args_in_dst.len(),
        });
    }

    let mut arg_materialization_stmts = Vec::with_capacity(actual_args_in_dst.len());
    let mut param_bindings = Vec::with_capacity(actual_args_in_dst.len());
    let mut subst = HashMap::<String, String>::new();

    for (param, actual_arg) in args.iter().zip(actual_args_in_dst.iter().copied()) {
        let temp_name = state_fresh_local_name(state, &format!("arg_{}", param.name));
        let decl = {
            let mut b = FirBuilder::new(dst_store);
            b.declare_var(
                temp_name.clone(),
                param.typ.clone(),
                AccessType::Stack,
                Some(actual_arg),
            )
        };
        subst.insert(param.name.clone(), temp_name.clone());
        arg_materialization_stmts.push(decl);
        param_bindings.push(FirMaterializedArgBinding {
            param_name: param.name.clone(),
            temp_name,
            typ: param.typ.clone(),
            declare_stmt: decl,
        });
    }

    prepare_callee_body_for_inlining_with_cloned_args(
        src_store,
        callee_decl,
        body_id,
        dst_store,
        state,
        arg_materialization_stmts,
        param_bindings,
        subst,
    )
}

/// Inline eligible FIR callsites in one pass over all function bodies in a module.
///
/// This implements the first callsite-rewrite milestone:
/// - analyze the module with [`analyze_fir_inliner`],
/// - rewrite every function body once,
/// - splice prepared callee statements for calls whose callee is an eligible
///   candidate **and** whose body has a canonical inlineable shape.
///
/// Current limitations (deferred to later milestones):
/// - one pass only (no fixpoint iteration),
/// - inlined callee bodies are not recursively re-inlined in the same pass,
/// - only canonical callee bodies ending with `Return(Some(v))` are inlined.
///
/// # Errors
/// Returns [`FirInlineRewriteError`] if module analysis fails or if the rewriter
/// encounters a cloning/preparation error on supported rewrite paths.
pub fn inline_fir_module_once(
    src_store: &FirStore,
    module: FirId,
    options: &FirInlineOptions,
) -> Result<(FirStore, FirId, FirInlineRewriteStats), FirInlineRewriteError> {
    let analysis = analyze_fir_inliner(src_store, module, options)?;
    let mut dst_store = FirStore::new();
    let mut state = FirHygienicCloneState::default();
    let mut stats = FirInlineRewriteStats::default();
    let rewrite_order = function_rewrite_order_by_scc(&analysis);

    let fn_decls: BTreeMap<String, FirId> = analysis
        .functions
        .iter()
        .map(|(name, summary)| (name.clone(), summary.decl_id))
        .collect();

    let module = rewrite_module_once(
        src_store,
        module,
        &analysis,
        &fn_decls,
        &rewrite_order,
        &mut dst_store,
        &mut state,
        &mut stats,
    )?;

    Ok((dst_store, module, stats))
}

#[allow(clippy::too_many_arguments)]
fn rewrite_module_once(
    src_store: &FirStore,
    module: FirId,
    analysis: &FirInlineAnalysis,
    fn_decls: &BTreeMap<String, FirId>,
    rewrite_order: &[String],
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
    stats: &mut FirInlineRewriteStats,
) -> Result<FirId, FirInlineRewriteError> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(src_store, module)
    else {
        return Err(FirInlineRewriteError::Analysis(
            FirInlineAnalysisError::RootNotModule,
        ));
    };

    let dsp_struct = clone_fir_hygienic_with_state(src_store, dsp_struct, dst_store, state)?.root;
    let static_decls =
        clone_fir_hygienic_with_state(src_store, static_decls, dst_store, state)?.root;
    let globals = rewrite_fun_section_once(
        src_store,
        globals,
        analysis,
        fn_decls,
        rewrite_order,
        dst_store,
        state,
        stats,
        FirFunctionSection::Globals,
    )?;
    let functions = rewrite_fun_section_once(
        src_store,
        functions,
        analysis,
        fn_decls,
        rewrite_order,
        dst_store,
        state,
        stats,
        FirFunctionSection::Functions,
    )?;

    // Sub-modules are self-contained programs with their own state and their
    // own two entry points; nothing in them is a callsite this pass rewrites,
    // so they are cloned across unchanged rather than walked for inlining.
    let mut cloned_sub_modules = Vec::new();
    if let FirMatch::Block(items) = match_fir(src_store, sub_modules) {
        for item in items {
            cloned_sub_modules
                .push(clone_fir_hygienic_with_state(src_store, item, dst_store, state)?.root);
        }
    }
    let mut b = FirBuilder::new(dst_store);
    Ok(b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &cloned_sub_modules,
    ))
}

#[allow(clippy::too_many_arguments)]
fn rewrite_fun_section_once(
    src_store: &FirStore,
    section_id: FirId,
    analysis: &FirInlineAnalysis,
    fn_decls: &BTreeMap<String, FirId>,
    rewrite_order: &[String],
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
    stats: &mut FirInlineRewriteStats,
    section_kind: FirFunctionSection,
) -> Result<FirId, FirInlineRewriteError> {
    let FirMatch::Block(items) = match_fir(src_store, section_id) else {
        return Err(FirInlineRewriteError::Analysis(
            FirInlineAnalysisError::InvalidModuleSection {
                section: "section",
                node: section_id,
            },
        ));
    };

    let mut body_ids_by_name = BTreeMap::<String, FirId>::new();
    for item in &items {
        if let FirMatch::DeclareFun {
            name,
            body: Some(body),
            ..
        } = match_fir(src_store, *item)
        {
            body_ids_by_name.insert(name, body);
        }
    }

    let mut rewritten_bodies = BTreeMap::<String, FirId>::new();
    for name in rewrite_order {
        let Some(summary) = analysis.functions.get(name) else {
            continue;
        };
        if summary.section != section_kind || !summary.has_body {
            continue;
        }
        let Some(body) = body_ids_by_name.get(name).copied() else {
            continue;
        };
        let rewritten = rewrite_function_body_once(
            src_store, body, analysis, fn_decls, dst_store, state, stats,
        )?;
        rewritten_bodies.insert(name.clone(), rewritten);
    }

    let mut out_items = Vec::with_capacity(items.len());
    for item in items {
        match match_fir(src_store, item) {
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: Some(_body),
                is_inline,
            } => {
                let body = *rewritten_bodies
                    .get(&name)
                    .expect("all function bodies in section should be rewritten");
                let mut b = FirBuilder::new(dst_store);
                out_items.push(b.declare_fun(name, typ, &args, Some(body), is_inline));
            }
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: None,
                is_inline,
            } => {
                let mut b = FirBuilder::new(dst_store);
                out_items.push(b.declare_fun(name, typ, &args, None, is_inline));
            }
            _ => {
                out_items
                    .push(clone_fir_hygienic_with_state(src_store, item, dst_store, state)?.root);
            }
        }
    }
    let mut b = FirBuilder::new(dst_store);
    Ok(b.block(&out_items))
}

fn rewrite_function_body_once(
    src_store: &FirStore,
    body: FirId,
    analysis: &FirInlineAnalysis,
    fn_decls: &BTreeMap<String, FirId>,
    dst_store: &mut FirStore,
    state: &mut FirHygienicCloneState,
    stats: &mut FirInlineRewriteStats,
) -> Result<FirId, FirInlineRewriteError> {
    let mut rw = InlineBodyRewriter {
        src: src_store,
        dst: dst_store,
        analysis,
        fn_decls,
        state,
        stats,
    };
    rw.rewrite_stmt_root(body)
}

/// Recognizes the canonical inlineable body shape produced by preparation.
///
/// The current rewrite stage only knows how to splice callee bodies that are:
/// 1. a `Block`,
/// 2. with no early `Return` in the prefix,
/// 3. ending in `Return(Some(value))`.
///
/// The returned pair is `(prefix_statements, returned_value)`.
fn canonical_inline_body_from_prepared(
    store: &FirStore,
    body: FirId,
) -> Option<(Vec<FirId>, FirId)> {
    let FirMatch::Block(stmts) = match_fir(store, body) else {
        return None;
    };
    let (last, prefix) = stmts.split_last()?;
    if prefix
        .iter()
        .any(|stmt| matches!(match_fir(store, *stmt), FirMatch::Return(_)))
    {
        return None;
    }
    let FirMatch::Return(Some(ret_value)) = match_fir(store, *last) else {
        return None;
    };
    Some((prefix.to_vec(), ret_value))
}

/// Root rewriter for one function body in one inline pass.
///
/// This wrapper owns the cross-callsite analysis and statistics references and
/// creates a fresh statement/value rewriter rooted in one hygienic clone state.
struct InlineBodyRewriter<'a, 'b> {
    src: &'a FirStore,
    dst: &'b mut FirStore,
    analysis: &'a FirInlineAnalysis,
    fn_decls: &'a BTreeMap<String, FirId>,
    state: &'b mut FirHygienicCloneState,
    stats: &'b mut FirInlineRewriteStats,
}

impl<'a, 'b> InlineBodyRewriter<'a, 'b> {
    /// Rewrites one function body root as a statement tree.
    fn rewrite_stmt_root(&mut self, root: FirId) -> Result<FirId, FirInlineRewriteError> {
        let mut inner = InlineStmtCloner {
            cloner: HygienicCloner::new(self.src, self.dst, self.state),
            analysis: self.analysis,
            fn_decls: self.fn_decls,
            stats: self.stats,
        };
        inner.rewrite_stmt_as_stmt(root)
    }
}

/// Statement/value rewriter used by one inline pass on one function body.
///
/// It interleaves hygienic cloning with conservative callsite expansion:
/// statements may expand to multiple statements (argument materialization plus
/// inlined callee prefix), while values return `(prefix_stmts, rewritten_value)`.
struct InlineStmtCloner<'a, 'b, 'c> {
    cloner: HygienicCloner<'a, 'b>,
    analysis: &'c FirInlineAnalysis,
    fn_decls: &'c BTreeMap<String, FirId>,
    stats: &'c mut FirInlineRewriteStats,
}

impl<'a, 'b, 'c> InlineStmtCloner<'a, 'b, 'c> {
    /// Rewrites one statement root, normalizing multi-statement expansions into a `Block`.
    fn rewrite_stmt_as_stmt(&mut self, id: FirId) -> Result<FirId, FirInlineRewriteError> {
        let stmts = self.rewrite_stmt_to_vec(id)?;
        if stmts.len() == 1 {
            Ok(stmts[0])
        } else {
            let mut b = FirBuilder::new(self.cloner.dst);
            Ok(b.block(&stmts))
        }
    }

    /// Rewrites a lexical block with a fresh local-rename scope.
    fn rewrite_block(&mut self, stmts: Vec<FirId>) -> Result<FirId, FirInlineRewriteError> {
        self.cloner.push_scope();
        let mut out = Vec::new();
        for stmt in stmts {
            out.extend(self.rewrite_stmt_to_vec(stmt)?);
        }
        self.cloner.pop_scope();
        let mut b = FirBuilder::new(self.cloner.dst);
        Ok(b.block(&out))
    }

    /// Rewrites one statement and returns the flattened emitted statement sequence.
    fn rewrite_stmt_to_vec(&mut self, id: FirId) -> Result<Vec<FirId>, FirInlineRewriteError> {
        let out = match match_fir(self.cloner.src, id) {
            FirMatch::Block(stmts) => vec![self.rewrite_block(stmts)?],
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            } => {
                let mut prefix = Vec::new();
                let init = if let Some(init_id) = init {
                    let (mut p, v) = self.rewrite_value(init_id)?;
                    prefix.append(&mut p);
                    Some(v)
                } else {
                    None
                };
                let name = if matches!(access, AccessType::Stack | AccessType::Loop) {
                    self.cloner
                        .bind_local_decl(id, &name, access, FirLocalRenameKind::DeclareVar)
                } else {
                    name
                };
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.declare_var(name, typ, access, init)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::DeclareTable {
                name,
                access,
                elem_type,
                values,
            } => {
                let mut prefix = Vec::new();
                let mut cloned_values = Vec::with_capacity(values.len());
                for v in values {
                    let (mut p, vv) = self.rewrite_value(v)?;
                    prefix.append(&mut p);
                    cloned_values.push(vv);
                }
                let name = if matches!(access, AccessType::Stack | AccessType::Loop) {
                    self.cloner
                        .bind_local_decl(id, &name, access, FirLocalRenameKind::DeclareTable)
                } else {
                    name
                };
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.declare_table(name, access, elem_type, &cloned_values)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                let (mut prefix, value) = self.rewrite_value(value)?;
                let remap = self.cloner.remap_name_by_access(&name, access);
                let out_access = self.cloner.remap_access(access, &name);
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.store_var(remap, out_access, value)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                let (mut p_idx, idx) = self.rewrite_value(index)?;
                let (mut p_val, val) = self.rewrite_value(value)?;
                let mut prefix = Vec::new();
                prefix.append(&mut p_idx);
                prefix.append(&mut p_val);
                let remap = self.cloner.remap_name_by_access(&name, access);
                let out_access = self.cloner.remap_access(access, &name);
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.store_table(remap, out_access, idx, val)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::Drop(v) => {
                let (mut prefix, v) = self.rewrite_value(v)?;
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.drop_(v)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::Return(v) => {
                let mut prefix = Vec::new();
                let v = if let Some(v) = v {
                    let (mut p, vv) = self.rewrite_value(v)?;
                    prefix.append(&mut p);
                    Some(vv)
                } else {
                    None
                };
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.ret(v)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let (mut prefix, cond) = self.rewrite_value(cond)?;
                let then_stmt = self.rewrite_stmt_as_stmt(then_block)?;
                let else_stmt = match else_block {
                    Some(v) => Some(self.rewrite_stmt_as_stmt(v)?),
                    None => None,
                };
                let stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.if_(cond, then_stmt, else_stmt)
                };
                prefix.push(stmt);
                prefix
            }
            FirMatch::Control { cond, stmt } => {
                let (mut prefix, cond) = self.rewrite_value(cond)?;
                let stmt = self.rewrite_stmt_as_stmt(stmt)?;
                let out_stmt = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.control(cond, stmt)
                };
                prefix.push(out_stmt);
                prefix
            }
            // Current stage: loops/switch are cloned hygienically but do not receive
            // inline-call rewriting in their nested expressions/bodies yet.
            FirMatch::ForLoop { .. }
            | FirMatch::SimpleForLoop { .. }
            | FirMatch::IteratorForLoop { .. }
            | FirMatch::WhileLoop { .. }
            | FirMatch::Switch { .. } => vec![self.cloner.clone_node(id)?],
            _ => vec![self.cloner.clone_node(id)?],
        };
        Ok(out)
    }

    /// Rewrites one value expression, returning any required prefix statements.
    ///
    /// Prefix statements preserve evaluation order for side-effecting argument
    /// materialization before the final value is consumed by the surrounding
    /// statement or expression node.
    fn rewrite_value(&mut self, id: FirId) -> Result<(Vec<FirId>, FirId), FirInlineRewriteError> {
        let node = match_fir(self.cloner.src, id);
        let out = match node {
            FirMatch::FunCall { name, args, typ } => {
                self.stats.callsites_seen += 1;
                let mut prefix = Vec::new();
                let mut rewritten_args = Vec::with_capacity(args.len());
                for arg in args {
                    let (mut p, v) = self.rewrite_value(arg)?;
                    prefix.append(&mut p);
                    rewritten_args.push(v);
                }

                let callee_decl = match self.fn_decls.get(&name).copied() {
                    Some(id) => id,
                    None => {
                        self.stats.callsites_skipped_unknown_callee += 1;
                        let call = {
                            let mut b = FirBuilder::new(self.cloner.dst);
                            b.fun_call(name, &rewritten_args, typ)
                        };
                        return Ok((prefix, call));
                    }
                };

                let candidate = self
                    .analysis
                    .candidate_decisions
                    .get(&name)
                    .map(|d| d.eligible)
                    .unwrap_or(false);
                if !candidate {
                    self.stats.callsites_skipped_non_candidate += 1;
                    let call = {
                        let mut b = FirBuilder::new(self.cloner.dst);
                        b.fun_call(name, &rewritten_args, typ)
                    };
                    (prefix, call)
                } else {
                    let prepared = prepare_callee_body_for_inlining_from_cloned_args(
                        self.cloner.src,
                        callee_decl,
                        &rewritten_args,
                        self.cloner.dst,
                        self.cloner.state,
                    )?;
                    if let Some((body_prefix, ret_value)) =
                        canonical_inline_body_from_prepared(self.cloner.dst, prepared.body)
                    {
                        self.stats.callsites_inlined += 1;
                        prefix.extend(prepared.arg_materialization_stmts);
                        prefix.extend(body_prefix);
                        (prefix, ret_value)
                    } else {
                        self.stats.callsites_skipped_unsupported_shape += 1;
                        let call = {
                            let mut b = FirBuilder::new(self.cloner.dst);
                            b.fun_call(name, &rewritten_args, typ)
                        };
                        (prefix, call)
                    }
                }
            }
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                let (prefix, index) = self.rewrite_value(index)?;
                let remap = self.cloner.remap_name_by_access(&name, access);
                let out_access = self.cloner.remap_access(access, &name);
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.load_table(remap, out_access, index, typ)
                };
                (prefix, v)
            }
            FirMatch::TeeVar {
                name,
                access,
                value,
                typ,
            } => {
                let (prefix, value) = self.rewrite_value(value)?;
                let remap = self.cloner.remap_name_by_access(&name, access);
                let out_access = self.cloner.remap_access(access, &name);
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.tee_var(remap, out_access, value, typ)
                };
                (prefix, v)
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                let (mut p1, lhs) = self.rewrite_value(lhs)?;
                let (mut p2, rhs) = self.rewrite_value(rhs)?;
                p1.append(&mut p2);
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.binop(op, lhs, rhs, typ)
                };
                (p1, v)
            }
            FirMatch::Neg { value, typ } => {
                let (prefix, value) = self.rewrite_value(value)?;
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.neg(value, typ)
                };
                (prefix, v)
            }
            FirMatch::Cast { typ, value } => {
                let (prefix, value) = self.rewrite_value(value)?;
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.cast(typ, value)
                };
                (prefix, v)
            }
            FirMatch::Bitcast { typ, value } => {
                let (prefix, value) = self.rewrite_value(value)?;
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.bitcast(typ, value)
                };
                (prefix, v)
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                let (mut p0, cond) = self.rewrite_value(cond)?;
                let (mut p1, then_value) = self.rewrite_value(then_value)?;
                let (mut p2, else_value) = self.rewrite_value(else_value)?;
                p0.append(&mut p1);
                p0.append(&mut p2);
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.select2(cond, then_value, else_value, typ)
                };
                (p0, v)
            }
            FirMatch::ValueArray { values, typ } => {
                let mut prefix = Vec::new();
                let mut out_vals = Vec::with_capacity(values.len());
                for v in values {
                    let (mut p, vv) = self.rewrite_value(v)?;
                    prefix.append(&mut p);
                    out_vals.push(vv);
                }
                let v = {
                    let mut b = FirBuilder::new(self.cloner.dst);
                    b.value_array(&out_vals, typ)
                };
                (prefix, v)
            }
            _ => (Vec::new(), self.cloner.clone_node(id)?),
        };
        Ok(out)
    }
}

/// Hygienic subtree cloner shared by the preparation and rewrite stages.
///
/// The cloner preserves FIR semantics while renaming stack/loop locals to avoid
/// capture, and can additionally remap `kFunArgs` references to materialized
/// temporary stack slots during inline preparation.
/// Rebuilds a module while removing pure `Drop` roots introduced only to
/// materialize a shared expression DAG during FIR construction.
///
/// This is a mandatory canonicalization boundary for verified FIR: a retained
/// `Drop` always has a potentially observable evaluation, while loads,
/// constants, and pure arithmetic roots are removed once for every backend.
/// Local names are deliberately preserved so textual output remains stable.
#[must_use]
pub fn sweep_scaffolding_drop_roots(src_store: &FirStore, module: FirId) -> (FirStore, FirId) {
    let (store, module, _) = sweep_scaffolding_drop_roots_with_mapping(src_store, module);
    (store, module)
}

/// Provenance-aware variant of [`sweep_scaffolding_drop_roots`].
///
/// The returned `(source, destination)` pairs cover every cloned node. A
/// source may occur more than once because lexical cloning can duplicate a
/// shared FIR node; callers must therefore treat the mapping as one-to-many.
#[must_use]
pub fn sweep_scaffolding_drop_roots_with_mapping(
    src_store: &FirStore,
    module: FirId,
) -> (FirStore, FirId, Vec<(FirId, FirId)>) {
    let mut dst_store = FirStore::new();
    let mut state = FirHygienicCloneState::default();
    let mut cloner = HygienicCloner::new(src_store, &mut dst_store, &mut state);
    cloner.preserve_local_names = true;
    cloner.sweep_scaffolding_drop_roots = true;
    cloner.push_scope();
    let module = cloner
        .clone_node(module)
        .expect("verified FIR must contain only cloneable node kinds");
    cloner.pop_scope();
    let mapping = std::mem::take(&mut cloner.clone_pairs);
    (dst_store, module, mapping)
}

/// Returns whether evaluating `value` has no observable side effect.
///
/// This predicate is deliberately conservative: ordinary foreign calls are
/// retained, while FIR's canonical math calls are pure when all arguments are
/// pure. It is shared by FIR canonicalization, verification, and vector-root
/// materialization so all three agree on when a [`FirMatch::Drop`] is dead.
#[must_use]
pub fn is_obviously_side_effect_free_value(store: &FirStore, value: FirId) -> bool {
    match match_fir(store, value) {
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
        | FirMatch::LoadVar { .. }
        | FirMatch::LoadVarAddress { .. }
        | FirMatch::NullValue { .. } => true,
        FirMatch::LoadTable { index, .. } => is_obviously_side_effect_free_value(store, index),
        FirMatch::ValueArray { values, .. } => values
            .iter()
            .all(|&item| is_obviously_side_effect_free_value(store, item)),
        FirMatch::BinOp { lhs, rhs, .. } => {
            is_obviously_side_effect_free_value(store, lhs)
                && is_obviously_side_effect_free_value(store, rhs)
        }
        FirMatch::Neg { value, .. }
        | FirMatch::Cast { value, .. }
        | FirMatch::Bitcast { value, .. } => is_obviously_side_effect_free_value(store, value),
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            is_obviously_side_effect_free_value(store, cond)
                && is_obviously_side_effect_free_value(store, then_value)
                && is_obviously_side_effect_free_value(store, else_value)
        }
        FirMatch::FunCall { name, args, .. } => {
            FirMathOp::from_symbol(&name).is_some()
                && args
                    .iter()
                    .all(|&arg| is_obviously_side_effect_free_value(store, arg))
        }
        _ => false,
    }
}

struct HygienicCloner<'a, 'b> {
    src: &'a FirStore,
    dst: &'b mut FirStore,
    state: &'b mut FirHygienicCloneState,
    scopes: Vec<HashMap<String, String>>,
    /// `kFunArgs` name substitutions, each with the access class the
    /// substituted reference takes.
    ///
    /// Inlining a callee parameter into a caller temp gives
    /// `(temp, AccessType::Stack)`. Flattening a table-generator sub-module
    /// instead binds its `table` parameter to the caller's own table, which
    /// lives in `Static` or `Struct` storage — hence the access class travels
    /// with the name rather than being assumed.
    fun_arg_subst: HashMap<String, (String, AccessType)>,
    /// `kStruct` name substitutions, each with the access class the
    /// substituted reference takes.
    ///
    /// Used when a sub-module's own state is merged into the enclosing program
    /// (renamed struct fields) or demoted to locals of the initialization
    /// function (`StackLocals`).
    struct_subst: HashMap<String, (String, AccessType)>,
    local_renames: Vec<FirLocalRename>,
    preserve_local_names: bool,
    sweep_scaffolding_drop_roots: bool,
    clone_pairs: Vec<(FirId, FirId)>,
}

impl<'a, 'b> HygienicCloner<'a, 'b> {
    /// Creates a new cloner with empty lexical scopes and no parameter substitutions.
    fn new(src: &'a FirStore, dst: &'b mut FirStore, state: &'b mut FirHygienicCloneState) -> Self {
        Self {
            src,
            dst,
            state,
            scopes: Vec::new(),
            fun_arg_subst: HashMap::new(),
            struct_subst: HashMap::new(),
            local_renames: Vec::new(),
            preserve_local_names: false,
            sweep_scaffolding_drop_roots: false,
            clone_pairs: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pops the innermost lexical rename scope.
    fn pop_scope(&mut self) {
        let _ = self.scopes.pop();
    }

    /// Clones a subtree under one temporary lexical scope.
    fn clone_in_new_scope(&mut self, id: FirId) -> Result<FirId, FirHygienicCloneError> {
        self.push_scope();
        let out = self.clone_node(id);
        self.pop_scope();
        out
    }

    /// Looks up the innermost active rename for a stack/loop local.
    fn lookup_local_rename(&self, name: &str) -> Option<&str> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).map(String::as_str))
    }

    /// Returns `name` or its currently active local rename.
    fn maybe_renamed_unqualified(&self, name: &str) -> String {
        self.lookup_local_rename(name)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| name.to_string())
    }

    /// Applies access-sensitive symbol remapping for cloned references.
    fn remap_name_by_access(&self, name: &str, access: AccessType) -> String {
        if access == AccessType::FunArgs {
            return self
                .fun_arg_subst
                .get(name)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| name.to_string());
        }
        if access == AccessType::Struct
            && let Some((renamed, _)) = self.struct_subst.get(name)
        {
            return renamed.clone();
        }
        if matches!(access, AccessType::Stack | AccessType::Loop) {
            self.lookup_local_rename(name)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| name.to_string())
        } else {
            name.to_string()
        }
    }

    fn fresh_local_name(&mut self, original: &str) -> String {
        state_fresh_local_name(self.state, original)
    }

    /// Adjusts the access class when a `kFunArgs` reference is materialized to a stack temp.
    fn remap_access(&self, access: AccessType, name: &str) -> AccessType {
        match access {
            AccessType::FunArgs => self
                .fun_arg_subst
                .get(name)
                .map_or(access, |(_, target)| *target),
            AccessType::Struct => self
                .struct_subst
                .get(name)
                .map_or(access, |(_, target)| *target),
            _ => access,
        }
    }

    /// Binds one newly declared local name in the innermost scope.
    fn bind_local_decl(
        &mut self,
        origin_node: FirId,
        original: &str,
        access: AccessType,
        kind: FirLocalRenameKind,
    ) -> String {
        let existing = self
            .scopes
            .last()
            .and_then(|scope| scope.get(original))
            .cloned();
        if let Some(existing) = existing {
            return existing;
        }
        let renamed = if self.preserve_local_names {
            original.to_string()
        } else {
            self.fresh_local_name(original)
        };
        let Some(scope) = self.scopes.last_mut() else {
            return original.to_string();
        };
        scope.insert(original.to_string(), renamed.clone());
        if !self.preserve_local_names {
            self.local_renames.push(FirLocalRename {
                origin_node,
                original: original.to_string(),
                renamed: renamed.clone(),
                access,
                kind,
            });
        }
        renamed
    }

    /// Clones the *items* of a sub-module block and returns them as a list.
    ///
    /// `module`/`sub_module` take the items and intern the enclosing block
    /// themselves, so cloning the block node directly would nest one block
    /// inside another and silently break every consumer that expects
    /// `sub_modules` to decode as a block of `SubModule` nodes.
    fn clone_sub_modules(&mut self, block: FirId) -> Result<Vec<FirId>, FirHygienicCloneError> {
        let FirMatch::Block(items) = match_fir(self.src, block) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.clone_node(item)?);
        }
        Ok(out)
    }

    /// Clones one FIR node, recursively renaming locals and remapping parameters as needed.
    fn clone_node(&mut self, id: FirId) -> Result<FirId, FirHygienicCloneError> {
        let node = match_fir(self.src, id);
        let out = match node {
            FirMatch::Unknown => return Err(FirHygienicCloneError::UnknownNode(id)),
            FirMatch::Int32 { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.int32(value)
            }
            FirMatch::Int64 { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.int64(value)
            }
            FirMatch::Float32 { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.float32(value)
            }
            FirMatch::Float64 { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.float64(value)
            }
            FirMatch::Bool { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.bool_(value)
            }
            FirMatch::Quad { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.quad(value)
            }
            FirMatch::FixedPoint { value, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.fixed_point(value)
            }
            FirMatch::ValueArray { values, typ } => {
                let mut cloned = Vec::with_capacity(values.len());
                for v in values {
                    cloned.push(self.clone_node(v)?);
                }
                let mut b = FirBuilder::new(self.dst);
                b.value_array(&cloned, typ)
            }
            FirMatch::Int32Array { values, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.int32_array(&values)
            }
            FirMatch::Float32Array { values, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.float32_array(&values)
            }
            FirMatch::Float64Array { values, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.float64_array(&values)
            }
            FirMatch::QuadArray { values, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.quad_array(&values)
            }
            FirMatch::FixedPointArray { values, .. } => {
                let mut b = FirBuilder::new(self.dst);
                b.fixed_point_array(&values)
            }
            FirMatch::LoadVar { name, access, typ } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let mut b = FirBuilder::new(self.dst);
                b.load_var(remap, out_access, typ)
            }
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let index = self.clone_node(index)?;
                let mut b = FirBuilder::new(self.dst);
                b.load_table(remap, out_access, index, typ)
            }
            FirMatch::LoadVarAddress { name, access, typ } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let mut b = FirBuilder::new(self.dst);
                b.load_var_address(remap, out_access, typ)
            }
            FirMatch::TeeVar {
                name,
                access,
                value,
                typ,
            } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let value = self.clone_node(value)?;
                let mut b = FirBuilder::new(self.dst);
                b.tee_var(remap, out_access, value, typ)
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                let lhs = self.clone_node(lhs)?;
                let rhs = self.clone_node(rhs)?;
                let mut b = FirBuilder::new(self.dst);
                b.binop(op, lhs, rhs, typ)
            }
            FirMatch::Neg { value, typ } => {
                let value = self.clone_node(value)?;
                let mut b = FirBuilder::new(self.dst);
                b.neg(value, typ)
            }
            FirMatch::Cast { typ, value } => {
                let value = self.clone_node(value)?;
                let mut b = FirBuilder::new(self.dst);
                b.cast(typ, value)
            }
            FirMatch::Bitcast { typ, value } => {
                let value = self.clone_node(value)?;
                let mut b = FirBuilder::new(self.dst);
                b.bitcast(typ, value)
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                let cond = self.clone_node(cond)?;
                let then_value = self.clone_node(then_value)?;
                let else_value = self.clone_node(else_value)?;
                let mut b = FirBuilder::new(self.dst);
                b.select2(cond, then_value, else_value, typ)
            }
            FirMatch::FunCall { name, args, typ } => {
                let mut cloned_args = Vec::with_capacity(args.len());
                for a in args {
                    cloned_args.push(self.clone_node(a)?);
                }
                let mut b = FirBuilder::new(self.dst);
                b.fun_call(name, &cloned_args, typ)
            }
            FirMatch::NullValue { typ } => {
                let mut b = FirBuilder::new(self.dst);
                b.null_value(typ)
            }
            FirMatch::NewDsp { name, typ } => {
                let mut b = FirBuilder::new(self.dst);
                b.new_dsp(name, typ)
            }
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            } => {
                let init = match init {
                    Some(v) => Some(self.clone_node(v)?),
                    None => None,
                };
                let name = if matches!(access, AccessType::Stack | AccessType::Loop) {
                    self.bind_local_decl(id, &name, access, FirLocalRenameKind::DeclareVar)
                } else {
                    name
                };
                let mut b = FirBuilder::new(self.dst);
                b.declare_var(name, typ, access, init)
            }
            FirMatch::DeclareTable {
                name,
                access,
                elem_type,
                values,
            } => {
                let mut cloned_values = Vec::with_capacity(values.len());
                for v in values {
                    cloned_values.push(self.clone_node(v)?);
                }
                let name = if matches!(access, AccessType::Stack | AccessType::Loop) {
                    self.bind_local_decl(id, &name, access, FirLocalRenameKind::DeclareTable)
                } else {
                    name
                };
                let mut b = FirBuilder::new(self.dst);
                b.declare_table(name, access, elem_type, &cloned_values)
            }
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body,
                is_inline,
            } => {
                let body = match body {
                    Some(body_id) => Some(self.clone_in_new_scope(body_id)?),
                    None => None,
                };
                let mut b = FirBuilder::new(self.dst);
                b.declare_fun(name, typ, &args, body, is_inline)
            }
            FirMatch::DeclareStructType { typ } => {
                let mut b = FirBuilder::new(self.dst);
                b.declare_struct_type(typ)
            }
            FirMatch::DeclareBufferIterators {
                name1,
                name2,
                channels,
                typ,
                mutable,
                chunk,
            } => {
                let name1 = self.bind_local_decl(
                    id,
                    &name1,
                    AccessType::Loop,
                    FirLocalRenameKind::BufferIterator,
                );
                let name2 = self.bind_local_decl(
                    id,
                    &name2,
                    AccessType::Loop,
                    FirLocalRenameKind::BufferIterator,
                );
                let mut b = FirBuilder::new(self.dst);
                b.declare_buffer_iterators(name1, name2, channels, typ, mutable, chunk)
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let value = self.clone_node(value)?;
                let mut b = FirBuilder::new(self.dst);
                b.store_var(remap, out_access, value)
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let index = self.clone_node(index)?;
                let value = self.clone_node(value)?;
                let mut b = FirBuilder::new(self.dst);
                b.store_table(remap, out_access, index, value)
            }
            FirMatch::ShiftArrayVar {
                name,
                access,
                delay,
            } => {
                let out_access = self.remap_access(access, &name);
                let remap = self.remap_name_by_access(&name, access);
                let mut b = FirBuilder::new(self.dst);
                b.shift_array_var(remap, out_access, delay)
            }
            FirMatch::Drop(v) => {
                let v = self.clone_node(v)?;
                let mut b = FirBuilder::new(self.dst);
                b.drop_(v)
            }
            FirMatch::NullStatement => {
                let mut b = FirBuilder::new(self.dst);
                b.null_statement()
            }
            FirMatch::Return(v) => {
                let v = match v {
                    Some(v) => Some(self.clone_node(v)?),
                    None => None,
                };
                let mut b = FirBuilder::new(self.dst);
                b.ret(v)
            }
            FirMatch::Block(stmts) => {
                self.push_scope();
                let mut cloned = Vec::with_capacity(stmts.len());
                for s in stmts {
                    if self.sweep_scaffolding_drop_roots
                        && let FirMatch::Drop(value) = match_fir(self.src, s)
                        && is_obviously_side_effect_free_value(self.src, value)
                    {
                        continue;
                    }
                    cloned.push(self.clone_node(s)?);
                }
                self.pop_scope();
                let mut b = FirBuilder::new(self.dst);
                b.block(&cloned)
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let cond = self.clone_node(cond)?;
                let then_block = self.clone_in_new_scope(then_block)?;
                let else_block = match else_block {
                    Some(v) => Some(self.clone_in_new_scope(v)?),
                    None => None,
                };
                let mut b = FirBuilder::new(self.dst);
                b.if_(cond, then_block, else_block)
            }
            FirMatch::Control { cond, stmt } => {
                let cond = self.clone_node(cond)?;
                let stmt = self.clone_node(stmt)?;
                let mut b = FirBuilder::new(self.dst);
                b.control(cond, stmt)
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => {
                self.push_scope();
                let renamed_var =
                    self.bind_local_decl(id, &var, AccessType::Loop, FirLocalRenameKind::LoopVar);
                let init = self.clone_node(init)?;
                let end = self.clone_node(end)?;
                let step = self.clone_node(step)?;
                let body = self.clone_node(body)?;
                self.pop_scope();
                let mut b = FirBuilder::new(self.dst);
                b.for_loop(renamed_var, init, end, step, body, is_reverse)
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => {
                self.push_scope();
                let renamed_var =
                    self.bind_local_decl(id, &var, AccessType::Loop, FirLocalRenameKind::LoopVar);
                let upper = self.clone_node(upper)?;
                let body = self.clone_node(body)?;
                self.pop_scope();
                let mut b = FirBuilder::new(self.dst);
                b.simple_for_loop(renamed_var, upper, body, is_reverse)
            }
            FirMatch::IteratorForLoop {
                iterators,
                is_reverse,
                body,
            } => {
                self.push_scope();
                let mut renamed_iterators = Vec::with_capacity(iterators.len());
                for it in &iterators {
                    renamed_iterators.push(self.bind_local_decl(
                        id,
                        it,
                        AccessType::Loop,
                        FirLocalRenameKind::IteratorVar,
                    ));
                }
                let iter_refs: Vec<&str> = renamed_iterators.iter().map(String::as_str).collect();
                let body = self.clone_node(body)?;
                self.pop_scope();
                let mut b = FirBuilder::new(self.dst);
                b.iterator_for_loop(&iter_refs, is_reverse, body)
            }
            FirMatch::WhileLoop { cond, body } => {
                let cond = self.clone_node(cond)?;
                let body = self.clone_in_new_scope(body)?;
                let mut b = FirBuilder::new(self.dst);
                b.while_loop(cond, body)
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                let cond = self.clone_node(cond)?;
                let mut cloned_cases = Vec::with_capacity(cases.len());
                for (value, body) in cases {
                    cloned_cases.push((value, self.clone_in_new_scope(body)?));
                }
                let default = match default {
                    Some(v) => Some(self.clone_in_new_scope(v)?),
                    None => None,
                };
                let mut b = FirBuilder::new(self.dst);
                b.switch(cond, &cloned_cases, default)
            }
            FirMatch::OpenBox { typ, label } => {
                let mut b = FirBuilder::new(self.dst);
                b.open_box(typ, label)
            }
            FirMatch::CloseBox => {
                let mut b = FirBuilder::new(self.dst);
                b.close_box()
            }
            FirMatch::AddButton { typ, label, var } => {
                let var = self.maybe_renamed_unqualified(&var);
                let mut b = FirBuilder::new(self.dst);
                b.add_button(typ, label, var)
            }
            FirMatch::AddSlider {
                typ,
                label,
                var,
                init,
                lo,
                hi,
                step,
            } => {
                let var = self.maybe_renamed_unqualified(&var);
                let mut b = FirBuilder::new(self.dst);
                b.add_slider(typ, label, var, SliderRange { init, lo, hi, step })
            }
            FirMatch::AddBargraph {
                typ,
                label,
                var,
                lo,
                hi,
            } => {
                let var = self.maybe_renamed_unqualified(&var);
                let mut b = FirBuilder::new(self.dst);
                b.add_bargraph(typ, label, var, lo, hi)
            }
            FirMatch::AddSoundfile { label, url, var } => {
                let var = self.maybe_renamed_unqualified(&var);
                let mut b = FirBuilder::new(self.dst);
                b.add_soundfile_with_url(label, url, var)
            }
            FirMatch::LoadSoundfileLength { var, part } => {
                let var = self.maybe_renamed_unqualified(&var);
                let part = self.clone_node(part)?;
                let mut b = FirBuilder::new(self.dst);
                b.load_soundfile_length(var, part)
            }
            FirMatch::LoadSoundfileRate { var, part } => {
                let var = self.maybe_renamed_unqualified(&var);
                let part = self.clone_node(part)?;
                let mut b = FirBuilder::new(self.dst);
                b.load_soundfile_rate(var, part)
            }
            FirMatch::LoadSoundfileBuffer {
                var,
                chan,
                part,
                idx,
                typ,
            } => {
                let var = self.maybe_renamed_unqualified(&var);
                let chan = self.clone_node(chan)?;
                let part = self.clone_node(part)?;
                let idx = self.clone_node(idx)?;
                let mut b = FirBuilder::new(self.dst);
                b.load_soundfile_buffer(var, chan, part, idx, typ)
            }
            FirMatch::AddMetaDeclare { var, key, value } => {
                let var = self.maybe_renamed_unqualified(&var);
                let mut b = FirBuilder::new(self.dst);
                b.add_meta_declare(var, key, value)
            }
            FirMatch::Label(label) => {
                let mut b = FirBuilder::new(self.dst);
                b.label(label)
            }
            FirMatch::Module {
                num_inputs,
                num_outputs,
                name,
                dsp_struct,
                globals,
                functions,
                static_decls,
                sub_modules,
            } => {
                let dsp_struct = self.clone_node(dsp_struct)?;
                let globals = self.clone_node(globals)?;
                let functions = self.clone_node(functions)?;
                let static_decls = self.clone_node(static_decls)?;
                let sub_modules = self.clone_sub_modules(sub_modules)?;
                let mut b = FirBuilder::new(self.dst);
                b.module(
                    num_inputs,
                    num_outputs,
                    name,
                    dsp_struct,
                    globals,
                    functions,
                    static_decls,
                    &sub_modules,
                )
            }
            FirMatch::SubModule {
                name,
                elem_type,
                dsp_struct,
                static_decls,
                globals,
                functions,
                sub_modules,
            } => {
                let dsp_struct = self.clone_node(dsp_struct)?;
                let static_decls = self.clone_node(static_decls)?;
                let globals = self.clone_node(globals)?;
                let functions = self.clone_node(functions)?;
                let sub_modules = self.clone_sub_modules(sub_modules)?;
                let mut b = FirBuilder::new(self.dst);
                b.sub_module(
                    name,
                    elem_type,
                    dsp_struct,
                    static_decls,
                    globals,
                    functions,
                    &sub_modules,
                )
            }
        };
        self.clone_pairs.push((id, out));
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
