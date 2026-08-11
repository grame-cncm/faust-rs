//! Signal simplification: memoized rewrite engine + local algebraic rules.
//!
//! Ported from C++ `compiler/normalize/simplify.cpp`.
// Internal module — wired into the Phase 1 normalform pipeline.
#![allow(dead_code)]
//!
//! # Architecture
//!
//! - [`simplify`]: public entry point.  Allocates a fresh per-pass cache and
//!   calls [`sig_map`] with [`simplification`] as the local rule function.
//! - [`sig_map`]: memoized depth-first graph traversal.  Maps a function over
//!   all nodes, short-circuiting on already-visited nodes, and breaking
//!   recursive cycles by inserting a sentinel (`None`) before descending into
//!   `Rec` nodes.
//! - [`simplification`]: local rewrite rules applied to a single node (after
//!   its children have already been simplified).
//!
//! # API mapping status
//! - `simplify(sig)` → [`simplify`]
//! - `sigMap(key, f, t)` → [`sig_map`] (internal, no global key needed)
//! - `simplification(sig)` → [`simplification`]

use std::collections::HashMap;

use signals::{BinOp, SigBuilder, SigId, SigMatch, match_sig};
use sigtype::SigType;
use tlib::TreeArena;

use crate::mterm::{is_minus_one, is_negative_num, is_num, is_one, is_zero, minus_num, mul_nums};
use crate::normalize::{normalize_add_term, normalize_delay_term, normalize_delay1_term};

// ─── Public entry points ──────────────────────────────────────────────────────

/// Simplify a signal tree with full type context.
///
/// Recursively maps [`simplification`] over the graph using a memoised
/// depth-first traversal.  Each unique signal node is simplified at most once
/// per call.
///
/// C++: `Tree simplify(Tree sig)` — uses `sigMap(gGlobal->SIMPLIFIED, …)`.
pub(crate) fn simplify(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sig: SigId,
) -> SigId {
    let mut cache = SimplifyCache::new();
    simplify_with_cache(arena, &mut cache, types, sig)
}

/// Per-pass cache for [`sig_map`].
///
/// C++ stores `simplify()` results on each tree node with the global
/// `SIMPLIFIED` property.  Rust keeps the cache explicit so callers can scope
/// it to one type context and still share it across co-dependent output roots.
#[derive(Default)]
pub(crate) struct SimplifyCache {
    nodes: HashMap<SigId, Option<SigId>>,
}

impl SimplifyCache {
    pub(crate) fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.nodes.len()
    }
}

/// Simplify a signal tree reusing an existing per-pass cache.
///
/// The cache is valid only for the arena and type map used by the caller.
pub(crate) fn simplify_with_cache(
    arena: &mut TreeArena,
    cache: &mut SimplifyCache,
    types: &HashMap<SigId, SigType>,
    sig: SigId,
) -> SigId {
    sig_map(arena, cache, types, sig)
}

/// Simplify a signal tree without type context.
///
/// Equivalent to `simplify` with an empty type map.  Sufficient for
/// compile-time constant folding at evaluation time, before type annotation
/// has been performed (e.g. slider bounds, route parameters, numeric boxing).
///
/// # Example
///
/// ```rust
/// # use normalize::simplify_const;
/// # use signals::{SigBuilder, SigMatch, match_sig};
/// # use tlib::TreeArena;
/// # let mut arena = TreeArena::default();
/// # let mut b = SigBuilder::new(&mut arena);
/// # let two   = b.int(2);
/// # let three  = b.int(3);
/// # let sum    = b.add(two, three);
/// let result = simplify_const(&mut arena, sum);
/// assert!(matches!(match_sig(&arena, result), SigMatch::Int(5)));
/// ```
///
/// C++ equivalent: `simplify(sig)` — called inside `eval.cpp` for constant
/// box expressions, using a global `SIMPLIFIED` cache as key.
pub fn simplify_const(arena: &mut TreeArena, sig: SigId) -> SigId {
    let types = HashMap::new();
    simplify(arena, &types, sig)
}

// ─── Graph traversal ──────────────────────────────────────────────────────────

/// Memoized depth-first simplification traversal.
///
/// Algorithm (mirroring C++ `sigMap`):
/// 1. Cache hit → return cached result (`None` in cache → return `sig` itself).
/// 2. `Rec(body)` → insert `None` (sentinel), recurse on body, return
///    `Rec(new_body)` (Rec stays unmodified but its body is simplified).
/// 3. Otherwise → collect raw `(kind, children)`, map each child, rebuild
///    via `arena.intern`, apply [`simplification`], cache and return result.
///
/// C++: `static Tree sigMap(Tree key, tfun f, Tree t)`.
fn sig_map(
    arena: &mut TreeArena,
    cache: &mut SimplifyCache,
    types: &HashMap<SigId, SigType>,
    sig: SigId,
) -> SigId {
    // 1. Cache check: None → return sig unchanged; Some(r) → return r.
    if let Some(cached) = cache.nodes.get(&sig) {
        return cached.unwrap_or(sig);
    }

    // 2. Rec node: mark sentinel before descending to avoid infinite loops.
    let rec_body = match match_sig(arena, sig) {
        SigMatch::Rec(body) => Some(body),
        _ => None,
    };
    if let Some(body) = rec_body {
        cache.nodes.insert(sig, None); // sentinel: if seen again, return sig
        let new_body = sig_map(arena, cache, types, body);
        return SigBuilder::new(arena).rec(new_body);
    }

    // 3. General case: get raw (kind, children) without holding an arena borrow.
    let (kind, children) = {
        let node = arena.node(sig).expect("sig_map: invalid SigId");
        (node.kind.clone(), node.children.as_slice().to_vec())
    };

    // Map each child recursively.
    let mut new_children: Vec<SigId> = Vec::with_capacity(children.len());
    for &c in &children {
        new_children.push(sig_map(arena, cache, types, c));
    }

    // Rebuild node with mapped children.
    let rebuilt = arena.intern(kind, &new_children);

    // Apply local simplification rules.
    let result = simplification(arena, types, rebuilt);

    // Cache the result.
    if result == sig {
        cache.nodes.insert(sig, None); // unchanged
    } else {
        cache.nodes.insert(sig, Some(result));
    }
    result
}

// ─── Local rewrite rules ──────────────────────────────────────────────────────

/// Apply local algebraic simplification rules to a single signal node.
///
/// Assumes all children have already been simplified by [`sig_map`].
/// Returns a simplified equivalent signal (may be the same `sig` if no rule
/// applies).
///
/// C++: `static Tree simplification(Tree sig)`.
fn simplification(arena: &mut TreeArena, types: &HashMap<SigId, SigType>, sig: SigId) -> SigId {
    match_simplification(arena, types, sig)
}

/// Inner implementation of simplification rules, separated to make borrow
/// scopes explicit.
fn match_simplification(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sig: SigId,
) -> SigId {
    // ── Math primitives (constant folding) ────────────────────────────────
    // C++ dispatched via xtended* getUserData(sig). In Rust these are explicit
    // SigMatch variants. Fold only when ALL arguments are numeric constants.

    // Single-argument math functions.
    let maybe_unary = match match_sig(arena, sig) {
        SigMatch::Acos(x) => Some(('a', x)),
        SigMatch::Asin(x) => Some(('b', x)),
        SigMatch::Atan(x) => Some(('c', x)),
        SigMatch::Cos(x) => Some(('d', x)),
        SigMatch::Sin(x) => Some(('e', x)),
        SigMatch::Tan(x) => Some(('f', x)),
        SigMatch::Exp(x) => Some(('g', x)),
        SigMatch::Exp10(x) => Some(('h', x)),
        SigMatch::Log(x) => Some(('i', x)),
        SigMatch::Log10(x) => Some(('j', x)),
        SigMatch::Sqrt(x) => Some(('k', x)),
        SigMatch::Abs(x) => Some(('l', x)),
        SigMatch::Floor(x) => Some(('m', x)),
        SigMatch::Ceil(x) => Some(('n', x)),
        SigMatch::Rint(x) => Some(('o', x)),
        SigMatch::Round(x) => Some(('p', x)),
        _ => None,
    };
    if let Some((tag, x)) = maybe_unary {
        let arg = match match_sig(arena, x) {
            SigMatch::Int(i) => Some(i as f64),
            SigMatch::Real(r) => Some(r),
            _ => None,
        };
        if let Some(v) = arg {
            let result = match tag {
                'a' => v.acos(),
                'b' => v.asin(),
                'c' => v.atan(),
                'd' => v.cos(),
                'e' => v.sin(),
                'f' => v.tan(),
                'g' => v.exp(),
                'h' => 10.0_f64.powf(v),
                'i' => v.ln(),
                'j' => v.log10(),
                'k' => v.sqrt(),
                'l' => v.abs(),
                'm' => v.floor(),
                'n' => v.ceil(),
                'o' => v.round(), // rint
                'p' => v.round(),
                _ => unreachable!(),
            };
            return SigBuilder::new(arena).real(result);
        }
        return sig;
    }

    // Two-argument math functions.
    let maybe_binary_math = match match_sig(arena, sig) {
        SigMatch::Atan2(x, y) => Some(('A', x, y)),
        SigMatch::Fmod(x, y) => Some(('B', x, y)),
        SigMatch::Remainder(x, y) => Some(('C', x, y)),
        SigMatch::Pow(x, y) => Some(('P', x, y)),
        SigMatch::Min(x, y) => Some(('D', x, y)),
        SigMatch::Max(x, y) => Some(('E', x, y)),
        _ => None,
    };
    if let Some((tag, x, y)) = maybe_binary_math {
        let vx = numeric_val(arena, x);
        let vy = numeric_val(arena, y);
        if let (Some(a), Some(b)) = (vx, vy) {
            let result = match tag {
                'A' => a.atan2(b),
                'B' => a % b, // fmod
                'C' => {
                    // remainder (round-to-nearest)
                    let q = (a / b).round();
                    a - q * b
                }
                'P' => a.powf(b),
                'D' => a.min(b),
                'E' => a.max(b),
                _ => unreachable!(),
            };
            // C++ parity: `min`/`max` are xtended primitives whose
            // `computeSigOutput` preserves the integer type when both
            // operands are `SigInt`.  All other two-arg math functions
            // (atan2, fmod, remainder, pow) always produce `SigReal`.
            let folded = if matches!(tag, 'D' | 'E')
                && matches!(match_sig(arena, x), SigMatch::Int(_))
                && matches!(match_sig(arena, y), SigMatch::Int(_))
            {
                SigBuilder::new(arena).int(result as i32)
            } else {
                SigBuilder::new(arena).real(result)
            };
            return if tag == 'P' {
                // Pow: apply normalize_add_term to the folded result (C++ special case).
                normalize_add_term(arena, types, folded)
            } else {
                folded
            };
        }
        // Non-constant Pow: apply normalize_add_term to the Pow expression.
        if tag == 'P' {
            return normalize_add_term(arena, types, sig);
        }
        return sig;
    }

    // ── BinOp ─────────────────────────────────────────────────────────────
    let binop_fields = match match_sig(arena, sig) {
        SigMatch::BinOp(op, t1, t2) => Some((op, t1, t2)),
        _ => None,
    };
    if let Some((op, t1, t2)) = binop_fields {
        // Constant folding: both numeric.
        if is_num(arena, t1)
            && is_num(arena, t2)
            && let Some(r) = fold_binop(op, t1, t2, arena)
        {
            return r;
        }

        // Negation rewrite rules.
        //   -n * (x - y)  →  n * (y - x)
        //   -1 * (x - y)  →  y - x
        //   (x - y) * -n  →  n * (y - x)
        //   (x - y) * -1  →  y - x
        if op == BinOp::Mul {
            if is_negative_num(arena, t1)
                && let SigMatch::BinOp(BinOp::Sub, v1, v2) = match_sig(arena, t2)
            {
                if is_minus_one(arena, t1) {
                    return SigBuilder::new(arena).sub(v2, v1);
                } else {
                    let pos_n = minus_num(arena, t1);
                    let sub = SigBuilder::new(arena).sub(v2, v1);
                    return SigBuilder::new(arena).mul(pos_n, sub);
                }
            }
            if is_negative_num(arena, t2)
                && let SigMatch::BinOp(BinOp::Sub, v1, v2) = match_sig(arena, t1)
            {
                if is_minus_one(arena, t2) {
                    return SigBuilder::new(arena).sub(v2, v1);
                } else {
                    let pos_n = minus_num(arena, t2);
                    let sub = SigBuilder::new(arena).sub(v2, v1);
                    return SigBuilder::new(arena).mul(pos_n, sub);
                }
            }

            // n * (m * x) → (n*m) * x   or   x  when n*m == 1
            // n * (x * m) → (n*m) * x   or   x  when n*m == 1
            if is_num(arena, t1) {
                let nested = match match_sig(arena, t2) {
                    SigMatch::BinOp(BinOp::Mul, v1, v2) if is_num(arena, v1) => {
                        Some((v1, v2, true))
                    }
                    SigMatch::BinOp(BinOp::Mul, v1, v2) if is_num(arena, v2) => {
                        Some((v2, v1, false))
                    }
                    _ => None,
                };
                if let Some((m, rest, _)) = nested {
                    let product = mul_nums(arena, t1, m);
                    if is_one(arena, product) {
                        return rest;
                    } else {
                        return SigBuilder::new(arena).mul(product, rest);
                    }
                }
            }
        }

        // Special: 0 - x → -1 * x
        if op == BinOp::Sub && is_zero(arena, t1) {
            let minus_one = SigBuilder::new(arena).int(-1);
            return SigBuilder::new(arena).mul(minus_one, t2);
        }

        // Neutral and absorbing element rules.
        if is_left_neutral(op, t1, arena) {
            return t2;
        }
        if is_left_absorbing(op, t1, arena) {
            return t1;
        }
        if is_right_neutral(op, t2, arena) {
            return t1;
        }
        if is_right_absorbing(op, t2, arena) {
            return t2;
        }

        // Self-operation rules: x op x
        if t1 == t2 {
            match op {
                BinOp::Sub => return SigBuilder::new(arena).int(0),
                BinOp::And | BinOp::Or => return t1,
                BinOp::Ge | BinOp::Le | BinOp::Eq => return SigBuilder::new(arena).int(1),
                BinOp::Gt | BinOp::Lt | BinOp::Ne | BinOp::Rem | BinOp::Xor => {
                    return SigBuilder::new(arena).int(0);
                }
                _ => {}
            }
        }

        // AND/OR with a boolean expression and literal 1.
        if matches!(op, BinOp::And | BinOp::Or) {
            if is_one(arena, t1) && is_sig_bool(arena, t2) {
                return if op == BinOp::And {
                    t2
                } else {
                    SigBuilder::new(arena).int(1)
                };
            }
            if is_one(arena, t2) && is_sig_bool(arena, t1) {
                return if op == BinOp::And {
                    t1
                } else {
                    SigBuilder::new(arena).int(1)
                };
            }
        }

        // Default BinOp path: normalize as an add-term.
        return normalize_add_term(arena, types, sig);
    }

    // ── Delay1 / Delay ────────────────────────────────────────────────────
    if let SigMatch::Delay1(t1) = match_sig(arena, sig) {
        return normalize_delay1_term(arena, types, t1);
    }
    if let SigMatch::Delay(t1, t2) = match_sig(arena, sig) {
        return normalize_delay_term(arena, types, t1, t2);
    }

    // ── Casts ─────────────────────────────────────────────────────────────
    if let SigMatch::IntCast(t1) = match_sig(arena, sig) {
        match match_sig(arena, t1) {
            SigMatch::Int(_) => return t1,
            SigMatch::Real(x) => return SigBuilder::new(arena).int(x as i32),
            _ => {}
        }
        return sig;
    }

    if let SigMatch::BitCast(_) = match_sig(arena, sig) {
        return sig;
    }

    if let SigMatch::FloatCast(t1) = match_sig(arena, sig) {
        match match_sig(arena, t1) {
            SigMatch::Int(i) => return SigBuilder::new(arena).real(f64::from(i)),
            SigMatch::Real(_) => return t1,
            _ => {}
        }
        return sig;
    }

    // ── Select2 ───────────────────────────────────────────────────────────
    if let SigMatch::Select2(t1, t2, t3) = match_sig(arena, sig) {
        match match_sig(arena, t1) {
            SigMatch::Int(0) => return t2, // select2(0, t, _) = t
            SigMatch::Int(_) => return t3, // select2(n≠0, _, t) = t
            _ => {}
        }
        if t2 == t3 {
            return t2; // select2(_, t, t) = t
        }
        return sig;
    }

    // ── Enable / Control ──────────────────────────────────────────────────
    // Enable(t1, 0) → 0; Enable(t1, 1) → t1; otherwise unchanged.
    if let SigMatch::Enable(t1, t2) = match_sig(arena, sig) {
        if is_zero(arena, t2) {
            return SigBuilder::new(arena).int(0);
        }
        if is_one(arena, t2) {
            return t1;
        }
        return sig;
    }

    // Control(t1, 0) → 0; Control(t1, 1) → t1; otherwise unchanged.
    if let SigMatch::Control(t1, t2) = match_sig(arena, sig) {
        if is_zero(arena, t2) {
            return SigBuilder::new(arena).int(0);
        }
        if is_one(arena, t2) {
            return t1;
        }
        return sig;
    }

    // ── Lowest / Highest ─────────────────────────────────────────────────
    // Extract the interval bound if type information is available.
    if let SigMatch::Lowest(t1) = match_sig(arena, sig) {
        if let Some(ty) = types.get(&t1) {
            let lo = ty.interval().lo();
            if lo.is_finite() {
                return SigBuilder::new(arena).real(lo);
            }
        }
        return sig;
    }
    if let SigMatch::Highest(t1) = match_sig(arena, sig) {
        if let Some(ty) = types.get(&t1) {
            let hi = ty.interval().hi();
            if hi.is_finite() {
                return SigBuilder::new(arena).real(hi);
            }
        }
        return sig;
    }

    // ── Default ───────────────────────────────────────────────────────────
    sig
}

// ─── BinOp constant folding ───────────────────────────────────────────────────

/// Compute `op(t1, t2)` when both are numeric constants.
///
/// Returns `None` for division/remainder by zero.
/// Faust `/` is real-valued even for integer literals, matching the C++ compiler.
fn fold_binop(op: BinOp, t1: SigId, t2: SigId, arena: &mut TreeArena) -> Option<SigId> {
    enum V {
        I(i32),
        F(f64),
    }
    let v1 = match match_sig(arena, t1) {
        SigMatch::Int(i) => V::I(i),
        SigMatch::Real(r) => V::F(r),
        _ => return None,
    };
    let v2 = match match_sig(arena, t2) {
        SigMatch::Int(i) => V::I(i),
        SigMatch::Real(r) => V::F(r),
        _ => return None,
    };

    // Pure integer case.
    if let (V::I(a), V::I(b)) = (&v1, &v2) {
        let (a, b) = (*a, *b);
        match op {
            BinOp::Add => return Some(SigBuilder::new(arena).int(a.wrapping_add(b))),
            BinOp::Sub => return Some(SigBuilder::new(arena).int(a.wrapping_sub(b))),
            BinOp::Mul => return Some(SigBuilder::new(arena).int(a.wrapping_mul(b))),
            BinOp::Div => {}
            BinOp::Rem => {
                if b == 0 {
                    return None;
                }
                return Some(SigBuilder::new(arena).int(a % b));
            }
            BinOp::Lsh => return Some(SigBuilder::new(arena).int(a << (b & 31))),
            BinOp::ARsh => return Some(SigBuilder::new(arena).int(a >> (b & 31))),
            BinOp::LRsh => {
                return Some(SigBuilder::new(arena).int(((a as u32) >> (b as u32 & 31)) as i32));
            }
            BinOp::Gt => return Some(SigBuilder::new(arena).int(i32::from(a > b))),
            BinOp::Lt => return Some(SigBuilder::new(arena).int(i32::from(a < b))),
            BinOp::Ge => return Some(SigBuilder::new(arena).int(i32::from(a >= b))),
            BinOp::Le => return Some(SigBuilder::new(arena).int(i32::from(a <= b))),
            BinOp::Eq => return Some(SigBuilder::new(arena).int(i32::from(a == b))),
            BinOp::Ne => return Some(SigBuilder::new(arena).int(i32::from(a != b))),
            BinOp::And => return Some(SigBuilder::new(arena).int(a & b)),
            BinOp::Or => return Some(SigBuilder::new(arena).int(a | b)),
            BinOp::Xor => return Some(SigBuilder::new(arena).int(a ^ b)),
        }
    }

    // At least one float — promote both to f64.
    let a = match &v1 {
        V::I(i) => f64::from(*i),
        V::F(f) => *f,
    };
    let b = match &v2 {
        V::I(i) => f64::from(*i),
        V::F(f) => *f,
    };
    let result = match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => {
            if b == 0.0 {
                return None;
            }
            a / b
        }
        BinOp::Rem => a % b,
        BinOp::Gt => f64::from(a > b),
        BinOp::Lt => f64::from(a < b),
        BinOp::Ge => f64::from(a >= b),
        BinOp::Le => f64::from(a <= b),
        BinOp::Eq => f64::from(a == b),
        BinOp::Ne => f64::from(a != b),
        // Bitwise ops: fall through to integer path via truncation
        BinOp::And => f64::from((a as i32) & (b as i32)),
        BinOp::Or => f64::from((a as i32) | (b as i32)),
        BinOp::Xor => f64::from((a as i32) ^ (b as i32)),
        BinOp::Lsh | BinOp::ARsh | BinOp::LRsh => {
            return None; // shift on float undefined
        }
    };
    Some(SigBuilder::new(arena).real(result))
}

// ─── Neutral / absorbing element queries ──────────────────────────────────────

/// True when `n` is the left-neutral element of `op`.
fn is_left_neutral(op: BinOp, n: SigId, arena: &TreeArena) -> bool {
    match op {
        BinOp::Add => is_zero(arena, n),
        BinOp::Mul => is_one(arena, n),
        BinOp::And => is_minus_one(arena, n),
        BinOp::Or | BinOp::Xor => is_zero(arena, n),
        _ => false,
    }
}

/// True when `n` is the left-absorbing element of `op`.
fn is_left_absorbing(op: BinOp, n: SigId, arena: &TreeArena) -> bool {
    match op {
        BinOp::Mul | BinOp::Div => is_zero(arena, n),
        BinOp::And => is_zero(arena, n),
        BinOp::Or => is_minus_one(arena, n),
        _ => false,
    }
}

/// True when `n` is the right-neutral element of `op`.
fn is_right_neutral(op: BinOp, n: SigId, arena: &TreeArena) -> bool {
    match op {
        BinOp::Add | BinOp::Sub => is_zero(arena, n),
        BinOp::Mul | BinOp::Div => is_one(arena, n),
        BinOp::And => is_minus_one(arena, n),
        BinOp::Or | BinOp::Xor => is_zero(arena, n),
        BinOp::Lsh | BinOp::ARsh | BinOp::LRsh => is_zero(arena, n),
        _ => false,
    }
}

/// True when `n` is the right-absorbing element of `op`.
fn is_right_absorbing(op: BinOp, n: SigId, arena: &TreeArena) -> bool {
    match op {
        BinOp::Mul => is_zero(arena, n),
        BinOp::And => is_zero(arena, n),
        BinOp::Or => is_minus_one(arena, n),
        _ => false,
    }
}

// ─── Boolean signal check ─────────────────────────────────────────────────────

/// True when `sig` is a boolean-valued signal (comparison or logical op over
/// boolean sub-expressions).
///
/// C++: `static bool isSigBool(Tree sig)`.
fn is_sig_bool(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::BinOp(op, x, y) => {
            is_bool_op(op) || (is_logical_op(op) && is_sig_bool(arena, x) && is_sig_bool(arena, y))
        }
        _ => false,
    }
}

fn is_bool_op(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne
    )
}

fn is_logical_op(op: BinOp) -> bool {
    matches!(op, BinOp::And | BinOp::Or | BinOp::Xor)
}

// ─── Numeric helpers ──────────────────────────────────────────────────────────

/// Extract a numeric value from a constant signal node, or `None`.
fn numeric_val(arena: &TreeArena, sig: SigId) -> Option<f64> {
    match match_sig(arena, sig) {
        SigMatch::Int(i) => Some(f64::from(i)),
        SigMatch::Real(r) => Some(r),
        _ => None,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use signals::SigMatch;

    use super::*;

    fn arena() -> TreeArena {
        TreeArena::new()
    }

    fn types() -> HashMap<SigId, SigType> {
        HashMap::new()
    }

    #[test]
    fn simplify_with_cache_reuses_seen_root() {
        let mut a = arena();
        let t = types();
        let one = SigBuilder::new(&mut a).int(1);
        let two = SigBuilder::new(&mut a).int(2);
        let add = SigBuilder::new(&mut a).add(one, two);

        let mut cache = SimplifyCache::new();
        let first = simplify_with_cache(&mut a, &mut cache, &t, add);
        let cached_nodes = cache.len();

        let second = simplify_with_cache(&mut a, &mut cache, &t, add);

        assert_eq!(match_sig(&a, first), SigMatch::Int(3));
        assert_eq!(first, second);
        assert_eq!(cache.len(), cached_nodes);
    }

    // ── Constant folding ──────────────────────────────────────────────────

    #[test]
    fn simplify_add_constants() {
        let mut a = arena();
        let t = types();
        let i3 = SigBuilder::new(&mut a).int(3);
        let i4 = SigBuilder::new(&mut a).int(4);
        let add = SigBuilder::new(&mut a).add(i3, i4);
        let r = simplify(&mut a, &t, add);
        assert_eq!(match_sig(&a, r), SigMatch::Int(7));
    }

    #[test]
    fn simplify_mul_constants() {
        let mut a = arena();
        let t = types();
        let i3 = SigBuilder::new(&mut a).int(3);
        let i4 = SigBuilder::new(&mut a).int(4);
        let mul = SigBuilder::new(&mut a).mul(i3, i4);
        let r = simplify(&mut a, &t, mul);
        assert_eq!(match_sig(&a, r), SigMatch::Int(12));
    }

    #[test]
    fn simplify_sub_constants() {
        let mut a = arena();
        let t = types();
        let i10 = SigBuilder::new(&mut a).int(10);
        let i3 = SigBuilder::new(&mut a).int(3);
        let sub = SigBuilder::new(&mut a).sub(i10, i3);
        let r = simplify(&mut a, &t, sub);
        assert_eq!(match_sig(&a, r), SigMatch::Int(7));
    }

    #[test]
    fn simplify_nested_constant_with_pow() {
        let mut a = arena();
        let t = types();
        let zero = SigBuilder::new(&mut a).int(0);
        let two = SigBuilder::new(&mut a).int(2);
        let pow = SigBuilder::new(&mut a).pow(zero, two);
        let inner = SigBuilder::new(&mut a).sub(zero, pow);
        let outer = SigBuilder::new(&mut a).sub(inner, zero);
        let cast = SigBuilder::new(&mut a).int_cast(outer);
        let eq = SigBuilder::new(&mut a).eq(cast, zero);

        let r = simplify(&mut a, &t, eq);
        assert_eq!(match_sig(&a, r), SigMatch::Int(1));
    }

    #[test]
    fn simplify_float_add() {
        let mut a = arena();
        let t = types();
        let r1 = SigBuilder::new(&mut a).real(1.5);
        let r2 = SigBuilder::new(&mut a).real(2.5);
        let add = SigBuilder::new(&mut a).add(r1, r2);
        let r = simplify(&mut a, &t, add);
        match match_sig(&a, r) {
            SigMatch::Real(v) => assert!((v - 4.0).abs() < 1e-10),
            other => panic!("expected Real(4.0), got {other:?}"),
        }
    }

    #[test]
    fn simplify_int_division_to_real_like_cpp() {
        let mut a = arena();
        let t = types();
        let i1 = SigBuilder::new(&mut a).int(1);
        let i3 = SigBuilder::new(&mut a).int(3);
        let div = SigBuilder::new(&mut a).div(i1, i3);
        let r = simplify(&mut a, &t, div);
        match match_sig(&a, r) {
            SigMatch::Real(v) => assert!((v - (1.0 / 3.0)).abs() < 1e-12),
            other => panic!("expected Real(1/3), got {other:?}"),
        }
    }

    // ── Neutral elements ──────────────────────────────────────────────────

    #[test]
    fn simplify_add_zero_left() {
        let mut a = arena();
        let t = types();
        let zero = SigBuilder::new(&mut a).int(0);
        let x = SigBuilder::new(&mut a).input(0);
        let add = SigBuilder::new(&mut a).add(zero, x);
        let r = simplify(&mut a, &t, add);
        assert_eq!(match_sig(&a, r), SigMatch::Input(0));
    }

    #[test]
    fn simplify_add_zero_right() {
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let zero = SigBuilder::new(&mut a).int(0);
        let add = SigBuilder::new(&mut a).add(x, zero);
        let r = simplify(&mut a, &t, add);
        assert_eq!(match_sig(&a, r), SigMatch::Input(0));
    }

    #[test]
    fn simplify_mul_one_left() {
        let mut a = arena();
        let t = types();
        let one = SigBuilder::new(&mut a).int(1);
        let x = SigBuilder::new(&mut a).input(0);
        let mul = SigBuilder::new(&mut a).mul(one, x);
        let r = simplify(&mut a, &t, mul);
        assert_eq!(match_sig(&a, r), SigMatch::Input(0));
    }

    #[test]
    fn simplify_mul_zero_left() {
        let mut a = arena();
        let t = types();
        let zero = SigBuilder::new(&mut a).int(0);
        let x = SigBuilder::new(&mut a).input(0);
        let mul = SigBuilder::new(&mut a).mul(zero, x);
        let r = simplify(&mut a, &t, mul);
        assert_eq!(match_sig(&a, r), SigMatch::Int(0));
    }

    // ── Negation rewrites ─────────────────────────────────────────────────

    #[test]
    fn simplify_minus_one_times_sub() {
        // -1 * (x - y) → y - x
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let sub = SigBuilder::new(&mut a).sub(x, y);
        let neg1 = SigBuilder::new(&mut a).int(-1);
        let mul = SigBuilder::new(&mut a).mul(neg1, sub);
        let r = simplify(&mut a, &t, mul);
        match match_sig(&a, r) {
            SigMatch::BinOp(BinOp::Sub, a_, b_) => {
                assert_eq!(a_, y);
                assert_eq!(b_, x);
            }
            other => panic!("expected y-x, got {other:?}"),
        }
    }

    #[test]
    fn simplify_sub_self_is_zero() {
        // x - x → 0  for any signal x
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let sub = SigBuilder::new(&mut a).sub(x, x);
        let r = simplify(&mut a, &t, sub);
        assert_eq!(match_sig(&a, r), SigMatch::Int(0));
    }

    // ── Casts ─────────────────────────────────────────────────────────────

    #[test]
    fn simplify_int_cast_of_real() {
        let mut a = arena();
        let t = types();
        let r = SigBuilder::new(&mut a).real(3.7);
        let cast = SigBuilder::new(&mut a).int_cast(r);
        let result = simplify(&mut a, &t, cast);
        assert_eq!(match_sig(&a, result), SigMatch::Int(3));
    }

    #[test]
    fn simplify_float_cast_of_int() {
        let mut a = arena();
        let t = types();
        let i = SigBuilder::new(&mut a).int(5);
        let cast = SigBuilder::new(&mut a).float_cast(i);
        let result = simplify(&mut a, &t, cast);
        match match_sig(&a, result) {
            SigMatch::Real(v) => assert!((v - 5.0).abs() < 1e-10),
            other => panic!("expected Real(5.0), got {other:?}"),
        }
    }

    // ── Select2 ───────────────────────────────────────────────────────────

    #[test]
    fn simplify_select2_zero_selector() {
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let zero = SigBuilder::new(&mut a).int(0);
        let sel = SigBuilder::new(&mut a).select2(zero, x, y);
        let r = simplify(&mut a, &t, sel);
        assert_eq!(r, x);
    }

    #[test]
    fn simplify_select2_nonzero_selector() {
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let one = SigBuilder::new(&mut a).int(1);
        let sel = SigBuilder::new(&mut a).select2(one, x, y);
        let r = simplify(&mut a, &t, sel);
        assert_eq!(r, y);
    }

    #[test]
    fn simplify_select2_same_branches() {
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let cond = SigBuilder::new(&mut a).input(1);
        let sel = SigBuilder::new(&mut a).select2(cond, x, x);
        let r = simplify(&mut a, &t, sel);
        assert_eq!(r, x);
    }

    // ── Enable / Control ──────────────────────────────────────────────────

    #[test]
    fn simplify_enable_with_one() {
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let one = SigBuilder::new(&mut a).int(1);
        let en = SigBuilder::new(&mut a).enable(x, one);
        let r = simplify(&mut a, &t, en);
        assert_eq!(r, x);
    }

    #[test]
    fn simplify_enable_with_zero() {
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let zero = SigBuilder::new(&mut a).int(0);
        let en = SigBuilder::new(&mut a).enable(x, zero);
        let r = simplify(&mut a, &t, en);
        assert_eq!(match_sig(&a, r), SigMatch::Int(0));
    }

    // ── Math primitive folding ────────────────────────────────────────────

    #[test]
    fn simplify_sin_of_zero() {
        let mut a = arena();
        let t = types();
        let zero = SigBuilder::new(&mut a).real(0.0);
        let sin_node = SigBuilder::new(&mut a).sin(zero);
        let r = simplify(&mut a, &t, sin_node);
        match match_sig(&a, r) {
            SigMatch::Real(v) => assert!(v.abs() < 1e-10),
            other => panic!("expected Real(0), got {other:?}"),
        }
    }

    // ── Memoization ───────────────────────────────────────────────────────

    #[test]
    fn simplify_shared_subexpression() {
        // x + 0 shared as both children: should be processed once, return x twice.
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let zero = SigBuilder::new(&mut a).int(0);
        let add_xz = SigBuilder::new(&mut a).add(x, zero); // x + 0 → x after simplify
        let add2 = SigBuilder::new(&mut a).add(add_xz, add_xz); // (x+0) + (x+0)
        let r = simplify(&mut a, &t, add2);
        // Result should be x + x (since x+0 → x)
        match match_sig(&a, r) {
            SigMatch::BinOp(BinOp::Add, a_, b_) => {
                assert_eq!(match_sig(&a, a_), SigMatch::Input(0));
                assert_eq!(match_sig(&a, b_), SigMatch::Input(0));
            }
            // normalize_add_term might further simplify x+x → 2*x
            SigMatch::BinOp(BinOp::Mul, coef, base) => {
                assert_eq!(match_sig(&a, coef), SigMatch::Int(2));
                assert_eq!(match_sig(&a, base), SigMatch::Input(0));
            }
            other => panic!("expected x+x or 2*x, got {other:?}"),
        }
    }

    /// `min(int, int)` must fold to `SigInt`, not `SigReal`.
    ///
    /// C++ parity: xtended `computeSigOutput` preserves the integer type when
    /// both operands of `min`/`max` are integers.  A regression here caused
    /// `poly(max(1,min(N,4)), x)` pattern matching to fail because the argument
    /// `max(1,min(2,4))` simplified to `SigReal(2.0)` instead of `SigInt(2)`,
    /// and the automaton stored `Constant(boxInt(2))` — so `boxReal(2.0) ≠
    /// boxInt(2)` → "no case rule matches" (`carre_volterra.dsp` regression).
    #[test]
    fn simplify_min_int_int_preserves_int_type() {
        let mut a = arena();
        let t = types();
        let i2 = SigBuilder::new(&mut a).int(2);
        let i4 = SigBuilder::new(&mut a).int(4);
        let m = SigBuilder::new(&mut a).min(i2, i4);
        let r = simplify(&mut a, &t, m);
        assert_eq!(
            match_sig(&a, r),
            SigMatch::Int(2),
            "min(int(2), int(4)) should fold to SigInt(2), not SigReal"
        );
    }

    #[test]
    fn simplify_max_int_int_preserves_int_type() {
        let mut a = arena();
        let t = types();
        let i1 = SigBuilder::new(&mut a).int(1);
        let i2 = SigBuilder::new(&mut a).int(2);
        let m = SigBuilder::new(&mut a).max(i1, i2);
        let r = simplify(&mut a, &t, m);
        assert_eq!(
            match_sig(&a, r),
            SigMatch::Int(2),
            "max(int(1), int(2)) should fold to SigInt(2), not SigReal"
        );
    }

    /// `min(real, real)` must still fold to `SigReal`.
    #[test]
    fn simplify_min_real_real_returns_real() {
        let mut a = arena();
        let t = types();
        let r1 = SigBuilder::new(&mut a).real(1.5);
        let r2 = SigBuilder::new(&mut a).real(3.0);
        let m = SigBuilder::new(&mut a).min(r1, r2);
        let result = simplify(&mut a, &t, m);
        assert!(
            matches!(match_sig(&a, result), SigMatch::Real(v) if (v - 1.5).abs() < 1e-12),
            "min(real(1.5), real(3.0)) should fold to SigReal(1.5)"
        );
    }
}
