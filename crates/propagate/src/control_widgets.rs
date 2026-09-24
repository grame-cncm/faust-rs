//! The control inputs and bargraphs of one box, in interface order.
//!
//! faust-rs extension, no C++ equivalent: the evaluator folds `cinputs(e)`,
//! `cinput(i, e)`, `coutputs(e)`, `coutput(i, e)` and the wildcard modulation
//! target `"*"` from this list (see
//! `porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`).
//!
//! The list is read off the grouped UI the propagation builds for the same box
//! ([`build_ui_program`]), so "the i-th control" is the i-th control of the
//! program's `buildUserInterface` and JSON: groups merged by label, the
//! children of each group sorted by raw label (`[n]` prefixes included) as the
//! C++ compiler sorts them, a widget reached through several paths of the DAG
//! counted once per group context. A control is identified by the widget box
//! and the group context of its occurrence, the key the UI builder gives it;
//! [`UiGroupContext`] rebuilds that key while walking the same box.

use super::*;
use ui::UiId;

/// One control input or bargraph of a box.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlWidget {
    /// The widget box, the same node as in the box walked.
    pub widget: BoxId,
    /// Widget family.
    pub kind: ControlKind,
    /// Folded default and range; `None` for buttons and checkboxes.
    pub range: Option<ControlRange>,
    /// The widget's path in the interface, outermost group first, the widget's
    /// own label last, every segment without its metadata. A synthesized root
    /// group is not part of it.
    pub path: Vec<String>,
}

/// The control inputs and the bargraphs of one box, each in interface order.
#[derive(Clone, Debug)]
pub struct ControlWidgets {
    /// Sliders, numentries, buttons and checkboxes.
    pub inputs: Vec<ControlWidget>,
    /// Bargraphs.
    pub outputs: Vec<ControlWidget>,
    /// Every `(widget, group context)` occurrence key, aliases included, to
    /// its index in `inputs`.
    input_index: AHashMap<(BoxId, u64), usize>,
}

impl ControlWidgets {
    fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_index: AHashMap::new(),
        }
    }

    /// Index in [`ControlWidgets::inputs`] of the control a widget occurrence
    /// denotes, `context` being [`UiGroupContext::key`] at the occurrence.
    #[must_use]
    pub fn input_index(&self, widget: BoxId, context: u64) -> Option<usize> {
        self.input_index.get(&(widget, context)).copied()
    }
}

/// The group context of a box occurrence, as the UI builder keys controls.
///
/// Start from [`UiGroupContext::default`] at the root of the box given to
/// [`control_widgets`], [`UiGroupContext::enter`] each group box on the way
/// down, and restart from the root context at the seeds of `fad` and `rad`,
/// which the UI builder walks without their enclosing groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiGroupContext {
    groups: Vec<UiGroupPathSegment>,
}

impl UiGroupContext {
    /// The context inside `group`, or `None` when `group` is not a group box.
    #[must_use]
    pub fn enter(&self, arena: &TreeArena, group: BoxId) -> Option<Self> {
        let (label, kind) = match match_box(arena, group) {
            BoxMatch::VGroup(label, _) => (label, UiGroupKind::Vertical),
            BoxMatch::HGroup(label, _) => (label, UiGroupKind::Horizontal),
            BoxMatch::TGroup(label, _) => (label, UiGroupKind::Tab),
            _ => return None,
        };
        let label = decode_box_label(arena, label);
        let normalized = normalize_group_label_navigation(&label, &self.groups, kind);
        let mut groups = normalized.parent_groups;
        groups.push(normalized.group);
        Some(Self { groups })
    }

    /// The key [`ControlWidgets::input_index`] expects.
    #[must_use]
    pub fn key(&self) -> u64 {
        group_path_hash(&self.groups)
    }
}

/// Lists the control inputs and bargraphs of `box_tree` in interface order.
///
/// Dead widgets are included: the list is taken on the box, before signals
/// are known, so a widget the simplified program no longer reads is still a
/// control input here (the compiled interface prunes it later).
#[must_use]
pub fn control_widgets(arena: &TreeArena, box_tree: FlatBoxId) -> ControlWidgets {
    let built = build_ui_program(arena, box_tree, &PropagateUiOptions::default());
    let program = &built.program;
    let mut order = Vec::new();
    let root_is_group = program.root_origin == UiRootOrigin::Explicit;
    collect_leaves(
        program,
        program.root,
        root_is_group,
        &mut Vec::new(),
        &mut order,
    );

    let mut widgets = ControlWidgets::new();
    let mut input_of_control = AHashMap::new();
    for (id, path) in order {
        let spec = &program.controls[id as usize];
        let Some(widget) = spec.source_node else {
            continue;
        };
        let entry = ControlWidget {
            widget,
            kind: spec.kind,
            range: spec.range,
            path,
        };
        match spec.kind {
            ControlKind::VBargraph | ControlKind::HBargraph => widgets.outputs.push(entry),
            ControlKind::Soundfile => {}
            _ => {
                input_of_control.insert(id, widgets.inputs.len());
                widgets.inputs.push(entry);
            }
        }
    }
    for (&key, id) in &built.control_ids {
        if let Some(&index) = input_of_control.get(id) {
            widgets.input_index.insert(key, index);
        }
    }
    widgets
}

/// Collects the control leaves under `node` in layout order, each with its
/// path (group labels, then the control's label).
fn collect_leaves(
    program: &UiProgram,
    node: UiId,
    label_is_segment: bool,
    groups: &mut Vec<String>,
    out: &mut Vec<(ControlId, Vec<String>)>,
) {
    match match_ui(&program.arena, node) {
        UiMatch::Group {
            label, children, ..
        } => {
            if label_is_segment {
                groups.push(label.to_owned());
            }
            for child in children {
                collect_leaves(program, child, true, groups, out);
            }
            if label_is_segment {
                groups.pop();
            }
        }
        UiMatch::InputControl(id) | UiMatch::OutputControl(id) => {
            let mut path = groups.clone();
            path.push(program.controls[id as usize].label.clone());
            out.push((id, path));
        }
        UiMatch::Soundfile(_) | UiMatch::Unknown => {}
    }
}
