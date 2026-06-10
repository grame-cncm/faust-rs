//! UI IR construction and matching helpers backed by `tlib::TreeArena`.
//!
//! # Source provenance (C++)
//! - `compiler/propagate/propagate.cpp`
//! - `compiler/signals/signals.hh`
//! - `compiler/signals/signals.cpp`
//! - `compiler/transform/signalFIRCompiler.hh`
//! - `compiler/generator/instructions_compiler.cpp`
//! - `compiler/generator/compile.cpp`
//!
//! # Public API mapping status
//! - Public construction API is [`UiBuilder`].
//! - Public inspection API is [`match_ui`] + [`UiMatch`].
//! - [`UiProgram`] is the canonical grouped-UI artifact owned after the
//!   `propagate` boundary in the Rust architecture contract.
//!
//! # Parity invariants
//! - UI nodes are represented as tagged trees with deterministic child order.
//! - Canonical grouped layout lives in this crate, not in backend-local
//!   heuristics.
//! - Group children preserve source traversal order through `cons`/`nil` lists.
//! - Controls are referenced by deterministic [`ControlId`] values rather than
//!   owning layout in DSP signal nodes.

use std::collections::{BTreeMap, BTreeSet};

use tlib::{NodeKind, TreeArena, TreeId, list_to_vec, vec_to_list};

pub const CRATE_NAME: &str = "ui";

/// UI node identifier in `TreeArena`.
pub type UiId = TreeId;

/// Stable control identifier joining DSP control references with grouped UI
/// layout.
///
/// Contract:
/// - allocated densely from `0..controls.len()` by `propagate`,
/// - stable for the lifetime of one [`UiProgram`],
/// - embedded in signal leaf widgets instead of duplicating labels/ranges in
///   signal IR,
/// - resolved later through [`UiProgram::control`] during FIR lowering and
///   runtime UI callback replay.
pub type ControlId = u32;

const UI_GROUP_TAG: &str = "UIGROUP";
const UI_METADATA_ENTRY_TAG: &str = "UIMETADATAENTRY";
const UI_INPUT_CONTROL_TAG: &str = "UIINPUTCONTROL";
const UI_OUTPUT_CONTROL_TAG: &str = "UIOUTPUTCONTROL";
const UI_SOUNDFILE_TAG: &str = "UISOUNDFILE";

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// Canonical group-orientation family for grouped UI layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i64)]
pub enum UiGroupKind {
    Vertical = 0,
    Horizontal = 1,
    Tab = 2,
}

impl UiGroupKind {
    #[must_use]
    /// Decodes one raw arena integer into a group orientation.
    pub fn from_raw(raw: i64) -> Option<Self> {
        match raw {
            0 => Some(Self::Vertical),
            1 => Some(Self::Horizontal),
            2 => Some(Self::Tab),
            _ => None,
        }
    }

    #[must_use]
    /// Returns the stable lowercase name used in dumps and diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
            Self::Tab => "tab",
        }
    }
}

/// Raw grouped-UI path segment used while normalizing Faust pathname labels.
///
/// This is intentionally earlier than [`UiGroupSpec`]:
/// - `raw_label` still preserves inline metadata text,
/// - pathname normalization happens before metadata extraction,
/// - later `split_label_metadata(...)` turns this raw segment into a canonical
///   [`UiGroupSpec`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UiGroupPathSegment {
    pub kind: UiGroupKind,
    pub raw_label: String,
}

/// Canonical grouped-UI path segment stored after metadata extraction.
///
/// Mapping status:
/// - `adapted` relative to the C++ cons-list path representation.
/// - Canonical Rust carrier for path-aware `UiProgram` insertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiGroupSpec {
    pub kind: UiGroupKind,
    pub label: String,
    pub metadata: UiMetadata,
}

/// Normalized widget-style pathname after rebasing against the current group
/// context.
///
/// # Source provenance (C++)
/// - `compiler/propagate/labels.cpp`
/// - `label2path(...)`
/// - `normalizePath(...)`
///
/// Parity rules:
/// - widget pathnames may rebase against the current explicit group stack,
/// - typed path segments such as `h:` / `v:` / `t:` become explicit grouped UI
///   ancestors,
/// - metadata is preserved in raw segment labels until later extraction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiNormalizedWidgetPath {
    pub groups: Vec<UiGroupPathSegment>,
    pub raw_label: String,
}

/// Normalized explicit-group placement after applying the Rust-only relative
/// navigation extension.
///
/// This is intentionally narrower than widget pathname normalization:
/// - only leading navigation operators are interpreted,
/// - the explicit source group still owns the final orientation,
/// - arbitrary intermediate `foo/bar` synthesis is out of scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiNormalizedGroupPath {
    pub parent_groups: Vec<UiGroupPathSegment>,
    pub group: UiGroupPathSegment,
}

/// Stable UI control family shared by layout and DSP control references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Button,
    Checkbox,
    VSlider,
    HSlider,
    NumEntry,
    VBargraph,
    HBargraph,
    Soundfile,
}

/// Canonical metadata entry list used by grouped UI labels and controls.
///
/// Entries are already normalized by [`split_label_metadata`]:
/// deterministic ordering, trimmed keys/values, and duplicate coalescing.
pub type UiMetadata = Vec<(String, String)>;

/// Builds the C++-parity ordering key for one widget/group: the label with
/// its numeric ordering prefix restored (`[n] label`).
///
/// The C++ compiler sequences each group's children by plain byte-wise
/// comparison of the RAW label — the `[n] ` prefix included — so `"[10]"`
/// sorts before `"[2]"`, and unnumbered labels interleave by their own
/// spelling (verified against C++ faust JSON output). The numeric prefix is
/// parsed into a digits-only metadata key with an empty value during label
/// splitting, so it can be reconstructed here for ordering purposes.
pub fn ordering_key_from_label(label: &str, metadata: &UiMetadata) -> String {
    for (key, value) in metadata {
        if value.is_empty() && !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_digit()) {
            return format!("[{key}] {label}");
        }
    }
    label.to_owned()
}

/// Numeric range metadata for slider-like controls.
///
/// This is the canonical UI-side carrier for widget default/range semantics
/// before FIR lowering translates them into backend-specific slider/bargraph
/// instructions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlRange {
    pub init: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

/// Canonical control-registry entry referenced by grouped UI layout and later
/// DSP/FIR lowering.
///
/// Rust keeps control semantics in this dedicated registry instead of
/// duplicating them in every grouped layout node. This is an `adapted`
/// representation versus the C++ path encoding, but it preserves behavior
/// while making later FIR/runtime lookup explicit and testable.
#[derive(Debug)]
pub struct ControlSpec {
    /// Stable registry key also embedded in signal UI leaf nodes.
    pub id: ControlId,
    /// Widget/bargraph family.
    pub kind: ControlKind,
    /// Final display label after inline metadata extraction.
    pub label: String,
    /// Canonical metadata extracted from the original Faust label.
    pub metadata: UiMetadata,
    /// Numeric range only for slider-like controls.
    pub range: Option<ControlRange>,
}

/// Source of the canonical root group stored in [`UiProgram`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiRootOrigin {
    /// The root came from an explicit Faust group in source.
    Explicit,
    /// The root was synthesized by grouped-UI construction in Rust.
    Synthesized,
}

/// Canonical grouped-UI artifact owned after the `propagate` boundary.
///
/// Rust architecture note:
/// - `porting/ui-ir-architecture-contract-2026-03-12-en.md`
///
/// Mapping status:
/// - `adapted` relative to the C++ internal clock-environment/path encoding.
/// - `1:1` behaviorally for grouped `buildUserInterface` ownership.
///
/// Main invariants:
/// - `root` always points at a group node,
/// - `controls[id as usize].id == id` for all registered controls,
/// - `emit_ui == false` designates the placeholder/no-emission program used by
///   UI-free signal compilation paths,
/// - root naming has already been canonicalized before this struct reaches FIR
///   lowering.
#[derive(Debug)]
pub struct UiProgram {
    /// Tree arena owning the grouped UI layout.
    pub arena: TreeArena,
    /// Canonical root group node.
    pub root: UiId,
    /// Dense registry of all referenced controls.
    pub controls: Vec<ControlSpec>,
    /// Whether the root group came from source or from synthesis.
    pub root_origin: UiRootOrigin,
    /// Whether downstream lowering should emit `buildUserInterface`.
    pub emit_ui: bool,
}

impl UiProgram {
    #[must_use]
    /// Builds one empty grouped-UI program with a canonical empty vertical root.
    pub fn empty() -> Self {
        let mut arena = TreeArena::new();
        let root = UiBuilder::new(&mut arena).vgroup("", &[]);
        Self {
            arena,
            root,
            controls: Vec::new(),
            root_origin: UiRootOrigin::Synthesized,
            emit_ui: false,
        }
    }

    #[must_use]
    /// Returns one control specification by stable [`ControlId`].
    pub fn control(&self, id: ControlId) -> Option<&ControlSpec> {
        self.controls.get(usize::try_from(id).ok()?)
    }

    #[must_use]
    /// Returns `true` when this program is the compatibility placeholder with
    /// no UI emission.
    ///
    /// [`UiProgram::empty`] still carries a canonical vertical root so
    /// downstream code can keep a simple "always has a root" invariant.
    pub fn is_empty(&self) -> bool {
        !self.emit_ui
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum UiPathOp {
    Root,
    Current,
    Parent,
    Group(UiGroupPathSegment),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ParsedUiLabelPath {
    Widget {
        ops: Vec<UiPathOp>,
        raw_label: String,
    },
    GroupNavigation {
        absolute: bool,
        parents: usize,
        raw_label: String,
    },
}

/// Normalizes one Faust widget label pathname against the current explicit
/// group stack.
///
/// Examples:
/// - `../volume`
/// - `/gain`
/// - `h:Oscillator/freq`
/// - `../gain [style:knob]`
#[must_use]
pub fn normalize_widget_label_path(
    full_label: &str,
    current_groups: &[UiGroupPathSegment],
) -> UiNormalizedWidgetPath {
    let ParsedUiLabelPath::Widget { ops, raw_label } = parse_widget_label_path(full_label) else {
        unreachable!("widget label parser always yields widget paths");
    };

    let mut normalized = current_groups.to_vec();
    for op in ops {
        match op {
            UiPathOp::Root => normalized.clear(),
            UiPathOp::Current => {}
            UiPathOp::Parent => {
                let _ = normalized.pop();
            }
            UiPathOp::Group(group) => normalized.push(group),
        }
    }

    UiNormalizedWidgetPath {
        groups: normalized,
        raw_label,
    }
}

/// Normalizes one explicit group label using the Rust-only relative navigation
/// extension.
///
/// Supported prefixes:
/// - `./`
/// - one or more `../`
/// - `/`
///
/// The final explicit group still keeps the orientation supplied by the source
/// `vgroup` / `hgroup` / `tgroup` node.
#[must_use]
pub fn normalize_group_label_navigation(
    full_label: &str,
    current_groups: &[UiGroupPathSegment],
    kind: UiGroupKind,
) -> UiNormalizedGroupPath {
    let ParsedUiLabelPath::GroupNavigation {
        absolute,
        parents,
        raw_label,
    } = parse_group_label_navigation(full_label)
    else {
        unreachable!("group label parser always yields group-navigation paths");
    };

    let mut parent_groups = if absolute {
        Vec::new()
    } else {
        current_groups.to_vec()
    };
    clamp_pop_groups(&mut parent_groups, parents);

    UiNormalizedGroupPath {
        parent_groups,
        group: UiGroupPathSegment { kind, raw_label },
    }
}

/// Splits one Faust UI label into its simplified display label and extracted
/// metadata declarations.
///
/// # Source provenance (C++)
/// - `compiler/generator/description.cpp`
/// - `extractMetadata(...)`
///
/// Parity rules:
/// - strips leading/trailing spaces and tabs from the final display label,
/// - strips leading/trailing spaces and tabs from metadata keys and values,
/// - preserves nested `[` / `]` inside metadata values,
/// - preserves escaped characters by removing the escape marker and keeping the
///   escaped byte in the corresponding output string,
/// - returns metadata in deterministic key/value-sorted order like the C++
///   `map<string, set<string>>` accumulation.
#[must_use]
pub fn split_label_metadata(full_label: &str) -> (String, UiMetadata) {
    #[derive(Clone, Copy)]
    enum State {
        Label,
        EscapeLabel,
        EscapeKey,
        EscapeValue,
        Key,
        Value,
    }

    let mut state = State::Label;
    let mut depth = 0_i32;
    let mut label = String::new();
    let mut key = String::new();
    let mut value = String::new();
    let mut metadata = BTreeMap::<String, BTreeSet<String>>::new();

    for ch in full_label.chars() {
        match state {
            State::Label => match ch {
                '\\' => state = State::EscapeLabel,
                '[' => {
                    state = State::Key;
                    depth += 1;
                }
                _ => label.push(ch),
            },
            State::EscapeLabel => {
                label.push(ch);
                state = State::Label;
            }
            State::EscapeKey => {
                key.push(ch);
                state = State::Key;
            }
            State::EscapeValue => {
                value.push(ch);
                state = State::Value;
            }
            State::Key => match ch {
                '\\' => state = State::EscapeKey,
                '[' => {
                    depth += 1;
                    key.push(ch);
                }
                ':' if depth == 1 => state = State::Value,
                ']' => {
                    depth -= 1;
                    if depth < 1 {
                        metadata
                            .entry(trim_space_tab(&key).to_owned())
                            .or_default()
                            .insert(String::new());
                        state = State::Label;
                        key.clear();
                        value.clear();
                    } else {
                        key.push(ch);
                    }
                }
                _ => key.push(ch),
            },
            State::Value => match ch {
                '\\' => state = State::EscapeValue,
                '[' => {
                    depth += 1;
                    value.push(ch);
                }
                ']' => {
                    depth -= 1;
                    if depth < 1 {
                        metadata
                            .entry(trim_space_tab(&key).to_owned())
                            .or_default()
                            .insert(trim_space_tab(&value).to_owned());
                        state = State::Label;
                        key.clear();
                        value.clear();
                    } else {
                        value.push(ch);
                    }
                }
                _ => value.push(ch),
            },
        }
    }

    let metadata = metadata
        .into_iter()
        .flat_map(|(key, values)| values.into_iter().map(move |value| (key.clone(), value)))
        .collect();
    (trim_space_tab(&label).to_owned(), metadata)
}

#[must_use]
/// Converts one raw grouped-UI pathname segment into its canonical stored form.
pub fn canonicalize_group_spec(segment: &UiGroupPathSegment) -> UiGroupSpec {
    let (label, metadata) = split_label_metadata(&segment.raw_label);
    UiGroupSpec {
        kind: segment.kind,
        label,
        metadata,
    }
}

/// Canonical builder API for constructing UI IR nodes.
///
/// Builder methods preserve source child order and encode grouped layout in the
/// tree representation consumed by [`match_ui`] and [`UiProgram`].
pub struct UiBuilder<'a> {
    arena: &'a mut TreeArena,
}

impl<'a> UiBuilder<'a> {
    #[must_use]
    /// Creates a `UiBuilder` bound to one mutable `TreeArena`.
    pub fn new(arena: &'a mut TreeArena) -> Self {
        Self { arena }
    }

    #[must_use]
    /// Builds one grouped UI node with the provided ordered child list.
    pub fn group(&mut self, kind: UiGroupKind, label: &str, children: &[UiId]) -> UiId {
        self.group_with_metadata(kind, label, &UiMetadata::new(), children)
    }

    #[must_use]
    /// Builds one grouped UI node plus already-extracted label metadata.
    pub fn group_with_metadata(
        &mut self,
        kind: UiGroupKind,
        label: &str,
        metadata: &[(String, String)],
        children: &[UiId],
    ) -> UiId {
        let kind = self.arena.int(kind as i64);
        let label = self.arena.string_lit(label);
        let metadata = encode_metadata_list(self.arena, metadata);
        let children = vec_to_list(self.arena, children);
        intern_tag(self.arena, UI_GROUP_TAG, &[kind, label, metadata, children])
    }

    #[must_use]
    /// Builds one vertical UI group.
    pub fn vgroup(&mut self, label: &str, children: &[UiId]) -> UiId {
        self.group(UiGroupKind::Vertical, label, children)
    }

    #[must_use]
    /// Builds one horizontal UI group.
    pub fn hgroup(&mut self, label: &str, children: &[UiId]) -> UiId {
        self.group(UiGroupKind::Horizontal, label, children)
    }

    #[must_use]
    /// Builds one tab UI group.
    pub fn tgroup(&mut self, label: &str, children: &[UiId]) -> UiId {
        self.group(UiGroupKind::Tab, label, children)
    }

    #[must_use]
    /// Builds one input-control leaf referencing a stable [`ControlId`].
    pub fn input_control(&mut self, control: ControlId) -> UiId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, UI_INPUT_CONTROL_TAG, &[control])
    }

    #[must_use]
    /// Builds one output-control leaf referencing a stable [`ControlId`].
    pub fn output_control(&mut self, control: ControlId) -> UiId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, UI_OUTPUT_CONTROL_TAG, &[control])
    }

    #[must_use]
    /// Builds one soundfile UI leaf referencing a stable [`ControlId`].
    pub fn soundfile(&mut self, control: ControlId) -> UiId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, UI_SOUNDFILE_TAG, &[control])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum UiDraftNode {
    Group {
        spec: UiGroupSpec,
        children: Vec<(String, usize)>, // (order_key, node_id)
    },
    InputControl(ControlId),
    OutputControl(ControlId),
    Soundfile(ControlId),
}

/// Path-aware grouped UI builder that incrementally assembles canonical layout
/// before TreeArena materialization.
///
/// Rust uses this builder instead of trying to mutate `TreeArena` groups in
/// place. This is an `adapted` representation, but it makes pathname rebasing,
/// group merging, and deterministic insertion order explicit and testable.
#[derive(Default)]
pub struct UiProgramBuilder {
    roots: Vec<usize>,
    nodes: Vec<UiDraftNode>,
}

impl UiProgramBuilder {
    #[must_use]
    /// Creates one empty path-aware UI builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensures that the full canonical group path exists and returns the last
    /// group node id.
    pub fn ensure_group_path(&mut self, path: &[UiGroupSpec]) -> Option<usize> {
        let mut parent = None;
        for spec in path {
            parent = Some(self.find_or_create_group(parent, spec.clone()));
        }
        parent
    }

    #[must_use]
    /// Looks up the terminal group id for one canonical path without creating
    /// missing ancestors.
    pub fn find_group_path(&self, path: &[UiGroupSpec]) -> Option<usize> {
        let mut parent = None;
        for spec in path {
            let child = self.find_child_group(parent, spec)?;
            parent = Some(child);
        }
        parent
    }

    #[must_use]
    /// Returns whether one draft group already has child nodes attached.
    pub fn group_has_children(&self, group: usize) -> bool {
        match &self.nodes[group] {
            UiDraftNode::Group { children, .. } => !children.is_empty(),
            UiDraftNode::InputControl(_)
            | UiDraftNode::OutputControl(_)
            | UiDraftNode::Soundfile(_) => false,
        }
    }

    /// Removes one empty draft group from its parent/root list.
    ///
    /// This is used by `propagate` to reserve source-order placeholders for
    /// explicit groups, then drop placeholders whose subtree ended up emitting
    /// no UI at all.
    pub fn remove_group_if_empty(&mut self, group: usize) -> bool {
        if self.group_has_children(group) {
            return false;
        }
        if let Some(parent) = self.find_parent(group) {
            if let UiDraftNode::Group { children, .. } = &mut self.nodes[parent] {
                children.retain(|(_, child)| *child != group);
            }
            return true;
        }
        self.roots.retain(|root| *root != group);
        true
    }

    /// Inserts one input control under the provided canonical group path.
    ///
    /// `order_key` is the numeric ordering index extracted from the widget's
    /// metadata (e.g. `[0:]` → `0`). Use [`ordering_key_from_metadata`] to
    /// derive it. Items with equal keys are kept in insertion order.
    pub fn insert_input_control(
        &mut self,
        path: &[UiGroupSpec],
        control: ControlId,
        order_key: String,
    ) {
        self.insert_leaf(path, UiDraftNode::InputControl(control), order_key);
    }

    /// Inserts one output control under the provided canonical group path.
    pub fn insert_output_control(
        &mut self,
        path: &[UiGroupSpec],
        control: ControlId,
        order_key: String,
    ) {
        self.insert_leaf(path, UiDraftNode::OutputControl(control), order_key);
    }

    /// Inserts one soundfile control under the provided canonical group path.
    pub fn insert_soundfile(
        &mut self,
        path: &[UiGroupSpec],
        control: ControlId,
        order_key: String,
    ) {
        self.insert_leaf(path, UiDraftNode::Soundfile(control), order_key);
    }

    #[must_use]
    /// Materializes the currently accumulated grouped UI forest into
    /// `TreeArena`-backed nodes and returns the top-level roots in insertion
    /// order.
    pub fn finish(self) -> (TreeArena, Vec<UiId>) {
        let mut arena = TreeArena::new();
        let mut roots = Vec::with_capacity(self.roots.len());
        for root in &self.roots {
            roots.push(self.materialize_node(*root, &mut arena));
        }
        (arena, roots)
    }

    fn insert_leaf(&mut self, path: &[UiGroupSpec], leaf: UiDraftNode, order_key: String) {
        let parent = self.ensure_group_path(path);
        let id = self.push_node(leaf);
        self.insert_child_sorted(parent, order_key, id);
    }

    fn find_or_create_group(&mut self, parent: Option<usize>, spec: UiGroupSpec) -> usize {
        if let Some(existing) = self.find_child_group(parent, &spec) {
            return existing;
        }
        let order_key = ordering_key_from_label(&spec.label, &spec.metadata);
        let id = self.push_node(UiDraftNode::Group {
            spec,
            children: Vec::new(),
        });
        self.insert_child_sorted(parent, order_key, id);
        id
    }

    fn find_child_group(&self, parent: Option<usize>, spec: &UiGroupSpec) -> Option<usize> {
        let ids = self.child_ids(parent);
        ids.into_iter().find(|child| match &self.nodes[*child] {
            UiDraftNode::Group {
                spec: child_spec, ..
            } => child_spec == spec,
            UiDraftNode::InputControl(_)
            | UiDraftNode::OutputControl(_)
            | UiDraftNode::Soundfile(_) => false,
        })
    }

    fn find_parent(&self, needle: usize) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .find_map(|(id, node)| match node {
                UiDraftNode::Group { children, .. }
                    if children.iter().any(|(_, child)| *child == needle) =>
                {
                    Some(id)
                }
                UiDraftNode::Group { .. }
                | UiDraftNode::InputControl(_)
                | UiDraftNode::OutputControl(_)
                | UiDraftNode::Soundfile(_) => None,
            })
    }

    fn push_node(&mut self, node: UiDraftNode) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    /// Returns child node IDs of `parent` in sorted order (by order key).
    fn child_ids(&self, parent: Option<usize>) -> Vec<usize> {
        match parent {
            Some(id) => match &self.nodes[id] {
                UiDraftNode::Group { children, .. } => {
                    children.iter().map(|(_, child)| *child).collect()
                }
                UiDraftNode::InputControl(_)
                | UiDraftNode::OutputControl(_)
                | UiDraftNode::Soundfile(_) => panic!("parent must be a group"),
            },
            None => self.roots.clone(),
        }
    }

    /// Inserts `child_id` into `parent`'s children list keeping the list
    /// sorted by `order_key`. Items with equal keys are appended after
    /// existing items with the same key (stable / insertion-order tiebreak).
    fn insert_child_sorted(&mut self, parent: Option<usize>, order_key: String, child_id: usize) {
        match parent {
            Some(id) => {
                if let UiDraftNode::Group { children, .. } = &mut self.nodes[id] {
                    let pos = children.partition_point(|(k, _)| k.as_str() <= order_key.as_str());
                    children.insert(pos, (order_key, child_id));
                } else {
                    panic!("parent must be a group");
                }
            }
            None => self.roots.push(child_id),
        }
    }

    fn materialize_node(&self, id: usize, arena: &mut TreeArena) -> UiId {
        match &self.nodes[id] {
            UiDraftNode::Group { spec, children } => {
                let children = children
                    .iter()
                    .map(|(_, child)| self.materialize_node(*child, arena))
                    .collect::<Vec<_>>();
                UiBuilder::new(arena).group_with_metadata(
                    spec.kind,
                    &spec.label,
                    &spec.metadata,
                    &children,
                )
            }
            UiDraftNode::InputControl(control) => UiBuilder::new(arena).input_control(*control),
            UiDraftNode::OutputControl(control) => UiBuilder::new(arena).output_control(*control),
            UiDraftNode::Soundfile(control) => UiBuilder::new(arena).soundfile(*control),
        }
    }
}

/// Canonical matcher view for one UI IR node.
///
/// This is the stable decoding surface for grouped UI trees. Callers should
/// prefer it over depending on raw `TreeArena` tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiMatch<'a> {
    Group {
        kind: UiGroupKind,
        label: &'a str,
        metadata: UiMetadata,
        children: Vec<UiId>,
    },
    InputControl(ControlId),
    OutputControl(ControlId),
    Soundfile(ControlId),
    Unknown,
}

/// Decodes one UI IR node into its canonical matcher view.
#[must_use]
pub fn match_ui(arena: &TreeArena, id: UiId) -> UiMatch<'_> {
    let Some(node) = arena.node(id) else {
        return UiMatch::Unknown;
    };
    match (&node.kind, node.children.as_slice()) {
        (NodeKind::Tag(tag), [kind, label, metadata, children])
            if arena.tag_name(*tag).unwrap_or("") == UI_GROUP_TAG =>
        {
            let Some(kind) = decode_group_kind(arena, *kind) else {
                return UiMatch::Unknown;
            };
            let Some(label) = decode_label(arena, *label) else {
                return UiMatch::Unknown;
            };
            let Some(metadata) = decode_metadata_list(arena, *metadata) else {
                return UiMatch::Unknown;
            };
            let Some(children) = list_to_vec(arena, *children) else {
                return UiMatch::Unknown;
            };
            UiMatch::Group {
                kind,
                label,
                metadata,
                children,
            }
        }
        (NodeKind::Tag(tag), [control])
            if arena.tag_name(*tag).unwrap_or("") == UI_INPUT_CONTROL_TAG =>
        {
            decode_control_id(arena, *control).map_or(UiMatch::Unknown, UiMatch::InputControl)
        }
        (NodeKind::Tag(tag), [control])
            if arena.tag_name(*tag).unwrap_or("") == UI_OUTPUT_CONTROL_TAG =>
        {
            decode_control_id(arena, *control).map_or(UiMatch::Unknown, UiMatch::OutputControl)
        }
        (NodeKind::Tag(tag), [control])
            if arena.tag_name(*tag).unwrap_or("") == UI_SOUNDFILE_TAG =>
        {
            decode_control_id(arena, *control).map_or(UiMatch::Unknown, UiMatch::Soundfile)
        }
        _ => UiMatch::Unknown,
    }
}

fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[UiId]) -> UiId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(NodeKind::Tag(tag_id), children)
}

fn decode_group_kind(arena: &TreeArena, id: UiId) -> Option<UiGroupKind> {
    match arena.kind(id) {
        Some(NodeKind::Int(raw)) => UiGroupKind::from_raw(*raw),
        _ => None,
    }
}

fn decode_control_id(arena: &TreeArena, id: UiId) -> Option<ControlId> {
    match arena.kind(id) {
        Some(NodeKind::Int(raw)) => u32::try_from(*raw).ok(),
        _ => None,
    }
}

fn decode_label(arena: &TreeArena, id: UiId) -> Option<&str> {
    match arena.kind(id) {
        Some(NodeKind::StringLiteral(value)) => Some(value),
        Some(NodeKind::Symbol(value)) => Some(value),
        _ => None,
    }
}

fn trim_space_tab(value: &str) -> &str {
    value.trim_matches(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r'))
}

fn clamp_pop_groups(groups: &mut Vec<UiGroupPathSegment>, count: usize) {
    for _ in 0..count {
        if groups.pop().is_none() {
            break;
        }
    }
}

fn parse_widget_label_path(full_label: &str) -> ParsedUiLabelPath {
    let (ops, raw_label) = parse_widget_path_ops(full_label);
    ParsedUiLabelPath::Widget { ops, raw_label }
}

fn parse_group_label_navigation(full_label: &str) -> ParsedUiLabelPath {
    let (absolute, parents, raw_label) = parse_group_navigation_prefix(full_label);
    ParsedUiLabelPath::GroupNavigation {
        absolute,
        parents,
        raw_label,
    }
}

fn parse_group_navigation_prefix(full_label: &str) -> (bool, usize, String) {
    let mut rest = full_label;
    let mut absolute = false;
    while rest.starts_with('/') {
        absolute = true;
        rest = &rest[1..];
    }

    let mut parents = 0_usize;
    loop {
        if let Some(next) = rest.strip_prefix("./") {
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix("../") {
            parents += 1;
            rest = next;
            continue;
        }
        break;
    }

    (absolute, parents, rest.to_owned())
}

fn parse_widget_path_ops(full_label: &str) -> (Vec<UiPathOp>, String) {
    let mut rest = full_label;
    let mut ops = Vec::new();

    while rest.starts_with('/') {
        ops.push(UiPathOp::Root);
        rest = &rest[1..];
    }

    loop {
        if let Some(next) = rest.strip_prefix("./") {
            ops.push(UiPathOp::Current);
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix("../") {
            ops.push(UiPathOp::Parent);
            rest = next;
            continue;
        }
        if let Some((kind, next_rest, raw_label)) = parse_typed_group_prefix(rest) {
            ops.push(UiPathOp::Group(UiGroupPathSegment {
                kind,
                raw_label: raw_label.to_owned(),
            }));
            rest = next_rest;
            continue;
        }
        break;
    }

    (ops, rest.to_owned())
}

fn parse_typed_group_prefix(rest: &str) -> Option<(UiGroupKind, &str, &str)> {
    let bytes = rest.as_bytes();
    if bytes.len() < 4 || bytes.get(1) != Some(&b':') {
        return None;
    }
    let kind = match bytes[0] {
        b'v' | b'V' => UiGroupKind::Vertical,
        b'h' | b'H' => UiGroupKind::Horizontal,
        b't' | b'T' => UiGroupKind::Tab,
        _ => return None,
    };
    let slash = rest[2..].find('/')?;
    let label_end = 2 + slash;
    let raw_label = &rest[2..label_end];
    let next_rest = &rest[label_end + 1..];
    Some((kind, next_rest, raw_label))
}

fn encode_metadata_list(arena: &mut TreeArena, metadata: &[(String, String)]) -> UiId {
    let mut entries = Vec::with_capacity(metadata.len());
    for (key, value) in metadata {
        let key = arena.string_lit(key);
        let value = arena.string_lit(value);
        entries.push(intern_tag(arena, UI_METADATA_ENTRY_TAG, &[key, value]));
    }
    vec_to_list(arena, &entries)
}

fn decode_metadata_list(arena: &TreeArena, id: UiId) -> Option<UiMetadata> {
    let entries = list_to_vec(arena, id)?;
    let mut metadata = Vec::with_capacity(entries.len());
    for entry in entries {
        let node = arena.node(entry)?;
        let (NodeKind::Tag(tag), [key, value]) = (&node.kind, node.children.as_slice()) else {
            return None;
        };
        if arena.tag_name(*tag).unwrap_or("") != UI_METADATA_ENTRY_TAG {
            return None;
        }
        metadata.push((
            decode_label(arena, *key)?.to_owned(),
            decode_label(arena, *value)?.to_owned(),
        ));
    }
    Some(metadata)
}
