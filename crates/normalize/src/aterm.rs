//! Additive term: a sum of [`Mterm`]s — `m₁ + m₂ + m₃ + …`
//!
//! Ported from C++ `compiler/normalize/aterm.cpp`.
// Internal module, used by `normalize`.
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

use crate::mterm::{self, Mterm, add_nums, gcd, is_num, is_zero, state_order, sub_nums};

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
    /// order ([`state_order`]: 0=Konst, 1=Block, 2=Samp, 3=depends on state).
    /// The constant bucket at order 0 forms the initial sum; the buckets of
    /// orders 2 and 1 are folded onto it from highest to lowest, each term
    /// joining as the left operand, using [`add_terms_with_sign`]; the terms
    /// that depend on state join last, as the right operand.
    ///
    /// That late state join (upstream commit `d4d51e5ed`, posterior to the
    /// pinned reference; registry DIFF-BEH-023) keeps the state-to-state path
    /// of a recursion clear of any foreign addition and leaves the slow part
    /// of the sum one subtree, which can be hoisted: `1 - c - o` is
    /// `(1 - c) - o`, where the reference builds `(1 - o) - c`. A two-term sum
    /// `{slow, state}` keeps its historical operand order `slow + state`.
    ///
    /// C++: `Tree aterm::normalizedTree() const`.
    pub(crate) fn normalized_tree(
        &self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
    ) -> SigId {
        // Orders 0..=2 are folded as in the reference; order 3 joins last.
        const STATE: usize = 3;
        const ORDERS: usize = STATE + 1;
        let zero = SigBuilder::new(arena).int(0);
        let mut p = [zero; ORDERS]; // positive terms by order
        let mut n = [zero; ORDERS]; // negative terms (made positive) by order

        // Collect mterms — clone to avoid arena borrow conflicts during tree building.
        let mterms: Vec<Mterm> = self.terms.values().cloned().collect();

        for m in &mterms {
            if m.is_negative(arena) {
                // negative_mode=true: invert sign for the tree
                let t = m.normalized_tree(arena, types, false, true);
                let ord = state_order(arena, types, t) as usize;
                n[ord] = simplifying_add(arena, n[ord], t);
            } else {
                let t = m.normalized_tree(arena, types, false, false);
                let ord = state_order(arena, types, t) as usize;
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

        // Fold from highest to lowest order, the state terms excepted.
        // C++: loop from order=3 down to 1; here from Samp (2) down to 1.
        for order in (1..STATE).rev() {
            let (s, r) = add_terms_with_sign(arena, false, n[order], sign, sum);
            sign = s;
            sum = r;

            let (s, r) = add_terms_with_sign(arena, true, p[order], sign, sum);
            sign = s;
            sum = r;
        }

        // The state terms join last, as the right operand (SUM first).
        let (s, r) = add_terms_with_sign(arena, sign, sum, false, n[STATE]);
        sign = s;
        sum = r;
        let (s, r) = add_terms_with_sign(arena, sign, sum, true, p[STATE]);
        sign = s;
        sum = r;

        if !sign {
            // Result is negative: wrap in -1 * SUM.
            let minus_one = SigBuilder::new(arena).int(-1);
            sum = SigBuilder::new(arena).mul(minus_one, sum);
        }

        sum
    }

    /// Return the greatest common divisor of any two mterms.
    ///
    /// Returns the gcd of highest complexity over all pairs of terms, the
    /// first pair in map order winning ties. Returns `1` (no common factor)
    /// when no pair has a gcd of non-zero complexity, in particular when there
    /// are fewer than two terms.
    ///
    /// C++ scans every pair (O(k²) per call, and `normalizeAddTerm` runs on
    /// every partial sum of a `:>` chain, so O(n³) for a sum of n terms).
    /// This scans only the pairs [`DivisorPairs`] reports, a superset of those
    /// whose gcd has a non-zero complexity, in the same order: the result, the
    /// tie-break and the nodes created are those of the full scan.
    ///
    /// C++: `mterm aterm::greatestDivisor() const`.
    pub(crate) fn greatest_divisor(
        &self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
    ) -> Mterm {
        let mut max_complexity = 0i32;
        let mut max_gcd = Mterm::from_int(arena, 1);
        // Most calls see one term (simplify normalizes every product as an
        // add-term): no pair, and no index to build.
        if self.terms.len() < 2 {
            return max_gcd;
        }

        let mterms: Vec<&Mterm> = self.terms.values().collect();
        let pairs = DivisorPairs::new(arena, &mterms);
        let mut partners = Vec::new();
        for i in 0..mterms.len() {
            // Ascending partners for ascending `i`: pairs are visited in the
            // order of the full scan, so the first strictly greater one wins.
            pairs.partners_after(i, mterms[i], &mut partners);
            for &j in &partners {
                #[cfg(test)]
                gcd_evaluations::record();
                let g = gcd(arena, mterms[i], mterms[j]);
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

/// The pairs of terms of an [`Aterm`] whose [`gcd`] can have a non-zero
/// complexity, for [`Aterm::greatest_divisor`].
///
/// `gcd(m1, m2).complexity()` adds 1 when the coefficients have the same
/// magnitude and that magnitude is not 1, and `(1 + order) · |e|` (order from
/// [`state_order`], at least 0, so the term is positive) for each
/// factor common to both terms with exponents of the same sign (`e` their
/// common exponent, never 0 then). It is 0 for every other pair, and a pair of
/// complexity 0 never replaces the current best. So the candidate pairs are
/// those sharing a factor with same-sign exponents, found through an inverted
/// index, or sharing a coefficient magnitude other than 1, found by bucket.
///
/// A sum of `k` independent terms (`:>` over `k` distinct signals, each with
/// coefficient ±1) has no candidate pair, where the full scan evaluates
/// `k(k-1)/2` of them.
///
/// The index is built per call and holds term positions only: no node is
/// created, and the candidates are a superset of the pairs of non-zero
/// complexity, every one still evaluated by [`gcd`] as before.
struct DivisorPairs {
    /// `(factor, exponent > 0)` → positions of the terms carrying that factor
    /// with an exponent of that sign, ascending.
    by_factor: HashMap<(SigId, bool), Vec<usize>>,
    /// Coefficient magnitude (`f64` bits) → positions of the terms with that
    /// magnitude, ascending. Magnitude 1 and NaN are left out: a gcd of
    /// magnitude 1 has no coefficient complexity, and `same_magnitude` never
    /// holds for NaN.
    by_magnitude: HashMap<u64, Vec<usize>>,
    /// Magnitude key of each term, by position.
    magnitudes: Vec<Option<u64>>,
}

impl DivisorPairs {
    fn new(arena: &TreeArena, mterms: &[&Mterm]) -> Self {
        let mut by_factor: HashMap<(SigId, bool), Vec<usize>> = HashMap::new();
        let mut by_magnitude: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut magnitudes = Vec::with_capacity(mterms.len());
        for (pos, m) in mterms.iter().enumerate() {
            for (&base, &exp) in &m.factors {
                if exp != 0 {
                    by_factor.entry((base, exp > 0)).or_default().push(pos);
                }
            }
            let key = magnitude_key(arena, m.coef);
            if let Some(key) = key {
                by_magnitude.entry(key).or_default().push(pos);
            }
            magnitudes.push(key);
        }
        Self {
            by_factor,
            by_magnitude,
            magnitudes,
        }
    }

    /// Fill `out` with the candidate partners `j > i` of term `i` (`m`),
    /// ascending and without duplicates.
    fn partners_after(&self, i: usize, m: &Mterm, out: &mut Vec<usize>) {
        fn after(i: usize, positions: &[usize], out: &mut Vec<usize>) {
            let start = positions.partition_point(|&pos| pos <= i);
            out.extend_from_slice(&positions[start..]);
        }
        out.clear();
        for (&base, &exp) in &m.factors {
            if exp != 0
                && let Some(positions) = self.by_factor.get(&(base, exp > 0))
            {
                after(i, positions, out);
            }
        }
        if let Some(positions) = self.magnitudes[i].and_then(|key| self.by_magnitude.get(&key)) {
            after(i, positions, out);
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// Bucket key of a numeric coefficient for [`DivisorPairs`]: the bits of its
/// absolute value as an `f64`, or `None` when it cannot contribute to a gcd's
/// complexity (magnitude 1, NaN, or not a number).
///
/// [`same_magnitude`](crate::mterm::same_magnitude) of two coefficients
/// implies equal keys (`i32 → f64` is exact and `abs` maps `-0.0` to `0.0`),
/// so the buckets miss no pair it accepts.
fn magnitude_key(arena: &TreeArena, coef: SigId) -> Option<u64> {
    let magnitude = match match_sig(arena, coef) {
        SigMatch::Int(n) => f64::from(n).abs(),
        SigMatch::Real(r) => r.abs(),
        _ => return None,
    };
    (magnitude != 1.0 && !magnitude.is_nan()).then(|| magnitude.to_bits())
}

/// Counts the [`gcd`] evaluations of [`Aterm::greatest_divisor`] on the
/// current thread, so tests can bound the work of a normalization.
#[cfg(test)]
pub(crate) mod gcd_evaluations {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn record() {
        COUNT.with(|count| count.set(count.get() + 1));
    }

    /// Evaluations since the last call on this thread.
    pub(crate) fn take() -> u64 {
        COUNT.with(|count| count.replace(0))
    }
}

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
    use interval::Interval;
    use signals::SigMatch;
    use sigtype::{Boolean, Computability, Nature, Variability, Vectorability, make_simple};

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

    /// The full pair scan of C++ `aterm::greatestDivisor`, the oracle of the
    /// pruned [`Aterm::greatest_divisor`].
    fn greatest_divisor_full_scan(
        at: &Aterm,
        a: &mut TreeArena,
        t: &HashMap<SigId, SigType>,
    ) -> Mterm {
        let mut max_complexity = 0i32;
        let mut max_gcd = Mterm::from_int(a, 1);
        let mterms: Vec<&Mterm> = at.terms.values().collect();
        for i in 0..mterms.len() {
            for j in (i + 1)..mterms.len() {
                let g = gcd(a, mterms[i], mterms[j]);
                let c = g.complexity(a, t);
                if c > max_complexity {
                    max_complexity = c;
                    max_gcd = g;
                }
            }
        }
        max_gcd
    }

    /// Deterministic xorshift generator for the property test below.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    fn simple(variability: Variability) -> SigType {
        make_simple(
            Nature::Real,
            variability,
            Computability::Init,
            Vectorability::Vect,
            Boolean::Num,
            Interval::new_default(),
        )
    }

    /// `proj(0, SYMREF(v))'`: the delayed output of a recursion, as a
    /// symbolic recursive body reads its own state.
    fn state_term(a: &mut TreeArena) -> SigId {
        let var = a.symbol("W");
        let self_ref = tlib::sym_ref(a, var);
        let proj = SigBuilder::new(a).proj(0, self_ref);
        SigBuilder::new(a).delay1(proj)
    }

    /// `1`, a control `c`, an audio input `x` and a state term `o`, typed.
    fn late_join_terms(a: &mut TreeArena, t: &mut HashMap<SigId, SigType>) -> [SigId; 4] {
        let one = SigBuilder::new(a).int(1);
        let c = SigBuilder::new(a).input(1);
        let x = SigBuilder::new(a).input(2);
        let o = state_term(a);
        t.insert(one, simple(Variability::Konst));
        t.insert(c, simple(Variability::Block));
        t.insert(x, simple(Variability::Samp));
        t.insert(o, simple(Variability::Samp));
        [one, c, x, o]
    }

    #[test]
    fn a_state_term_joins_the_sum_last() {
        // `1 - c - o`: the reference builds `(1 - o) - c`, so `1 - o` rounds
        // and the slow `1 - c` cannot be hoisted; upstream d4d51e5ed builds
        // `(1 - c) - o` (faust-coopiano's damper openness never reached 0).
        let mut a = arena();
        let mut t = types();
        let [one, c, _, o] = late_join_terms(&mut a, &mut t);
        let one_minus_c = SigBuilder::new(&mut a).sub(one, c);
        let sum = SigBuilder::new(&mut a).sub(one_minus_c, o);
        let tree = Aterm::from_sig(&mut a, &t, sum).normalized_tree(&mut a, &t);
        assert_eq!(tree, sum, "{}", signals::dump_sig_readable(&a, tree));
    }

    #[test]
    fn a_slow_and_a_state_term_keep_their_operand_order() {
        // `{slow, state}` keeps `slow + state`, its historical shape.
        let mut a = arena();
        let mut t = types();
        let [_, c, _, o] = late_join_terms(&mut a, &mut t);
        for sum in [
            SigBuilder::new(&mut a).add(c, o),
            SigBuilder::new(&mut a).add(o, c),
        ] {
            let tree = Aterm::from_sig(&mut a, &t, sum).normalized_tree(&mut a, &t);
            let expected = SigBuilder::new(&mut a).add(c, o);
            assert_eq!(tree, expected, "{}", signals::dump_sig_readable(&a, tree));
        }
    }

    #[test]
    fn the_state_term_comes_after_the_audio_and_slow_terms() {
        // `o + x + c`: the off-path terms form `c + x`, then `+ o`.
        let mut a = arena();
        let mut t = types();
        let [_, c, x, o] = late_join_terms(&mut a, &mut t);
        let ox = SigBuilder::new(&mut a).add(o, x);
        let sum = SigBuilder::new(&mut a).add(ox, c);
        let tree = Aterm::from_sig(&mut a, &t, sum).normalized_tree(&mut a, &t);
        let cx = SigBuilder::new(&mut a).add(c, x);
        let expected = SigBuilder::new(&mut a).add(cx, o);
        assert_eq!(tree, expected, "{}", signals::dump_sig_readable(&a, tree));
        // A negative state term joins as a subtraction on the right.
        let x_minus_o = SigBuilder::new(&mut a).sub(x, o);
        let sum = SigBuilder::new(&mut a).add(x_minus_o, c);
        let tree = Aterm::from_sig(&mut a, &t, sum).normalized_tree(&mut a, &t);
        let expected = SigBuilder::new(&mut a).sub(cx, o);
        assert_eq!(tree, expected, "{}", signals::dump_sig_readable(&a, tree));
    }

    #[test]
    fn a_state_factor_weighs_one_more_than_an_audio_one() {
        let mut a = arena();
        let mut t = types();
        let [_, c, x, o] = late_join_terms(&mut a, &mut t);
        let one = SigBuilder::new(&mut a).int(1);
        let weight = |a: &mut TreeArena, f: SigId| {
            Mterm {
                coef: one,
                factors: BTreeMap::from([(f, 1)]),
            }
            .complexity(a, &t)
        };
        assert_eq!(weight(&mut a, c), 2);
        assert_eq!(weight(&mut a, x), 3);
        assert_eq!(weight(&mut a, o), 4);
    }

    #[test]
    fn greatest_divisor_matches_the_full_pair_scan() {
        // Random aterms over a few factors of different orders, exponents of
        // both signs (0 included: `Mterm::from_sig` does not clean up), and
        // coefficients mixing ±1, zero, equal magnitudes and Int/Real twins,
        // so ties and coefficient-only gcds are frequent. The pruned scan must
        // return the same gcd (hence the same tie-break) and create no node.
        let mut a = arena();
        let mut t = types();
        let mut pool: Vec<SigId> = (0..6).map(|i| SigBuilder::new(&mut a).input(i)).collect();
        // A factor that depends on state, which weighs one more.
        pool.push(state_term(&mut a));
        t.insert(pool[0], simple(Variability::Konst));
        t.insert(pool[1], simple(Variability::Block));
        let mut coefs: Vec<SigId> = [1, -1, 2, -2, 3, 0]
            .into_iter()
            .map(|k| SigBuilder::new(&mut a).int(k))
            .collect();
        coefs.extend(
            [1.0, -1.0, 2.0, 0.5, -0.5, 0.0, -0.0]
                .into_iter()
                .map(|k| SigBuilder::new(&mut a).real(k)),
        );
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut nontrivial = 0;
        for _ in 0..2000 {
            let mut at = Aterm::zero();
            for _ in 0..rng.below(12) {
                let mut factors = BTreeMap::new();
                for _ in 0..rng.below(4) {
                    let exp = rng.below(5) as i32 - 2;
                    factors.insert(pool[rng.below(pool.len())], exp);
                }
                let coef = coefs[rng.below(coefs.len())];
                at.add_mterm(&mut a, &Mterm { coef, factors });
            }
            let nodes = a.len();
            let pruned = at.greatest_divisor(&mut a, &t);
            let full = greatest_divisor_full_scan(&at, &mut a, &t);
            assert_eq!(a.len(), nodes, "greatest_divisor must create no node");
            assert_eq!(pruned.coef, full.coef);
            assert_eq!(pruned.factors, full.factors);
            if full.complexity(&a, &t) > 0 {
                nontrivial += 1;
            }
        }
        assert!(nontrivial > 1000, "too few non-trivial gcds: {nontrivial}");
    }

    #[test]
    fn long_sum_normalization_evaluates_no_trivial_gcd() {
        // `:>` over N signals builds `((x0 + x1) + x2) + …` and simplify
        // normalizes every partial sum. The full pair scan evaluated
        // Σ k(k-1)/2 ≈ N³/6 gcds over the chain (22 million for N = 512),
        // all of complexity 0 here: the terms share no factor, and their
        // coefficients are ±1 or of distinct magnitudes.
        const N: i32 = 512;
        let mut a = arena();
        let t = types();
        let mut b = SigBuilder::new(&mut a);
        let mut sum = b.input(0);
        for k in 1..N {
            let x = b.input(k);
            let term = match k % 3 {
                0 => x,
                1 => {
                    let c = b.real(0.5 + f64::from(k) / 1024.0);
                    b.mul(c, x)
                }
                _ => {
                    let c = b.int(-1);
                    b.mul(c, x)
                }
            };
            sum = b.add(sum, term);
        }
        gcd_evaluations::take();
        let simplified = crate::simplify::simplify(&mut a, &t, sum);
        assert_eq!(gcd_evaluations::take(), 0);
        let at = Aterm::from_sig(&mut a, &t, simplified);
        assert_eq!(at.terms.len(), N as usize, "no term may be lost or merged");
    }

    #[test]
    fn long_sum_with_a_shared_coefficient_stays_linear() {
        // `0.5·x0 + 0.5·x1 + …`: every partial sum factorizes to `0.5·(…)`,
        // so each level sees two terms and evaluates one gcd per divisor
        // search (two searches: one finds 0.5, one finds nothing).
        const N: i32 = 512;
        let mut a = arena();
        let t = types();
        let mut b = SigBuilder::new(&mut a);
        let half = b.real(0.5);
        let x0 = b.input(0);
        let mut sum = b.mul(half, x0);
        for k in 1..N {
            let x = b.input(k);
            let term = b.mul(half, x);
            sum = b.add(sum, term);
        }
        gcd_evaluations::take();
        crate::simplify::simplify(&mut a, &t, sum);
        let evaluations = gcd_evaluations::take();
        assert!(
            evaluations <= 2 * N as u64,
            "{evaluations} gcd evaluations for a chain of {N} terms"
        );
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
