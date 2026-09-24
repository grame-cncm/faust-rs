//! `cinputs(e)`, `cinput(i, e)`, `coutputs(e)`, `coutput(i, e)`: a program's
//! controls as box lists.
//!
//! faust-rs extension, no C++ equivalent. Contract in
//! `porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`
//! section 3; syntax in `docs/control-inputs-en.md`.
//!
//! Each arm follows `inputs(e)` (`BoxMatch::Inputs` in `lib.rs`): `e` is
//! evaluated and lowered by [`a2sb`], then its widgets are listed by
//! [`propagate::control_widgets`], in the order of the interface the program
//! would show (groups merged by label, children sorted by raw label). Dead
//! widgets are listed, as `inputs(_ : !)` counts a cut input: the list is the
//! control inputs present in the box, not the pruned compiled interface. A widget
//! box reached under several group paths is one control per path, as in the
//! interface. The result is folded at evaluation:
//!
//! - `cinputs(e)`: the `par` of the N widget boxes, the same nodes as in `e`
//!   (`0 : !`, no input and no output, when N = 0), so that
//!   `outputs(cinputs(e))` is N;
//! - `cinput(i, e)`: `(widget, init, min, max, step)`, the four numbers the
//!   widget's own evaluated parameters, `0, 0, 1, 1` for a button or checkbox;
//! - `coutputs(e)`, `coutput(i, e)`: the same for bargraphs, the tuple being
//!   `(bargraph, min, max)`. A bargraph box has one input and one output.

use std::sync::Arc;

use propagate::{ControlWidgets, control_widgets};

use super::*;

/// Which list a primitive reads.
#[derive(Clone, Copy)]
pub(crate) enum ControlList {
    Inputs,
    Outputs,
}

/// Evaluates `cinputs(inner)` or `coutputs(inner)` to the `par` of the widget
/// boxes, in interface order.
pub(crate) fn eval_control_list(
    arena: &mut TreeArena,
    node: TreeId,
    inner: TreeId,
    list: ControlList,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let primitive = match list {
        ControlList::Inputs => "cinputs",
        ControlList::Outputs => "coutputs",
    };
    let widgets = lowered_control_widgets(arena, node, inner, primitive, env, loop_detector)?.1;
    let boxes: Vec<TreeId> = entries(&widgets, list)
        .iter()
        .map(|entry| entry.widget)
        .collect();
    let mut b = BoxBuilder::new(arena);
    if boxes.is_empty() {
        let zero = b.int(0);
        let cut = b.cut();
        return Ok(b.seq(zero, cut));
    }
    Ok(par_list(&mut b, &boxes))
}

/// Evaluates `cinput(index, inner)` or `coutput(index, inner)` to the tuple of
/// one control.
pub(crate) fn eval_control_entry(
    arena: &mut TreeArena,
    node: TreeId,
    index: TreeId,
    inner: TreeId,
    list: ControlList,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let primitive = match list {
        ControlList::Inputs => "cinput",
        ControlList::Outputs => "coutput",
    };
    let index_value = eval_control_index(arena, node, index, inner, primitive, env, loop_detector)?;
    let widgets = lowered_control_widgets(arena, node, inner, primitive, env, loop_detector)?.1;
    let list_entries = entries(&widgets, list);
    let entry = usize::try_from(index_value)
        .ok()
        .and_then(|i| list_entries.get(i))
        .ok_or(EvalError::ControlIndexOutOfRange {
            node,
            primitive,
            index: i64::from(index_value),
            count: list_entries.len(),
        })?;
    let widget = entry.widget;
    let values: Vec<TreeId> = match match_box(arena, widget) {
        BoxMatch::HSlider(_, init, min, max, step)
        | BoxMatch::VSlider(_, init, min, max, step)
        | BoxMatch::NumEntry(_, init, min, max, step) => vec![widget, init, min, max, step],
        BoxMatch::Button(_) | BoxMatch::Checkbox(_) => {
            let mut b = BoxBuilder::new(arena);
            let (zero, one) = (b.int(0), b.int(1));
            vec![widget, zero, zero, one, one]
        }
        BoxMatch::HBargraph(_, min, max) | BoxMatch::VBargraph(_, min, max) => {
            vec![widget, min, max]
        }
        _ => {
            return Err(EvalError::InternalError {
                message: format!("{primitive}: control list entry is not a widget box"),
            });
        }
    };
    Ok(par_list(&mut BoxBuilder::new(arena), &values))
}

/// Evaluates the index of `cinput(index, inner)` to a compile-time integer.
///
/// An index that is not one is [`EvalError::ControlIndexNotConstant`], named
/// by its source text. When `inner` is itself a constant the arguments are
/// likely swapped (`cinput(freq, 0)` for `cinput(0, freq)`), and the error
/// says so.
fn eval_control_index(
    arena: &mut TreeArena,
    node: TreeId,
    index: TreeId,
    inner: TreeId,
    primitive: &'static str,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<i32, EvalError> {
    let value = eval_box(arena, index, env, loop_detector)?;
    match eval_box_to_i32(arena, value) {
        Ok(i) => Ok(i),
        // an index that divides by zero is that error, as for `par` counts
        Err(division @ EvalError::DivisionByZero { .. }) => Err(division),
        Err(_) => {
            let swapped = eval_box(arena, inner, env, loop_detector)
                .ok()
                .is_some_and(|value| eval_box_to_i32(arena, value).is_ok());
            Err(EvalError::ControlIndexNotConstant {
                node,
                primitive,
                index: source_text(arena, index),
                expression: swapped.then(|| source_text(arena, inner)),
            })
        }
    }
}

/// An unevaluated argument as the user wrote it, shortened for a message.
fn source_text(arena: &TreeArena, node: TreeId) -> String {
    const MAX_CHARS: usize = 60;
    let text = boxes::box_pp(arena, node, 0, boxes::FloatSize::Single)
        .unwrap_or_else(|_| "the index".to_owned());
    if text.chars().count() <= MAX_CHARS {
        return text;
    }
    let head: String = text.chars().take(MAX_CHARS - 1).collect();
    format!("{head}…")
}

/// Evaluates and lowers `inner`, then lists its controls (cached per lowered
/// box). Returns the lowered box with the list.
pub(crate) fn lowered_control_widgets(
    arena: &mut TreeArena,
    node: TreeId,
    inner: TreeId,
    primitive: &'static str,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<(TreeId, Arc<ControlWidgets>), EvalError> {
    let value = eval_box(arena, inner, env, loop_detector)?;
    let lowered = a2sb(arena, value, loop_detector)?;
    let widgets = controls_of_lowered(arena, lowered, loop_detector)
        .ok_or(EvalError::InvalidControlListOperand { node, primitive })?;
    Ok((lowered, widgets))
}

/// The controls of an already lowered box, or `None` when it is not a block
/// diagram propagation accepts.
pub(crate) fn controls_of_lowered(
    arena: &TreeArena,
    lowered: TreeId,
    loop_detector: &mut LoopDetector,
) -> Option<Arc<ControlWidgets>> {
    if let Some(cached) = loop_detector.control_widgets_cache.get(&lowered) {
        return Some(Arc::clone(cached));
    }
    let flat = try_build_flat_box(arena, lowered).ok()?;
    let widgets = Arc::new(control_widgets(arena, flat));
    loop_detector
        .control_widgets_cache
        .insert(lowered, Arc::clone(&widgets));
    Some(widgets)
}

fn entries(widgets: &ControlWidgets, list: ControlList) -> &[propagate::ControlWidget] {
    match list {
        ControlList::Inputs => &widgets.inputs,
        ControlList::Outputs => &widgets.outputs,
    }
}

/// `(a, b, c)` as the parser builds it: `par` is right-associative,
/// `a, (b, c)`.
fn par_list(b: &mut BoxBuilder<'_>, boxes: &[TreeId]) -> TreeId {
    let (&last, rest) = boxes.split_last().expect("non-empty box list");
    rest.iter().rev().fold(last, |acc, &prev| b.par(prev, acc))
}
