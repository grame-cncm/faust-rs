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
use crate::loop_detector::ApplyMemoKind;

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
    let result = on_deep_stack(|| match fun {
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
                let arg = arena
                    .hd(larg)
                    .ok_or(EvalError::MalformedListNode { node: larg })?;
                let f = beta_reduce(arena, &closure, id, body, arg, loop_detector)?;
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
    });
    loop_detector.leave_structural();
    result
}

/// Applies an abstraction closure to one argument: the body evaluated with the
/// parameter bound, memoized in [`LoopDetector::apply_memo`].
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp`
/// - `applyList` (memo `gApplyMemo`) / `realApplyList` (closure branch)
///
/// Mapping status: `adapted`. C++ memoizes `applyList(fun, larg)` whole; Rust
/// memoizes each beta-reduction, keyed by the abstraction, the identity of the
/// environment it closes over and the evaluated argument, which is everything
/// the reduct depends on. The rest of the argument list is then applied by the
/// caller, through this memo again when the reduct is itself an abstraction,
/// so a curried application hits step by step: a hit returns the same closure,
/// over the same layer, as the miss that stored it. The memo matters because
/// the body is evaluated in a layer of its own, which [`LoopDetector::eval_cache`]
/// has never seen: a call inside a lambda body, `f(m) : \(x).(x + f(m))`, was
/// re-evaluated at every application, `2^D` for a chain of depth `D`.
fn beta_reduce(
    arena: &mut TreeArena,
    closure: &ClosureValue,
    id: TreeId,
    body: TreeId,
    arg: TreeId,
    loop_detector: &mut LoopDetector,
) -> Result<EvalValue, EvalError> {
    let key = ApplyMemoKey {
        fun: closure.expr,
        env_key: closure.env.frame_key(),
        args: arg,
    };
    if let Some(hit) = loop_detector.apply_memo_get(&key, ApplyMemoKind::Abstraction) {
        return Ok(hit);
    }
    let param_name = ident_name(arena, id)?;
    let value = eval_value(arena, arg, &closure.env, loop_detector)?;
    let mut scoped = closure.env.push_scope();
    let sym = arena.intern_symbol(&param_name);
    scoped.bind_value(sym, value);
    let f = eval_value(arena, body, &scoped, loop_detector)?;
    loop_detector.apply_memo_insert(key, &closure.env, &f);
    Ok(f)
}

/// Advances a partially-applied pattern matcher with one or more arguments.
///
/// The matcher keeps one per-rule environment vector. Every successful step may
/// refine those environments until a final state is reached, at which point the
/// selected RHS is evaluated under the captured rule-local environment.
///
/// A fresh matcher given at least as many arguments as its rules take goes
/// through [`LoopDetector::apply_memo`] first, keyed by its `case`, the
/// environment it was evaluated in and those arguments: the C++ `gApplyMemo`
/// entry of a `boxPatternMatcher` in its initial state. That is what makes a
/// recursion with overlapping calls, `fib(n) = fib(n-1) + fib(n-2)` or
/// `ma.chebychev`, linear instead of exponential: each call evaluates its
/// rule in fresh layers, where the eval cache never hits. Arguments past the
/// rule arity are applied to the result afterwards, as in C++.
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
    if let Some(origin) = pm.fresh_env.take() {
        let arity = case_expected_arity(arena, pm.original_rules)?;
        if let Some((args, rest)) = split_arguments(arena, larg, arity)? {
            let key = ApplyMemoKey {
                fun: pm.case_expr,
                env_key: origin.frame_key(),
                args,
            };
            let selected = match loop_detector.apply_memo_get(&key, ApplyMemoKind::Case) {
                Some(hit) => hit,
                None => {
                    let selected = apply_pattern_matcher_value(
                        arena,
                        pm,
                        args,
                        env,
                        loop_detector,
                        call_site,
                    )?;
                    loop_detector.apply_memo_insert(key, &origin, &selected);
                    selected
                }
            };
            return apply_value_list_value(arena, selected, rest, env, loop_detector, call_site);
        }
    }

    loop_detector.enter_structural()?;
    let mut dispatched = false;
    let result = on_deep_stack(|| {
        let raw_arg = arena
            .hd(larg)
            .ok_or(EvalError::MalformedListNode { node: larg })?;
        // C++ parity: case dispatch sees numeric arguments after the same
        // compile-time simplification pass used by pattern preparation. Without
        // this, selector expressions like `((l != 0) & ...) * 2` remain residual
        // box trees and only catch-all rules match.
        let arg = crate::simplify::box_simplification_in(arena, raw_arg, &mut loop_detector.fold);
        let arg = pattern_matcher::simplify_dispatch_argument(
            arena,
            &pm.automaton,
            pm.state,
            arg,
            &mut loop_detector.fold,
        );
        // A dispatch on numeric patterns is recorded, so that a recursion that
        // exhausts a depth budget without ever dispatching on a number, its
        // argument going past the numeric patterns to the variable rule each
        // time, is reported as such.
        let patterns =
            pattern_matcher::unreachable_numeric_patterns(arena, &pm.automaton, pm.state, arg);
        let nonconstant = if !patterns.is_empty() {
            let mut patterns = patterns;
            // in increasing order, not the automaton's transition order
            let value = |arena: &TreeArena, p: TreeId| match match_box(arena, p) {
                BoxMatch::Int(i) => f64::from(i),
                BoxMatch::Real(x) => x,
                _ => f64::NAN,
            };
            patterns.sort_by(|a, b| value(arena, *a).total_cmp(&value(arena, *b)));
            let patterns = patterns
                .into_iter()
                .map(|p| {
                    boxes::box_pp(arena, p, 0, boxes::FloatSize::Single)
                        .unwrap_or_else(|_| "a number".to_owned())
                })
                .collect();
            Some(Some(crate::loop_detector::NonConstantArgument {
                argument: arg,
                patterns,
            }))
        } else if pattern_matcher::dispatches_on_number(arena, &pm.automaton, pm.state, arg) {
            Some(None)
        } else {
            None
        };
        if let Some(nonconstant) = nonconstant {
            loop_detector
                .numeric_dispatches
                .push(crate::loop_detector::NumericDispatch {
                    rules: pm.original_rules,
                    position: pm.rev_param_list.len() + 1,
                    nonconstant,
                });
            dispatched = true;
        }
        let (new_state, _) = pattern_matcher::apply_pattern_matcher(
            arena,
            &pm.automaton,
            pm.state,
            arg,
            &mut pm.envs,
            &mut loop_detector.fold,
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
    });
    if dispatched {
        loop_detector.numeric_dispatches.pop();
    }
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

/// Splits an argument list after its first `n` elements, `n > 0`.
///
/// Returns `None` when the list is shorter. The prefix is `larg` itself when
/// it has exactly `n` elements, so the common case allocates nothing;
/// otherwise it is rebuilt, hash-consed, from the arena.
fn split_arguments(
    arena: &mut TreeArena,
    larg: TreeId,
    n: usize,
) -> Result<Option<(TreeId, TreeId)>, EvalError> {
    let mut prefix = Vec::with_capacity(n);
    let mut rest = larg;
    while prefix.len() < n {
        if arena.is_nil(rest) {
            return Ok(None);
        }
        prefix.push(
            arena
                .hd(rest)
                .ok_or(EvalError::MalformedListNode { node: rest })?,
        );
        rest = arena
            .tl(rest)
            .ok_or(EvalError::MalformedListNode { node: rest })?;
    }
    if arena.is_nil(rest) {
        return Ok(Some((larg, rest)));
    }
    Ok(Some((vec_to_list(arena, &prefix), rest)))
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
    let lowered = a2sb(arena, id, loop_detector).ok();
    let cache = &mut loop_detector.box_arity_cache;
    lowered
        .and_then(|lowered| infer_box_arity_cached(arena, lowered, cache))
        .or_else(|| infer_box_arity_cached(arena, id, cache))
}

/// Per-box memo of [`infer_box_arity_cached`] verdicts, including `None`.
///
/// The arena is hash-consed and append-only, so a box's arity never changes
/// and the memo is exact for the life of the arena; [`LoopDetector`] owns one
/// for the whole evaluation pass.
pub(crate) type BoxArityCache = ahash::HashMap<TreeId, Option<(usize, usize)>>;

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
///
/// This entry point walks with a call-local memo, so one probe is linear in
/// the number of distinct nodes of the box DAG rather than in its number of
/// paths. Callers that own a [`LoopDetector`] should prefer
/// [`infer_box_arity_cached`] with its `box_arity_cache`, which also
/// amortizes the walk across probes.
pub(crate) fn infer_box_arity(arena: &TreeArena, id: TreeId) -> Option<(usize, usize)> {
    let mut cache = BoxArityCache::with_hasher(ahash::RandomState::new());
    infer_box_arity_cached(arena, id, &mut cache)
}

/// [`infer_box_arity`] memoized in `cache`.
///
/// The memo is what keeps the oracle affordable on shared box DAGs. Before it
/// existed (2026-09-16) the walk revisited a shared subtree once per path to
/// it, and `apply_list` probes it for every primitive application: the seed
/// `p1` of the `fad` in `optimizers.lib`'s `lsq_1D`, mentioned a dozen times
/// by an interpolated delay and carrying a K-lane initial estimate, made
/// evaluation cost about 0.2 s per lane (11 s at K = 48) where the whole
/// program compiles in a few hundred milliseconds with the memo.
pub(crate) fn infer_box_arity_cached(
    arena: &TreeArena,
    id: TreeId,
    cache: &mut BoxArityCache,
) -> Option<(usize, usize)> {
    if let Some(&hit) = cache.get(&id) {
        return hit;
    }
    let result = infer_box_arity_uncached(arena, id, cache);
    cache.insert(id, result);
    result
}

/// Recursive body of [`infer_box_arity_cached`]; children go through the memo.
fn infer_box_arity_uncached(
    arena: &TreeArena,
    id: TreeId,
    cache: &mut BoxArityCache,
) -> Option<(usize, usize)> {
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
        // C++ `getBoxType`: `ffarity(ff)` inputs, the signature's length less
        // its return type and name list, and one output. Without it `f(1)` on
        // a three-input foreign function was the bare `1 : f`, not the C++
        // `1, _, _ : f`.
        BoxMatch::FFun(ff) => {
            let BoxMatch::Ffunction(signature, _, _) = match_box(arena, ff) else {
                return None;
            };
            let inputs = list_to_vec(arena, signature).ok()?.len().checked_sub(2)?;
            Some((inputs, 1))
        }
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
            infer_box_arity_cached(arena, inner, cache)
        }
        BoxMatch::Symbolic(_, inner) => {
            let (ins, outs) = infer_box_arity_cached(arena, inner, cache)?;
            Some((ins.checked_add(1)?, outs))
        }
        BoxMatch::Seq(left, right) => {
            let (ins1, outs1) = infer_box_arity_cached(arena, left, cache)?;
            let (ins2, outs2) = infer_box_arity_cached(arena, right, cache)?;
            if outs1 != ins2 {
                return None;
            }
            Some((ins1, outs2))
        }
        BoxMatch::Par(left, right) => {
            let (ins1, outs1) = infer_box_arity_cached(arena, left, cache)?;
            let (ins2, outs2) = infer_box_arity_cached(arena, right, cache)?;
            Some((ins1.checked_add(ins2)?, outs1.checked_add(outs2)?))
        }
        BoxMatch::Split(left, right) => {
            let (ins1, outs1) = infer_box_arity_cached(arena, left, cache)?;
            let (ins2, outs2) = infer_box_arity_cached(arena, right, cache)?;
            if outs1 != ins2 && (outs1 == 0 || !ins2.is_multiple_of(outs1)) {
                return None;
            }
            Some((ins1, outs2))
        }
        BoxMatch::Merge(left, right) => {
            let (ins1, outs1) = infer_box_arity_cached(arena, left, cache)?;
            let (ins2, outs2) = infer_box_arity_cached(arena, right, cache)?;
            if outs1 != ins2 && (ins2 == 0 || !outs1.is_multiple_of(ins2)) {
                return None;
            }
            Some((ins1, outs2))
        }
        BoxMatch::Rec(left, right) => {
            let (ins1, outs1) = infer_box_arity_cached(arena, left, cache)?;
            let (ins2, outs2) = infer_box_arity_cached(arena, right, cache)?;
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
            let (seed_ins, seed_outs) = infer_box_arity_cached(arena, seed, cache)?;
            if seed_outs != 1 {
                return None;
            }
            let (exp_ins, exp_outs) = infer_box_arity_cached(arena, exp, cache)?;
            Some((exp_ins.max(seed_ins), exp_outs * 2))
        }
        BoxMatch::ReverseAD(exp, seeds) => {
            let (exp_ins, exp_outs) = infer_box_arity_cached(arena, exp, cache)?;
            let (seeds_ins, seeds_outs) = infer_box_arity_cached(arena, seeds, cache)?;
            if exp_outs == 0 || seeds_outs == 0 {
                return None;
            }
            Some((exp_ins.max(seeds_ins), exp_outs + seeds_outs))
        }
        BoxMatch::Ondemand(inner) | BoxMatch::Upsampling(inner) | BoxMatch::Downsampling(inner) => {
            let (ins, outs) = infer_box_arity_cached(arena, inner, cache)?;
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use boxes::{BoxBuilder, BoxMatch, match_box};
    use tlib::{TreeArena, TreeId};

    use super::{infer_box_arity, infer_box_arity_for_apply};
    use crate::LoopDetector;
    use crate::loop_detector::ApplyMemoKind;

    /// `levels` times `(x, x) : +` over an integer: a DAG of `4 * levels`
    /// nodes with `2^levels` root-to-leaf paths.
    fn shared_dag(arena: &mut TreeArena, levels: u32) -> TreeId {
        let mut b = BoxBuilder::new(arena);
        let mut x = b.int(1);
        for _ in 0..levels {
            let pair = b.par(x, x);
            let add = b.add();
            x = b.seq(pair, add);
        }
        x
    }

    /// Regression for the 2026-09-16 `fad` compile-time blow-up: the arity
    /// oracle must be linear in the nodes of a shared box DAG, not in its
    /// paths. At 64 levels an unmemoized walk would take 2^64 steps; the
    /// budget below is three orders of magnitude above the memoized cost of a
    /// debug build.
    #[test]
    fn infer_box_arity_is_linear_on_a_shared_dag() {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("infer-box-arity-shared-dag".to_owned())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let mut arena = TreeArena::new();
                let dag = shared_dag(&mut arena, 64);
                let plain = infer_box_arity(&arena, dag);
                let mut detector = LoopDetector::new();
                let for_apply = infer_box_arity_for_apply(&mut arena, dag, &mut detector);
                let cached_nodes = detector.box_arity_cache.len();
                let _ = sender.send((plain, for_apply, cached_nodes));
            })
            .expect("spawn arity worker");
        let (plain, for_apply, cached_nodes) = receiver
            .recv_timeout(Duration::from_secs(60))
            .expect("arity probe of a 64-level shared DAG must finish within the budget");
        assert_eq!(plain, Some((0, 1)));
        assert_eq!(for_apply, Some((0, 1)));
        assert!(
            cached_nodes >= 64,
            "the loop detector must keep one verdict per distinct node, got {cached_nodes}"
        );
    }

    /// What one evaluation of `process` left behind, for the apply memo tests.
    struct Evaluated {
        /// The lowered `process`, flattened over `par` into integer leaves;
        /// `None` for a leaf that is not an integer.
        outputs: Vec<Option<i32>>,
        probes: [u64; 2],
        hits: [u64; 2],
    }

    /// Evaluates `process` in `source` on a worker, failing the test if it does
    /// not finish within a minute: the regressions these tests guard against
    /// are exponential, and would otherwise hang the suite instead of failing.
    fn evaluate_within_budget(source: &str) -> Evaluated {
        let source = source.to_owned();
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("apply-memo".to_owned())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let parsed = parser::parse_program(&source, "apply_memo.dsp");
                assert!(parsed.errors.is_empty(), "{source}: {:?}", parsed.errors);
                let mut arena = parsed.state.arena;
                let root = parsed.root.expect("a parsed program");
                let mut env = crate::Environment::empty();
                crate::definitions::bind_definitions(&mut arena, root, &mut env)
                    .expect("definitions bind");
                let mut detector = LoopDetector::new();
                let entry = BoxBuilder::new(&mut arena).ident("process");
                let value = crate::eval_value(&mut arena, entry, &env, &mut detector)
                    .expect("process evaluates");
                let lowered =
                    crate::a2sb_value(&mut arena, value, &mut detector).expect("process lowers");
                let mut outputs = Vec::new();
                let mut stack = vec![lowered];
                while let Some(id) = stack.pop() {
                    match match_box(&arena, id) {
                        BoxMatch::Par(left, right) => {
                            stack.push(right);
                            stack.push(left);
                        }
                        BoxMatch::Int(n) => outputs.push(Some(n)),
                        _ => outputs.push(None),
                    }
                }
                let _ = sender.send(Evaluated {
                    outputs,
                    probes: detector.apply_memo_probes,
                    hits: detector.apply_memo_hits,
                });
            })
            .expect("spawn evaluation worker");
        receiver
            .recv_timeout(Duration::from_secs(60))
            .expect("the evaluation must finish within the budget")
    }

    /// `f_i(m) = f_{i-1}(m) : \(x).(x + f_{i-1}(m))`: the second call of each
    /// level is in the body of a lambda, evaluated in a layer of its own that
    /// the `(expr, env)` cache has never seen. Without the apply memo the chain
    /// costs `2^D`, here `2^48`; with it, each level is reduced once and the
    /// call in the lambda hits.
    #[test]
    fn a_call_in_a_lambda_body_is_reduced_once() {
        const D: u64 = 48;
        let mut source = String::from("f0(m) = m;\n");
        for i in 1..=D {
            source += &format!("f{i}(m) = f{p}(m) : \\(x).(x + f{p}(m));\n", p = i - 1);
        }
        source += &format!("process = f{D}(hslider(\"m\", 1, 0, 2, 0.01));\n");
        let evaluated = evaluate_within_budget(&source);
        let abstraction = ApplyMemoKind::Abstraction as usize;
        assert!(
            evaluated.hits[abstraction] >= D,
            "each level's call in the lambda hits: {} hits for {D} levels",
            evaluated.hits[abstraction]
        );
        assert!(
            evaluated.probes[abstraction] <= 8 * D,
            "the reductions are linear in the depth: {} probes for {D} levels",
            evaluated.probes[abstraction]
        );
    }

    /// The upstream benchmark of `gApplyMemo`: a `case` recursion with
    /// overlapping calls is linear with the memo and exponential without it.
    /// Each call evaluates its rule in fresh layers, so only the memo of the
    /// matcher's application, keyed by the `case` and its environment, sees
    /// the repeat.
    #[test]
    fn an_overlapping_case_recursion_is_linear() {
        let evaluated = evaluate_within_budget(
            "fib(0) = 0;\nfib(1) = 1;\nfib(n) = fib(n-1) + fib(n-2);\nprocess = fib(40);\n",
        );
        assert_eq!(evaluated.outputs, vec![Some(102_334_155)]);
        let case = ApplyMemoKind::Case as usize;
        assert!(
            evaluated.hits[case] >= 38,
            "fib(n-2) hits at every level: {} hits",
            evaluated.hits[case]
        );
        assert!(
            evaluated.probes[case] <= 100,
            "one miss and one hit per level: {} probes",
            evaluated.probes[case]
        );
    }

    /// The key names the environment the function closes over: one
    /// abstraction, or one `case`, evaluated in two environments is two
    /// functions, applied to the same argument.
    #[test]
    fn the_apply_memo_keys_on_the_closed_over_environment() {
        let evaluated = evaluate_within_budget(
            "f(a) = \\(x).(x + a);\n\
             g(a) = case { (0) => a; (n) => n; };\n\
             process = f(1)(10), f(2)(10), g(1)(0), g(2)(0), g(1)(0);\n",
        );
        assert_eq!(
            evaluated.outputs,
            vec![Some(11), Some(12), Some(1), Some(2), Some(1)]
        );
    }

    /// A matcher bound to a parameter is shared by every use of `p`, and the
    /// apply memo keys it like the fresh matcher of `f(3)`: `p(3)` and `f(3)`
    /// are one entry. Until 2026-10-16 binding a pattern variable wrote into
    /// the rule environments in place, so `p(3)` still saw `x = 2` and fell
    /// through to the second rule (`DIFF-GAP-018`); with the memo on shared
    /// matchers, `f(3)` would then answer `100` too. Values from C++
    /// (`8eebea429` and 2.90.8): `3, 4, 4`.
    #[test]
    fn a_shared_matcher_does_not_answer_for_a_fresh_one() {
        let evaluated = evaluate_within_budget(
            "f = case { (x) => x + 1; (3) => 100; };\n\
             h(p) = p(2), p(3);\n\
             process = h(f), f(3);\n",
        );
        assert_eq!(evaluated.outputs.len(), 3);
        assert_eq!(evaluated.outputs[0], Some(3));
        assert_eq!(
            evaluated.outputs[1],
            Some(4),
            "p(3) does not see the x = 2 of p(2)"
        );
        assert_eq!(evaluated.outputs[2], Some(4), "f(3) selects its first rule");
    }

    /// A `case` passed to itself through a recursion with overlapping calls:
    /// `k` is a `boxPatternMatcher` handle, and every application of it goes
    /// through a copy read from the handle store. The copy is as fresh as the
    /// matcher it was made from, so the memo keys it and the recursion is
    /// linear, as with C++ `gApplyMemo` (2.90.8); without that, it is
    /// exponential, as in the reference `8eebea429` (50 s at `n = 25`).
    #[test]
    fn a_case_handle_passed_through_a_recursion_is_memoized() {
        let evaluated = evaluate_within_budget(
            "fibk(k, 0) = 0;\nfibk(k, 1) = 1;\nfibk(k, n) = k(k, n-1) + k(k, n-2);\n\
             process = fibk(fibk, 40);\n",
        );
        assert_eq!(evaluated.outputs, vec![Some(102_334_155)]);
        let case = ApplyMemoKind::Case as usize;
        assert!(
            evaluated.hits[case] >= 38,
            "k(k, n-2) hits at every level: {} hits",
            evaluated.hits[case]
        );
        assert!(
            evaluated.probes[case] <= 100,
            "one miss and one hit per level: {} probes",
            evaluated.probes[case]
        );
    }

    /// Every application of a shared matcher starts from the matcher's own
    /// bindings, not from those of the previous application: a `case` passed
    /// as an argument, a nonlinear rule, a partially applied matcher and an
    /// outer definition named like a pattern variable. Values from C++
    /// (`8eebea429` and 2.90.8); each one failed with `FRS-EVAL-0099` before.
    #[test]
    fn a_shared_matcher_keeps_its_applications_apart() {
        for (source, expected) in [
            (
                "h(p) = p(2), p(3);\nprocess = h(case { (x) => x + 1; });\n",
                vec![3, 4],
            ),
            (
                "f = case { (x, x) => 1; (x, y) => 2; };\n\
                 h(p) = p(1, 1), p(1, 2), p(2, 2), p(3)(3), p(3)(4);\n\
                 process = h(f);\n",
                vec![1, 2, 1, 1, 2],
            ),
            (
                "f = case { (x, x) => 1; (x, y) => 2; };\n\
                 h(q) = q(1), q(2), q(1);\n\
                 process = h(f(1));\n",
                vec![1, 2, 1],
            ),
            (
                "g = case { (x, y) => x * 10 + y; };\n\
                 h(q) = q(2), q(3);\n\
                 process = h(g(1)), g(4, 5);\n",
                vec![12, 13, 45],
            ),
            (
                "x = 5;\nf = case { (x) => x; (y) => 7; };\n\
                 h(p) = p(3), p(4);\n\
                 process = h(f), f(9);\n",
                vec![3, 4, 9],
            ),
        ] {
            let evaluated = evaluate_within_budget(source);
            let expected: Vec<_> = expected.into_iter().map(Some).collect();
            assert_eq!(evaluated.outputs, expected, "{source}");
        }
    }

    /// The adaptation itself: an application binds its pattern variables in
    /// layers of its own, above the one barrier all rules share, and leaves the
    /// rule environments of the matcher it was given untouched, so that a
    /// copy of the matcher applied afterwards starts from the same state.
    #[test]
    fn applying_a_matcher_leaves_its_rule_environments_untouched() {
        let parsed =
            parser::parse_program("f = case { (x) => x + 1; (3) => 100; };\n", "persist.dsp");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let mut arena = parsed.state.arena;
        let root = parsed.root.expect("a parsed program");
        let mut env = crate::Environment::empty();
        crate::definitions::bind_definitions(&mut arena, root, &mut env).expect("definitions bind");
        let mut detector = LoopDetector::new();
        let f = BoxBuilder::new(&mut arena).ident("f");
        let Ok(crate::EvalValue::PatternMatcher(pm)) =
            crate::eval_value(&mut arena, f, &env, &mut detector)
        else {
            panic!("f evaluates to a pattern matcher");
        };
        let barrier = pm.envs[0].clone().expect("a viable rule");
        assert!(
            pm.envs
                .iter()
                .all(|e| e.as_ref().is_some_and(|e| e.same_identity(&barrier))),
            "every rule starts from the same barrier"
        );
        for (arg, expected) in [(2, 3), (3, 4), (2, 3)] {
            let arg = BoxBuilder::new(&mut arena).int(arg);
            let nil = arena.nil();
            let larg = arena.cons(arg, nil);
            let applied = super::apply_pattern_matcher_value(
                &mut arena,
                pm.clone(),
                larg,
                &env,
                &mut detector,
                None,
            )
            .expect("the matcher applies");
            let lowered = crate::force_value_to_box(&mut arena, applied, &mut detector)
                .expect("the result lowers");
            assert_eq!(match_box(&arena, lowered), BoxMatch::Int(expected));
            let (_, is_barrier, bindings) = barrier.layer_snapshot();
            assert!(
                is_barrier && bindings.is_empty(),
                "the barrier is never written"
            );
            assert!(
                pm.envs
                    .iter()
                    .all(|e| e.as_ref().is_some_and(|e| e.same_identity(&barrier))),
                "the matcher applied keeps its rule environments"
            );
        }
    }
}
