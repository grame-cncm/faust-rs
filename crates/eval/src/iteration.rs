//! Iterative `ipar` / `iseq` / `isum` / `iprod` lowering.
//!
//! Expands the four iteration forms of Faust box IR into their equivalent
//! parallel/sequential compositions by substituting the loop variable with
//! successive integer literals and composing the results:
//! - `iterate_par` / `iterate_seq` — expand to `par(A, B)` / `seq(A, B)` trees;
//! - `iterate_sum` / `iterate_prod` — expand to `add` / `mul` chains;
//! - `iteration_var_name` / `eval_non_negative_count` — parse and validate the
//!   loop header.

use super::*;

/// Returns the identifier name used as iterative binder in `ipar/iseq/isum/iprod`.
///
/// The parser should already enforce identifier syntax here, but `eval` keeps
/// the check local so malformed trees created programmatically still fail with a
/// typed evaluator error instead of panicking.
pub(crate) fn iteration_var_name(arena: &TreeArena, id: TreeId) -> Result<String, EvalError> {
    match match_box(arena, id) {
        BoxMatch::Ident(name) => Ok(name.to_owned()),
        _ => Err(EvalError::NonIdentifierIterationVariable { node: id }),
    }
}

/// Evaluates iterative count expression and enforces a non-negative integer result.
pub(crate) fn eval_non_negative_count(
    arena: &mut TreeArena,
    count_expr: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<usize, EvalError> {
    let count = eval_box(arena, count_expr, env, loop_detector)?;
    if let Ok(v) = eval_box_to_i32(arena, count) {
        return match v {
            v if v < 0 => Err(EvalError::NegativeIterationCount {
                value: i64::from(v),
            }),
            v => usize::try_from(v).map_err(|_| EvalError::IterationCountTooLarge {
                value: i64::from(v),
            }),
        };
    }
    match match_box(arena, count) {
        BoxMatch::Int(v) if v < 0 => Err(EvalError::NegativeIterationCount {
            value: i64::from(v),
        }),
        BoxMatch::Int(v) => usize::try_from(v).map_err(|_| EvalError::IterationCountTooLarge {
            value: i64::from(v),
        }),
        BoxMatch::Real(x) => {
            let i = x as i64;
            if (i as f64) == x && i >= 0 {
                usize::try_from(i).map_err(|_| EvalError::IterationCountTooLarge { value: i })
            } else if x < 0.0 {
                Err(EvalError::NegativeIterationCount { value: x as i64 })
            } else {
                Err(EvalError::IterationCountNotInt { node: count })
            }
        }
        _ => Err(EvalError::IterationCountNotInt { node: count }),
    }
}

/// Evaluates iterative body with one bound loop index (`i`).
///
/// Each expansion step pushes one child lexical scope, binds the iteration
/// variable to the current integer index, and then evaluates the body under that
/// scope. The binding uses a normal environment entry so iteration variables are
/// visible to all evaluator features that consult lexical scope, including
/// label interpolation.
pub(crate) fn eval_iter_body(
    arena: &mut TreeArena,
    var_name: &str,
    i: usize,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let mut scoped = env.push_scope();
    let i_as_i64 =
        i64::try_from(i).map_err(|_| EvalError::IterationCountTooLarge { value: i64::MAX })?;
    let ival = arena.int(i_as_i64);
    // var_name is a &str parameter (not borrowed from arena) — intern is safe here.
    let sym = arena.intern_symbol(var_name);
    scoped.bind(sym, ival);
    eval_box(arena, body, &scoped, loop_detector)
}

/// Returns the C++-compatible empty-iteration neutral box (`route(0,0,par(0,0))`).
pub(crate) fn empty_iteration_route(arena: &mut TreeArena) -> TreeId {
    let mut b = BoxBuilder::new(arena);
    let z0 = b.int(0);
    let z1 = b.int(0);
    let spec = b.par(z0, z1);
    b.route(z0, z1, spec)
}

/// Returns the neutral element for `iseq(i, 0, body)` following C++ `neutralExpSeq`.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp`
/// - `neutralExpSeq`
///
/// Mapping status: `adapted`.
/// C++ evaluates the body once with `i = 0`, lowers the result with `a2sb`,
/// and constructs an identity bus whose width matches the body outputs when the
/// body has equal input/output arity. Only a real `0 -> 0` body uses the empty
/// `route(0,0,par(0,0))` neutral element.
pub(crate) fn neutral_seq_body(
    arena: &mut TreeArena,
    var_name: &str,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let evaluated = eval_iter_body(arena, var_name, 0, body, env, loop_detector)?;
    let lowered = a2sb(arena, evaluated, loop_detector)?;
    let Some((ins, outs)) =
        infer_box_arity_cached(arena, lowered, &mut loop_detector.box_arity_cache)
    else {
        return Err(EvalError::InternalError {
            message: "seq(i,0,body) neutral arity could not be inferred".to_owned(),
        });
    };
    if ins != outs {
        return Err(EvalError::InternalError {
            message: format!(
                "seq(i,0,body) requires matching input/output arity, got {ins} -> {outs}"
            ),
        });
    }
    if outs == 0 {
        return Ok(empty_iteration_route(arena));
    }
    let mut b = BoxBuilder::new(arena);
    let mut bus = b.wire();
    for _ in 1..outs {
        let mut b = BoxBuilder::new(arena);
        let wire = b.wire();
        bus = b.par(bus, wire);
    }
    Ok(bus)
}

/// Expands `ipar(i,n,body)` into nested `par` composition.
///
/// Expansion order matches the C++ evaluator: the rightmost branch (`n - 1`) is
/// built first, then earlier iterations are prepended so the final tree keeps
/// the observable left-to-right bus order expected by later passes.
pub(crate) fn iterate_par(
    arena: &mut TreeArena,
    index: TreeId,
    count: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let var_name = iteration_var_name(arena, index)?;
    let n = eval_non_negative_count(arena, count, env, loop_detector)?;
    if n == 0 {
        // par(i, 0, X) = empty block (0 inputs, 0 outputs), not a neutral-seq identity.
        // iterate_sum/prod use the same convention; only iseq uses neutral_seq_body.
        return Ok(empty_iteration_route(arena));
    }
    let mut res = eval_iter_body(arena, &var_name, n - 1, body, env, loop_detector)?;
    for i in (0..(n - 1)).rev() {
        let left = eval_iter_body(arena, &var_name, i, body, env, loop_detector)?;
        res = {
            let mut b = BoxBuilder::new(arena);
            b.par(left, res)
        };
    }
    Ok(res)
}

/// Expands `iseq(i,n,body)` into nested `seq` composition.
///
/// Like [`iterate_par`], this preserves the source iteration order by building
/// the tail first and prepending earlier bodies during the fold.
pub(crate) fn iterate_seq(
    arena: &mut TreeArena,
    index: TreeId,
    count: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let var_name = iteration_var_name(arena, index)?;
    let n = eval_non_negative_count(arena, count, env, loop_detector)?;
    if n == 0 {
        return neutral_seq_body(arena, &var_name, body, env, loop_detector);
    }
    let mut res = eval_iter_body(arena, &var_name, n - 1, body, env, loop_detector)?;
    for i in (0..(n - 1)).rev() {
        let left = eval_iter_body(arena, &var_name, i, body, env, loop_detector)?;
        res = {
            let mut b = BoxBuilder::new(arena);
            b.seq(left, res)
        };
    }
    Ok(res)
}

/// Expands `isum(i,n,body)` into a fold using `add` primitive.
///
/// The sum starts at iteration `0` and folds left using the primitive `+`
/// wiring convention (`par(lhs, rhs) : add`).
pub(crate) fn iterate_sum(
    arena: &mut TreeArena,
    index: TreeId,
    count: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let var_name = iteration_var_name(arena, index)?;
    let n = eval_non_negative_count(arena, count, env, loop_detector)?;
    if n == 0 {
        return Ok(empty_iteration_route(arena));
    }
    let mut res = eval_iter_body(arena, &var_name, 0, body, env, loop_detector)?;
    for i in 1..n {
        let rhs = eval_iter_body(arena, &var_name, i, body, env, loop_detector)?;
        let pair = {
            let mut b = BoxBuilder::new(arena);
            b.par(res, rhs)
        };
        let add = {
            let mut b = BoxBuilder::new(arena);
            b.add()
        };
        res = {
            let mut b = BoxBuilder::new(arena);
            b.seq(pair, add)
        };
    }
    Ok(res)
}

/// Expands `iprod(i,n,body)` into a fold using `mul` primitive.
/// Expands `iprod(i,n,body)` into a fold using `mul` primitive.
///
/// This mirrors [`iterate_sum`] but uses multiplicative composition.
pub(crate) fn iterate_prod(
    arena: &mut TreeArena,
    index: TreeId,
    count: TreeId,
    body: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let var_name = iteration_var_name(arena, index)?;
    let n = eval_non_negative_count(arena, count, env, loop_detector)?;
    if n == 0 {
        return Ok(empty_iteration_route(arena));
    }
    let mut res = eval_iter_body(arena, &var_name, 0, body, env, loop_detector)?;
    for i in 1..n {
        let rhs = eval_iter_body(arena, &var_name, i, body, env, loop_detector)?;
        let pair = {
            let mut b = BoxBuilder::new(arena);
            b.par(res, rhs)
        };
        let mul = {
            let mut b = BoxBuilder::new(arena);
            b.mul()
        };
        res = {
            let mut b = BoxBuilder::new(arena);
            b.seq(pair, mul)
        };
    }
    Ok(res)
}
