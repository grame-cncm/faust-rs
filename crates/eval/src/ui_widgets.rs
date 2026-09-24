//! UI widget evaluation — sliders, buttons, groups, bargraphs, soundfile.
//!
//! Each public function evaluates the label and parameters of one Faust widget
//! constructor node and rebuilds it with the evaluated values, mirroring the
//! C++ `evalBoxWidget(...)` / `evalBoxGroup(...)` family in
//! `compiler/evaluate/eval.cpp`.
//!
//! Slider-like widgets (`vslider`, `hslider`, `numentry`) share a common
//! `eval_slider_like` helper that validates the four numeric parameters
//! (init, min, max, step) and calls `simplify_slider_param` on each.

use super::*;

/// Evaluates one label node and re-interns the resulting string literal in the arena.
///
/// Widget/group constructors in box IR still store labels as tree nodes, so the
/// string returned by [`eval_label_node`] must be converted back into a canonical
/// literal node before rebuilding the enclosing widget.
pub(crate) fn evaluated_label_node(
    arena: &mut TreeArena,
    label: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let text = eval_label_node(arena, label, env, loop_detector)?;
    Ok(arena.string_lit(&text))
}

/// Evaluates one `button` label and rebuilds the widget node.
pub(crate) fn eval_button(
    arena: &mut TreeArena,
    label: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    Ok(BoxBuilder::new(arena).button(label))
}

/// Evaluates one `checkbox` label and rebuilds the widget node.
pub(crate) fn eval_checkbox(
    arena: &mut TreeArena,
    label: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    Ok(BoxBuilder::new(arena).checkbox(label))
}

/// Evaluates one `vslider` widget node, simplifying label and numeric params.
pub(crate) fn eval_vslider(
    arena: &mut TreeArena,
    label: TreeId,
    params: [TreeId; 4],
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    eval_slider_like(
        arena,
        SliderKind::VSlider,
        label,
        params,
        env,
        loop_detector,
    )
}

/// Evaluates one `hslider` widget node, simplifying label and numeric params.
pub(crate) fn eval_hslider(
    arena: &mut TreeArena,
    label: TreeId,
    params: [TreeId; 4],
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    eval_slider_like(
        arena,
        SliderKind::HSlider,
        label,
        params,
        env,
        loop_detector,
    )
}

/// Evaluates one `nentry` (numeric entry) widget node, simplifying label and numeric params.
pub(crate) fn eval_num_entry(
    arena: &mut TreeArena,
    label: TreeId,
    params: [TreeId; 4],
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    eval_slider_like(
        arena,
        SliderKind::NumEntry,
        label,
        params,
        env,
        loop_detector,
    )
}

enum SliderKind {
    VSlider,
    HSlider,
    NumEntry,
}

/// Builds a slider-family UI box (`vslider` / `hslider` / `nentry`).
///
/// Shared by the three public evaluators: it evaluates the `label` node and
/// reduces each of the four numeric parameters (current, min, max, step) to a
/// constant via [`simplify_slider_param`], then constructs the box for the
/// requested [`SliderKind`]. Mirrors the per-kind `eval2double` reduction in C++
/// `eval.cpp`.
fn eval_slider_like(
    arena: &mut TreeArena,
    kind: SliderKind,
    label: TreeId,
    params: [TreeId; 4],
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    // C++ eval.cpp: each numeric parameter is reduced via eval2double(…)
    // which calls boxPropagateSig + simplify internally.  We do the same by
    // calling eval_box then simplifying the result to a boxReal literal when
    // possible, matching C++ `tree(eval2double(param, …))`.
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    let kind_name = match kind {
        SliderKind::VSlider => "vslider",
        SliderKind::HSlider => "hslider",
        SliderKind::NumEntry => "nentry",
    };
    let [cur, min, max, step] = params;
    let widget = |parameter| WidgetParameter {
        widget: kind_name,
        label,
        parameter,
    };
    let cur = simplify_slider_param(arena, cur, widget("init"), env, loop_detector)?;
    let min = simplify_slider_param(arena, min, widget("min"), env, loop_detector)?;
    let max = simplify_slider_param(arena, max, widget("max"), env, loop_detector)?;
    let step = simplify_slider_param(arena, step, widget("step"), env, loop_detector)?;

    // Mirror C++ `checkRange`: if all three are known constants, verify init ∈ [min, max].
    let as_f64 = |node| match match_box(arena, node) {
        BoxMatch::Real(x) => Some(x),
        BoxMatch::Int(i) => Some(f64::from(i)),
        _ => None,
    };
    if let (Some(init_val), Some(min_val), Some(max_val)) = (as_f64(cur), as_f64(min), as_f64(max))
        && (init_val < min_val || init_val > max_val)
    {
        let label_text = label_node_text(arena, label).unwrap_or("").to_owned();
        return Err(EvalError::SliderInitOutOfRange {
            kind: kind_name,
            label: label_text,
            init_bits: init_val.to_bits(),
            min_bits: min_val.to_bits(),
            max_bits: max_val.to_bits(),
        });
    }

    let mut b = BoxBuilder::new(arena);
    Ok(match kind {
        SliderKind::VSlider => b.vslider(label, cur, min, max, step),
        SliderKind::HSlider => b.hslider(label, cur, min, max, step),
        SliderKind::NumEntry => b.num_entry(label, cur, min, max, step),
    })
}

/// The widget parameter [`simplify_slider_param`] evaluates, for its error.
#[derive(Clone, Copy)]
pub(crate) struct WidgetParameter {
    widget: &'static str,
    /// The evaluated label node.
    label: TreeId,
    parameter: &'static str,
}

/// Evaluates a slider/bargraph numeric parameter with the same semantics as
/// C++ `eval2double`: `eval_box` followed by `propagate + simplify → boxReal`.
///
/// A parameter that does not reduce to a number is an error, as in C++:
/// [`EvalError::WidgetParameterNotConstant`], carrying the arity of the
/// evaluated box so that the message tells the two C++ refusals apart (a box
/// that is not `0→1`, `not a constant expression of type : (0->1)`, and a `0→1`
/// box whose signal is not a number, `the parameter must be a real constant
/// numerical expression`). Before 2026-09-24 the evaluated box was kept and
/// the UI builder read it as 0: `0.5 : \(x).(hslider("a", x, 0, 1, 0.1))`
/// compiled with an init of 0.
///
/// # C++ equivalent
///
/// `tree(eval2double(param, visited, localValEnv))` for slider/bargraph params
/// in `compiler/evaluate/eval.cpp`, whose `tree2double` (`tlib/tree.cpp`)
/// raises the second refusal.
pub(crate) fn simplify_slider_param(
    arena: &mut TreeArena,
    param: TreeId,
    widget: WidgetParameter,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let evaled = eval_box(arena, param, env, loop_detector)?;
    match eval_box_to_f64(arena, evaled) {
        Ok(x) => Ok(BoxBuilder::new(arena).real(x)),
        Err(division @ EvalError::DivisionByZero { .. }) => Err(division),
        Err(_) => Err(EvalError::WidgetParameterNotConstant {
            node: param,
            widget: widget.widget,
            label: label_node_text(arena, widget.label)
                .unwrap_or("")
                .to_owned(),
            parameter: widget.parameter,
            expression: boxes::box_pp(arena, param, 0, boxes::FloatSize::Single)
                .unwrap_or_else(|_| "the parameter".to_owned()),
            arity: infer_box_arity_for_apply(arena, evaled, loop_detector),
        }),
    }
}

/// Evaluates one `soundfile` widget.
///
/// Only label interpolation and channel expression evaluation happen here. Full
/// runtime/path semantics are still handled later in `propagate`, just like in
/// the C++ split between evaluation and box-to-signal lowering.
pub(crate) fn eval_soundfile(
    arena: &mut TreeArena,
    label: TreeId,
    chan: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    // C++ eval.cpp: `tree(eval2int(chan, visited, localValEnv))`.
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    let evaled_chan = eval_box(arena, chan, env, loop_detector)?;
    let chan = if let Ok(n) = eval_box_to_i32(arena, evaled_chan) {
        BoxBuilder::new(arena).int(n)
    } else {
        evaled_chan
    };
    Ok(BoxBuilder::new(arena).soundfile(label, chan))
}

/// Evaluates one vertical UI group by interpolating its label and body.
pub(crate) fn eval_vgroup(
    arena: &mut TreeArena,
    label: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    let body = eval_box(arena, body, env, loop_detector)?;
    Ok(BoxBuilder::new(arena).vgroup(label, body))
}

/// Evaluates one horizontal UI group by interpolating its label and body.
pub(crate) fn eval_hgroup(
    arena: &mut TreeArena,
    label: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    let body = eval_box(arena, body, env, loop_detector)?;
    Ok(BoxBuilder::new(arena).hgroup(label, body))
}

/// Evaluates one tab UI group by interpolating its label and body.
pub(crate) fn eval_tgroup(
    arena: &mut TreeArena,
    label: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    let body = eval_box(arena, body, env, loop_detector)?;
    Ok(BoxBuilder::new(arena).tgroup(label, body))
}

/// Evaluates one vertical bargraph node.
pub(crate) fn eval_vbargraph(
    arena: &mut TreeArena,
    label: TreeId,
    min: TreeId,
    max: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    // C++ uses eval2double for bargraph min/max.
    let widget = |parameter| WidgetParameter {
        widget: "vbargraph",
        label,
        parameter,
    };
    let min = simplify_slider_param(arena, min, widget("min"), env, loop_detector)?;
    let max = simplify_slider_param(arena, max, widget("max"), env, loop_detector)?;
    Ok(BoxBuilder::new(arena).vbargraph(label, min, max))
}

/// Evaluates one horizontal bargraph node.
pub(crate) fn eval_hbargraph(
    arena: &mut TreeArena,
    label: TreeId,
    min: TreeId,
    max: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let label = evaluated_label_node(arena, label, env, loop_detector)?;
    // C++ uses eval2double for bargraph min/max.
    let widget = |parameter| WidgetParameter {
        widget: "hbargraph",
        label,
        parameter,
    };
    let min = simplify_slider_param(arena, min, widget("min"), env, loop_detector)?;
    let max = simplify_slider_param(arena, max, widget("max"), env, loop_detector)?;
    Ok(BoxBuilder::new(arena).hbargraph(label, min, max))
}
