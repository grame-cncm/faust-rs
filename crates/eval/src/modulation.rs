//! Modulation circuit evaluation and widget rewriting.
//!
//! Implements the Faust `modulate(target, circuit, body)` form:
//! - `eval_modulation` — evaluates target label and optional modulation circuit,
//!   then implants the circuit around matching widgets in the fully-evaluated body;
//! - `eval_modulation_label` — evaluates the label argument of a modulation node;
//! - `eval_modulation_circuit` — evaluates and arity-checks the circuit argument;
//! - `implant_modulation` / `implant_widget_if_match` — tree-walking rewriters
//!   that splice the circuit around every widget whose path matches the target;
//! - `widget_matches` / `modulation_target_path` — path-matching predicates.
//!
//! Source provenance (C++): `compiler/evaluate/eval.cpp` modulation branch +
//! `compiler/transform/boxModulationImplanter.cpp`.

use super::*;

/// Evaluates one modulation form and rewrites matching widgets in the body.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp` modulation branch
/// - `compiler/transform/boxModulationImplanter.cpp`
///
/// This is an adapted Rust port of the same semantics:
/// - evaluate the target label and optional modulation circuit,
/// - validate modulation-circuit arity,
/// - fully evaluate the body and lower residual closures with [`a2sb`],
/// - implant the circuit around widgets whose path matches the target.
///
/// The current implementation supports literal/group-path matching, which is
/// sufficient for the production corpus and the parity fixtures in this
/// repository.
///
/// One important adaptation from C++ is that Rust performs the full rewrite on
/// the already-evaluated and `a2sb`-lowered body. This keeps `propagate` free of
/// residual closures while still preserving the observable modulation behavior.
pub(crate) fn eval_modulation(
    arena: &mut TreeArena,
    modulation_node: TreeId,
    var: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let target_label = eval_modulation_label(arena, var, env, loop_detector)?;
    let target_path = modulation_target_path(&target_label);
    let modulation_circuit =
        eval_modulation_circuit(arena, modulation_node, var, env, loop_detector)?;
    let Some((inputs, outputs)) = infer_box_arity_cached(
        arena,
        modulation_circuit,
        &mut loop_detector.box_arity_cache,
    ) else {
        return Err(EvalError::InvalidModulationCircuit {
            node: modulation_node,
            reason: "circuit should evaluate to a block diagram",
        });
    };
    if inputs > 2 {
        return Err(EvalError::InvalidModulationCircuit {
            node: modulation_node,
            reason: "circuit should have no more than 2 inputs",
        });
    }
    if outputs != 1 {
        return Err(EvalError::InvalidModulationCircuit {
            node: modulation_node,
            reason: "circuit should have exactly 1 output",
        });
    }

    let slot = if inputs == 2 {
        Some(fresh_slot(arena, loop_detector))
    } else {
        None
    };
    let evaluated_body = eval_box(arena, body, env, loop_detector)?;
    let lowered_body = a2sb(arena, evaluated_body, loop_detector)?;
    let rewritten = implant_modulation(
        arena,
        lowered_body,
        &ModulationRewrite {
            target_path: &target_path,
            slot,
            inputs_number: inputs,
            modulation_circuit,
        },
        &mut Vec::new(),
    );

    if rewritten == lowered_body {
        Ok(lowered_body)
    } else if let Some(slot) = slot {
        let mut b = BoxBuilder::new(arena);
        Ok(b.symbolic(slot, rewritten))
    } else {
        Ok(rewritten)
    }
}

/// Immutable modulation rewrite context derived from one evaluated modulation node.
///
/// Grouping these fields keeps the recursive transformer signatures short and
/// makes the C++-parallel invariants explicit at the call site.
struct ModulationRewrite<'a> {
    target_path: &'a [String],
    slot: Option<TreeId>,
    inputs_number: usize,
    modulation_circuit: TreeId,
}

/// Evaluates the modulation target to a plain label string.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp`
/// - `evalLabel(...)`
///
/// C++ accepts richer label syntax than plain string literals. Rust currently
/// routes target labels through the same `%ident` interpolation engine used for
/// UI labels and then strips metadata wrappers so later matching operates only
/// on the path-bearing label text.
///
/// The returned string is therefore not the raw label source but the
/// post-interpolation, metadata-free target used by the modulation implanter.
pub(crate) fn eval_modulation_label(
    arena: &mut TreeArena,
    var: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<String, EvalError> {
    let label_node = arena
        .hd(var)
        .ok_or(EvalError::MalformedListNode { node: var })?;
    let label = eval_label_node(arena, label_node, env, loop_detector)?;
    Ok(strip_label_metadata(&label).to_owned())
}

/// Evaluates the optional modulation circuit, defaulting to multiplication.
///
/// Faust modulation syntax allows the circuit part to be omitted; the default is
/// multiplication. When a circuit is present, Rust evaluates it like an ordinary
/// box expression, lowers residual closures through [`a2sb`], and then checks
/// only the lightweight local arity constraints needed by modulation rewriting.
pub(crate) fn eval_modulation_circuit(
    arena: &mut TreeArena,
    modulation_node: TreeId,
    var: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let circuit = arena
        .tl(var)
        .ok_or(EvalError::MalformedListNode { node: var })?;
    if arena.is_nil(circuit) {
        let mut b = BoxBuilder::new(arena);
        return Ok(b.mul());
    }
    let evaluated = eval_box(arena, circuit, env, loop_detector)?;
    let lowered = a2sb(arena, evaluated, loop_detector)?;
    if infer_box_arity_cached(arena, lowered, &mut loop_detector.box_arity_cache).is_none() {
        return Err(EvalError::InvalidModulationCircuit {
            node: modulation_node,
            reason: "circuit should evaluate to a block diagram",
        });
    }
    Ok(lowered)
}

/// Recursively implants one modulation circuit into matching widgets.
///
/// The traversal keeps an explicit `group_stack` of already-entered UI labels so
/// widget matching can reconstruct the effective path seen by the user. Only
/// widget/group families receive modulation-specific treatment; every other node
/// is rebuilt structurally if any child changes.
fn implant_modulation(
    arena: &mut TreeArena,
    expr: TreeId,
    rewrite: &ModulationRewrite<'_>,
    group_stack: &mut Vec<String>,
) -> TreeId {
    match match_box(arena, expr) {
        BoxMatch::Button(label) | BoxMatch::Checkbox(label) => {
            implant_widget_if_match(arena, expr, label, rewrite, group_stack)
        }
        BoxMatch::VSlider(label, cur, min, max, step) => {
            let rebuilt = {
                let cur = implant_modulation(arena, cur, rewrite, group_stack);
                let min = implant_modulation(arena, min, rewrite, group_stack);
                let max = implant_modulation(arena, max, rewrite, group_stack);
                let step = implant_modulation(arena, step, rewrite, group_stack);
                let mut b = BoxBuilder::new(arena);
                b.vslider(label, cur, min, max, step)
            };
            implant_widget_if_match(arena, rebuilt, label, rewrite, group_stack)
        }
        BoxMatch::HSlider(label, cur, min, max, step) => {
            let rebuilt = {
                let cur = implant_modulation(arena, cur, rewrite, group_stack);
                let min = implant_modulation(arena, min, rewrite, group_stack);
                let max = implant_modulation(arena, max, rewrite, group_stack);
                let step = implant_modulation(arena, step, rewrite, group_stack);
                let mut b = BoxBuilder::new(arena);
                b.hslider(label, cur, min, max, step)
            };
            implant_widget_if_match(arena, rebuilt, label, rewrite, group_stack)
        }
        BoxMatch::NumEntry(label, cur, min, max, step) => {
            let rebuilt = {
                let cur = implant_modulation(arena, cur, rewrite, group_stack);
                let min = implant_modulation(arena, min, rewrite, group_stack);
                let max = implant_modulation(arena, max, rewrite, group_stack);
                let step = implant_modulation(arena, step, rewrite, group_stack);
                let mut b = BoxBuilder::new(arena);
                b.num_entry(label, cur, min, max, step)
            };
            implant_widget_if_match(arena, rebuilt, label, rewrite, group_stack)
        }
        BoxMatch::VBargraph(label, min, max) => {
            let rebuilt = {
                let min = implant_modulation(arena, min, rewrite, group_stack);
                let max = implant_modulation(arena, max, rewrite, group_stack);
                let mut b = BoxBuilder::new(arena);
                b.vbargraph(label, min, max)
            };
            implant_widget_if_match(arena, rebuilt, label, rewrite, group_stack)
        }
        BoxMatch::HBargraph(label, min, max) => {
            let rebuilt = {
                let min = implant_modulation(arena, min, rewrite, group_stack);
                let max = implant_modulation(arena, max, rewrite, group_stack);
                let mut b = BoxBuilder::new(arena);
                b.hbargraph(label, min, max)
            };
            implant_widget_if_match(arena, rebuilt, label, rewrite, group_stack)
        }
        BoxMatch::VGroup(label, inner) => {
            group_stack.push(strip_label_node(arena, label));
            let rewritten = implant_modulation(arena, inner, rewrite, group_stack);
            group_stack.pop();
            let mut b = BoxBuilder::new(arena);
            b.vgroup(label, rewritten)
        }
        BoxMatch::HGroup(label, inner) => {
            group_stack.push(strip_label_node(arena, label));
            let rewritten = implant_modulation(arena, inner, rewrite, group_stack);
            group_stack.pop();
            let mut b = BoxBuilder::new(arena);
            b.hgroup(label, rewritten)
        }
        BoxMatch::TGroup(label, inner) => {
            group_stack.push(strip_label_node(arena, label));
            let rewritten = implant_modulation(arena, inner, rewrite, group_stack);
            group_stack.pop();
            let mut b = BoxBuilder::new(arena);
            b.tgroup(label, rewritten)
        }
        _ => {
            let Some(node) = arena.node(expr).cloned() else {
                return expr;
            };
            if node.children.is_empty() {
                return expr;
            }

            let mut rebuilt = Vec::with_capacity(node.children.len());
            let mut changed = false;
            for child in node.children.as_slice().iter().copied() {
                let rewritten = implant_modulation(arena, child, rewrite, group_stack);
                if rewritten != child {
                    changed = true;
                }
                rebuilt.push(rewritten);
            }

            if changed {
                arena.intern(node.kind, &rebuilt)
            } else {
                expr
            }
        }
    }
}

/// Applies the modulation circuit around one widget when its path matches.
///
/// The three supported arities mirror the C++ implanter:
/// - 0 inputs: the modulation circuit fully replaces the widget,
/// - 1 input: the widget output is piped through the modulation circuit,
/// - 2 inputs: the widget is paired with the modulation slot/carry signal.
fn implant_widget_if_match(
    arena: &mut TreeArena,
    widget: TreeId,
    label: TreeId,
    rewrite: &ModulationRewrite<'_>,
    group_stack: &[String],
) -> TreeId {
    if !widget_matches_modulation_target(arena, label, rewrite.target_path, group_stack) {
        return widget;
    }
    let mut b = BoxBuilder::new(arena);
    match rewrite.inputs_number {
        0 => rewrite.modulation_circuit,
        1 => b.seq(widget, rewrite.modulation_circuit),
        2 => {
            let slot = rewrite.slot.expect("two-input modulation requires a slot");
            let pair = b.par(widget, slot);
            b.seq(pair, rewrite.modulation_circuit)
        }
        _ => widget,
    }
}

/// Returns `true` when the effective widget path matches the modulation target.
///
/// Matching is done on metadata-free path segments. Rust currently uses
/// subsequence matching on the normalized textual path representation, which is
/// sufficient for the active corpus and mirrors the practical C++ behavior for
/// the supported subset.
pub(crate) fn widget_matches_modulation_target(
    arena: &TreeArena,
    label: TreeId,
    target_path: &[String],
    group_stack: &[String],
) -> bool {
    let Some(label) = label_node_text(arena, label) else {
        return false;
    };
    let mut widget_path = Vec::with_capacity(group_stack.len() + 1);
    widget_path.push(strip_label_metadata(label).to_owned());
    for group in group_stack.iter().rev() {
        widget_path.push(group.clone());
    }
    is_subsequence(target_path, &widget_path)
}

/// Normalizes one modulation target label string into path segments.
///
/// Empty segments are discarded so both `a/b` and `/a//b/` normalize to the
/// same semantic path vector.
pub(crate) fn modulation_target_path(label: &str) -> Vec<String> {
    label
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(strip_label_metadata)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .rev()
        .collect()
}
