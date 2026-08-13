//! Add-term and delay-term normalization.
//!
//! Ported from C++ `compiler/normalize/normalize.cpp`.
// Internal module — wired into the simplify/normalform pipeline in Phase 2.
#![allow(dead_code)]
//!
//! # Functions
//!
//! - [`normalize_add_term`]: factor-extract loop on an [`Aterm`].
//! - [`normalize_delay1_term`]: 1-sample delay normalization (delegates to
//!   [`normalize_delay_term`] with `d = 1`).
//! - [`normalize_delay_term`]: full delay normalization; dispatches to
//!   [`clock_normalize_delay_term`] for clocked signals, otherwise returns
//!   `sigDelay(s, d)` unchanged.
//!
//! # API mapping status
//! - `normalizeAddTerm(t)` → [`normalize_add_term`]
//! - `normalizeDelay1Term(s)` → [`normalize_delay1_term`]
//! - `normalizeDelayTerm(s, d)` → [`normalize_delay_term`]
//! - `clockNormalizeDelayTerm(clk, s, d)` → [`clock_normalize_delay_term`]

use std::collections::HashMap;

use signals::{BinOp, SigBuilder, SigId, SigMatch, match_sig};
use sigtype::SigType;
use tlib::TreeArena;
#[cfg(debug_assertions)]
use tlib::{check_de_bruijn_coherence, is_de_bruijn_closed};

use crate::aterm::Aterm;
use crate::mterm::sig_order;

// ─── Add-term normalization ───────────────────────────────────────────────────

/// Compute the add-normal form of a signal expression `t`.
///
/// Algorithm:
/// 1. Decompose `t` into an [`Aterm`].
/// 2. Compute its `greatest_divisor` `D`.
/// 3. While `D.is_not_zero() && D.complexity() > 0`: factorize by `D`,
///    recompute `D`.
/// 4. Return `aterm.normalized_tree()`.
///
/// C++: `Tree normalizeAddTerm(Tree t)`.
pub(crate) fn normalize_add_term(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    t: SigId,
) -> SigId {
    #[cfg(debug_assertions)]
    if is_de_bruijn_closed(arena, t)
        && let Err(e) = check_de_bruijn_coherence(arena, t)
    {
        panic!(
            "normalize_add_term received an incoherent De Bruijn tree: {e}\n\
             Closed De Bruijn trees must be coherent before normalization."
        );
    }

    let mut a = Aterm::from_sig(arena, types, t);
    let mut d = a.greatest_divisor(arena, types);
    while d.is_not_zero(arena) && d.complexity(arena, types) > 0 {
        a = a.factorize(arena, types, &d);
        d = a.greatest_divisor(arena, types);
    }
    a.normalized_tree(arena, types)
}

// ─── Delay-term normalization ─────────────────────────────────────────────────

/// Compute the normal form of a 1-sample delay `s'`.
///
/// Delegates to [`normalize_delay_term`] with `d = sigInt(1)`.
///
/// C++: `Tree normalizeDelay1Term(Tree s)`.
pub(crate) fn normalize_delay1_term(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    s: SigId,
) -> SigId {
    let one = SigBuilder::new(arena).int(1);
    normalize_delay_term(arena, types, s, one)
}

/// Compute the normal form of a delay expression `s @ d`.
///
/// If `s` is a clocked signal (`SigMatch::Clocked(clk, inner)`), dispatch to
/// [`clock_normalize_delay_term`].  Otherwise return `sigDelay(s, d)` directly
/// — no further algebraic transformation is possible without clock context.
///
/// C++: `Tree normalizeDelayTerm(Tree s, Tree d)`.
pub(crate) fn normalize_delay_term(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    s: SigId,
    d: SigId,
) -> SigId {
    // Check whether s is a clocked signal.
    if let SigMatch::Clocked(clk, _inner) = match_sig(arena, s) {
        let stripped_s = unclock(arena, s);
        let stripped_d = unclock(arena, d);
        clock_normalize_delay_term(arena, types, clk, stripped_s, stripped_d)
    } else {
        SigBuilder::new(arena).delay(s, d)
    }
}

// ─── Clock-scoped delay normalization ────────────────────────────────────────

/// Compute the normal form of a delay expression `(clk :: s) @ d`.
///
/// Applies the following rewrite rules inside the clock scope:
///
/// | Pattern | Rule |
/// |---------|------|
/// | `s @ 0` | `clk :: s` (or `sigDelay(clk :: s, 0)` for projections) |
/// | `0 @ d` | `clk :: 0` |
/// | `(k * s) @ d` where `k` has order < 2 | `k * ((clk :: s) @ d)` |
/// | `(s / k) @ d` where `k` has order < 2 | `((clk :: s) @ d) / k` |
/// | `(s @ n) @ m` where `n` has order < 2 | `(clk :: s) @ (n + m)` (fold) |
/// | otherwise | `sigDelay(clk :: s, d)` |
///
/// C++: `Tree clockNormalizeDelayTerm(Tree clock, Tree s, Tree d)`.
pub(crate) fn clock_normalize_delay_term(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    clock: SigId,
    s: SigId,
    d: SigId,
) -> SigId {
    // Rule: s @ 0 → clock :: s
    if is_zero(arena, d) {
        if let SigMatch::Proj(_, _) = match_sig(arena, s) {
            let clocked_s = SigBuilder::new(arena).clocked(clock, s);
            return SigBuilder::new(arena).delay(clocked_s, d);
        }
        return SigBuilder::new(arena).clocked(clock, s);
    }

    // Rule: 0 @ d → clock :: 0
    if is_zero(arena, s) {
        return SigBuilder::new(arena).clocked(clock, s);
    }

    // Rule: (k * s) @ d → k * ((clock :: s) @ d)  when order(k) < 2
    if let SigMatch::BinOp(BinOp::Mul, x, y) = match_sig(arena, s) {
        let xu = unclock(arena, x);
        let yu = unclock(arena, y);
        if sig_order(types, xu) < 2 {
            let inner = clock_normalize_delay_term(arena, types, clock, yu, d);
            return SigBuilder::new(arena).mul(xu, inner);
        } else if sig_order(types, yu) < 2 {
            let inner = clock_normalize_delay_term(arena, types, clock, xu, d);
            return SigBuilder::new(arena).mul(yu, inner);
        } else {
            let clocked_s = SigBuilder::new(arena).clocked(clock, s);
            return SigBuilder::new(arena).delay(clocked_s, d);
        }
    }

    // Rule: (s / k) @ d → ((clock :: s) @ d) / k  when order(k) < 2
    if let SigMatch::BinOp(BinOp::Div, x, y) = match_sig(arena, s) {
        let xu = unclock(arena, x);
        let yu = unclock(arena, y);
        if sig_order(types, yu) < 2 {
            let inner = clock_normalize_delay_term(arena, types, clock, xu, d);
            return SigBuilder::new(arena).div(inner, yu);
        } else {
            let clocked_s = SigBuilder::new(arena).clocked(clock, s);
            return SigBuilder::new(arena).delay(clocked_s, d);
        }
    }

    // Rule: (s @ n) @ m → (clock :: s) @ (n + m)  when order(n) < 2
    if let SigMatch::Delay(x, y) = match_sig(arena, s) {
        let xu = unclock(arena, x);
        let yu = unclock(arena, y);
        if sig_order(types, yu) < 2 {
            let sum = SigBuilder::new(arena).add(d, yu);
            return clock_normalize_delay_term(arena, types, clock, xu, sum);
        } else {
            let clocked_s = SigBuilder::new(arena).clocked(clock, s);
            return SigBuilder::new(arena).delay(clocked_s, d);
        }
    }

    // Fallthrough: emit sigDelay(clock :: s, d)
    let clocked_s = SigBuilder::new(arena).clocked(clock, s);
    SigBuilder::new(arena).delay(clocked_s, d)
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Return `true` if `sig` is the integer or real constant zero.
fn is_zero(arena: &TreeArena, sig: SigId) -> bool {
    crate::mterm::is_zero(arena, sig)
}

/// Strip all `Clocked` wrappers from a signal, returning the innermost node.
///
/// C++: `static Tree unclock(Tree s)`.
fn unclock(arena: &mut TreeArena, s: SigId) -> SigId {
    // We need to iteratively peel Clocked wrappers.
    // Each call to match_sig borrows arena immutably; after we have the inner
    // SigId we can recurse.
    let inner = match match_sig(arena, s) {
        SigMatch::Clocked(_clk, inner) => Some(inner),
        _ => None,
    };
    if let Some(i) = inner {
        unclock(arena, i)
    } else {
        s
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

    // ── normalize_add_term ────────────────────────────────────────────────────

    #[test]
    fn normalize_add_term_constant_fold() {
        // 3 + 4 → 7
        let mut a = arena();
        let t = types();
        let i3 = SigBuilder::new(&mut a).int(3);
        let i4 = SigBuilder::new(&mut a).int(4);
        let add = SigBuilder::new(&mut a).add(i3, i4);
        let r = normalize_add_term(&mut a, &t, add);
        assert_eq!(match_sig(&a, r), SigMatch::Int(7));
    }

    #[test]
    fn normalize_add_term_passthrough_non_add() {
        // A single input signal x — not an add expression.
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let r = normalize_add_term(&mut a, &t, x);
        // Should return x unchanged (no factors to extract, one mterm).
        assert_eq!(match_sig(&a, r), SigMatch::Input(0));
    }

    #[test]
    fn normalize_add_term_factorize() {
        // 2*x + 4*x*y — gcd contains x → factorized to x*(2 + 4*y)
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let two = SigBuilder::new(&mut a).int(2);
        let s1 = SigBuilder::new(&mut a).mul(two, x);
        let four = SigBuilder::new(&mut a).int(4);
        let xy = SigBuilder::new(&mut a).mul(x, y);
        let s2 = SigBuilder::new(&mut a).mul(four, xy);
        let add = SigBuilder::new(&mut a).add(s1, s2);
        let r = normalize_add_term(&mut a, &t, add);
        // Result should be a Mul node (factorized form).
        match match_sig(&a, r) {
            SigMatch::BinOp(BinOp::Mul, _, _) => {}
            other => panic!("expected Mul (factorized), got {other:?}"),
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "normalize_add_term received an incoherent De Bruijn tree")]
    fn normalize_add_term_debug_rejects_incoherent_de_bruijn_input() {
        let mut a = arena();
        let t = types();
        let zero_level = tlib::de_bruijn_ref(&mut a, 0);
        let root = tlib::de_bruijn_rec(&mut a, zero_level);

        let _ = normalize_add_term(&mut a, &t, root);
    }

    // ── normalize_delay_term ──────────────────────────────────────────────────

    #[test]
    fn normalize_delay_non_clocked_returns_delay_node() {
        // x @ 5 for a non-clocked signal → sigDelay(x, 5)
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let five = SigBuilder::new(&mut a).int(5);
        let r = normalize_delay_term(&mut a, &t, x, five);
        match match_sig(&a, r) {
            SigMatch::Delay(s, d) => {
                assert_eq!(s, x);
                assert_eq!(match_sig(&a, d), SigMatch::Int(5));
            }
            other => panic!("expected Delay, got {other:?}"),
        }
    }

    #[test]
    fn normalize_delay1_term_produces_delay_of_one() {
        // normalize_delay1_term(x) → sigDelay(x, 1) for non-clocked x
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let r = normalize_delay1_term(&mut a, &t, x);
        match match_sig(&a, r) {
            SigMatch::Delay(s, d) => {
                assert_eq!(s, x);
                assert_eq!(match_sig(&a, d), SigMatch::Int(1));
            }
            other => panic!("expected Delay(x, 1), got {other:?}"),
        }
    }

    // ── clock_normalize_delay_term ────────────────────────────────────────────

    #[test]
    fn clock_delay_zero_returns_clocked_signal() {
        // (clk :: s) @ 0 → clk :: s
        let mut a = arena();
        let t = types();
        let clk = SigBuilder::new(&mut a).input(0);
        let s = SigBuilder::new(&mut a).input(1);
        let zero = SigBuilder::new(&mut a).int(0);
        let r = clock_normalize_delay_term(&mut a, &t, clk, s, zero);
        match match_sig(&a, r) {
            SigMatch::Clocked(c, inner) => {
                assert_eq!(c, clk);
                assert_eq!(inner, s);
            }
            other => panic!("expected Clocked, got {other:?}"),
        }
    }

    #[test]
    fn clock_delay_zero_signal_returns_clocked_zero() {
        // (clk :: 0) @ d → clk :: 0
        let mut a = arena();
        let t = types();
        let clk = SigBuilder::new(&mut a).input(0);
        let zero_sig = SigBuilder::new(&mut a).int(0);
        let d = SigBuilder::new(&mut a).int(5);
        let r = clock_normalize_delay_term(&mut a, &t, clk, zero_sig, d);
        match match_sig(&a, r) {
            SigMatch::Clocked(c, inner) => {
                assert_eq!(c, clk);
                assert_eq!(match_sig(&a, inner), SigMatch::Int(0));
            }
            other => panic!("expected Clocked(clk, 0), got {other:?}"),
        }
    }

    #[test]
    fn unclock_strips_single_wrapper() {
        let mut a = arena();
        let clk = SigBuilder::new(&mut a).input(0);
        let inner = SigBuilder::new(&mut a).input(1);
        let clocked = SigBuilder::new(&mut a).clocked(clk, inner);
        assert_eq!(unclock(&mut a, clocked), inner);
    }

    #[test]
    fn unclock_passthrough_plain_signal() {
        let mut a = arena();
        let x = SigBuilder::new(&mut a).input(0);
        assert_eq!(unclock(&mut a, x), x);
    }
}
