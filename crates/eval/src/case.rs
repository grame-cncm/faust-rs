//! Case and pattern rule evaluation.
//!
//! Handles the `case` expression form in Faust box IR:
//! - `rule_parts` — splits a rule node into its pattern list and body;
//! - `case_expected_arity` — derives the expected argument count from the first rule;
//! - `eval_rule_list` / `eval_pattern_list` / `eval_pattern` — evaluate and
//!   match rule sets against supplied argument lists;
//! - `pattern_simplification` — reduces pattern expressions to normal form
//!   before matching.

use super::*;

/// Decodes a case rule node into `(lhs_patterns, rhs_expr)`.
pub(crate) fn rule_parts(arena: &TreeArena, rule: TreeId) -> Result<(TreeId, TreeId), EvalError> {
    let lhs = arena
        .hd(rule)
        .ok_or(EvalError::MalformedCaseNode { node: rule })?;
    let rhs = arena
        .tl(rule)
        .ok_or(EvalError::MalformedCaseNode { node: rule })?;
    Ok((lhs, rhs))
}

/// Returns expected argument arity for a case-rule set (first source rule arity).
pub(crate) fn case_expected_arity(
    arena: &TreeArena,
    rules_rev: TreeId,
) -> Result<usize, EvalError> {
    let mut rules = list_to_vec(arena, rules_rev)?;
    rules.reverse();
    let Some(first_rule) = rules.first().copied() else {
        return Err(EvalError::MalformedCaseNode { node: rules_rev });
    };
    let (first_lhs, _first_rhs) = rule_parts(arena, first_rule)?;
    Ok(list_to_vec(arena, first_lhs)?.len())
}

/// Evaluates a case-rule list for matching.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp`
/// - `evalRuleList`
/// - `evalRule`
/// - `evalPatternList`
/// - `evalPattern`
///
/// Only the left-hand side patterns are evaluated and simplified. The right-hand
/// side remains unchanged so it can later be evaluated in the chosen rule
/// environment.
/// Evaluates every rule of a `case` expression under the current lexical environment.
///
/// Rule evaluation is split from matcher construction so patterns can first be
/// simplified and normalized exactly once, after which the resulting rule list
/// is suitable for automaton caching.
pub(crate) fn eval_rule_list(
    arena: &mut TreeArena,
    rules_rev: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let rules = list_to_vec(arena, rules_rev)?;
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let (lhs, rhs) = rule_parts(arena, rule)?;
        let lhs_eval = eval_pattern_list(arena, lhs, env, loop_detector)?;
        out.push(arena.cons(lhs_eval, rhs));
    }
    Ok(vec_to_list(arena, &out))
}

/// Evaluates each pattern of one rule, preserving parser list order.
/// Evaluates a list of case-pattern nodes left-to-right.
///
/// Each pattern goes through [`eval_pattern`] so compile-time numeric
/// simplification and scope-barrier-sensitive behavior are applied uniformly
/// before the matcher sees the rule.
pub(crate) fn eval_pattern_list(
    arena: &mut TreeArena,
    patterns: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let items = list_to_vec(arena, patterns)?;
    let mut out = Vec::with_capacity(items.len());
    for pattern in items {
        out.push(eval_pattern(arena, pattern, env, loop_detector)?);
    }
    Ok(vec_to_list(arena, &out))
}

/// Evaluates and simplifies one pattern before automaton construction.
///
/// This restores the C++ `evalPattern` behavior: lexical identifiers are resolved
/// in the current environment, then constant-only numeric subgraphs are folded so
/// patterns like `(1+1)` match the same way they do in the C++ compiler.
/// Evaluates one pattern expression in the current lexical environment.
///
/// Pattern evaluation is stricter than ordinary RHS evaluation: after normal
/// evaluation the result is passed through [`pattern_simplification`] so numeric
/// constant expressions such as `(1+1)` can match literal values at runtime.
pub(crate) fn eval_pattern(
    arena: &mut TreeArena,
    pattern: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let evaluated = eval_box(arena, pattern, env, loop_detector)?;
    Ok(pattern_simplification(
        arena,
        evaluated,
        &mut loop_detector.fold,
    ))
}

/// Simplifies a pattern after evaluation, mirroring C++ `patternSimplification`.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp` — `patternSimplification` (line 773)
///
/// Algorithm (exact C++ match):
/// 1. Try to reduce the whole expression to a numeric literal via full
///    signal propagation + `simplify()` (delegates to `simplify_pattern`,
///    which is the Rust equivalent of C++ `isBoxNumeric`).
/// 2. If that fails AND the node is a `PatternOp`
///    (Par / Seq / Split / Merge / Rec only — matches C++ `isBoxPatternOp`),
///    recurse into its two children.
/// 3. Otherwise return the pattern unchanged.
///
/// Note: HGroup / VGroup / TGroup / Route are **not** PatternOps in C++ and
/// are returned unchanged without recursion.
///
/// The patterns of a rule are evaluated again at each application, under the
/// environment of the call, so `fold`, the caches of the evaluation pass,
/// keeps the folding of a growing value incremental.
pub(crate) fn pattern_simplification(
    arena: &mut TreeArena,
    pattern: TreeId,
    fold: &mut crate::simplify::FoldCache,
) -> TreeId {
    // (a) Try full constant folding on the whole expression first.
    let folded = crate::simplify::simplify_pattern_in(arena, pattern, fold);
    if folded != pattern {
        return folded;
    }
    // (b) Recurse into PatternOp children (Par/Seq/Split/Merge/Rec only).
    match match_box(arena, pattern) {
        BoxMatch::Par(a, b) => {
            let sa = pattern_simplification(arena, a, fold);
            let sb = pattern_simplification(arena, b, fold);
            BoxBuilder::new(arena).par(sa, sb)
        }
        BoxMatch::Seq(a, b) => {
            let sa = pattern_simplification(arena, a, fold);
            let sb = pattern_simplification(arena, b, fold);
            BoxBuilder::new(arena).seq(sa, sb)
        }
        BoxMatch::Split(a, b) => {
            let sa = pattern_simplification(arena, a, fold);
            let sb = pattern_simplification(arena, b, fold);
            BoxBuilder::new(arena).split(sa, sb)
        }
        BoxMatch::Merge(a, b) => {
            let sa = pattern_simplification(arena, a, fold);
            let sb = pattern_simplification(arena, b, fold);
            BoxBuilder::new(arena).merge(sa, sb)
        }
        BoxMatch::Rec(a, b) => {
            let sa = pattern_simplification(arena, a, fold);
            let sb = pattern_simplification(arena, b, fold);
            BoxBuilder::new(arena).rec(sa, sb)
        }
        // (c) Everything else (HGroup/VGroup/TGroup/Route/…) — unchanged.
        _ => pattern,
    }
}
