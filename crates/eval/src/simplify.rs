//! Box and signal algebraic simplification, and compile-time constant extraction.
//!
//! Implements the evaluator-level simplification layer:
//! - `propagate_box_and_simplify` — propagates a 0→1 box and simplifies the
//!   resulting signal; building block for all constant folding in the evaluator;
//! - `eval_box_to_*` — try to extract an `i32` or `f64` literal from a box;
//! - `simplify_pattern` — reduces a pattern box to normal form before matching;
//! - `flatten_route_spec` / `normalize_route_spec` — route wire-list helpers;
//! - `box_simplification` / `numeric_box_simplification` — structural
//!   simplifications applied after evaluation;
//! - `is_numerical_tuple_box` — predicate for compile-time numeric tuples;
//! - `try_fold_seq_numeric` — folds `seq(numeric_tuple, f)` at compile time.
//!
//! Source provenance (C++): `compiler/evaluate/eval.cpp` — `isBoxNumeric`,
//! `eval2double`, `eval2int`, `numericBoxSimplification`.

use super::*;

/// Propagates a 0→1 box with no inputs, then algebraically simplifies the
/// resulting signal.
///
/// Returns `None` if the box cannot be flattened or has the wrong arity.
///
/// This is the building block for all compile-time constant extraction in the
/// evaluator.
///
/// # C++ equivalent
///
/// ```cpp
/// Tree lsignals = boxPropagateSig(gGlobal->nil, box, makeSigInputList(0));
/// Tree s        = simplify(hd(lsignals));
/// ```
///
/// Called by `isBoxNumeric`, `eval2double`, `eval2int`, and
/// `numericBoxSimplification` in `compiler/evaluate/eval.cpp`.
pub(crate) fn propagate_box_and_simplify(arena: &mut TreeArena, box_id: TreeId) -> Option<SigId> {
    let flat = try_build_flat_box(arena, box_id).ok()?;
    let mut cache = ArityCache::new();
    let signals = propagate_typed(arena, flat, &[], &mut cache).ok()?;
    let [sig] = signals.as_slice() else {
        return None;
    };
    Some(simplify_const(arena, *sig))
}

/// Tries to reduce a box to a numeric literal for pattern matching.
///
/// If `box_id` represents a compile-time numeric constant (possibly hidden
/// behind arithmetic like `max(1, min(6, 4))`), returns the corresponding
/// `boxInt(n)` or `boxReal(x)`.  Otherwise returns `box_id` unchanged.
///
/// When the propagation yields `sigReal(x)` but `x` is an exact integer
/// (e.g. `2.0`), we return `boxInt(x as i32)` so that the pattern matcher's
/// tree-identity check succeeds against integer pattern constants like
/// `poly(2, x)`.  This mirrors the C++ pipeline where `max/min` on integers
/// stays in the integer domain.
///
/// # C++ equivalent
///
/// `Tree simplifyPattern(Tree value)` in `compiler/evaluate/eval.cpp`.
pub(crate) fn simplify_pattern(arena: &mut TreeArena, box_id: TreeId) -> TreeId {
    // Fast path: already a literal — return unchanged, no type coercion.
    //
    // C++ `isBoxNumeric` short-circuits on any `boxInt` / `boxReal` literal and
    // returns it as-is, without converting integer-valued floats (`1.0`) to
    // `boxInt(1)`.  The automaton stores pattern constants as their original
    // `TreeId` (e.g. `float_bits(0x3ff0000000000000)` for `1.0`), and matching
    // is a `TreeId` equality test.  Converting `1.0 → int(1)` here would make
    // `foo(1.0) = 456;` never match the call `foo(1.0)`.
    match match_box(arena, box_id) {
        BoxMatch::Int(_) | BoxMatch::Real(_) => return box_id,
        _ => {}
    }
    let Some(sig) = propagate_box_and_simplify(arena, box_id) else {
        return box_id;
    };
    // For arithmetic expressions, the signal type determines the result type:
    // - `SigInt` for integer-only operations (e.g. `1+1`, `max(int,int)`).
    //   C++ xtended `computeSigOutput` for `min`/`max` preserves the integer
    //   type when both operands are integers; `normalize/src/simplify.rs` now
    //   mirrors this so `max(1, min(2, 4))` folds to `SigInt(2)`.
    // - `SigReal` for anything involving a float or real-valued ops (e.g. `/`).
    //   The pattern `foo(4.0/2.0) = 789;` stores `boxReal(2.0)` and the
    //   argument simplifies to the same — no coercion needed.
    let value = match match_sig(arena, sig) {
        SigMatch::Int(i) => Some(NumericLit::Int(i)),
        SigMatch::Real(x) => Some(NumericLit::Real(x)),
        _ => None,
    };
    match value {
        Some(NumericLit::Int(i)) => BoxBuilder::new(arena).int(i),
        Some(NumericLit::Real(x)) => BoxBuilder::new(arena).real(x),
        None => box_id,
    }
}

/// Converts a 0→1 box to an `f64` compile-time constant.
///
/// Returns [`EvalError::NotAConstantExpression`] if the box is not a scalar
/// constant of type (0→1) or cannot be reduced to a numeric value.
///
/// # C++ equivalent
///
/// `static double eval2double(Tree exp, Tree visited, Tree localValEnv)` in
/// `compiler/evaluate/eval.cpp`.
pub(crate) fn eval_box_to_f64(arena: &mut TreeArena, box_id: TreeId) -> Result<f64, EvalError> {
    let sig = propagate_box_and_simplify(arena, box_id)
        .ok_or(EvalError::NotAConstantExpression { node: box_id })?;
    match match_sig(arena, sig) {
        SigMatch::Real(x) => Ok(x),
        SigMatch::Int(i) => Ok(f64::from(i)),
        _ => Err(EvalError::NotAConstantExpression { node: box_id }),
    }
}

/// Converts a 0→1 box to an `i32` compile-time constant.
///
/// Returns [`EvalError::NotAConstantExpression`] if the box is not a scalar
/// constant of type (0→1) or cannot be reduced to a numeric value.
///
/// # C++ equivalent
///
/// `static int eval2int(Tree exp, Tree visited, Tree localValEnv)` in
/// `compiler/evaluate/eval.cpp`.
pub(crate) fn eval_box_to_i32(arena: &mut TreeArena, box_id: TreeId) -> Result<i32, EvalError> {
    let sig = propagate_box_and_simplify(arena, box_id)
        .ok_or(EvalError::NotAConstantExpression { node: box_id })?;
    match match_sig(arena, sig) {
        SigMatch::Int(i) => Ok(i),
        SigMatch::Real(x) => Ok(x as i32),
        _ => Err(EvalError::NotAConstantExpression { node: box_id }),
    }
}

// ─── Route parameter normalization ─────────────────────────────────────────────

/// Converts a 0→1 box to a `boxInt(n)` node.
///
/// Used to normalise the `ins` and `outs` arguments of a `route` at
/// evaluation time, mirroring the C++ `boxPropagateSig` + `sigList2vecInt`
/// pattern used in `compiler/evaluate/eval.cpp` for the `isBoxRoute` branch.
pub(crate) fn eval_box_to_int_node(
    arena: &mut TreeArena,
    box_id: TreeId,
) -> Result<TreeId, EvalError> {
    let n = eval_box_to_i32(arena, box_id)?;
    Ok(BoxBuilder::new(arena).int(n))
}

/// Converts a 0→N constant box into a canonical right-spine `Par` tree of
/// `boxInt` leaves.
///
/// This follows the C++ `isBoxRoute` path in `eval.cpp`, where the route
/// specification is propagated as a whole and then converted back from the
/// resulting integer signal list.
pub(crate) fn eval_box_to_int_list_node(arena: &mut TreeArena, box_id: TreeId) -> Option<TreeId> {
    let (inputs, outputs) = infer_box_arity(arena, box_id)?;
    if inputs != 0 || outputs == 0 {
        return None;
    }
    let flat = try_build_flat_box(arena, box_id).ok()?;
    let mut cache = ArityCache::new();
    let signals = propagate_typed(arena, flat, &[], &mut cache).ok()?;
    if signals.len() != outputs {
        return None;
    }

    let mut ints = Vec::with_capacity(outputs);
    for sig in signals {
        let sig = simplify_const(arena, sig);
        let value = match match_sig(arena, sig) {
            SigMatch::Int(i) => i,
            SigMatch::Real(x) => {
                let i = x as i32;
                if (i as f64) != x {
                    return None;
                }
                i
            }
            _ => return None,
        };
        ints.push(BoxBuilder::new(arena).int(value));
    }

    let mut result = *ints.last()?;
    for leaf in ints[..ints.len() - 1].iter().rev() {
        result = BoxBuilder::new(arena).par(*leaf, result);
    }
    Some(result)
}

/// Recursively collects the leaves of a right-spine `Par` tree.
///
/// `route(2,2, 1,1, 2,2)` stores the wire pairs as
/// `par(int(1), par(int(1), par(int(2), int(2))))`.  Flattening extracts
/// `[int(1), int(1), int(2), int(2)]` in order.
pub(crate) fn flatten_route_spec(arena: &TreeArena, spec: TreeId, out: &mut Vec<TreeId>) {
    match match_box(arena, spec) {
        BoxMatch::Par(a, b) => {
            flatten_route_spec(arena, a, out);
            flatten_route_spec(arena, b, out);
        }
        _ => out.push(spec),
    }
}

/// Re-evaluates the route wire-pair spec to ensure every leaf is a `boxInt`
/// and rebuilds the tree in the canonical right-spine form.
///
/// # C++ equivalent
///
/// `static Tree normalizeRouteList(Tree routes)` in
/// `compiler/evaluate/eval.cpp`.
pub(crate) fn normalize_route_spec(arena: &mut TreeArena, spec: TreeId) -> TreeId {
    // Phase 1: collect leaves with an immutable borrow.
    let mut leaves: Vec<TreeId> = Vec::new();
    flatten_route_spec(arena, spec, &mut leaves);
    let n = leaves.len();
    if n == 0 {
        return spec;
    }
    // Phase 2: convert each leaf to i32 → boxInt (mutable borrow).
    let mut int_leaves: Vec<TreeId> = Vec::with_capacity(n);
    for leaf in leaves {
        if let Ok(i) = eval_box_to_i32(arena, leaf) {
            int_leaves.push(BoxBuilder::new(arena).int(i));
        } else {
            int_leaves.push(leaf); // pattern var / wire / slot — keep as-is
        }
    }
    // Phase 3: rebuild right-spine Par (C++ normalizeRouteList order).
    let mut result = int_leaves[n - 1];
    for i in (0..n - 1).rev() {
        result = BoxBuilder::new(arena).par(int_leaves[i], result);
    }
    result
}

// ─── Seq numeric folding ───────────────────────────────────────────────────────

/// Returns `true` if `box_id` is a parallel composition of numeric literals
/// (`boxInt` / `boxReal`), possibly nested.
///
/// Used as a guard before attempting compile-time Seq folding.
///
/// # C++ equivalent
///
/// `static bool isNumericalTuple(Tree box, siglist& L)` in
/// `compiler/evaluate/eval.cpp`.
/// Returns `true` if `box_id` is a parallel composition of **integer** literals only.
///
/// Used to decide whether a Real result from seq folding should be
/// cast back to Int (preserving integer semantics for pattern matching).
pub(crate) fn all_inputs_are_int(arena: &TreeArena, box_id: TreeId) -> bool {
    match match_box(arena, box_id) {
        BoxMatch::Int(_) => true,
        BoxMatch::Real(_) => false,
        BoxMatch::Par(l, r) => all_inputs_are_int(arena, l) && all_inputs_are_int(arena, r),
        _ => false,
    }
}

/// Returns `true` if `box_id` is a tree of numeric literals connected only by `par`.
///
/// Used by [`try_fold_seq_numeric`] to decide whether the left-hand side of a
/// `seq` expression can be treated as a static input tuple for compile-time folding.
pub(crate) fn is_numerical_tuple_box(arena: &TreeArena, box_id: TreeId) -> bool {
    match match_box(arena, box_id) {
        BoxMatch::Int(_) | BoxMatch::Real(_) => true,
        BoxMatch::Par(l, r) => is_numerical_tuple_box(arena, l) && is_numerical_tuple_box(arena, r),
        _ => false,
    }
}

/// Tries to fold `seq(a1, a2)` into a single numeric box literal.
///
/// Requires `a1` to be a numerical tuple (see [`is_numerical_tuple_box`]).
/// Propagates `a2` with the signals from `a1` as its inputs and simplifies
/// the result; if propagation yields exactly one output signal and that
/// simplified signal is a numeric constant, returns the corresponding
/// `boxInt(n)` or `boxReal(x)`.
///
/// Returns `None` if the expression cannot be reduced.
///
/// # C++ equivalent
///
/// The body of the `isBoxSeq` branch in `compiler/evaluate/eval.cpp`:
/// ```cpp
/// Tree lres = boxPropagateSig(nil, a2, lsig);
/// if (isList(lres) && isNil(tl(lres))) {
///     Tree r = simplify(hd(lres));
///     if (isNum(r)) { return r; }
/// }
/// ```
pub(crate) fn try_fold_seq_numeric(
    arena: &mut TreeArena,
    a1: TreeId,
    a2: TreeId,
) -> Option<TreeId> {
    // C++ folds only when propagation produces exactly one output. Multi-output
    // sequences such as `(0,0,1,1) : par(i,2,+)` must remain structured graphs,
    // not collapse to the first propagated constant.
    let seq = BoxBuilder::new(arena).seq(a1, a2);
    let flat = try_build_flat_box(arena, seq).ok()?;
    let mut cache = ArityCache::new();
    let signals = propagate_typed(arena, flat, &[], &mut cache).ok()?;
    let [sig] = signals.as_slice() else {
        return None;
    };
    let sig = simplify_const(arena, *sig);
    // Both SigInt/SigReal and BoxInt/BoxReal share the same underlying NodeKind
    // (NodeKind::Int / NodeKind::FloatBits), so the SigId IS the BoxId.
    //
    // When all inputs are Int and the result is a Real that happens to be an
    // exact integer (e.g. `4/2 → Real(2.0)`), convert back to Int.  This is
    // critical for pattern matching: `S(i,i)` compares tree nodes by identity,
    // so `Real(2.0) != Int(2)` would cause the match to fail.
    // C++ eval keeps integer semantics at the box level for this reason.
    match match_sig(arena, sig) {
        SigMatch::Int(_) => Some(sig),
        SigMatch::Real(x)
            if all_inputs_are_int(arena, a1)
                && x.fract() == 0.0
                && x >= f64::from(i32::MIN)
                && x <= f64::from(i32::MAX) =>
        {
            Some(BoxBuilder::new(arena).int(x as i32))
        }
        SigMatch::Real(_) => Some(sig),
        _ => None,
    }
}

// ─── Box simplification ────────────────────────────────────────────────────────

/// Property name under which a box's simplified form is memoized on the arena.
const SIMPLIFIED_BOX_PROPERTY: &str = "simplified-box";

/// Memoised entry point: simplify `box_id` by replacing any 0→1 sub-expression
/// that propagates to a compile-time constant with the corresponding
/// `boxInt(n)` or `boxReal(x)` literal.
///
/// The result is memoized **on the arena**, so a given subtree is simplified
/// once per compilation, as C++ does through `gGlobal->gSimplifiedBoxProperty`.
///
/// The memo used to be a `HashMap` supplied by the caller, and the dominant
/// caller — `apply.rs`, on every pattern-match dispatch — allocated a fresh one
/// per argument. Simplification was therefore re-run from scratch at every
/// level of a deep dispatch recursion, which made evaluation 67 % of total
/// compile time and this the single largest cost in the compiler
/// (`porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`).
///
/// Arena ownership is what makes the memo sound as well as fast. `TreeId`s are
/// only meaningful for the arena that issued them, so a memo with a longer life
/// — a `thread_local`, say — silently returns another compilation's trees;
/// `xtask compile-profile`, which compiles the whole corpus in one process,
/// dies on the stack when that is tried.
///
/// # C++ equivalent
///
/// `static Tree boxSimplification(Tree box)` in
/// `compiler/evaluate/eval.cpp`.
pub(crate) fn box_simplification(arena: &mut TreeArena, box_id: TreeId) -> TreeId {
    let key = arena.property_key(SIMPLIFIED_BOX_PROPERTY);
    if let Some(cached) = arena.node_property(box_id, key) {
        return cached;
    }
    let result = numeric_box_simplification(arena, box_id);
    arena.set_node_property(box_id, key, result);
    result
}

/// Tries to reduce a 0→1 box to a numeric literal; recurses into composite
/// boxes otherwise.
///
/// # C++ equivalent
///
/// `static Tree numericBoxSimplification(Tree box)` in
/// `compiler/evaluate/eval.cpp`.
pub(crate) fn numeric_box_simplification(
    arena: &mut TreeArena,
    box_id: TreeId,
) -> TreeId {
    // Fast path: already a numeric literal.
    match match_box(arena, box_id) {
        BoxMatch::Int(_) | BoxMatch::Real(_) => return box_id,
        _ => {}
    }
    // General path: propagate + simplify → try to extract a numeric constant.
    if let Some(sig) = propagate_box_and_simplify(arena, box_id) {
        match match_sig(arena, sig) {
            SigMatch::Real(x) => {
                // Observable C++ parity:
                // compile-time boolean/integer expressions can sometimes reach
                // box simplification as exact reals (`1.0`, `0.0`) after signal
                // propagation. The C++ evaluator still treats these as integer
                // constants in downstream contexts such as pattern matching for
                // `case` dispatch. Collapse exact integer reals back to
                // `boxInt` here so residual `case` applications see the same
                // constant class on examples like `routes.lib`'s
                // `comparatorDirections(...)`.
                let i = x as i32;
                if (i as f64) == x {
                    return BoxBuilder::new(arena).int(i);
                }
                return BoxBuilder::new(arena).real(x);
            }
            SigMatch::Int(i) => {
                return BoxBuilder::new(arena).int(i);
            }
            _ => {}
        }
    }
    // Not a numeric constant: simplify children recursively.
    inside_box_simplification(arena, box_id)
}

/// Recurses into composite boxes, calling [`box_simplification`] on each
/// child sub-diagram.
///
/// Leaf nodes (primitives, UI widgets, slots, waveforms, …) are returned
/// unchanged.
///
/// # C++ equivalent
///
/// `static Tree insideBoxSimplification(Tree box)` in
/// `compiler/evaluate/eval.cpp`.
pub(crate) fn inside_box_simplification(
    arena: &mut TreeArena,
    box_id: TreeId,
) -> TreeId {
    match match_box(arena, box_id) {
        // ── Leaves — return unchanged ──────────────────────────────────────
        BoxMatch::Int(_)
        | BoxMatch::Real(_)
        | BoxMatch::Cut
        | BoxMatch::Wire
        // Primitive operators (Prim0–Prim5 in C++ — operator boxes in Rust)
        | BoxMatch::Add | BoxMatch::Sub | BoxMatch::Mul | BoxMatch::Div | BoxMatch::Rem
        | BoxMatch::Pow | BoxMatch::Fmod | BoxMatch::Remainder
        | BoxMatch::And | BoxMatch::Or | BoxMatch::Xor | BoxMatch::Lsh | BoxMatch::Rsh | BoxMatch::LRsh
        | BoxMatch::Lt  | BoxMatch::Le  | BoxMatch::Gt  | BoxMatch::Ge
        | BoxMatch::Eq  | BoxMatch::Ne  | BoxMatch::Atan2
        | BoxMatch::Floor | BoxMatch::Ceil | BoxMatch::Round | BoxMatch::Rint
        | BoxMatch::Abs | BoxMatch::Min | BoxMatch::Max
        | BoxMatch::IntCast | BoxMatch::FloatCast
        | BoxMatch::Delay | BoxMatch::Delay1 | BoxMatch::Prefix
        | BoxMatch::ReadOnlyTable | BoxMatch::WriteReadTable
        | BoxMatch::Select2 | BoxMatch::Select3 | BoxMatch::AssertBounds
        | BoxMatch::Lowest | BoxMatch::Highest
        | BoxMatch::Attach | BoxMatch::Enable | BoxMatch::Control
        | BoxMatch::Acos | BoxMatch::Asin | BoxMatch::Atan
        | BoxMatch::Cos  | BoxMatch::Sin  | BoxMatch::Tan
        | BoxMatch::Exp  | BoxMatch::Exp10 | BoxMatch::Log  | BoxMatch::Log10 | BoxMatch::Sqrt
        // Foreign function / constant / variable
        | BoxMatch::FFun(_)
        | BoxMatch::FConst(_, _, _)
        | BoxMatch::FVar(_, _, _)
        // UI widgets (C++ isBoxVSlider / HSlider / NumEntry / Bargraph …)
        | BoxMatch::Button(_)
        | BoxMatch::Checkbox(_)
        | BoxMatch::VSlider(_, _, _, _, _)
        | BoxMatch::HSlider(_, _, _, _, _)
        | BoxMatch::NumEntry(_, _, _, _, _)
        | BoxMatch::VBargraph(_, _, _)
        | BoxMatch::HBargraph(_, _, _)
        // Slot (pattern variable in symbolic boxes)
        | BoxMatch::Slot(_)
        // Waveform: always in normal form (has 1 child = size)
        | BoxMatch::Waveform(_)
        // Sound file
        | BoxMatch::Soundfile(_, _) => box_id,

        // ── Recursive on 1 child ──────────────────────────────────────────
        BoxMatch::VGroup(label, body) => {
            let sb = box_simplification(arena, body);
            let mut bld = BoxBuilder::new(arena);
            bld.vgroup(label, sb)
        }
        BoxMatch::HGroup(label, body) => {
            let sb = box_simplification(arena, body);
            let mut bld = BoxBuilder::new(arena);
            bld.hgroup(label, sb)
        }
        BoxMatch::TGroup(label, body) => {
            let sb = box_simplification(arena, body);
            let mut bld = BoxBuilder::new(arena);
            bld.tgroup(label, sb)
        }
        BoxMatch::Symbolic(slot, body) => {
            let sb = box_simplification(arena, body);
            let mut bld = BoxBuilder::new(arena);
            bld.symbolic(slot, sb)
        }

        // ── Recursive on 2 children ───────────────────────────────────────
        BoxMatch::Seq(a, b) => {
            let sa = box_simplification(arena, a);
            let sb = box_simplification(arena, b);
            let mut bld = BoxBuilder::new(arena);
            bld.seq(sa, sb)
        }
        BoxMatch::Par(a, b) => {
            let sa = box_simplification(arena, a);
            let sb = box_simplification(arena, b);
            let mut bld = BoxBuilder::new(arena);
            bld.par(sa, sb)
        }
        BoxMatch::Split(a, b) => {
            let sa = box_simplification(arena, a);
            let sb = box_simplification(arena, b);
            let mut bld = BoxBuilder::new(arena);
            bld.split(sa, sb)
        }
        BoxMatch::Merge(a, b) => {
            let sa = box_simplification(arena, a);
            let sb = box_simplification(arena, b);
            let mut bld = BoxBuilder::new(arena);
            bld.merge(sa, sb)
        }
        BoxMatch::Rec(a, b) => {
            let sa = box_simplification(arena, a);
            let sb = box_simplification(arena, b);
            let mut bld = BoxBuilder::new(arena);
            bld.rec(sa, sb)
        }

        // ── Metadata: simplify body, keep metadata list ───────────────────
        BoxMatch::Metadata(body, meta) => {
            let sb = box_simplification(arena, body);
            let mut bld = BoxBuilder::new(arena);
            bld.metadata(sb, meta)
        }

        // ── Route: simplify ins/outs, keep spec ──────────────────────────
        BoxMatch::Route(ins, outs, routes) => {
            let si = box_simplification(arena, ins);
            let so = box_simplification(arena, outs);
            let mut bld = BoxBuilder::new(arena);
            bld.route(si, so, routes)
        }

        // ── Unknown / not yet handled: return unchanged ───────────────────
        _ => box_id,
    }
}

// ─── Evaluate label node ───────────────────────────────────────────────────────

#[cfg(test)]
mod memo_tests {
    use super::*;
    use boxes::BoxBuilder;

    /// V4: the memo is written, so a second simplification is a lookup.
    ///
    /// Without an observable hit counter, what is checkable is that the entry
    /// exists afterwards and that the second call agrees with the first. A
    /// version that computed correctly but never stored would pass every
    /// correctness test in the suite while leaving the cost exactly where it
    /// was — which is the state this phase set out to fix, so it needs its own
    /// assertion rather than being implied by the timings.
    #[test]
    fn simplifying_a_box_records_it_on_the_arena() {
        let mut arena = TreeArena::new();
        // `(2, 3) : +` — a 0->1 box that simplifies to the literal 5.
        let two = BoxBuilder::new(&mut arena).int(2);
        let three = BoxBuilder::new(&mut arena).int(3);
        let par = BoxBuilder::new(&mut arena).par(two, three);
        let add = BoxBuilder::new(&mut arena).add();
        let expr = BoxBuilder::new(&mut arena).seq(par, add);

        let key = arena.property_key(SIMPLIFIED_BOX_PROPERTY);
        assert!(
            arena.node_property(expr, key).is_none(),
            "a fresh arena must carry no memo"
        );

        let first = box_simplification(&mut arena, expr);
        assert_eq!(
            arena.node_property(expr, key),
            Some(first),
            "the simplified form must be recorded on the node"
        );
        assert_eq!(
            box_simplification(&mut arena, expr),
            first,
            "a second simplification must agree with the first"
        );
    }

    /// A fresh arena starts with an empty memo, which is the whole reason the
    /// arena is the right owner: invalidation is not a policy anyone has to
    /// remember, it is the drop.
    #[test]
    fn a_new_arena_starts_with_no_memo() {
        let mut first = TreeArena::new();
        let key = first.property_key(SIMPLIFIED_BOX_PROPERTY);
        let node = BoxBuilder::new(&mut first).int(7);
        let _ = box_simplification(&mut first, node);
        assert!(first.node_property(node, key).is_some());

        let mut second = TreeArena::new();
        let key2 = second.property_key(SIMPLIFIED_BOX_PROPERTY);
        assert!(
            second.node_property(node, key2).is_none(),
            "a second arena must not see the first arena's memo"
        );
    }
}
