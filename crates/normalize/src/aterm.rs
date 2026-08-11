//! Additive term: a sum of [`Mterm`]s — `m₁ + m₂ + m₃ + …`
//!
//! Ported from C++ `compiler/normalize/aterm.cpp`.
// Internal module — wired into the simplify/normalform pipeline in Phase 2.
#![allow(dead_code)]
//!
//! An [`Aterm`] decomposes an additive signal expression into its canonical
//! multiplicative fragments.  Terms with the same *signature* (i.e. the same
//! set of non-constant factors) are merged into a single [`Mterm`] whose
//! coefficient is updated accordingly.
//!
//! # API mapping status
//! - `aterm()` → [`Aterm::zero`]
//! - `aterm(Tree t)` → [`Aterm::from_sig`]
//! - `operator+=(Tree)` / `operator-=(Tree)` → [`Aterm::add_sig`] / [`Aterm::sub_sig`]
//! - `operator+=(mterm)` / `operator-=(mterm)` → [`Aterm::add_mterm`] / [`Aterm::sub_mterm`]
//! - `normalizedTree()` → [`Aterm::normalized_tree`]
//! - `greatestDivisor()` → [`Aterm::greatest_divisor`]
//! - `factorize(d)` → [`Aterm::factorize`]
//! - `simplifyingAdd(t1, t2)` → [`simplifying_add`]

use std::collections::{BTreeMap, HashMap};

use signals::{BinOp, SigBuilder, SigId, SigMatch, match_sig};
use sigtype::SigType;
use tlib::TreeArena;

use crate::mterm::{self, Mterm, add_nums, gcd, is_num, is_zero, sig_order, sub_nums};

// ─── Aterm ────────────────────────────────────────────────────────────────────

/// An additive term: a canonical sum of [`Mterm`]s.
///
/// Internally stored as a map from *signature* (the [`Mterm`] without its
/// coefficient) to the full [`Mterm`].  Terms with identical signatures are
/// merged by adding their coefficients.
///
/// C++ equivalent: `class aterm` with `fSig2MTerms: std::map<Tree, mterm>`.
pub(crate) struct Aterm {
    /// Signature → mterm map.  `BTreeMap` for deterministic ordering matching
    /// C++ `std::map<Tree, mterm>`.
    terms: BTreeMap<SigId, Mterm>,
}

impl Aterm {
    /// Create an empty additive term (equivalent to `0`).
    ///
    /// C++: `aterm()`.
    pub(crate) fn zero() -> Self {
        Self {
            terms: BTreeMap::new(),
        }
    }

    /// Decompose a signal tree `t` into an additive term.
    ///
    /// Recursively splits additions and subtractions, accumulating [`Mterm`]s.
    ///
    /// C++: `aterm(Tree t)` — delegates to `*this += t`.
    pub(crate) fn from_sig(
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        t: SigId,
    ) -> Self {
        let mut a = Self::zero();
        a.add_sig(arena, types, t);
        a
    }

    /// Add an additive signal expression in place.
    ///
    /// Recursively decomposes `t` through Add/Sub nodes; any other node is
    /// wrapped in a single [`Mterm`] and merged into the map.
    ///
    /// C++: `const aterm& operator+=(Tree t)`.
    pub(crate) fn add_sig(
        &mut self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        t: SigId,
    ) {
        // Extract classification before borrowing arena mutably again.
        enum Op {
            Plus(SigId, SigId),
            Minus(SigId, SigId),
            Leaf,
        }
        let op = match match_sig(arena, t) {
            SigMatch::BinOp(BinOp::Add, x, y) => Op::Plus(x, y),
            SigMatch::BinOp(BinOp::Sub, x, y) => Op::Minus(x, y),
            _ => Op::Leaf,
        };
        match op {
            Op::Plus(x, y) => {
                self.add_sig(arena, types, x);
                self.add_sig(arena, types, y);
            }
            Op::Minus(x, y) => {
                self.add_sig(arena, types, x);
                self.sub_sig(arena, types, y);
            }
            Op::Leaf => {
                let m = Mterm::from_sig(arena, t);
                self.add_mterm(arena, &m);
            }
        }
    }

    /// Subtract an additive signal expression in place.
    ///
    /// Mirrors [`add_sig`][Self::add_sig] but negates the contribution.
    ///
    /// C++: `const aterm& operator-=(Tree t)`.
    pub(crate) fn sub_sig(
        &mut self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        t: SigId,
    ) {
        enum Op {
            Plus(SigId, SigId),
            Minus(SigId, SigId),
            Leaf,
        }
        let op = match match_sig(arena, t) {
            SigMatch::BinOp(BinOp::Add, x, y) => Op::Plus(x, y),
            SigMatch::BinOp(BinOp::Sub, x, y) => Op::Minus(x, y),
            _ => Op::Leaf,
        };
        match op {
            Op::Plus(x, y) => {
                self.sub_sig(arena, types, x);
                self.sub_sig(arena, types, y);
            }
            Op::Minus(x, y) => {
                self.sub_sig(arena, types, x);
                self.add_sig(arena, types, y);
            }
            Op::Leaf => {
                let m = Mterm::from_sig(arena, t);
                self.sub_mterm(arena, &m);
            }
        }
    }

    /// Merge an [`Mterm`] into this term (addition).
    ///
    /// If a term with the same signature already exists, their coefficients are
    /// summed.  Otherwise the mterm is inserted as a new entry.
    ///
    /// C++: `const aterm& operator+=(const mterm& m)`.
    pub(crate) fn add_mterm(&mut self, arena: &mut TreeArena, m: &Mterm) {
        let sig = m.signature_tree(arena);
        if let Some(existing) = self.terms.get_mut(&sig) {
            existing.add_mterm(arena, m);
        } else {
            self.terms.insert(sig, m.clone());
        }
    }

    /// Merge an [`Mterm`] into this term (subtraction).
    ///
    /// Negates `m` before merging: if the signature is new, inserts `m * (−1)`;
    /// otherwise subtracts `m` from the existing mterm.
    ///
    /// C++: `const aterm& operator-=(const mterm& m)`.
    pub(crate) fn sub_mterm(&mut self, arena: &mut TreeArena, m: &Mterm) {
        let sig = m.signature_tree(arena);
        if let Some(existing) = self.terms.get_mut(&sig) {
            existing.sub_mterm(arena, m);
        } else {
            let neg_one = Mterm::from_int(arena, -1);
            let negated = m.mul(arena, &neg_one);
            self.terms.insert(sig, negated);
        }
    }

    /// Reconstruct the canonical additive signal tree.
    ///
    /// Terms are separated into positive (`P`) and negative (`N`) buckets by
    /// variability order (0=Konst, 1=Block, 2=Samp).  The buckets are then
    /// folded from highest to lowest order using [`add_terms_with_sign`], with
    /// the constant bucket at order 0 forming the initial sum via subtraction.
    ///
    /// C++: `Tree aterm::normalizedTree() const`.
    pub(crate) fn normalized_tree(
        &self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
    ) -> SigId {
        const ORDERS: usize = 3;
        let zero = SigBuilder::new(arena).int(0);
        let mut p = [zero; ORDERS]; // positive terms by order
        let mut n = [zero; ORDERS]; // negative terms (made positive) by order

        // Collect mterms — clone to avoid arena borrow conflicts during tree building.
        let mterms: Vec<Mterm> = self.terms.values().cloned().collect();

        for m in &mterms {
            if m.is_negative(arena) {
                // negative_mode=true: invert sign for the tree
                let t = m.normalized_tree(arena, types, false, true);
                let ord = sig_order(types, t) as usize;
                n[ord] = simplifying_add(arena, n[ord], t);
            } else {
                let t = m.normalized_tree(arena, types, false, false);
                let ord = sig_order(types, t) as usize;
                p[ord] = simplifying_add(arena, p[ord], t);
            }
        }

        // Initial SUM = P[0] - N[0] (constant-order terms).
        //
        // C++ starts with `subNums(P[0], N[0])`. Some Rust constant-order terms
        // are not numeric literals after the current pass (for example
        // normalized UI-constant expressions), so retain the generic fallback
        // for those while keeping the numeric path canonical. In particular,
        // `0 - 0` must remain a positive zero rather than becoming `-1 * 0`.
        let (mut sign, mut sum) = if is_num(arena, p[0]) && is_num(arena, n[0]) {
            (true, sub_nums(arena, p[0], n[0]))
        } else {
            add_terms_with_sign(arena, true, p[0], false, n[0])
        };

        // Fold from highest to lowest order.
        // C++: loop from order=3 down to 1; here from (ORDERS-1) down to 1.
        for order in (1..ORDERS).rev() {
            let (s, r) = add_terms_with_sign(arena, false, n[order], sign, sum);
            sign = s;
            sum = r;

            let (s, r) = add_terms_with_sign(arena, true, p[order], sign, sum);
            sign = s;
            sum = r;
        }

        if !sign {
            // Result is negative: wrap in -1 * SUM.
            let minus_one = SigBuilder::new(arena).int(-1);
            sum = SigBuilder::new(arena).mul(minus_one, sum);
        }

        sum
    }

    /// Return the greatest common divisor of any two mterms.
    ///
    /// Iterates all pairs and returns the [`Mterm`] with the highest complexity.
    /// Returns `1` (no common factor) when there are fewer than two terms.
    ///
    /// C++: `mterm aterm::greatestDivisor() const`.
    pub(crate) fn greatest_divisor(
        &self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
    ) -> Mterm {
        let mut max_complexity = 0i32;
        let mut max_gcd = Mterm::from_int(arena, 1);

        let mterms: Vec<Mterm> = self.terms.values().cloned().collect();
        for i in 0..mterms.len() {
            for j in (i + 1)..mterms.len() {
                let g = gcd(arena, &mterms[i], &mterms[j]);
                let c = g.complexity(arena, types);
                if c > max_complexity {
                    max_complexity = c;
                    max_gcd = g;
                }
            }
        }
        max_gcd
    }

    /// Factorize `d` out of this aterm.
    ///
    /// Splits the terms into those that are exact multiples of `d` (forming
    /// quotient aterm `Q`) and the remainder `A`.  Returns `A + d * Q`.
    ///
    /// C++: `aterm aterm::factorize(const mterm& d)`.
    pub(crate) fn factorize(
        &self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        d: &Mterm,
    ) -> Aterm {
        let mut a = Aterm::zero(); // remainder (terms not divisible by d)
        let mut q = Aterm::zero(); // quotients of terms divisible by d

        let mterms: Vec<Mterm> = self.terms.values().cloned().collect();
        for t in &mterms {
            if t.has_divisor(arena, d) {
                let quot = t.div(arena, d);
                q.add_mterm(arena, &quot);
            } else {
                a.add_mterm(arena, t);
            }
        }

        // A += sigMul(d.normalizedTree(), Q.normalizedTree())
        let d_tree = d.normalized_tree(arena, types, false, false);
        let q_tree = q.normalized_tree(arena, types);
        let mul = SigBuilder::new(arena).mul(d_tree, q_tree);
        a.add_sig(arena, types, mul);
        a
    }
}

// ─── Free functions ───────────────────────────────────────────────────────────

/// Combine two signal trees as a sum, folding numeric constants and
/// ordering non-constant operands by their `SigId` for determinism.
///
/// Rules (in order):
/// - Both numeric: fold to a single constant via [`add_nums`].
/// - `t1 == 0`: return `t2`.
/// - `t2 == 0`: return `t1`.
/// - Otherwise: `sigAdd(min(t1,t2), max(t1,t2))` (SigId ordering replaces
///   C++ `serial()` ordering for determinism).
///
/// C++: `Tree simplifyingAdd(Tree t1, Tree t2)`.
pub(crate) fn simplifying_add(arena: &mut TreeArena, t1: SigId, t2: SigId) -> SigId {
    let num1 = mterm::is_num(arena, t1);
    let num2 = mterm::is_num(arena, t2);

    if num1 && num2 {
        return add_nums(arena, t1, t2);
    }
    if is_zero(arena, t1) {
        return t2;
    }
    if is_zero(arena, t2) {
        return t1;
    }
    // Order by SigId (hash-cons id) for deterministic tree shape.
    // C++ uses `t1->serial() <= t2->serial()`.
    if t1 <= t2 {
        SigBuilder::new(arena).add(t1, t2)
    } else {
        SigBuilder::new(arena).add(t2, t1)
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Combine two signed partial sums into a new signed partial sum.
///
/// Implements the following truth table (0 denotes the zero signal):
///
/// | p1  | v1 | p2  | v2 | p3  | v3      |
/// |-----|----|-----|----|-----|---------|
/// | any | 0  | p2  | v2 | p2  | v2      |
/// | p1  | v1 | any | 0  | p1  | v1      |
/// | +   | v1 | +   | v2 | +   | v1+v2   |
/// | +   | v1 | −   | v2 | +   | v1−v2   |
/// | −   | v1 | +   | v2 | +   | v2−v1   |
/// | −   | v1 | −   | v2 | −   | v1+v2   |
///
/// Returns `(sign: bool, value: SigId)`.
///
/// C++: `static void addTermsWithSign(bool p1, Tree v1, bool p2, Tree v2, bool& p3, Tree& v3)`.
fn add_terms_with_sign(
    arena: &mut TreeArena,
    p1: bool,
    v1: SigId,
    p2: bool,
    v2: SigId,
) -> (bool, SigId) {
    if is_zero(arena, v1) {
        return (p2, v2);
    }
    if is_zero(arena, v2) {
        return (p1, v1);
    }
    match (p1, p2) {
        (true, true) => (true, SigBuilder::new(arena).add(v1, v2)),
        (true, false) => (true, SigBuilder::new(arena).sub(v1, v2)),
        (false, true) => (true, SigBuilder::new(arena).sub(v2, v1)),
        (false, false) => (false, SigBuilder::new(arena).add(v1, v2)),
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

    // ── simplifying_add ──────────────────────────────────────────────────────

    #[test]
    fn simplifying_add_two_ints() {
        let mut a = arena();
        let i3 = SigBuilder::new(&mut a).int(3);
        let i4 = SigBuilder::new(&mut a).int(4);
        let r = simplifying_add(&mut a, i3, i4);
        assert_eq!(match_sig(&a, r), SigMatch::Int(7));
    }

    #[test]
    fn simplifying_add_zero_left() {
        let mut a = arena();
        let zero = SigBuilder::new(&mut a).int(0);
        let x = SigBuilder::new(&mut a).input(0);
        let r = simplifying_add(&mut a, zero, x);
        assert_eq!(r, x);
    }

    #[test]
    fn simplifying_add_zero_right() {
        let mut a = arena();
        let x = SigBuilder::new(&mut a).input(0);
        let zero = SigBuilder::new(&mut a).int(0);
        let r = simplifying_add(&mut a, x, zero);
        assert_eq!(r, x);
    }

    #[test]
    fn simplifying_add_nonzero_produces_add_node() {
        let mut a = arena();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let r = simplifying_add(&mut a, x, y);
        match match_sig(&a, r) {
            SigMatch::BinOp(BinOp::Add, _, _) => {}
            other => panic!("expected Add, got {other:?}"),
        }
    }

    // ── Aterm construction ────────────────────────────────────────────────────

    #[test]
    fn aterm_from_add_signal() {
        // aterm(x + y) should have two mterms (one for x, one for y).
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let add = SigBuilder::new(&mut a).add(x, y);
        let at = Aterm::from_sig(&mut a, &t, add);
        assert_eq!(at.terms.len(), 2);
    }

    #[test]
    fn aterm_from_sub_signal() {
        // aterm(x - y): two mterms; the mterm for y should be negative.
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let sub = SigBuilder::new(&mut a).sub(x, y);
        let at = Aterm::from_sig(&mut a, &t, sub);
        assert_eq!(at.terms.len(), 2);
        // mterm for y should be negative (coefficient −1)
        let m_y = at.terms.values().find(|m| m.factors.contains_key(&y));
        assert!(m_y.is_some());
        assert!(m_y.unwrap().is_negative(&a));
    }

    #[test]
    fn aterm_merges_same_signature() {
        // aterm(x + x) = 2*x — one mterm with coefficient 2.
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let add = SigBuilder::new(&mut a).add(x, x);
        let at = Aterm::from_sig(&mut a, &t, add);
        assert_eq!(at.terms.len(), 1, "same-signature terms should merge");
        let m = at.terms.values().next().unwrap();
        assert_eq!(match_sig(&a, m.coef), SigMatch::Int(2));
    }

    // ── normalized_tree ───────────────────────────────────────────────────────

    #[test]
    fn aterm_normalized_tree_constant() {
        // aterm(3 + 4) should normalize to int(7).
        let mut a = arena();
        let t = types();
        let i3 = SigBuilder::new(&mut a).int(3);
        let i4 = SigBuilder::new(&mut a).int(4);
        let add = SigBuilder::new(&mut a).add(i3, i4);
        let at = Aterm::from_sig(&mut a, &t, add);
        let tree = at.normalized_tree(&mut a, &t);
        assert_eq!(match_sig(&a, tree), SigMatch::Int(7));
    }

    #[test]
    fn aterm_normalized_tree_roundtrip() {
        // aterm(x + y).normalizedTree() should be an Add node.
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let add = SigBuilder::new(&mut a).add(x, y);
        let at = Aterm::from_sig(&mut a, &t, add);
        let tree = at.normalized_tree(&mut a, &t);
        match match_sig(&a, tree) {
            SigMatch::BinOp(BinOp::Add, _, _) => {}
            other => panic!("expected Add node, got {other:?}"),
        }
    }

    // ── greatest_divisor ──────────────────────────────────────────────────────

    #[test]
    fn greatest_divisor_of_two_multiples() {
        // aterm(2*x + 4*x*y): two distinct-signature mterms sharing factor x.
        // gcd should contain x as a factor (complexity > 0).
        //
        // Note: 2*x + 4*x merges into a single mterm 6*x (same signature),
        // so we need different signatures to exercise the pairwise path.
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let two = SigBuilder::new(&mut a).int(2);
        let s1 = SigBuilder::new(&mut a).mul(two, x); // 2*x
        let four = SigBuilder::new(&mut a).int(4);
        let xy = SigBuilder::new(&mut a).mul(x, y);
        let s2 = SigBuilder::new(&mut a).mul(four, xy); // 4*x*y
        let add = SigBuilder::new(&mut a).add(s1, s2);
        let at = Aterm::from_sig(&mut a, &t, add);
        assert_eq!(at.terms.len(), 2, "distinct signatures should not merge");
        let g = at.greatest_divisor(&mut a, &t);
        // gcd(2*x, 4*x*y): coef differs (not same_magnitude) → coef=1;
        // common factor x^min(1,1)=x^1 → gcd has x.
        assert!(g.factors.contains_key(&x), "gcd should contain factor x");
    }

    #[test]
    fn greatest_divisor_no_common_factor() {
        // aterm(x + y): gcd → 1, complexity = 0
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let y = SigBuilder::new(&mut a).input(1);
        let add = SigBuilder::new(&mut a).add(x, y);
        let at = Aterm::from_sig(&mut a, &t, add);
        let g = at.greatest_divisor(&mut a, &t);
        assert_eq!(g.complexity(&a, &t), 0, "gcd of x and y has complexity 0");
    }

    // ── factorize ─────────────────────────────────────────────────────────────

    #[test]
    fn factorize_extracts_common_factor() {
        // aterm(2*x + 4*x).factorize(x) → aterm with x*(2+4) = x*6
        let mut a = arena();
        let t = types();
        let x = SigBuilder::new(&mut a).input(0);
        let two = SigBuilder::new(&mut a).int(2);
        let s1 = SigBuilder::new(&mut a).mul(two, x);
        let four = SigBuilder::new(&mut a).int(4);
        let s2 = SigBuilder::new(&mut a).mul(four, x);
        let add = SigBuilder::new(&mut a).add(s1, s2);
        let at = Aterm::from_sig(&mut a, &t, add);

        let d = Mterm::from_sig(&mut a, x);
        let factored = at.factorize(&mut a, &t, &d);
        let tree = factored.normalized_tree(&mut a, &t);
        // Should produce a Mul node (x * (2+4)) or equivalent
        match match_sig(&a, tree) {
            SigMatch::BinOp(BinOp::Mul, _, _) => {}
            other => panic!("expected Mul node, got {other:?}"),
        }
    }
}
