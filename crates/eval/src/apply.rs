//! Box application, arity inference, and list/par conversion.
//!
//! Implements the core function-application machinery of the Faust evaluator:
//! - `rev_eval_list` / `apply_value_list*` — evaluate and apply argument lists;
//! - `apply_list` — recursive application that peels one argument per step;
//! - `larg2par` — converts an n-tuple `par` abstraction into a multi-argument form;
//! - `concat_lists` / `nwires` — list and wire-count utilities;
//! - `infer_box_arity` / `infer_box_arity_for_apply` — box input/output arity
//!   inference used during application and route construction.

use super::*;

/// Evaluates argument list nodes and returns the reversed evaluated list.
///
/// This mirrors the C++ parser/evaluator list convention where argument lists are
/// accumulated in reverse order.
/// Evaluates one application argument list into reverse order.
///
/// Application in Faust stores arguments as a cons-list. Evaluating in reverse
/// order lets later application helpers consume the list head-first without an
/// extra full reversal step, mirroring the C++ `revEvalList(...)` contract.
pub(crate) fn rev_eval_list(
    arena: &mut TreeArena,
    mut list: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let mut result = arena.nil();
    while !arena.is_nil(list) {
        let head = arena
            .hd(list)
            .ok_or(EvalError::MalformedListNode { node: list })?;
        let value = eval_box(arena, head, env, loop_detector)?;
        result = arena.cons(value, result);
        list = arena
            .tl(list)
            .ok_or(EvalError::MalformedListNode { node: list })?;
    }
    Ok(result)
}

/// Applies an evaluated function-like box to an evaluated argument list.
///
/// Behavior summary:
/// - `abstr`: beta-like application in lexical scope.
/// - `case`: pattern-match dispatch when sufficiently applied, otherwise lowers to
///   non-closure style `seq(par(args + implicit_wires), case)` for C++ parity.
/// - other node families: C++-compatible non-closure lowering to `seq(par(args), fun)`,
///   including implicit wire insertion for partial applications.
///
/// This is the box-returning wrapper around [`apply_value_list_value`]. It is
/// used by evaluation paths that must stay in box IR even though intermediate
/// application may produce closures or pattern matchers.
pub(crate) fn apply_value_list(
    arena: &mut TreeArena,
    fun: EvalValue,
    larg: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
    call_site: Option<TreeId>,
) -> Result<TreeId, EvalError> {
    let value = apply_value_list_value(arena, fun, larg, env, loop_detector, call_site)?;
    force_value_to_box(arena, value, loop_detector)
}

/// Applies an evaluator value to zero or more arguments.
///
/// This is the host-side equivalent of the C++ `applyList(...)` family after
/// closure materialization. It handles:
/// - plain box application,
/// - abstraction beta-reduction with captured environments,
/// - partial application of closures,
/// - pattern-matcher progression for `case`.
pub(crate) fn apply_value_list_value(
    arena: &mut TreeArena,
    fun: EvalValue,
    larg: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
    call_site: Option<TreeId>,
) -> Result<EvalValue, EvalError> {
    if arena.is_nil(larg) {
        return Ok(fun);
    }

    loop_detector.enter_structural()?;
    let result = (|| match fun {
        EvalValue::Box(fun) => Ok(EvalValue::Box(apply_list(
            arena,
            fun,
            larg,
            env,
            loop_detector,
            call_site,
        )?)),
        EvalValue::Closure(closure) => match match_box(arena, closure.expr) {
            BoxMatch::Ident(_) => {
                let forced = eval_value(arena, closure.expr, &closure.env, loop_detector)?;
                apply_value_list_value(arena, forced, larg, env, loop_detector, call_site)
            }
            BoxMatch::Environment => Err(EvalError::TooManyArguments {
                node: call_site.unwrap_or(closure.expr),
                expected: 0,
                got: list_to_vec(arena, larg)?.len(),
            }),
            BoxMatch::Abstr(id, body) => {
                let param_name = ident_name(arena, id)?;
                let arg = arena
                    .hd(larg)
                    .ok_or(EvalError::MalformedListNode { node: larg })?;
                let arg = eval_value(arena, arg, &closure.env, loop_detector)?;
                let mut scoped = closure.env.push_scope();
                let sym = arena.intern_symbol(&param_name);
                scoped.bind_value(sym, arg);
                let f = eval_value(arena, body, &scoped, loop_detector)?;
                let tl = arena
                    .tl(larg)
                    .ok_or(EvalError::MalformedListNode { node: larg })?;
                apply_value_list_value(arena, f, tl, env, loop_detector, call_site)
            }
            _ => {
                let fun = force_value_to_box(arena, EvalValue::Closure(closure), loop_detector)?;
                Ok(EvalValue::Box(apply_list(
                    arena,
                    fun,
                    larg,
                    env,
                    loop_detector,
                    call_site,
                )?))
            }
        },
        EvalValue::PatternMatcher(pm) => {
            apply_pattern_matcher_value(arena, pm, larg, env, loop_detector, call_site)
        }
    })();
    loop_detector.leave_structural();
    result
}

/// Advances a partially-applied pattern matcher with one or more arguments.
///
/// The matcher keeps one per-rule environment vector. Every successful step may
/// refine those environments until a final state is reached, at which point the
/// selected RHS is evaluated under the captured rule-local environment.
pub(crate) fn apply_pattern_matcher_value(
    arena: &mut TreeArena,
    mut pm: PatternMatcherValue,
    larg: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
    call_site: Option<TreeId>,
) -> Result<EvalValue, EvalError> {
    if arena.is_nil(larg) {
        return Ok(EvalValue::PatternMatcher(pm));
    }

    loop_detector.enter_structural()?;
    let result = (|| {
        let raw_arg = arena
            .hd(larg)
            .ok_or(EvalError::MalformedListNode { node: larg })?;
        // C++ parity: case dispatch sees numeric arguments after the same
        // compile-time simplification pass used by pattern preparation. Without
        // this, selector expressions like `((l != 0) & ...) * 2` remain residual
        // box trees and only catch-all rules match.
        let arg = {
            let mut cache = ahash::HashMap::with_hasher(ahash::RandomState::new());
            box_simplification(arena, &mut cache, raw_arg)
        };
        let (new_state, _) = pattern_matcher::apply_pattern_matcher(
            arena,
            &pm.automaton,
            pm.state,
            arg,
            &mut pm.envs,
        );
        let Some(new_state) = new_state else {
            // The rejected argument is part of the attempt: it is what the
            // matcher was dispatching on when every rule died.
            let mut arguments = pm.rev_param_list.clone();
            arguments.push(arg);
            return Err(EvalError::PatternMatchFailed {
                node: pm.original_rules,
                arguments,
            });
        };
        pm.state = new_state;
        pm.rev_param_list.push(arg);
        let tl = arena
            .tl(larg)
            .ok_or(EvalError::MalformedListNode { node: larg })?;

        if !pm.automaton.final_state(pm.state) {
            return apply_value_list_value(
                arena,
                EvalValue::PatternMatcher(pm),
                tl,
                env,
                loop_detector,
                call_site,
            );
        }

        for rule_marker in &pm.automaton.states[pm.state].rules {
            if let Some(rule_env) = pm.envs[rule_marker.r].take() {
                let rhs = pm.automaton.rhs[rule_marker.r];
                let result = eval_value(arena, rhs, &rule_env, loop_detector)?;
                return apply_value_list_value(arena, result, tl, env, loop_detector, call_site);
            }
        }

        Err(EvalError::PatternMatchFailed {
            node: pm.original_rules,
            arguments: pm.rev_param_list.clone(),
        })
    })();
    loop_detector.leave_structural();
    result
}

/// Applies a first-order box expression to an argument list.
///
/// This helper implements the non-closure application rules that still exist in
/// Faust after parser lowering, including implicit wire insertion for
/// under-applied non-prefix primitives. When the callee is not directly
/// first-order, callers should use [`apply_value_list_value`] instead.
pub(crate) fn apply_list(
    arena: &mut TreeArena,
    fun: TreeId,
    larg: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
    call_site: Option<TreeId>,
) -> Result<TreeId, EvalError> {
    if arena.is_nil(larg) {
        return Ok(fun);
    }
    match match_box(arena, fun) {
        BoxMatch::Abstr(id, body) => {
            let param_name = ident_name(arena, id)?;
            let arg = arena
                .hd(larg)
                .ok_or(EvalError::MalformedListNode { node: larg })?;
            let mut scoped = env.push_scope();
            // intern_symbol: param_name is an owned String, not borrowed from arena.
            let sym = arena.intern_symbol(&param_name);
            scoped.bind(sym, arg);
            let f = eval_box(arena, body, &scoped, loop_detector)?;
            let tl = arena
                .tl(larg)
                .ok_or(EvalError::MalformedListNode { node: larg })?;
            apply_list(arena, f, tl, env, loop_detector, call_site)
        }
        BoxMatch::Case(rules) => {
            let expected = case_expected_arity(arena, rules)?;
            let got = list_to_vec(arena, larg)?.len();
            if got < expected {
                // C++ parity (`applyList` on under-applied closures): keep the case form
                // and insert implicit wires for missing arguments instead of evaluating
                // the case immediately.
                let missing = expected - got;
                let wires = nwires(arena, missing);
                let lowered_larg = concat_lists(arena, larg, wires)?;
                let args_par = larg2par(arena, lowered_larg)?;
                let mut b = BoxBuilder::new(arena);
                return Ok(b.seq(args_par, fun));
            }
            let pm = eval_case_value(arena, fun, rules, env, loop_detector)?;
            let applied = apply_value_list_value(arena, pm, larg, env, loop_detector, call_site)?;
            force_value_to_box(arena, applied, loop_detector)
        }
        BoxMatch::PatternMatcher(key_node) => {
            // Retrieve the partially-applied PM from the side-table and
            // continue matching via the standard PM application path.
            let key = match match_box(arena, key_node) {
                BoxMatch::Int(k) => k,
                _ => {
                    return Err(EvalError::InternalError {
                        message: "boxPatternMatcher key is not an integer".to_owned(),
                    });
                }
            };
            let pm = loop_detector
                .get_pm(key)
                .ok_or_else(|| EvalError::InternalError {
                    message: format!("boxPatternMatcher key {} not found in PM store", key),
                })?;
            let applied =
                apply_pattern_matcher_value(arena, pm, larg, env, loop_detector, call_site)?;
            force_value_to_box(arena, applied, loop_detector)
        }
        BoxMatch::Closure(key_node) => {
            // Retrieve the closure from the side-table and apply it via the
            // standard closure application path.
            let key = match match_box(arena, key_node) {
                BoxMatch::Int(k) => k,
                _ => {
                    return Err(EvalError::InternalError {
                        message: "boxClosure key is not an integer".to_owned(),
                    });
                }
            };
            let cv = loop_detector
                .get_closure(key)
                .ok_or_else(|| EvalError::InternalError {
                    message: format!("boxClosure key {} not found in closure store", key),
                })?;
            let applied = apply_value_list_value(
                arena,
                EvalValue::Closure(cv),
                larg,
                env,
                loop_detector,
                call_site,
            )?;
            force_value_to_box(arena, applied, loop_detector)
        }
        _ => {
            // C++ parity (`applyList`): for non-closures, insert implicit wires when
            // partially applying a function, and reject over-application.
            let maybe_fun_arity = infer_box_arity_for_apply(arena, fun, loop_detector);
            let maybe_larg_outputs = list_outputs_for_apply(arena, larg, loop_detector);
            let mut lowered_larg = larg;

            if let (Some((ins, _outs)), Some(larg_outs)) = (maybe_fun_arity, maybe_larg_outputs) {
                if larg_outs > ins {
                    return Err(EvalError::TooManyArguments {
                        node: call_site.unwrap_or(fun),
                        expected: ins,
                        got: larg_outs,
                    });
                }
                let missing = ins - larg_outs;
                if missing > 0 {
                    let wires = nwires(arena, missing);
                    lowered_larg = if larg_outs == 1 && is_binary_primitive_non_prefix(arena, fun) {
                        concat_lists(arena, wires, larg)?
                    } else {
                        concat_lists(arena, larg, wires)?
                    };
                }
            }

            let args_par = larg2par(arena, lowered_larg)?;
            let mut b = BoxBuilder::new(arena);
            Ok(b.seq(args_par, fun))
        }
    }
}

/// Converts parser-style argument list to parallel composition tree.
///
/// Example: `[a,b,c] -> par(a, par(b, c))`.
pub(crate) fn larg2par(arena: &mut TreeArena, larg: TreeId) -> Result<TreeId, EvalError> {
    if arena.is_nil(larg) {
        return Err(EvalError::EmptyArgumentList { node: larg });
    }
    let head = arena
        .hd(larg)
        .ok_or(EvalError::MalformedListNode { node: larg })?;
    let tail = arena
        .tl(larg)
        .ok_or(EvalError::MalformedListNode { node: larg })?;
    if arena.is_nil(tail) {
        Ok(head)
    } else {
        let right = larg2par(arena, tail)?;
        let mut b = BoxBuilder::new(arena);
        Ok(b.par(head, right))
    }
}

/// Concatenates two parser-style lists while preserving element order.
pub(crate) fn concat_lists(
    arena: &mut TreeArena,
    left: TreeId,
    right: TreeId,
) -> Result<TreeId, EvalError> {
    if arena.is_nil(left) {
        return Ok(right);
    }
    let head = arena
        .hd(left)
        .ok_or(EvalError::MalformedListNode { node: left })?;
    let tail = arena
        .tl(left)
        .ok_or(EvalError::MalformedListNode { node: left })?;
    let rest = concat_lists(arena, tail, right)?;
    Ok(arena.cons(head, rest))
}

/// Builds a parser-style list containing `n` wire nodes.
pub(crate) fn nwires(arena: &mut TreeArena, n: usize) -> TreeId {
    let mut out = arena.nil();
    for _ in 0..n {
        let wire = BoxBuilder::new(arena).wire();
        out = arena.cons(wire, out);
    }
    out
}

/// Computes total output arity for a list of application arguments.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp`
/// - `boxlistOutputs(...)`
///
/// C++ is intentionally permissive here. During non-closure partial
/// application, arguments have already been evaluated, but some residual
/// symbolic/recursive forms may still defeat the lightweight local arity probe.
/// In that situation `boxlistOutputs(...)` falls back to counting the argument
/// as a single output so `applyList(...)` can still insert the missing implicit
/// wire for under-applied binary primitives.
///
/// Rust needs the same fallback for parity. Without it, expressions such as
/// `*(button("play") : trigger(n))` keep the raw `arg : *` shape instead of
/// being rewritten to `(_, arg) : *`, which later fails in `propagate` with a
/// spurious `1 != 2` sequential composition mismatch.
pub(crate) fn list_outputs_for_apply(
    arena: &mut TreeArena,
    mut list: TreeId,
    loop_detector: &mut LoopDetector,
) -> Option<usize> {
    let mut total = 0usize;
    while !arena.is_nil(list) {
        let head = arena.hd(list)?;
        let outs =
            infer_box_arity_for_apply(arena, head, loop_detector).map_or(1, |(_, outs)| outs);
        total = total.checked_add(outs)?;
        list = arena.tl(list)?;
    }
    Some(total)
}

/// Infers box arity after `a2sb` lowering, with fallback to pre-lowering arity.
///
/// Unlike [`infer_box_arity`], this variant runs `a2sb` first so that residual
/// symbolic boxes (e.g. partially-applied `selectbus`) report the correct arity
/// before application attempts to match them against argument lists.
pub(crate) fn infer_box_arity_for_apply(
    arena: &mut TreeArena,
    id: TreeId,
    loop_detector: &mut LoopDetector,
) -> Option<(usize, usize)> {
    // C++ `applyList` / `boxlistOutputs` always run `a2sb(...)` before
    // `getBoxType(...)` when probing local arity for application lowering.
    // Doing the same here avoids under-counting residual symbolic boxes such as
    // partially applied `selectbus(...)`, which otherwise fall back to
    // "1 output" and trigger spurious implicit wires.
    a2sb(arena, id, loop_detector)
        .ok()
        .and_then(|lowered| infer_box_arity(arena, lowered))
        .or_else(|| infer_box_arity(arena, id))
}

/// Local arity inference used by non-closure application lowering.
///
/// Returns `(inputs, outputs)` for the subset needed in `apply_list`.
/// `None` means arity is unknown or invalid for this fast-path inference.
/// Infers `(inputs, outputs)` for the evaluator-supported first-order box subset.
///
/// This lightweight arity oracle is intentionally narrower than the dedicated
/// `propagate::box_arity_typed(...)` contract. It exists for local evaluator tasks
/// such as under-application handling and label-placeholder constant checks
/// where pulling the full propagate error surface would be unnecessarily heavy.
pub(crate) fn infer_box_arity(arena: &TreeArena, id: TreeId) -> Option<(usize, usize)> {
    match match_box(arena, id) {
        BoxMatch::Int(_) | BoxMatch::Real(_) => Some((0, 1)),
        BoxMatch::Slot(_) => Some((0, 1)),
        BoxMatch::Wire => Some((1, 1)),
        BoxMatch::Cut => Some((1, 0)),
        BoxMatch::Add
        | BoxMatch::Sub
        | BoxMatch::Mul
        | BoxMatch::Div
        | BoxMatch::Rem
        | BoxMatch::And
        | BoxMatch::Or
        | BoxMatch::Xor
        | BoxMatch::Lsh
        | BoxMatch::Rsh
        | BoxMatch::LRsh
        | BoxMatch::Lt
        | BoxMatch::Le
        | BoxMatch::Gt
        | BoxMatch::Ge
        | BoxMatch::Eq
        | BoxMatch::Ne
        | BoxMatch::Pow
        | BoxMatch::Atan2
        | BoxMatch::Fmod
        | BoxMatch::Remainder
        | BoxMatch::Delay
        | BoxMatch::Min
        | BoxMatch::Max
        | BoxMatch::Prefix
        | BoxMatch::Attach
        | BoxMatch::Enable
        | BoxMatch::Control => Some((2, 1)),
        BoxMatch::Delay1
        | BoxMatch::IntCast
        | BoxMatch::FloatCast
        | BoxMatch::Acos
        | BoxMatch::Asin
        | BoxMatch::Atan
        | BoxMatch::Cos
        | BoxMatch::Sin
        | BoxMatch::Tan
        | BoxMatch::Exp
        | BoxMatch::Exp10
        | BoxMatch::Log
        | BoxMatch::Log10
        | BoxMatch::Sqrt
        | BoxMatch::Abs
        | BoxMatch::Floor
        | BoxMatch::Ceil
        | BoxMatch::Rint
        | BoxMatch::Round
        | BoxMatch::Lowest
        | BoxMatch::Highest => Some((1, 1)),
        BoxMatch::ReadOnlyTable | BoxMatch::Select2 | BoxMatch::AssertBounds => Some((3, 1)),
        BoxMatch::Select3 => Some((4, 1)),
        BoxMatch::WriteReadTable => Some((5, 1)),
        BoxMatch::FConst(_, _, _) | BoxMatch::FVar(_, _, _) => Some((0, 1)),
        BoxMatch::Button(_)
        | BoxMatch::Checkbox(_)
        | BoxMatch::VSlider(_, _, _, _, _)
        | BoxMatch::HSlider(_, _, _, _, _)
        | BoxMatch::NumEntry(_, _, _, _, _) => Some((0, 1)),
        BoxMatch::Waveform(_) => Some((0, 2)),
        BoxMatch::VBargraph(_, _, _) | BoxMatch::HBargraph(_, _, _) => Some((1, 1)),
        BoxMatch::Soundfile(_, chan) => {
            let BoxMatch::Int(channels) = match_box(arena, chan) else {
                return None;
            };
            let channels = usize::try_from(channels).ok()?;
            Some((2, channels.checked_add(2)?))
        }
        BoxMatch::VGroup(_, inner) | BoxMatch::HGroup(_, inner) | BoxMatch::TGroup(_, inner) => {
            infer_box_arity(arena, inner)
        }
        BoxMatch::Symbolic(_, inner) => {
            let (ins, outs) = infer_box_arity(arena, inner)?;
            Some((ins.checked_add(1)?, outs))
        }
        BoxMatch::Seq(left, right) => {
            let (ins1, outs1) = infer_box_arity(arena, left)?;
            let (ins2, outs2) = infer_box_arity(arena, right)?;
            if outs1 != ins2 {
                return None;
            }
            Some((ins1, outs2))
        }
        BoxMatch::Par(left, right) => {
            let (ins1, outs1) = infer_box_arity(arena, left)?;
            let (ins2, outs2) = infer_box_arity(arena, right)?;
            Some((ins1.checked_add(ins2)?, outs1.checked_add(outs2)?))
        }
        BoxMatch::Split(left, right) => {
            let (ins1, outs1) = infer_box_arity(arena, left)?;
            let (ins2, outs2) = infer_box_arity(arena, right)?;
            if outs1 != ins2 && (outs1 == 0 || !ins2.is_multiple_of(outs1)) {
                return None;
            }
            Some((ins1, outs2))
        }
        BoxMatch::Merge(left, right) => {
            let (ins1, outs1) = infer_box_arity(arena, left)?;
            let (ins2, outs2) = infer_box_arity(arena, right)?;
            if outs1 != ins2 && (ins2 == 0 || !outs1.is_multiple_of(ins2)) {
                return None;
            }
            Some((ins1, outs2))
        }
        BoxMatch::Rec(left, right) => {
            let (ins1, outs1) = infer_box_arity(arena, left)?;
            let (ins2, outs2) = infer_box_arity(arena, right)?;
            if ins2 > outs1 || outs2 > ins1 {
                return None;
            }
            Some((ins1 - outs2, outs1))
        }
        BoxMatch::Environment => Some((0, 0)),
        BoxMatch::Route(ins, outs, _) => {
            let BoxMatch::Int(ins_n) = match_box(arena, ins) else {
                return None;
            };
            let BoxMatch::Int(outs_n) = match_box(arena, outs) else {
                return None;
            };
            let ins_n = usize::try_from(ins_n).ok()?;
            let outs_n = usize::try_from(outs_n).ok()?;
            Some((ins_n, outs_n))
        }
        BoxMatch::Inputs(_) | BoxMatch::Outputs(_) => Some((0, 1)),
        BoxMatch::ForwardAD(exp, seed) => {
            let (_, seed_outs) = infer_box_arity(arena, seed)?;
            if seed_outs != 1 {
                return None;
            }
            let (exp_ins, exp_outs) = infer_box_arity(arena, exp)?;
            let (seed_ins, _) = infer_box_arity(arena, seed)?;
            Some((exp_ins.max(seed_ins), exp_outs * 2))
        }
        BoxMatch::ReverseAD(exp, seeds) => {
            let (exp_ins, exp_outs) = infer_box_arity(arena, exp)?;
            let (seeds_ins, seeds_outs) = infer_box_arity(arena, seeds)?;
            if exp_outs == 0 || seeds_outs == 0 {
                return None;
            }
            Some((exp_ins.max(seeds_ins), exp_outs + seeds_outs))
        }
        BoxMatch::Ondemand(inner) | BoxMatch::Upsampling(inner) | BoxMatch::Downsampling(inner) => {
            let (ins, outs) = infer_box_arity(arena, inner)?;
            Some((ins.checked_add(1)?, outs))
        }
        _ => None,
    }
}

/// Returns true for primitive binary operators that are not `prefix`.
pub(crate) fn is_binary_primitive_non_prefix(arena: &TreeArena, id: TreeId) -> bool {
    matches!(
        match_box(arena, id),
        BoxMatch::Add
            | BoxMatch::Sub
            | BoxMatch::Mul
            | BoxMatch::Div
            | BoxMatch::Rem
            | BoxMatch::And
            | BoxMatch::Or
            | BoxMatch::Xor
            | BoxMatch::Lsh
            | BoxMatch::Rsh
            | BoxMatch::LRsh
            | BoxMatch::Lt
            | BoxMatch::Le
            | BoxMatch::Gt
            | BoxMatch::Ge
            | BoxMatch::Eq
            | BoxMatch::Ne
            | BoxMatch::Pow
            | BoxMatch::Atan2
            | BoxMatch::Fmod
            | BoxMatch::Remainder
            | BoxMatch::Delay
            | BoxMatch::Min
            | BoxMatch::Max
            | BoxMatch::Attach
            | BoxMatch::Enable
            | BoxMatch::Control
    )
}
