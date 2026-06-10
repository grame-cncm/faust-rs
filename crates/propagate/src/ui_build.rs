//! Grouped UI collection for propagation.
//!
//! This module walks validated flat box DAGs, registers control widgets, and
//! builds the canonical `UiProgram` owned by propagation output. It deduplicates
//! shared source nodes while preserving distinct group-path contexts.

use super::*;

/// Internal grouped-UI collector used while traversing a validated flat box.
///
/// This keeps UI ownership local to propagation:
/// - the UI tree is built in its own arena,
/// - controls are registered exactly once and assigned dense [`ControlId`]s,
/// - source widget/group labels are decoded before FIR/backend stages.
///
/// The `visited` cache deduplicates DAG traversal: the flat box tree after
/// `eval` is a **DAG** (the same `FlatBoxId` may be reachable via multiple
/// composition paths when the same variable/slider is used in several
/// positions).  Without deduplication each occurrence would create a ghost
/// `ControlSpec` entry while overwriting the `control_ids` mapping — producing
/// spurious slider fields in `buildUserInterface` that are never referenced
/// in the compute loop.  The cache is indexed by `FlatBoxId` so that any
/// subtree (not just widget leaves) is processed at most once.
struct UiCollector {
    builder: UiProgramBuilder,
    controls: Vec<ControlSpec>,
    control_ids: ControlIds,
    /// Secondary index mapping a bare `BoxId` to the first `ControlId` registered for it.
    /// Used to detect that the same source node has already been registered under a
    /// different group-path context (e.g. a slider that appears in both the body and the
    /// seed of a `fad(…)` call) and to create a cross-context alias rather than a second
    /// `ControlSpec` entry.
    node_primary_id: AHashMap<BoxId, ControlId>,
    /// Memoisation table for the DAG walk — keyed by `(FlatBoxId, group_path_hash)` to allow
    /// the same structural widget to be registered once per distinct group context.
    visited: AHashMap<(FlatBoxId, u64), UiCollectSummary>,
}

impl UiCollector {
    fn new() -> Self {
        Self {
            builder: UiProgramBuilder::new(),
            controls: Vec::new(),
            control_ids: ControlIds::new(),
            node_primary_id: AHashMap::new(),
            visited: AHashMap::new(),
        }
    }

    fn finish(self, options: &PropagateUiOptions) -> UiBuildOutput {
        let (mut arena, roots) = self.builder.finish();
        let keep_existing_root = matches!(roots.as_slice(), [only] if matches!(match_ui(&arena, *only), UiMatch::Group { .. }));
        let (root, root_origin) = if keep_existing_root {
            (
                rewrite_root_group_label(&mut arena, roots[0], options),
                UiRootOrigin::Explicit,
            )
        } else {
            (
                synthesize_ui_root_group(&mut arena, &options.synthesized_root_label, &roots),
                UiRootOrigin::Synthesized,
            )
        };
        UiBuildOutput {
            program: UiProgram {
                arena,
                root,
                controls: self.controls,
                root_origin,
                emit_ui: true,
            },
            control_ids: self.control_ids,
        }
    }

    fn register_control(
        &mut self,
        source_node: BoxId,
        context_hash: u64,
        kind: ControlKind,
        label: String,
        metadata: UiMetadata,
        range: Option<ControlRange>,
    ) -> ControlId {
        let key = (source_node, context_hash);
        // Deduplicate: the same widget in the same group context must not be registered twice
        // (e.g. a slider variable referenced from two branches of the signal DAG).
        if let Some(&existing_id) = self.control_ids.get(&key) {
            return existing_id;
        }
        // Cross-context deduplication: if the same source node was already registered under
        // a different group-path context (e.g. a slider that appears in both the body and
        // the seed of a `fad(…)` call), reuse its existing ControlId and only add an alias
        // for the new context key. This prevents duplicate ControlSpec entries for what is
        // semantically one widget referenced from multiple positions in the box DAG.
        if let Some(&primary_id) = self.node_primary_id.get(&source_node) {
            self.control_ids.insert(key, primary_id);
            return primary_id;
        }
        let id =
            ControlId::try_from(self.controls.len()).expect("control registry index fits in u32");
        self.controls.push(ControlSpec {
            id,
            kind,
            label,
            metadata,
            range,
        });
        self.control_ids.insert(key, id);
        self.node_primary_id.insert(source_node, id);
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn input_control(
        &mut self,
        source_node: BoxId,
        path: &[UiGroupSpec],
        context_hash: u64,
        kind: ControlKind,
        label: String,
        metadata: UiMetadata,
        range: Option<ControlRange>,
    ) {
        let order_key = ui::ordering_key_from_label(&label, &metadata);
        let id = self.register_control(source_node, context_hash, kind, label, metadata, range);
        self.builder.insert_input_control(path, id, order_key);
    }

    #[allow(clippy::too_many_arguments)]
    fn output_control(
        &mut self,
        source_node: BoxId,
        path: &[UiGroupSpec],
        context_hash: u64,
        kind: ControlKind,
        label: String,
        metadata: UiMetadata,
        range: Option<ControlRange>,
    ) {
        let order_key = ui::ordering_key_from_label(&label, &metadata);
        let id = self.register_control(source_node, context_hash, kind, label, metadata, range);
        self.builder.insert_output_control(path, id, order_key);
    }

    fn soundfile(
        &mut self,
        source_node: BoxId,
        path: &[UiGroupSpec],
        context_hash: u64,
        label: String,
        metadata: UiMetadata,
    ) {
        let order_key = ui::ordering_key_from_label(&label, &metadata);
        let id = self.register_control(
            source_node,
            context_hash,
            ControlKind::Soundfile,
            label,
            metadata,
            None,
        );
        self.builder.insert_soundfile(path, id, order_key);
    }
}

fn synthesize_ui_root_group(arena: &mut TreeArena, label: &str, children: &[TreeId]) -> TreeId {
    ui::UiBuilder::new(arena).vgroup(label, children)
}

fn rewrite_root_group_label(
    arena: &mut TreeArena,
    root: TreeId,
    options: &PropagateUiOptions,
) -> TreeId {
    match match_ui(arena, root) {
        UiMatch::Group {
            kind,
            label,
            metadata,
            children,
        } if label.is_empty() && !options.synthesized_root_label.is_empty() => ui::UiBuilder::new(
            arena,
        )
        .group_with_metadata(kind, &options.synthesized_root_label, &metadata, &children),
        _ => root,
    }
}

/// Final products of grouped-UI extraction before signal lowering resumes.
///
/// `control_ids` is the bridge from source widget box nodes to stable
/// [`ControlId`]s embedded later in signal UI leaves.
pub(crate) struct UiBuildOutput {
    pub(crate) program: UiProgram,
    pub(crate) control_ids: ControlIds,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UiCollectSummary {
    has_ui: bool,
    preserve_ancestor_chain: bool,
}

/// Builds the canonical grouped-UI artifact for one validated flat box tree.
///
/// The returned [`UiProgram`] is already normalized for later phases:
/// - widget pathname labels have been rebased against the explicit group stack
///   like C++ `normalizePath(...)`,
/// - inline label metadata has been extracted,
/// - the root group has been synthesized or renamed according to
///   [`PropagateUiOptions`],
/// - every referenced control has a stable dense [`ControlId`].
pub(crate) fn build_ui_program(
    source_arena: &TreeArena,
    box_tree: FlatBoxId,
    options: &PropagateUiOptions,
) -> UiBuildOutput {
    let mut collector = UiCollector::new();
    let _ = collect_ui_nodes(source_arena, box_tree, &[], &mut collector);
    collector.finish(options)
}

/// Collects grouped UI nodes reachable from one validated flat box subtree.
///
/// Traversal follows the same semantic source tree used for DSP propagation,
/// but only UI-bearing families contribute concrete UI nodes:
/// widgets, bargraphs, soundfiles, and grouping wrappers. Composition-only DSP
/// nodes recurse structurally in deterministic source order.
///
/// Parity/adaptation note:
/// - widget labels still follow current C++ pathname rebasing,
/// - Rust additionally allows relative navigation on explicit group labels,
/// - placeholder explicit groups are kept only when their subtree contributes
///   UI or when a rebased explicit descendant needs the ancestor chain to stay
///   visible.
fn collect_ui_nodes(
    source_arena: &TreeArena,
    box_tree: FlatBoxId,
    current_groups: &[UiGroupPathSegment],
    collector: &mut UiCollector,
) -> UiCollectSummary {
    // DAG deduplication: the flat box tree after `eval` is a structural DAG —
    // the same arena node (e.g. a slider passed as a function argument) can be
    // reached from multiple composition paths.  The cache key includes the
    // group-path hash so that the same structural widget appearing under different
    // UI group contexts is processed once per context rather than only once total.
    let context_hash = group_path_hash(current_groups);
    if let Some(&cached) = collector.visited.get(&(box_tree, context_hash)) {
        return cached;
    }

    let kind = flat_node_kind(source_arena, box_tree).expect("validated flat box must decode");
    let result = match kind {
        FlatNodeKind::Button => {
            let BoxMatch::Button(label) = match_box(source_arena, box_tree.as_tree_id()) else {
                unreachable!("flat button node must decode to BoxMatch::Button")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::Button,
                label,
                metadata,
                None,
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::Checkbox => {
            let BoxMatch::Checkbox(label) = match_box(source_arena, box_tree.as_tree_id()) else {
                unreachable!("flat checkbox node must decode to BoxMatch::Checkbox")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::Checkbox,
                label,
                metadata,
                None,
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::VSlider => {
            let BoxMatch::VSlider(label, init, min, max, step) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat vslider node must decode to BoxMatch::VSlider")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::VSlider,
                label,
                metadata,
                Some(ControlRange {
                    init: decode_box_scalar(source_arena, init),
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: decode_box_scalar(source_arena, step),
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::HSlider => {
            let BoxMatch::HSlider(label, init, min, max, step) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat hslider node must decode to BoxMatch::HSlider")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::HSlider,
                label,
                metadata,
                Some(ControlRange {
                    init: decode_box_scalar(source_arena, init),
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: decode_box_scalar(source_arena, step),
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::NumEntry => {
            let BoxMatch::NumEntry(label, init, min, max, step) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat numentry node must decode to BoxMatch::NumEntry")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::NumEntry,
                label,
                metadata,
                Some(ControlRange {
                    init: decode_box_scalar(source_arena, init),
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: decode_box_scalar(source_arena, step),
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::VBargraph => {
            let BoxMatch::VBargraph(label, min, max) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat vbargraph node must decode to BoxMatch::VBargraph")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.output_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::VBargraph,
                label,
                metadata,
                Some(ControlRange {
                    init: 0.0,
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: 0.0,
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::HBargraph => {
            let BoxMatch::HBargraph(label, min, max) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat hbargraph node must decode to BoxMatch::HBargraph")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.output_control(
                box_tree.as_tree_id(),
                &path,
                context_hash,
                ControlKind::HBargraph,
                label,
                metadata,
                Some(ControlRange {
                    init: 0.0,
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: 0.0,
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::Soundfile => {
            let BoxMatch::Soundfile(label, _) = match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat soundfile node must decode to BoxMatch::Soundfile")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.soundfile(box_tree.as_tree_id(), &path, context_hash, label, metadata);
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::VGroup { body } => collect_group_ui(
            source_arena,
            body,
            current_groups,
            collector,
            UiGroupKind::Vertical,
            box_tree.as_tree_id(),
        ),
        FlatNodeKind::HGroup { body } => collect_group_ui(
            source_arena,
            body,
            current_groups,
            collector,
            UiGroupKind::Horizontal,
            box_tree.as_tree_id(),
        ),
        FlatNodeKind::TGroup { body } => collect_group_ui(
            source_arena,
            body,
            current_groups,
            collector,
            UiGroupKind::Tab,
            box_tree.as_tree_id(),
        ),
        FlatNodeKind::ForwardAD { body, seed } => {
            let body_s = collect_ui_nodes(source_arena, body, current_groups, collector);
            // Differentiation seeds are global parameters whose UI position is
            // independent of any group that wraps the surrounding `fad(…)` call.
            // Visiting with an empty context ensures that subsequent references to
            // the same seed node (e.g. in the Rec feedback branch) hit the cache
            // rather than being registered as duplicate controls.
            let seed_s = collect_ui_nodes(source_arena, seed, &[], collector);
            UiCollectSummary {
                has_ui: body_s.has_ui || seed_s.has_ui,
                preserve_ancestor_chain: body_s.preserve_ancestor_chain
                    || seed_s.preserve_ancestor_chain,
            }
        }
        FlatNodeKind::ReverseAD { body, seeds } => {
            let body_s = collect_ui_nodes(source_arena, body, current_groups, collector);
            // Same rationale as ForwardAD: seed parameters are registered without
            // the surrounding group context so later references can find them.
            let seeds_s = collect_ui_nodes(source_arena, seeds, &[], collector);
            UiCollectSummary {
                has_ui: body_s.has_ui || seeds_s.has_ui,
                preserve_ancestor_chain: body_s.preserve_ancestor_chain
                    || seeds_s.preserve_ancestor_chain,
            }
        }
        FlatNodeKind::Symbolic { body }
        | FlatNodeKind::Metadata { body }
        | FlatNodeKind::Ondemand(body)
        | FlatNodeKind::Upsampling(body)
        | FlatNodeKind::Downsampling(body) => {
            collect_ui_nodes(source_arena, body, current_groups, collector)
        }
        FlatNodeKind::Seq(left, right)
        | FlatNodeKind::Par(left, right)
        | FlatNodeKind::Split(left, right)
        | FlatNodeKind::Merge(left, right)
        | FlatNodeKind::Rec(left, right) => {
            let left_summary = collect_ui_nodes(source_arena, left, current_groups, collector);
            let right_summary = collect_ui_nodes(source_arena, right, current_groups, collector);
            UiCollectSummary {
                has_ui: left_summary.has_ui || right_summary.has_ui,
                preserve_ancestor_chain: left_summary.preserve_ancestor_chain
                    || right_summary.preserve_ancestor_chain,
            }
        }
        FlatNodeKind::Int
        | FlatNodeKind::Real
        | FlatNodeKind::Wire
        | FlatNodeKind::Cut
        | FlatNodeKind::Slot
        | FlatNodeKind::Prim1
        | FlatNodeKind::Prim2
        | FlatNodeKind::Prim3
        | FlatNodeKind::Prim4
        | FlatNodeKind::Prim5
        | FlatNodeKind::FFun
        | FlatNodeKind::FConst
        | FlatNodeKind::FVar
        | FlatNodeKind::Waveform
        | FlatNodeKind::Environment
        | FlatNodeKind::Route
        | FlatNodeKind::Inputs
        | FlatNodeKind::Outputs => UiCollectSummary::default(),
    };
    collector.visited.insert((box_tree, context_hash), result);
    result
}

fn collect_group_ui(
    source_arena: &TreeArena,
    body: FlatBoxId,
    current_groups: &[UiGroupPathSegment],
    collector: &mut UiCollector,
    kind: UiGroupKind,
    group_node: BoxId,
) -> UiCollectSummary {
    let label = match match_box(source_arena, group_node) {
        BoxMatch::VGroup(label, _) | BoxMatch::HGroup(label, _) | BoxMatch::TGroup(label, _) => {
            decode_box_label(source_arena, label)
        }
        _ => unreachable!("flat group node must decode to a group box"),
    };
    let normalized = normalize_group_label_navigation(&label, current_groups, kind);
    let mut nested_groups = normalized.parent_groups;
    nested_groups.push(normalized.group);

    let path = canonical_group_path(&nested_groups);
    let terminal_preexisting = collector.builder.find_group_path(&path);
    let terminal = collector
        .builder
        .ensure_group_path(&path)
        .expect("explicit group path must yield a terminal group");

    let summary = collect_ui_nodes(source_arena, body, &nested_groups, collector);
    let keep_group =
        collector.builder.group_has_children(terminal) || summary.preserve_ancestor_chain;
    if !keep_group && terminal_preexisting.is_none() {
        let removed = collector.builder.remove_group_if_empty(terminal);
        debug_assert!(
            removed,
            "fresh explicit group placeholder should be removable"
        );
    }

    UiCollectSummary {
        has_ui: summary.has_ui,
        preserve_ancestor_chain: keep_group,
    }
}

/// Converts one raw explicit-group stack into its canonical stored UI path.
///
/// Metadata extraction happens after pathname normalization so segments such as
/// `../gain [style:knob]` first rebase to the correct group and only then split
/// the final widget label and group metadata.
fn canonical_group_path(path: &[UiGroupPathSegment]) -> Vec<UiGroupSpec> {
    path.iter().map(canonicalize_group_spec).collect()
}

pub(crate) fn decode_box_label(arena: &TreeArena, node: BoxId) -> String {
    if let BoxMatch::Ident(value) = match_box(arena, node) {
        return value.to_string();
    }
    match arena.kind(node) {
        Some(NodeKind::StringLiteral(value)) | Some(NodeKind::Symbol(value)) => value.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn decode_box_scalar(arena: &TreeArena, node: BoxId) -> f64 {
    match match_box(arena, node) {
        BoxMatch::Int(value) => f64::from(value),
        BoxMatch::Real(value) => value,
        _ => 0.0,
    }
}
