//! Translation layer: box expression → [`Schema`] tree.
//!
//! Entry points:
//! - [`generate_schema`] — non-folding path (flat single-file output).
//! - [`generate_folded_inside`] — folding path used by `draw_schema` when
//!   `config.fold_threshold > 0`.
//!
//! Mirrors the C++ `generateDiagramSchema` / `generateInsideSchema` pair in
//! `drawschema.cpp`.
//!
//! C++ reference: `compiler/draw/drawschema.cpp`.

use std::collections::{HashMap, HashSet, VecDeque};

use boxes::{BoxComplexity, BoxId, BoxMatch, FloatSize, box_pp_bounded, match_box};
use tlib::{NodeKind, TreeArena, tree_to_str};

use crate::DrawConfig;
use crate::schema::{COLOR_LINK, COLOR_NORMAL, COLOR_NUM, COLOR_SLOT, COLOR_UI, Schema};
use crate::schemas::{
    block::make_block,
    cable::{CableSchema, ConnectorSchema, CutSchema},
    composed::make_decorate,
    merge::make_merge,
    multirate::{make_downsampling, make_ondemand, make_upsampling},
    par::make_par,
    rec::make_rec,
    route::make_route,
    seq::make_seq,
    split::make_split,
};

// ─── Folding state ────────────────────────────────────────────────────────────

/// Mutable state threaded through the folding translation pass.
///
/// Mirrors the C++ globals `gPendingExp`, `gDrawnExp`, `gBackLink`,
/// `gSchemaFileName`, and `gFoldingFlag` from `drawschema.cpp`.
pub struct FoldState<'a> {
    /// `BoxId → definition name` populated by the evaluator.
    pub def_names: &'a HashMap<BoxId, String>,
    /// Queue of `(box_id, diagram_name, back_link_file)` waiting to be drawn.
    pub pending: &'a mut VecDeque<(BoxId, String, String)>,
    /// Set of already-scheduled/drawn box ids (prevents duplicate files).
    pub drawn: &'a mut HashSet<BoxId>,
    /// File name of the diagram currently being generated (for back-links).
    pub current_file: String,
    /// Whether folding is active for this session.
    pub folding: bool,
    /// Per-expression complexity threshold (`gFoldComplexity`).
    pub fold_complexity: usize,
    /// The file of every diagram scheduled so far, which a link names.
    pub files: &'a mut FileNames,
    /// What the drawing knows of the DAG: complexities, sharing, the names
    /// given to shared nodes, the schemas prepared for scheduled diagrams.
    pub memo: &'a mut DrawMemo,
}

/// The print budget (nodes and characters) of the expression that names a
/// shared node folded without a definition name.
///
/// C++ reference: `drawschema.cpp`, `kSharedNamePrintBudget`.
const SHARED_NAME_PRINT_BUDGET: usize = 1024;

/// What one drawing computes once about its DAG.
///
/// C++ keeps these as tree properties (`boxComplexity`, the definition names
/// it synthesizes) or in an `Occur` scoped to the drawing (#1290).
#[derive(Default)]
pub struct DrawMemo {
    complexity: BoxComplexity,
    /// Per node, the edges from its distinct parents (C++ `Occur`).
    parents: HashMap<BoxId, usize>,
    /// The expression that names a folded shared node without a definition.
    synthesized: HashMap<BoxId, String>,
    /// The inside schema of a scheduled diagram, built when it was scheduled
    /// (to know its arity) and drawn when its file is written.
    prepared: HashMap<BoxId, Box<dyn Schema>>,
    /// The inputs and outputs of every scheduled diagram.
    arity: HashMap<BoxId, (usize, usize)>,
}

impl DrawMemo {
    /// The memo of a drawing of `root`: its parent counts, from one walk over
    /// the DAG with a visited set, every child of every node counted once
    /// per distinct parent, the root not counted.
    ///
    /// C++ reference: `occur.cpp`, `Occur::countOccurrences` (#1290).
    #[must_use]
    pub fn new(arena: &TreeArena, root: BoxId) -> Self {
        let mut parents: HashMap<BoxId, usize> = HashMap::new();
        let mut visited = HashSet::from([root]);
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            for &child in arena.children(node).unwrap_or(&[]) {
                *parents.entry(child).or_insert(0) += 1;
                if visited.insert(child) {
                    stack.push(child);
                }
            }
        }
        Self {
            parents,
            ..Self::default()
        }
    }

    /// The complexity of `b`, each distinct box scored once per drawing.
    pub fn complexity(&mut self, arena: &TreeArena, b: BoxId) -> usize {
        self.complexity.of(arena, b)
    }

    /// The schema prepared for `b` when it was scheduled, if any.
    pub fn take_prepared(&mut self, b: BoxId) -> Option<Box<dyn Schema>> {
        self.prepared.remove(&b)
    }

    /// Whether `b` has more than one parent in the DAG.
    fn is_shared(&self, b: BoxId) -> bool {
        self.parents.get(&b).is_some_and(|&n| n > 1)
    }
}

/// How the files of a drawing are named.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileNaming {
    /// C++ `legalFileName`: up to 16 alphanumerics of the name and the box's
    /// identity in hex (`decoder-000034e8.svg`; `process.svg` for a root named
    /// `process`). Unique, but the suffix is a tree index, which moves
    /// whenever anything evaluated before the box changes, a library edit
    /// included (C++ uses the tree's address, which moves on every run).
    #[default]
    BoxIdentity,
    /// The name alone (`feedbackNetwork.svg`), then `feedbackNetwork-2.svg`
    /// for the second distinct diagram of that name, and so on in drawing
    /// order. Names are compared without case, so two files never differ by
    /// case alone on a case-insensitive file system. The same program gives
    /// the same files, whatever was compiled before it.
    DefinitionName,
}

/// The file of each diagram of one drawing, assigned when the diagram is
/// first scheduled, so that a link and the file it points to agree.
#[derive(Debug, Default)]
pub struct FileNames {
    naming: FileNaming,
    files: HashMap<BoxId, String>,
    /// Per stem, lowercased: how many diagrams carry it.
    stems: HashMap<String, usize>,
}

impl FileNames {
    /// An empty assignment under `naming`.
    #[must_use]
    pub fn new(naming: FileNaming) -> Self {
        Self {
            naming,
            ..Self::default()
        }
    }

    /// The file of diagram `b`, named `name`: the one it was given, or a new
    /// one.
    pub fn assign(&mut self, b: BoxId, name: &str) -> String {
        self.assign_named(b, name, false)
    }

    /// The file of diagram `b`, named by an expression rather than a
    /// definition: its stem is `diagram`, as an expression is no file name.
    pub fn assign_expression(&mut self, b: BoxId) -> String {
        self.assign_named(b, "diagram", true)
    }

    fn assign_named(&mut self, b: BoxId, name: &str, expression: bool) -> String {
        if let Some(file) = self.files.get(&b) {
            return file.clone();
        }
        let file = match self.naming {
            FileNaming::BoxIdentity if expression => format!("diagram-{:08x}.svg", b.as_u32()),
            FileNaming::BoxIdentity => legal_file_name(name, b),
            FileNaming::DefinitionName => {
                let stem = definition_stem(name);
                // a stem has no `-`, so a suffixed name never meets a plain one
                let count = self.stems.entry(stem.to_ascii_lowercase()).or_insert(0);
                *count += 1;
                if *count == 1 {
                    format!("{stem}.svg")
                } else {
                    format!("{stem}-{count}.svg")
                }
            }
        };
        self.files.insert(b, file.clone());
        file
    }
}

/// The stem of [`FileNaming::DefinitionName`]: the name's ASCII alphanumerics
/// and underscores, at most 64, or `diagram` when none is left.
fn definition_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(64)
        .collect();
    if stem.is_empty() {
        "diagram".to_owned()
    } else {
        stem
    }
}

// ─── Top-level entries ─────────────────────────────────────────────────────────

/// Flat (non-folding) path: convert `b` into a schema tree inline.
///
/// C++ reference: `drawschema.cpp:359` — `generateDiagramSchema` (non-fold path).
pub fn generate_schema(arena: &TreeArena, b: BoxId, config: &DrawConfig) -> Box<dyn Schema> {
    generate_inside(arena, b, config)
}

/// Folding path: generate the *inside* of diagram `b`, scheduling any
/// sufficiently complex named sub-diagrams as separate files.
///
/// Called once per queued diagram in `draw_schema`'s main loop.
/// C++ reference: `drawschema.cpp:234` — `writeSchemaFile` (calls `generateInsideSchema`).
pub fn generate_folded_inside(
    arena: &TreeArena,
    b: BoxId,
    config: &DrawConfig,
    state: &mut FoldState<'_>,
) -> Box<dyn Schema> {
    generate_inside_folded(arena, b, config, state)
}

/// Decide whether sub-diagram `b` should be folded (separate file) or inlined.
///
/// Folded when folding is on, `b` is complex enough, and it is named or
/// shared (more than one parent in the DAG): a shared node expanded at every
/// occurrence makes the drawing grow with the expanded tree, exponentially
/// for `ma.chebychev(n)` (C++ #1290). An occurrence of a diagram already
/// scheduled is a link to its file, as in C++; it is never expanded again.
///
/// C++ reference: `drawschema.cpp:359` — `generateDiagramSchema`.
fn generate_diagram_schema(
    arena: &TreeArena,
    b: BoxId,
    config: &DrawConfig,
    state: &mut FoldState<'_>,
) -> Box<dyn Schema> {
    let def_name = state.def_names.get(&b);
    if state.folding
        && (def_name.is_some() || state.memo.is_shared(b))
        && state.memo.complexity(arena, b) >= state.fold_complexity
    {
        let (name, link) = match def_name {
            Some(name) => (name.clone(), state.files.assign(b, name)),
            None => {
                let name = state
                    .memo
                    .synthesized
                    .entry(b)
                    .or_insert_with(|| expression_name(arena, b))
                    .clone();
                (name, state.files.assign_expression(b))
            }
        };
        if state.drawn.insert(b) {
            let back = state.current_file.clone();
            state.pending.push_back((b, name.clone(), back));
            // Built now for its arity, with b's own file as the current one so
            // that its sub-diagrams link back to it, and drawn with b's file.
            let enclosing = std::mem::replace(&mut state.current_file, link.clone());
            let inside = generate_inside_folded(arena, b, config, state);
            state.current_file = enclosing;
            state
                .memo
                .arity
                .insert(b, (inside.inputs(), inside.outputs()));
            state.memo.prepared.insert(b, inside);
        }
        let (inputs, outputs) = match state.memo.arity.get(&b) {
            Some(&arity) => arity,
            // the root, which no sub-diagram contains
            None => {
                let inside = generate_inside(arena, b, config);
                (inside.inputs(), inside.outputs())
            }
        };
        let label = truncate_name(name, config.max_name_size);
        return make_block(inputs, outputs, label, COLOR_LINK, link);
    }

    // Decoration: named sub-diagram that isn't pure routing gets a dashed border.
    if let Some(def_name) = def_name
        && !is_pure_routing(arena, b)
    {
        let label = truncate_name(def_name.clone(), config.max_name_size);
        return make_decorate(generate_inside_folded(arena, b, config, state), 10.0, label);
    }

    generate_inside_folded(arena, b, config, state)
}

/// The name of a folded shared node without a definition name: its own
/// expression, printed within a budget (C++ `mBoxName`).
fn expression_name(arena: &TreeArena, b: BoxId) -> String {
    box_pp_bounded(arena, b, 0, FloatSize::Single, SHARED_NAME_PRINT_BUDGET)
        .unwrap_or_else(|_| "diagram".to_owned())
}

// ─── Inside dispatch ─────────────────────────────────────────────────────────

/// Folding variant of [`generate_inside`]: composition children go through
/// [`generate_diagram_schema`] so they can be folded into separate files.
///
/// C++ reference: `drawschema.cpp:396` — `generateInsideSchema` (folding path).
fn generate_inside_folded(
    arena: &TreeArena,
    b: BoxId,
    config: &DrawConfig,
    state: &mut FoldState<'_>,
) -> Box<dyn Schema> {
    let max = config.max_name_size;
    let tn = |s: String| truncate_name(s, max);

    // Leaf nodes are identical to the non-folding path.
    // Composition nodes recurse through generate_diagram_schema.
    match match_box(arena, b) {
        // ── Compositions: recurse through diagram dispatcher ──────────
        BoxMatch::Seq(a, c) => make_seq(
            generate_diagram_schema(arena, a, config, state),
            generate_diagram_schema(arena, c, config, state),
        ),
        BoxMatch::Par(a, c) => make_par(
            generate_diagram_schema(arena, a, config, state),
            generate_diagram_schema(arena, c, config, state),
        ),
        BoxMatch::Split(a, c) => make_split(
            generate_diagram_schema(arena, a, config, state),
            generate_diagram_schema(arena, c, config, state),
        ),
        BoxMatch::Merge(a, c) => make_merge(
            generate_diagram_schema(arena, a, config, state),
            generate_diagram_schema(arena, c, config, state),
        ),
        BoxMatch::Rec(a, c) => make_rec(
            generate_diagram_schema(arena, a, config, state),
            generate_diagram_schema(arena, c, config, state),
        ),
        // ── Groups ────────────────────────────────────────────────────
        BoxMatch::VGroup(label, body) => {
            let name = tn(format!("vgroup({})", extract_name(arena, label)));
            make_decorate(
                generate_diagram_schema(arena, body, config, state),
                10.0,
                name,
            )
        }
        BoxMatch::HGroup(label, body) => {
            let name = tn(format!("hgroup({})", extract_name(arena, label)));
            make_decorate(
                generate_diagram_schema(arena, body, config, state),
                10.0,
                name,
            )
        }
        BoxMatch::TGroup(label, body) => {
            let name = tn(format!("tgroup({})", extract_name(arena, label)));
            make_decorate(
                generate_diagram_schema(arena, body, config, state),
                10.0,
                name,
            )
        }
        // ── Metadata ─────────────────────────────────────────────────
        BoxMatch::Metadata(a, _) => generate_diagram_schema(arena, a, config, state),
        // ── Multi-rate wrappers ───────────────────────────────────────
        BoxMatch::Ondemand(inner) => {
            make_ondemand(generate_inside_folded(arena, inner, config, state))
        }
        BoxMatch::Upsampling(inner) => {
            make_upsampling(generate_inside_folded(arena, inner, config, state))
        }
        BoxMatch::Downsampling(inner) => {
            make_downsampling(generate_inside_folded(arena, inner, config, state))
        }
        // ── Symbolic abstraction ──────────────────────────────────────
        BoxMatch::Symbolic(a, body) => {
            let slot = generate_slot_schema(arena, a);
            generate_abstraction_folded(arena, slot, body, config, state)
        }
        // ── AD wrappers ───────────────────────────────────────────────
        BoxMatch::ForwardAD(expr, seed) => {
            let inner = generate_diagram_schema(arena, expr, config, state);
            let seed_outs = generate_inside(arena, seed, config).outputs().max(1);
            make_decorate(
                fad_augment_tangents(inner, seed_outs),
                10.0,
                "fad".to_owned(),
            )
        }
        BoxMatch::ReverseAD(expr, seeds) => {
            let inner = generate_diagram_schema(arena, expr, config, state);
            let seeds_outs = generate_inside(arena, seeds, config).outputs().max(1);
            make_decorate(
                rad_augment_gradients(inner, seeds_outs),
                10.0,
                "rad".to_owned(),
            )
        }
        // ── Everything else: delegate to flat generate_inside ─────────
        _ => generate_inside(arena, b, config),
    }
}

/// Extend an inner schema with zero-input output stubs for AD wrappers.
///
/// Used to reconcile the draw-level schema arity with the compiled arity:
///
/// - **ForwardAD** (`fad(expr, seed)`):
///   `outputs = expr.outputs × (1 + seed.outputs)` — call with
///   `n_extra = inner.outputs() * seed_outs`, `label = "∂"`.
///
/// - **ReverseAD** (`rad(expr, seeds)`):
///   `outputs = expr.outputs + seeds.outputs` — call with
///   `n_extra = seeds_outs`, `label = "∇"`.
///
/// If `n_extra == 0` the inner schema is returned unchanged.
fn ad_augment_extra_outputs(
    inner: Box<dyn Schema>,
    n_extra: usize,
    label: &str,
) -> Box<dyn Schema> {
    use crate::schemas::par::make_par;
    if n_extra == 0 {
        return inner;
    }
    make_par(inner, make_block(0, n_extra, label, COLOR_SLOT, ""))
}

/// Convenience wrapper for `ForwardAD`: appends `inner.outputs × seed_outs`
/// tangent-lane stubs labelled "∂".
fn fad_augment_tangents(inner: Box<dyn Schema>, seed_outs: usize) -> Box<dyn Schema> {
    let n_tangent = inner.outputs() * seed_outs;
    ad_augment_extra_outputs(inner, n_tangent, "\u{2202}")
}

/// Convenience wrapper for `ReverseAD`: appends `seeds_outs` gradient-lane
/// stubs labelled "∇".
fn rad_augment_gradients(inner: Box<dyn Schema>, seeds_outs: usize) -> Box<dyn Schema> {
    ad_augment_extra_outputs(inner, seeds_outs, "\u{2207}")
}

/// Non-folding map of one `BoxId` to a schema.
///
/// C++ reference: `drawschema.cpp:396` — `generateInsideSchema`.
fn generate_inside(arena: &TreeArena, b: BoxId, config: &DrawConfig) -> Box<dyn Schema> {
    let max = config.max_name_size;
    // Short-circuit for purely static short labels — helper for dynamic names.
    let tn = |s: String| truncate_name(s, max);

    match match_box(arena, b) {
        // ── Leaf values ──────────────────────────────────────────────
        BoxMatch::Int(i) => make_block(0, 1, i.to_string(), COLOR_NUM, ""),
        BoxMatch::Real(r) => make_block(0, 1, format_real(r), COLOR_NUM, ""),
        BoxMatch::Waveform(_) => make_block(0, 2, "waveform{...}", COLOR_NORMAL, ""),
        BoxMatch::Wire => Box::new(CableSchema::new(1)),
        BoxMatch::Cut => Box::new(CutSchema::new()),

        // ── Binary arithmetic primitives (2 → 1) ─────────────────────
        BoxMatch::Add => make_block(2, 1, "+", COLOR_NORMAL, ""),
        BoxMatch::Sub => make_block(2, 1, "-", COLOR_NORMAL, ""),
        BoxMatch::Mul => make_block(2, 1, "*", COLOR_NORMAL, ""),
        BoxMatch::Div => make_block(2, 1, "/", COLOR_NORMAL, ""),
        BoxMatch::Rem => make_block(2, 1, "%", COLOR_NORMAL, ""),
        BoxMatch::And => make_block(2, 1, "&", COLOR_NORMAL, ""),
        BoxMatch::Or => make_block(2, 1, "|", COLOR_NORMAL, ""),
        BoxMatch::Xor => make_block(2, 1, "xor", COLOR_NORMAL, ""),
        BoxMatch::Lsh => make_block(2, 1, "<<", COLOR_NORMAL, ""),
        BoxMatch::Rsh => make_block(2, 1, ">>", COLOR_NORMAL, ""),
        BoxMatch::Lt => make_block(2, 1, "<", COLOR_NORMAL, ""),
        BoxMatch::Le => make_block(2, 1, "<=", COLOR_NORMAL, ""),
        BoxMatch::Gt => make_block(2, 1, ">", COLOR_NORMAL, ""),
        BoxMatch::Ge => make_block(2, 1, ">=", COLOR_NORMAL, ""),
        BoxMatch::Eq => make_block(2, 1, "==", COLOR_NORMAL, ""),
        BoxMatch::Ne => make_block(2, 1, "!=", COLOR_NORMAL, ""),
        BoxMatch::Pow => make_block(2, 1, "pow", COLOR_NORMAL, ""),
        BoxMatch::Atan2 => make_block(2, 1, "atan2", COLOR_NORMAL, ""),
        BoxMatch::Fmod => make_block(2, 1, "fmod", COLOR_NORMAL, ""),
        BoxMatch::Remainder => make_block(2, 1, "remainder", COLOR_NORMAL, ""),
        BoxMatch::Min => make_block(2, 1, "min", COLOR_NORMAL, ""),
        BoxMatch::Max => make_block(2, 1, "max", COLOR_NORMAL, ""),
        BoxMatch::Delay => make_block(2, 1, "@", COLOR_NORMAL, ""),

        // ── Unary math primitives (1 → 1) ────────────────────────────
        BoxMatch::Acos => make_block(1, 1, "acos", COLOR_NORMAL, ""),
        BoxMatch::Asin => make_block(1, 1, "asin", COLOR_NORMAL, ""),
        BoxMatch::Atan => make_block(1, 1, "atan", COLOR_NORMAL, ""),
        BoxMatch::Cos => make_block(1, 1, "cos", COLOR_NORMAL, ""),
        BoxMatch::Sin => make_block(1, 1, "sin", COLOR_NORMAL, ""),
        BoxMatch::Tan => make_block(1, 1, "tan", COLOR_NORMAL, ""),
        BoxMatch::Exp => make_block(1, 1, "exp", COLOR_NORMAL, ""),
        BoxMatch::Exp10 => make_block(1, 1, "exp10", COLOR_NORMAL, ""),
        BoxMatch::Log => make_block(1, 1, "log", COLOR_NORMAL, ""),
        BoxMatch::Log10 => make_block(1, 1, "log10", COLOR_NORMAL, ""),
        BoxMatch::Sqrt => make_block(1, 1, "sqrt", COLOR_NORMAL, ""),
        BoxMatch::Abs => make_block(1, 1, "abs", COLOR_NORMAL, ""),
        BoxMatch::Floor => make_block(1, 1, "floor", COLOR_NORMAL, ""),
        BoxMatch::Ceil => make_block(1, 1, "ceil", COLOR_NORMAL, ""),
        BoxMatch::Rint => make_block(1, 1, "rint", COLOR_NORMAL, ""),
        BoxMatch::Round => make_block(1, 1, "round", COLOR_NORMAL, ""),
        BoxMatch::IntCast => make_block(1, 1, "int", COLOR_NORMAL, ""),
        BoxMatch::FloatCast => make_block(1, 1, "float", COLOR_NORMAL, ""),
        BoxMatch::Delay1 => make_block(1, 1, "mem", COLOR_NORMAL, ""),
        BoxMatch::Prefix => make_block(2, 1, "prefix", COLOR_NORMAL, ""),

        // ── Select (1-based control + n data inputs) ─────────────────
        BoxMatch::Select2 => make_block(3, 1, "select2", COLOR_NORMAL, ""),
        BoxMatch::Select3 => make_block(4, 1, "select3", COLOR_NORMAL, ""),

        // ── Tables ───────────────────────────────────────────────────
        BoxMatch::ReadOnlyTable => make_block(3, 1, "rdtable", COLOR_NORMAL, ""),
        BoxMatch::WriteReadTable => make_block(5, 1, "rwtable", COLOR_NORMAL, ""),

        // ── Misc primitives ──────────────────────────────────────────
        BoxMatch::AssertBounds => make_block(3, 1, "assertbounds", COLOR_NORMAL, ""),
        BoxMatch::Lowest => make_block(1, 1, "lowest", COLOR_NORMAL, ""),
        BoxMatch::Highest => make_block(1, 1, "highest", COLOR_NORMAL, ""),
        BoxMatch::Attach => make_block(2, 2, "attach", COLOR_NORMAL, ""),
        BoxMatch::Enable => make_block(2, 2, "enable", COLOR_NORMAL, ""),
        BoxMatch::Control => make_block(2, 2, "control", COLOR_NORMAL, ""),

        // ── Composition operators ─────────────────────────────────────
        BoxMatch::Seq(a, b) => make_seq(
            generate_inside(arena, a, config),
            generate_inside(arena, b, config),
        ),
        BoxMatch::Par(a, b) => make_par(
            generate_inside(arena, a, config),
            generate_inside(arena, b, config),
        ),
        BoxMatch::Split(a, b) => make_split(
            generate_inside(arena, a, config),
            generate_inside(arena, b, config),
        ),
        BoxMatch::Merge(a, b) => make_merge(
            generate_inside(arena, a, config),
            generate_inside(arena, b, config),
        ),
        BoxMatch::Rec(a, b) => make_rec(
            generate_inside(arena, a, config),
            generate_inside(arena, b, config),
        ),

        // ── Metadata: transparent pass-through ───────────────────────
        BoxMatch::Metadata(a, _b) => generate_inside(arena, a, config),

        // ── Groups: decorate with a labeled dashed border ────────────
        BoxMatch::VGroup(label, body) => {
            let name = tn(format!("vgroup({})", extract_name(arena, label)));
            make_decorate(generate_schema(arena, body, config), 10.0, name)
        }
        BoxMatch::HGroup(label, body) => {
            let name = tn(format!("hgroup({})", extract_name(arena, label)));
            make_decorate(generate_schema(arena, body, config), 10.0, name)
        }
        BoxMatch::TGroup(label, body) => {
            let name = tn(format!("tgroup({})", extract_name(arena, label)));
            make_decorate(generate_schema(arena, body, config), 10.0, name)
        }

        // ── UI elements ───────────────────────────────────────────────
        BoxMatch::Button(label) => {
            let s = tn(format!("button({})", extract_name(arena, label)));
            make_block(0, 1, s, COLOR_UI, "")
        }
        BoxMatch::Checkbox(label) => {
            let s = tn(format!("checkbox({})", extract_name(arena, label)));
            make_block(0, 1, s, COLOR_UI, "")
        }
        BoxMatch::VSlider(label, cur, min, max_id, step) => {
            let s = tn(format!(
                "vslider({}, {}, {}, {}, {})",
                extract_name(arena, label),
                format_node(arena, cur),
                format_node(arena, min),
                format_node(arena, max_id),
                format_node(arena, step),
            ));
            make_block(0, 1, s, COLOR_UI, "")
        }
        BoxMatch::HSlider(label, cur, min, max_id, step) => {
            let s = tn(format!(
                "hslider({}, {}, {}, {}, {})",
                extract_name(arena, label),
                format_node(arena, cur),
                format_node(arena, min),
                format_node(arena, max_id),
                format_node(arena, step),
            ));
            make_block(0, 1, s, COLOR_UI, "")
        }
        BoxMatch::NumEntry(label, cur, min, max_id, step) => {
            let s = tn(format!(
                "nentry({}, {}, {}, {}, {})",
                extract_name(arena, label),
                format_node(arena, cur),
                format_node(arena, min),
                format_node(arena, max_id),
                format_node(arena, step),
            ));
            make_block(0, 1, s, COLOR_UI, "")
        }
        BoxMatch::HBargraph(label, min, max_id) => {
            let s = tn(format!(
                "hbargraph({}, {}, {})",
                extract_name(arena, label),
                format_node(arena, min),
                format_node(arena, max_id),
            ));
            make_block(1, 1, s, COLOR_UI, "")
        }
        BoxMatch::VBargraph(label, min, max_id) => {
            let s = tn(format!(
                "vbargraph({}, {}, {})",
                extract_name(arena, label),
                format_node(arena, min),
                format_node(arena, max_id),
            ));
            make_block(1, 1, s, COLOR_UI, "")
        }
        BoxMatch::Soundfile(label, chan) => {
            let n = extract_int(arena, chan).unwrap_or(1) as usize;
            let s = tn(format!("soundfile({}, {})", extract_name(arena, label), n));
            make_block(2, 2 + n, s, COLOR_UI, "")
        }

        // ── Route ─────────────────────────────────────────────────────
        BoxMatch::Route(a, b, c) => {
            let ins = extract_int(arena, a).unwrap_or(0).max(0) as usize;
            let outs = extract_int(arena, b).unwrap_or(0).max(0) as usize;
            let routes = flatten_int_tree(arena, c);
            make_route(ins, outs, routes, config.draw_route_frame)
        }

        // ── Multi-rate wrappers ───────────────────────────────────────
        BoxMatch::Ondemand(inner) => make_ondemand(generate_inside(arena, inner, config)),
        BoxMatch::Upsampling(inner) => make_upsampling(generate_inside(arena, inner, config)),
        BoxMatch::Downsampling(inner) => make_downsampling(generate_inside(arena, inner, config)),

        // ── Slots (lambda variable placeholders) ─────────────────────
        BoxMatch::Slot(i) => {
            let name = format!("[{i}]");
            make_block(0, 1, name, COLOR_SLOT, "")
        }

        // ── Abstraction / Symbolic ────────────────────────────────────
        BoxMatch::Symbolic(a, b) => {
            let slot = generate_slot_schema(arena, a);
            generate_abstraction(arena, slot, b, config)
        }

        // ── FFI / foreign items ───────────────────────────────────────
        BoxMatch::FFun(ff) => {
            let name = tn(extract_name(arena, ff));
            make_block(1, 1, name, COLOR_NORMAL, "")
        }
        BoxMatch::Ffunction(ff, _typ, _file) => {
            let name = tn(extract_name(arena, ff));
            make_block(1, 1, name, COLOR_NORMAL, "")
        }
        BoxMatch::FConst(_typ, name, _file) => {
            let s = tn(extract_name(arena, name));
            make_block(0, 1, s, COLOR_NORMAL, "")
        }
        BoxMatch::FVar(_typ, name, _file) => {
            let s = tn(extract_name(arena, name));
            make_block(0, 1, s, COLOR_NORMAL, "")
        }

        // ── Named identifier ─────────────────────────────────────────
        BoxMatch::Ident(s) => make_block(0, 1, tn(s.to_owned()), COLOR_LINK, ""),

        // ── Environment ──────────────────────────────────────────────
        BoxMatch::Environment => make_block(0, 0, "environment{...}", COLOR_NORMAL, ""),

        // ── Inverter (*(-1)) shorthand ────────────────────────────────
        // Note: isInverter check in C++ looks at gGlobal->gInverter table.
        // Without that table we can't detect inverters; they fall through to Mul.
        // The InverterSchema can be produced by the caller if needed.

        // ── AD wrappers ───────────────────────────────────────────────
        BoxMatch::ForwardAD(expr, seed) => {
            let inner = generate_inside(arena, expr, config);
            let seed_outs = generate_inside(arena, seed, config).outputs().max(1);
            make_decorate(
                fad_augment_tangents(inner, seed_outs),
                10.0,
                "fad".to_owned(),
            )
        }
        BoxMatch::ReverseAD(expr, seeds) => {
            let inner = generate_inside(arena, expr, config);
            let seeds_outs = generate_inside(arena, seeds, config).outputs().max(1);
            make_decorate(
                rad_augment_gradients(inner, seeds_outs),
                10.0,
                "rad".to_owned(),
            )
        }

        // ── Anything else: placeholder block ─────────────────────────
        _ => make_block(1, 1, "?", COLOR_NORMAL, ""),
    }
}

// ─── Abstraction helper ──────────────────────────────────────────────────────

/// Build an abstraction schema by placing input slots before the body.
///
/// C++ reference: `drawschema.cpp:654` — `generateAbstractionSchema`.
fn generate_abstraction(
    arena: &TreeArena,
    mut x: Box<dyn Schema>,
    body: BoxId,
    config: &DrawConfig,
) -> Box<dyn Schema> {
    let mut t = body;
    while let BoxMatch::Symbolic(a, b) = match_box(arena, t) {
        let slot = generate_slot_schema(arena, a);
        x = make_par(x, slot);
        t = b;
    }
    make_seq(x, generate_inside(arena, t, config))
}

/// Folding variant of [`generate_abstraction`].
fn generate_abstraction_folded(
    arena: &TreeArena,
    mut x: Box<dyn Schema>,
    body: BoxId,
    config: &DrawConfig,
    state: &mut FoldState<'_>,
) -> Box<dyn Schema> {
    let mut t = body;
    while let BoxMatch::Symbolic(a, b) = match_box(arena, t) {
        let slot = generate_slot_schema(arena, a);
        x = make_par(x, slot);
        t = b;
    }
    make_seq(x, generate_diagram_schema(arena, t, config, state))
}

/// Build a 1→0 input-slot block schema.
///
/// C++ reference: `drawschema.cpp:627` — `generateInputSlotSchema`.
fn generate_slot_schema(arena: &TreeArena, slot_id: BoxId) -> Box<dyn Schema> {
    let name = match match_box(arena, slot_id) {
        BoxMatch::Slot(i) => format!("[{i}]"),
        _ => "slot".to_owned(),
    };
    make_block(1, 0, name, COLOR_SLOT, "")
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Return a legal SVG file name for a named sub-diagram.
///
/// Keeps up to 16 alphanumeric characters from `name`.  Unless the result is
/// `"process"` (the root), appends `-<box_id_hex>` to guarantee uniqueness.
///
/// C++ reference: `drawschema.cpp:291` — `legalFileName`.
pub fn legal_file_name(name: &str, box_id: BoxId) -> String {
    let mut stem: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    // a file name starting with `-` reads as an option, one starting with a
    // digit is no name (C++ `legalFileName`, since `068fb9a80`)
    if !stem.starts_with(|c: char| c.is_ascii_alphabetic()) {
        stem = "diagram".to_owned();
    }
    if stem == "process" {
        "process.svg".to_owned()
    } else {
        format!("{stem}-{:08x}.svg", box_id.as_u32())
    }
}

/// Returns `true` if the diagram contains only routing (no logic/UI boxes).
///
/// Pure-routing sub-diagrams are not decorated with a named border even when
/// they have a definition name, matching C++ `isPureRouting`.
///
/// C++ reference: `drawschema.cpp:332` — `isPureRouting`.
fn is_pure_routing(arena: &TreeArena, b: BoxId) -> bool {
    match match_box(arena, b) {
        BoxMatch::Cut | BoxMatch::Wire | BoxMatch::Slot(_) => true,
        BoxMatch::Seq(a, c)
        | BoxMatch::Par(a, c)
        | BoxMatch::Split(a, c)
        | BoxMatch::Merge(a, c) => is_pure_routing(arena, a) && is_pure_routing(arena, c),
        BoxMatch::Metadata(a, _) => is_pure_routing(arena, a),
        _ => false,
    }
}

/// Truncate a display name to at most `max` chars: keep first and last thirds.
///
/// C++ reference: `blockSchema.cpp` — constructor truncation with `gMaxNameSize`.
fn truncate_name(s: String, max: usize) -> String {
    if max == 0 || s.len() <= max {
        return s;
    }
    let third = (max / 3).max(1);
    let head = s.get(..third).unwrap_or(&s[..s.len().min(third)]);
    let tail_start = s.len().saturating_sub(third);
    let tail = &s[tail_start..];
    format!("{head}...{tail}")
}

/// Format a real number the same way C++ `boxpp` does: trim trailing zeros.
fn format_real(r: f64) -> String {
    if r.fract() == 0.0 && r.abs() < 1e15 {
        format!("{r:.1}")
    } else {
        format!("{r}")
    }
}

/// Extract a single integer value from a box node (for route dimensions).
fn extract_int(arena: &TreeArena, b: BoxId) -> Option<i32> {
    match match_box(arena, b) {
        BoxMatch::Int(i) => Some(i),
        BoxMatch::Real(r) => Some(r as i32),
        _ => None,
    }
}

/// Flatten a parallel tree of integers into a `Vec<usize>` (for route tables).
///
/// C++ reference: `drawschema.cpp:172` — `isIntTree`.
fn flatten_int_tree(arena: &TreeArena, b: BoxId) -> Vec<usize> {
    let mut out = Vec::new();
    flatten_int_tree_inner(arena, b, &mut out);
    out
}

fn flatten_int_tree_inner(arena: &TreeArena, b: BoxId, out: &mut Vec<usize>) {
    match match_box(arena, b) {
        BoxMatch::Int(i) => out.push(i.max(0) as usize),
        BoxMatch::Real(r) => out.push(r.max(0.0) as usize),
        BoxMatch::Par(x, y) => {
            flatten_int_tree_inner(arena, x, out);
            flatten_int_tree_inner(arena, y, out);
        }
        _ => {}
    }
}

/// Extract a display name from a label / identifier node.
fn extract_name(arena: &TreeArena, b: BoxId) -> String {
    // tree_to_str handles Symbol nodes; check StringLiteral separately
    match arena.kind(b) {
        Some(NodeKind::Symbol(s)) | Some(NodeKind::StringLiteral(s)) => return s.to_string(),
        _ => {}
    }
    if let Some(s) = tree_to_str(arena, b) {
        return s.to_owned();
    }
    match match_box(arena, b) {
        BoxMatch::Ident(s) => s.to_owned(),
        BoxMatch::Int(i) => i.to_string(),
        BoxMatch::Real(r) => format_real(r),
        _ => "?".to_owned(),
    }
}

/// Format a numeric node for UI parameter display.
fn format_node(arena: &TreeArena, b: BoxId) -> String {
    match match_box(arena, b) {
        BoxMatch::Int(i) => i.to_string(),
        BoxMatch::Real(r) => format_real(r),
        _ => extract_name(arena, b),
    }
}

// ─── Public helpers ───────────────────────────────────────────────────────────

/// Build a `TopSchema` wrapper around a generated schema.
///
/// The inner schema is padded with connector stubs for each input/output,
/// then wrapped in a `TopSchema` (white background, title, output arrows).
///
/// C++ reference: `drawschema.cpp:261` — `writeSchemaFile` inner lambda.
pub fn make_top_schema(
    inner: Box<dyn Schema>,
    name: impl Into<String>,
    link: impl Into<String>,
) -> Box<dyn Schema> {
    use crate::schemas::composed::make_top;

    let ins = inner.inputs();
    let outs = inner.outputs();
    let with_inputs = add_schema_inputs(ins, inner);
    let with_outputs = add_schema_outputs(outs, with_inputs);
    make_top(with_outputs, 20.0, name, link)
}

/// Prepend `n` connector stubs as inputs.
///
/// C++ reference: `drawschema.cpp:665` — `addSchemaInputs`.
fn add_schema_inputs(n: usize, x: Box<dyn Schema>) -> Box<dyn Schema> {
    if n == 0 {
        return x;
    }
    let mut y: Option<Box<dyn Schema>> = None;
    for _ in 0..n {
        let z: Box<dyn Schema> = Box::new(ConnectorSchema::new());
        y = Some(match y {
            None => z,
            Some(p) => make_par(p, z),
        });
    }
    make_seq(y.unwrap(), x)
}

/// Append `n` connector stubs as outputs.
///
/// C++ reference: `drawschema.cpp:683` — `addSchemaOutputs`.
fn add_schema_outputs(n: usize, x: Box<dyn Schema>) -> Box<dyn Schema> {
    if n == 0 {
        return x;
    }
    let mut y: Option<Box<dyn Schema>> = None;
    for _ in 0..n {
        let z: Box<dyn Schema> = Box::new(ConnectorSchema::new());
        y = Some(match y {
            None => z,
            Some(p) => make_par(p, z),
        });
    }
    make_seq(x, y.unwrap())
}
