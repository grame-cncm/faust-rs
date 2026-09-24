//! Modulation circuit evaluation and widget rewriting.
//!
//! Implements the Faust `modulate(target, circuit, body)` form:
//! - `eval_modulation` — evaluates target label and optional modulation circuit,
//!   then implants the circuit around matching widgets in the fully-evaluated body;
//! - `eval_modulation_label` — evaluates the label argument of a modulation node;
//! - `eval_modulation_circuit` — evaluates and arity-checks the circuit argument;
//! - `implant_modulation` / `implant_widget_if_match` — tree-walking rewriters
//!   that splice the circuit around every widget whose path matches the target;
//! - `widget_matches` / `modulation_target_path` — path-matching predicates;
//! - `eval_wildcard_modulation` / `implant_wildcard` — the faust-rs wildcard
//!   target `"*"` (`"group/*"`): every control input matched, one extra input
//!   per control, in interface order.
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
/// Targets are matched on the widgets' labels and groups as the reference
/// does (metadata removed, a label's own `h:sub/x` path decoded, group prefixes
/// matched in order); a target that matches nothing keeps the body, keeps the
/// dangling slot of a two-input modulator, and records
/// [`EvalWarning::ModulationNoMatch`]. Verified against faust 2.88.1 on the
/// `tests/corpus/modulation_*.dsp` fixtures (samples, arity, interface).
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

    if let Some(group_path) = wildcard_group_path(&target_path) {
        return eval_wildcard_modulation(
            arena,
            modulation_node,
            &target_label,
            group_path,
            &WildcardCircuit {
                inputs,
                circuit: modulation_circuit,
            },
            body,
            env,
            loop_detector,
        );
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
        // No widget matched. The body is kept as it is; a two-input modulator
        // still gets its extra input, dangling, as the reference compiler
        // always wraps the slot (`boxSymbolic(slot, mbody)` in `eval.cpp`), so
        // the arity of the program does not depend on the match.
        loop_detector.warnings.push(EvalWarning::ModulationNoMatch {
            node: modulation_node,
            target: target_label,
        });
    }
    match slot {
        Some(slot) => {
            let mut b = BoxBuilder::new(arena);
            Ok(b.symbolic(slot, rewritten))
        }
        None => Ok(rewritten),
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
    Ok(strip_label_metadata(&label))
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
/// The widget's path is its own label's segments (the label, then the groups
/// the label opens, innermost first: [`widget_label_path_segments`]) followed
/// by the enclosing groups, innermost first, every segment without metadata.
/// The target's segments (name first) must appear in it in order, not
/// necessarily adjacent: `"a/x"` matches `x` under `hgroup("a", vgroup("b", ...))`.
///
/// This is the reference algorithm read as one relation: C++ strips each
/// target group as the traversal enters the matching group (`matchGroup`,
/// outermost first, unrelated groups skipped) and asks at the widget whether
/// what remains is a subsequence of the label's own path
/// (`isPathMatchingLabel`); the two agree on every path. A target that names
/// only a group matches every widget under it, in both compilers.
pub(crate) fn widget_matches_modulation_target(
    arena: &TreeArena,
    label: TreeId,
    target_path: &[String],
    group_stack: &[String],
) -> bool {
    let Some(label) = label_node_text(arena, label) else {
        return false;
    };
    let mut widget_path = widget_label_path_segments(label);
    widget_path.reserve(group_stack.len());
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
        .rev()
        .collect()
}

/// The group prefix of a wildcard target, innermost group first, or `None`
/// when the target is a literal label.
///
/// A target is a wildcard when its last segment is `*` (`target_path` holds
/// the segments name first): `"*"` gives an empty prefix, `"amp/*"` gives
/// `["amp"]`. `*` is a whole segment; `"stage*"` is a literal label.
fn wildcard_group_path(target_path: &[String]) -> Option<&[String]> {
    match target_path.split_first() {
        Some((name, groups)) if name == "*" => Some(groups),
        _ => None,
    }
}

/// The modulation circuit of a wildcard modulation and its input count.
struct WildcardCircuit {
    inputs: usize,
    circuit: TreeId,
}

/// Evaluates a modulation whose target is a wildcard (`"*"`, `"group/*"`).
///
/// faust-rs extension, no C++ equivalent (the reference parses the label and
/// matches nothing). Contract in
/// `porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`
/// section 3.3:
///
/// - the controls are those of `cinputs(body)`, in the same interface order
///   ([`controls_of_lowered`]); bargraphs are never matched;
/// - a control matches when the group prefix of the target is a subsequence of
///   its group path, innermost first (the literal rule, the `*` standing for
///   the control's own label);
/// - a two-input circuit gets one fresh slot **per matched control**, where a
///   literal target shares one slot between its matches, and the slots become
///   the first inputs of the result in interface order: the i-th extra input
///   drives the i-th matched control;
/// - a target that matches no control is an error
///   ([`EvalError::ModulationWildcardNoMatch`]).
#[allow(clippy::too_many_arguments)]
fn eval_wildcard_modulation(
    arena: &mut TreeArena,
    modulation_node: TreeId,
    target_label: &str,
    group_path: &[String],
    circuit: &WildcardCircuit,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let evaluated_body = eval_box(arena, body, env, loop_detector)?;
    let lowered_body = a2sb(arena, evaluated_body, loop_detector)?;
    let widgets = controls_of_lowered(arena, lowered_body, loop_detector).ok_or(
        EvalError::InvalidControlListOperand {
            node: modulation_node,
            primitive: "a wildcard modulation",
        },
    )?;
    let mut slots: Vec<Option<TreeId>> = Vec::with_capacity(widgets.inputs.len());
    for entry in &widgets.inputs {
        let (_label, groups) = entry
            .path
            .split_last()
            .expect("a control path ends with the control's label");
        let innermost_first: Vec<String> = groups.iter().rev().cloned().collect();
        let matched = is_subsequence(group_path, &innermost_first);
        // A zero- or one-input circuit adds no input: the slot is only a mark.
        slots.push(matched.then(|| {
            if circuit.inputs == 2 {
                fresh_slot(arena, loop_detector)
            } else {
                circuit.circuit
            }
        }));
    }
    if slots.iter().all(Option::is_none) {
        return Err(EvalError::ModulationWildcardNoMatch {
            node: modulation_node,
            target: target_label.to_owned(),
        });
    }
    let mut walk = WildcardWalk {
        widgets: &widgets,
        slots: &slots,
        circuit,
        memo: ahash::HashMap::with_hasher(ahash::RandomState::new()),
    };
    let mut rewritten = implant_wildcard(
        arena,
        lowered_body,
        &propagate::UiGroupContext::default(),
        &mut walk,
    );
    if circuit.inputs == 2 {
        // Last slot innermost: the first matched control is the first input.
        let mut b = BoxBuilder::new(arena);
        for slot in slots.iter().rev().flatten() {
            rewritten = b.symbolic(*slot, rewritten);
        }
    }
    Ok(rewritten)
}

/// State of one wildcard rewrite.
struct WildcardWalk<'a> {
    widgets: &'a propagate::ControlWidgets,
    /// Per control input, in interface order: its slot (two-input circuit),
    /// a mark (other circuits), or `None` when it is not matched.
    slots: &'a [Option<TreeId>],
    circuit: &'a WildcardCircuit,
    /// Rewritten box per `(box, group context)`: the lowered body is a DAG.
    memo: ahash::HashMap<(TreeId, u64), TreeId>,
}

/// Rewrites every occurrence of a matched control input under `expr`.
///
/// The walk keeps the group context the UI builder keys controls with
/// ([`propagate::UiGroupContext`]), so an occurrence is resolved to the same
/// control `cinputs` lists; like the UI builder it restarts from the root
/// context at the seeds of `fad` and `rad`, whose widgets alias the body's.
fn implant_wildcard(
    arena: &mut TreeArena,
    expr: TreeId,
    context: &propagate::UiGroupContext,
    walk: &mut WildcardWalk<'_>,
) -> TreeId {
    let key = (expr, context.key());
    if let Some(&done) = walk.memo.get(&key) {
        return done;
    }
    let rewritten = match match_box(arena, expr) {
        BoxMatch::Button(_)
        | BoxMatch::Checkbox(_)
        | BoxMatch::VSlider(..)
        | BoxMatch::HSlider(..)
        | BoxMatch::NumEntry(..) => {
            let slot = walk
                .widgets
                .input_index(expr, context.key())
                .and_then(|index| walk.slots[index]);
            match slot {
                None => expr,
                Some(slot) => {
                    let mut b = BoxBuilder::new(arena);
                    match walk.circuit.inputs {
                        0 => walk.circuit.circuit,
                        1 => b.seq(expr, walk.circuit.circuit),
                        _ => {
                            let pair = b.par(expr, slot);
                            b.seq(pair, walk.circuit.circuit)
                        }
                    }
                }
            }
        }
        BoxMatch::VBargraph(..) | BoxMatch::HBargraph(..) => expr,
        BoxMatch::VGroup(label, inner) => {
            let inner = implant_wildcard_group(arena, expr, inner, context, walk);
            BoxBuilder::new(arena).vgroup(label, inner)
        }
        BoxMatch::HGroup(label, inner) => {
            let inner = implant_wildcard_group(arena, expr, inner, context, walk);
            BoxBuilder::new(arena).hgroup(label, inner)
        }
        BoxMatch::TGroup(label, inner) => {
            let inner = implant_wildcard_group(arena, expr, inner, context, walk);
            BoxBuilder::new(arena).tgroup(label, inner)
        }
        BoxMatch::ForwardAD(body, seed) => {
            let body = implant_wildcard(arena, body, context, walk);
            let seed = implant_wildcard(arena, seed, &propagate::UiGroupContext::default(), walk);
            BoxBuilder::new(arena).forward_ad(body, seed)
        }
        BoxMatch::ReverseAD(body, seeds) => {
            let body = implant_wildcard(arena, body, context, walk);
            let seeds = implant_wildcard(arena, seeds, &propagate::UiGroupContext::default(), walk);
            BoxBuilder::new(arena).reverse_ad(body, seeds)
        }
        _ => match arena.node(expr).cloned() {
            Some(node) if !node.children.is_empty() => {
                let mut rebuilt = Vec::with_capacity(node.children.len());
                let mut changed = false;
                for child in node.children.as_slice().iter().copied() {
                    let child_rewritten = implant_wildcard(arena, child, context, walk);
                    changed |= child_rewritten != child;
                    rebuilt.push(child_rewritten);
                }
                if changed {
                    arena.intern(node.kind, &rebuilt)
                } else {
                    expr
                }
            }
            _ => expr,
        },
    };
    walk.memo.insert(key, rewritten);
    rewritten
}

/// Rewrites the body of the group box `group` in the context it opens.
fn implant_wildcard_group(
    arena: &mut TreeArena,
    group: TreeId,
    inner: TreeId,
    context: &propagate::UiGroupContext,
    walk: &mut WildcardWalk<'_>,
) -> TreeId {
    let inside = context
        .enter(arena, group)
        .expect("a group box enters a group context");
    implant_wildcard(arena, inner, &inside, walk)
}
