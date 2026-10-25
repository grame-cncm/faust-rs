//! Incremental normalization of a sum extended by new terms.
//!
//! [`simplify`](crate::simplify) normalizes every `Add`/`Sub` node bottom-up,
//! and a sum built by `:>` is a left chain, so each partial sum
//! `s_k = s_{k-1} ± t_k` used to be decomposed, searched for a divisor and
//! folded back in full: O(k) per prefix, O(N²) per sum. The C++ reference
//! (`normalizeAddTerm`) has the same structure. This module keeps, for each
//! sum it produced, the state that built it, and extends that state when the
//! parent is that sum plus or minus new terms.
//!
//! # Exactness
//!
//! The result must be the node the full path ([`normalize_add_term`]) returns,
//! and the arena must receive the same new nodes in the same order. `SigId`
//! order drives the `BTreeMap` orders of [`Aterm`] and [`Mterm`] and the operand
//! order of `simplifying_add`, so equal-valued trees are not enough.
//!
//! On `T ± y`, where `T` has a state, the full path:
//! 1. decomposes `T` and `y` leaf by leaf;
//! 2. evaluates `gcd` on the candidate pairs;
//! 3. folds every term in map order;
//! 4. combines the folds.
//!
//! Only some of that work creates nodes:
//! - in step 1, only the *pending* leaves of `T` (the terms appended when `T`
//!   was built, decomposed now for the first time) and the leaves of `y`; the
//!   other leaves of `T` re-intern nodes an earlier parent created;
//! - in step 2, only `int(1)`, which already exists;
//! - in step 3, only the terms of `y`, which come last in map order;
//! - step 4.
//!
//! The fast path runs exactly those operations, in that order, through the
//! same primitives. It falls back to the full path, from scratch, as soon as a
//! check fails. Every operation it ran before failing is one the full path runs
//! at that point too, so the fallback creates the remaining nodes where the
//! full path would. The checks:
//!
//! - every pending leaf decomposes to a mterm whose tree is the stored one (the
//!   same factors and sign, and the same coefficient or the same `±1` class);
//!   the pending leaves are those met by a walk of `T` that stops at the
//!   folds of the previous state, and the walk must meet exactly them;
//! - every term of `y` has a signature absent from `T` and from the other new
//!   terms (no merge, whose order would matter), and greater than every
//!   signature of `T` (an append in map order, so the folds continue);
//! - every new term has a gcd of complexity 0 with each of its candidates.
//!   The state invariant says every pair of `T` has complexity 0, so
//!   `greatest_divisor` is 1 and no factorization runs, as in the full path.
//!
//! Debug builds recompute every fast-path result by the full path on the same
//! arena. They check that the full path returns the same node and creates no
//! node, so a fast path that missed a node the full path creates is caught.
//!
//! Plan and measurements: `porting/long-fir-iir-compile-cost-plan-2026-10-25-en.md`.

use std::collections::{HashMap, HashSet};
use std::ops::Bound;

use signals::{BinOp, SigBuilder, SigId, SigMatch, match_sig};
use sigtype::SigType;
use tlib::TreeArena;

use crate::aterm::{Aterm, SumFolds, magnitude_key};
use crate::mterm::{Mterm, gcd, is_minus_one, is_one, is_zero};

/// Sums of fewer terms keep no state: the full path is cheap there, and most
/// such sums are never extended. A chain reaches this size through the full
/// path and continues incrementally from it.
const MIN_STATE_TERMS: usize = 8;

/// States of the sums one [`simplify`](crate::simplify) pass produced, keyed
/// by the sum. Valid for one arena and one type map, like the
/// [`SimplifyCache`](crate::simplify::SimplifyCache) that owns it. A state is
/// removed when a parent consumes it; a second parent of the same sum takes
/// the full path.
#[derive(Default)]
pub(crate) struct SumMemo {
    states: HashMap<SigId, SumState>,
    #[cfg(test)]
    pub(crate) stats: SumMemoStats,
}

/// Fast-path and fallback counts, for tests.
#[cfg(test)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SumMemoStats {
    pub(crate) extended: usize,
    pub(crate) fell_back: usize,
    pub(crate) stored: usize,
}

/// What built one sum `T`.
struct SumState {
    /// The terms of `T`. A term an earlier parent decomposed holds the mterm
    /// that decomposition gave; a pending one holds the mterm it was built
    /// from.
    aterm: Aterm,
    /// Signatures of the pending terms.
    pending: HashSet<SigId>,
    /// The folds before the pending terms were appended. A walk of `T` that
    /// stops at them meets exactly the pending terms.
    pending_roots: SumFolds,
    /// The folds of `T`, before their combination.
    folds: SumFolds,
    /// Divisor candidates of the terms of `T`.
    index: DivisorIndex,
    /// The greatest signature of `T`.
    last_key: SigId,
}

/// The candidate index of [`Aterm::greatest_divisor`], kept across extensions:
/// a term's candidates are the terms sharing one of its factors with an
/// exponent of the same sign, or its coefficient magnitude.
#[derive(Default)]
struct DivisorIndex {
    by_factor: HashMap<(SigId, bool), Vec<SigId>>,
    by_magnitude: HashMap<u64, Vec<SigId>>,
}

impl DivisorIndex {
    fn insert(&mut self, arena: &TreeArena, key: SigId, m: &Mterm) {
        for (&base, &exp) in &m.factors {
            if exp != 0 {
                self.by_factor.entry((base, exp > 0)).or_default().push(key);
            }
        }
        if let Some(magnitude) = magnitude_key(arena, m.coef) {
            self.by_magnitude.entry(magnitude).or_default().push(key);
        }
    }

    /// The candidates of `m`, without duplicates.
    fn candidates(&self, arena: &TreeArena, m: &Mterm, out: &mut Vec<SigId>) {
        out.clear();
        for (&base, &exp) in &m.factors {
            if exp != 0
                && let Some(keys) = self.by_factor.get(&(base, exp > 0))
            {
                out.extend_from_slice(keys);
            }
        }
        if let Some(keys) = magnitude_key(arena, m.coef).and_then(|k| self.by_magnitude.get(&k)) {
            out.extend_from_slice(keys);
        }
        out.sort_unstable();
        out.dedup();
    }
}

impl SumMemo {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Normalize `t` as [`normalize_add_term`] does, extending the state of
    /// an operand when the checks of the module documentation hold.
    pub(crate) fn normalize_add_term(
        &mut self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        t: SigId,
    ) -> SigId {
        #[cfg(debug_assertions)]
        crate::normalize::debug_check_coherent_input(arena, t);

        if let Some(result) = self.try_extend(arena, types, t) {
            #[cfg(test)]
            {
                self.stats.extended += 1;
            }
            #[cfg(debug_assertions)]
            check_against_full_path(arena, types, t, result);
            return result;
        }
        #[cfg(test)]
        {
            self.stats.fell_back += 1;
        }
        self.normalize_full(arena, types, t)
    }

    /// The full path, keeping a state for the result when it can be extended.
    fn normalize_full(
        &mut self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        t: SigId,
    ) -> SigId {
        // The steps of `normalize_add_term`, with the folds kept.
        let mut a = Aterm::from_sig(arena, types, t);
        let mut d = a.greatest_divisor(arena, types);
        let mut factored = false;
        while d.is_not_zero(arena) && d.complexity(arena, types) > 0 {
            a = a.factorize(arena, types, &d);
            d = a.greatest_divisor(arena, types);
            factored = true;
        }
        let empty = SumFolds::empty(arena);
        let mut folds = empty;
        let mut leaves = true;
        for m in a.terms.values() {
            let tree = folds.push(arena, types, m);
            leaves &= is_sum_leaf(arena, tree);
        }
        let (positive, sum) = folds.combine_signed(arena);
        let result = wrap_sign(arena, positive, sum);

        // Every pair of a state has a gcd of complexity 0: true here when no
        // factorization ran, since `greatest_divisor` saw every candidate.
        if !factored
            && positive
            && leaves
            && a.terms.len() >= MIN_STATE_TERMS
            && a.terms.values().all(|m| is_plain_term(arena, m))
        {
            let mut index = DivisorIndex::default();
            for (&key, m) in &a.terms {
                index.insert(arena, key, m);
            }
            let last_key = *a.terms.keys().next_back().expect("a sum of terms");
            let pending = a.terms.keys().copied().collect();
            self.states.insert(
                result,
                SumState {
                    aterm: a,
                    pending,
                    pending_roots: empty,
                    folds,
                    index,
                    last_key,
                },
            );
            #[cfg(test)]
            {
                self.stats.stored += 1;
            }
        }
        result
    }

    /// The fast path: `Some(result)` when `t` is `T ± y` or `y + T` with a
    /// state for `T` and every check holds, `None` otherwise (the caller then
    /// runs the full path from scratch).
    fn try_extend(
        &mut self,
        arena: &mut TreeArena,
        types: &HashMap<SigId, SigType>,
        t: SigId,
    ) -> Option<SigId> {
        let (op, x, y) = match match_sig(arena, t) {
            SigMatch::BinOp(op @ (BinOp::Add | BinOp::Sub), x, y) => (op, x, y),
            _ => return None,
        };
        // The full path decomposes `x` then `y`; `y`'s sign flips under `Sub`.
        let (holder, holder_first, other, other_positive) = if self.states.contains_key(&x) {
            (x, true, y, op == BinOp::Add)
        } else if op == BinOp::Add && self.states.contains_key(&y) {
            (y, false, x, true)
        } else {
            return None;
        };
        let mut state = self.states.remove(&holder)?;

        let mut new_terms = NewTerms::default();
        if !holder_first {
            decompose_new(arena, other, other_positive, &state, &mut new_terms)?;
        }
        if !take_pending(arena, holder, &mut state) {
            return None;
        }
        if holder_first {
            decompose_new(arena, other, other_positive, &state, &mut new_terms)?;
        }
        let mut new_terms = new_terms.terms;
        if new_terms.is_empty() {
            return None;
        }
        new_terms.sort_unstable_by_key(|&(key, _)| key);
        if new_terms[0].0 <= state.last_key {
            return None;
        }

        // Divisors: each new term against the old terms and the new ones
        // before it, `gcd(lower key, higher key)` as the full scan orders it.
        let mut candidates = Vec::new();
        for (key, m) in new_terms {
            state.index.candidates(arena, &m, &mut candidates);
            for &c in &candidates {
                let g = gcd(arena, &state.aterm.terms[&c], &m);
                #[cfg(test)]
                if mutation::active(mutation::Mutation::SkipDivisorCheck) {
                    continue;
                }
                if g.complexity(arena, types) > 0 {
                    return None;
                }
            }
            state.index.insert(arena, key, &m);
            state.aterm.terms.insert(key, m);
            state.pending.insert(key);
        }

        // The full path folds every term in map order; the old ones give back
        // the stored folds, the new ones come last.
        state.pending_roots = state.folds;
        let mut leaves = true;
        let mut plain = true;
        let mut last_key = state.last_key;
        let appended = (Bound::Excluded(state.last_key), Bound::Unbounded);
        #[cfg(test)]
        let skip = usize::from(mutation::active(mutation::Mutation::DropNewTerm));
        #[cfg(not(test))]
        let skip = 0;
        for (&key, m) in state.aterm.terms.range(appended).skip(skip) {
            let tree = state.folds.push(arena, types, m);
            leaves &= is_sum_leaf(arena, tree);
            plain &= is_plain_term(arena, m);
            last_key = key;
        }
        state.last_key = last_key;
        let (positive, sum) = state.folds.combine_signed(arena);
        let result = wrap_sign(arena, positive, sum);
        if positive && leaves && plain {
            self.states.insert(result, state);
        }
        Some(result)
    }
}

/// The terms of `y`, in decomposition order, and their signatures.
#[derive(Default)]
struct NewTerms {
    terms: Vec<(SigId, Mterm)>,
    keys: HashSet<SigId>,
}

/// Decompose `sig` as `Aterm::add_sig` (`positive`) or `Aterm::sub_sig` would,
/// leaf by leaf, into `out`. `None` when a leaf's signature is already in
/// the state or in `out`: the full path would merge it there, an operation
/// whose order matters, so the fast path stops before it.
///
fn decompose_new(
    arena: &mut TreeArena,
    sig: SigId,
    positive: bool,
    state: &SumState,
    out: &mut NewTerms,
) -> Option<()> {
    match match_sig(arena, sig) {
        SigMatch::BinOp(BinOp::Add, x, y) => {
            decompose_new(arena, x, positive, state, out)?;
            decompose_new(arena, y, positive, state, out)
        }
        SigMatch::BinOp(BinOp::Sub, x, y) => {
            decompose_new(arena, x, positive, state, out)?;
            decompose_new(arena, y, !positive, state, out)
        }
        _ => {
            let (key, m) = decompose_leaf(arena, sig, positive);
            if state.aterm.terms.contains_key(&key) || !out.keys.insert(key) {
                return None;
            }
            out.terms.push((key, m));
            Some(())
        }
    }
}

/// One leaf as `Aterm::add_mterm` / `Aterm::sub_mterm` meet it on a new
/// signature: `Mterm::from_sig`, then the signature, then for a subtracted
/// leaf the product by `-1`, in that order.
fn decompose_leaf(arena: &mut TreeArena, leaf: SigId, positive: bool) -> (SigId, Mterm) {
    let m = Mterm::from_sig(arena, leaf);
    let key = m.signature_tree(arena);
    if positive {
        (key, m)
    } else {
        let minus_one = Mterm::from_int(arena, -1);
        (key, m.mul(arena, &minus_one))
    }
}

/// Decompose the pending terms of `holder` (the sum `state` built), in the
/// order the full decomposition of `holder` meets them, and replace each
/// stored mterm by the one its decomposition gives. `false` when the walk
/// meets a leaf that is not a pending term, a pending term whose tree would
/// change, or not every pending term.
fn take_pending(arena: &mut TreeArena, holder: SigId, state: &mut SumState) -> bool {
    let roots = state.pending_roots;
    let stops: Vec<SigId> = roots.p.iter().chain(&roots.n).copied().collect();
    let mut met = 0usize;
    let complete =
        walk_pending(arena, holder, true, &stops, state, &mut met) && met == state.pending.len();
    state.pending.clear();
    complete
}

fn walk_pending(
    arena: &mut TreeArena,
    sig: SigId,
    positive: bool,
    stops: &[SigId],
    state: &mut SumState,
    met: &mut usize,
) -> bool {
    if stops.contains(&sig) {
        return true;
    }
    match match_sig(arena, sig) {
        SigMatch::BinOp(BinOp::Add, x, y) => {
            walk_pending(arena, x, positive, stops, state, met)
                && walk_pending(arena, y, positive, stops, state, met)
        }
        SigMatch::BinOp(BinOp::Sub, x, y) => {
            walk_pending(arena, x, positive, stops, state, met)
                && walk_pending(arena, y, !positive, stops, state, met)
        }
        _ => {
            let (key, m) = decompose_leaf(arena, sig, positive);
            if !state.pending.contains(&key) {
                return false;
            }
            let Some(stored) = state.aterm.terms.get_mut(&key) else {
                return false;
            };
            if !same_tree(arena, stored, &m, positive) {
                return false;
            }
            *stored = m;
            *met += 1;
            true
        }
    }
}

/// Whether `decomposed`, the decomposition of the tree of `stored`, has that
/// same tree: the same factors and sign, and a coefficient the tree spells
/// the same way (the same node, or omitted as `1` / `-1` in both).
fn same_tree(arena: &TreeArena, stored: &Mterm, decomposed: &Mterm, positive: bool) -> bool {
    if stored.factors.is_empty() || stored.factors != decomposed.factors {
        return false;
    }
    let negative = stored.is_negative(arena);
    if negative != decomposed.is_negative(arena) || negative == positive {
        return false;
    }
    stored.coef == decomposed.coef
        || (positive && is_one(arena, stored.coef) && is_one(arena, decomposed.coef))
        || (!positive && is_minus_one(arena, stored.coef) && is_minus_one(arena, decomposed.coef))
}

/// A term a state may hold: some factor and a non-zero coefficient. A
/// constant joins the numeric combination of the order-0 buckets, and a zero
/// term is dropped from the tree; neither would be met again as its own leaf.
fn is_plain_term(arena: &TreeArena, m: &Mterm) -> bool {
    !m.factors.is_empty() && !is_zero(arena, m.coef)
}

/// Whether a term tree is one leaf for the decomposition of the sum: not an
/// `Add`/`Sub`, which the parent would split.
fn is_sum_leaf(arena: &TreeArena, tree: SigId) -> bool {
    !matches!(
        match_sig(arena, tree),
        SigMatch::BinOp(BinOp::Add | BinOp::Sub, _, _)
    )
}

/// The final step of `SumFolds::combine`.
fn wrap_sign(arena: &mut TreeArena, positive: bool, sum: SigId) -> SigId {
    if positive {
        sum
    } else {
        let minus_one = SigBuilder::new(arena).int(-1);
        SigBuilder::new(arena).mul(minus_one, sum)
    }
}

/// The checker of the fast path: the full path, run second on the same arena,
/// must return the same node and create none.
#[cfg(debug_assertions)]
fn check_against_full_path(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    t: SigId,
    fast: SigId,
) {
    let nodes = arena.len();
    let full = crate::normalize::normalize_add_term(arena, types, t);
    assert_eq!(
        full, fast,
        "sum_chain: the incremental normalization of {t:?} differs from the full path"
    );
    assert_eq!(
        arena.len(),
        nodes,
        "sum_chain: the full path created nodes the incremental normalization of {t:?} did not"
    );
}

/// Deliberate faults, to show that the checker rejects a wrong fast path.
#[cfg(test)]
pub(crate) mod mutation {
    use std::cell::Cell;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(crate) enum Mutation {
        None,
        /// A new term sharing a factor with an old one does not stop the
        /// fast path, where the full path factorizes.
        SkipDivisorCheck,
        /// The fast path folds one new term fewer.
        DropNewTerm,
    }

    thread_local! {
        static ACTIVE: Cell<Mutation> = const { Cell::new(Mutation::None) };
    }

    pub(crate) fn active(m: Mutation) -> bool {
        ACTIVE.with(|a| a.get() == m)
    }

    /// Run `f` with `m` active.
    pub(crate) fn with<R>(m: Mutation, f: impl FnOnce() -> R) -> R {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                ACTIVE.with(|a| a.set(Mutation::None));
            }
        }
        ACTIVE.with(|a| a.set(m));
        let _reset = Reset;
        f()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use signals::{SigBuilder, SigId};
    use sigtype::SigType;
    use tlib::TreeArena;

    use super::mutation::{self, Mutation};
    use super::{MIN_STATE_TERMS, SumMemoStats};
    use crate::aterm::Aterm;
    use crate::simplify::{SimplifyCache, simplify_with_cache};

    const N: i32 = 96;

    /// How each new term joins the partial sum.
    #[derive(Clone, Copy)]
    enum Join {
        /// `s + t`, the `:>` chain.
        Left,
        /// `t + s`: the state is the right operand.
        Right,
        /// `s + t` and `s - t` alternately.
        Alternating,
    }

    /// Term `k`: a product of two leaves no other term carries, with an
    /// integer, a real or a negative coefficient, so that the terms share no
    /// factor and land in both buckets. `x * c`, with `x` built first, is the
    /// signature `normalized_tree` spells, so the signatures of successive
    /// terms are successive nodes: each term is appended in map order.
    fn term(b: &mut SigBuilder<'_>, k: i32) -> SigId {
        let x = b.input(k);
        let c = b.input(10_000 + k);
        let xc = b.mul(x, c);
        match k % 4 {
            0 => xc,
            1 => {
                let r = b.real(0.25 + f64::from(k) / 512.0);
                b.mul(r, xc)
            }
            2 => {
                let m = b.int(-1);
                b.mul(m, xc)
            }
            _ => {
                let r = b.real(-1.5 - f64::from(k) / 512.0);
                b.mul(xc, r)
            }
        }
    }

    fn chain(arena: &mut TreeArena, join: Join, last: Option<SigId>) -> SigId {
        let mut b = SigBuilder::new(arena);
        let mut sum = term(&mut b, 0);
        for k in 1..N {
            let t = term(&mut b, k);
            sum = match join {
                Join::Left => b.add(sum, t),
                Join::Right => b.add(t, sum),
                Join::Alternating if k % 2 == 0 => b.sub(sum, t),
                Join::Alternating => b.add(sum, t),
            };
        }
        match last {
            Some(t) => b.add(sum, t),
            None => sum,
        }
    }

    /// The chain plus `input(20_000) * input(10_000)`, which shares the factor
    /// `input(10_000)` with term 0 and comes last in map order.
    fn with_shared_factor(arena: &mut TreeArena) -> SigId {
        let sum = chain(arena, Join::Left, None);
        let mut b = SigBuilder::new(arena);
        let y = b.input(20_000);
        let c0 = b.input(10_000);
        let shared = b.mul(y, c0);
        b.add(sum, shared)
    }

    /// Simplify `root` with one cache; every fast-path result is checked
    /// against the full path by the debug-build checker as it is produced.
    fn simplify(arena: &mut TreeArena, root: SigId) -> (SigId, SumMemoStats) {
        let types: HashMap<SigId, SigType> = HashMap::new();
        let mut cache = SimplifyCache::new();
        let out = simplify_with_cache(arena, &mut cache, &types, root);
        (out, cache.sum_stats())
    }

    fn terms_of(arena: &mut TreeArena, sum: SigId) -> usize {
        Aterm::from_sig(arena, &HashMap::new(), sum).terms.len()
    }

    #[test]
    fn a_long_chain_is_extended_one_term_at_a_time() {
        for join in [Join::Left, Join::Right, Join::Alternating] {
            let mut arena = TreeArena::new();
            let root = chain(&mut arena, join, None);
            let (out, stats) = simplify(&mut arena, root);
            assert_eq!(terms_of(&mut arena, out), N as usize);
            // Prefixes below the state size take the full path; the first one
            // at that size stores a state, and every later one extends it.
            let first = MIN_STATE_TERMS as i32;
            assert_eq!(stats.extended as i32, N - first, "{stats:?}");
            assert_eq!(stats.stored, 1, "{stats:?}");
        }
    }

    #[test]
    fn a_term_sharing_a_factor_takes_the_full_path_and_is_factorized() {
        let mut arena = TreeArena::new();
        let root = with_shared_factor(&mut arena);
        let (out, stats) = simplify(&mut arena, root);
        // `c0 * (x0 + y)` replaces two terms.
        assert_eq!(terms_of(&mut arena, out), N as usize);
        assert!(stats.fell_back > 0 && stats.extended > 0, "{stats:?}");
    }

    /// A term whose signature comes before the last one of the sum would
    /// change every fold after it: the full path builds those nodes, so the
    /// fast path steps aside.
    #[test]
    fn a_term_inserted_before_the_end_of_the_map_order_takes_the_full_path() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        // Built first, so its signature precedes every term of the chain.
        let early_x = b.input(30_000);
        let early_c = b.input(30_001);
        let early = b.mul(early_x, early_c);
        let root = chain(&mut arena, Join::Left, Some(early));
        let (out, stats) = simplify(&mut arena, root);
        assert_eq!(terms_of(&mut arena, out), N as usize + 1);
        assert_eq!(
            stats.extended as i32,
            N - MIN_STATE_TERMS as i32,
            "{stats:?}"
        );
    }

    #[test]
    fn a_sum_used_twice_is_extended_once() {
        let mut arena = TreeArena::new();
        let s = chain(&mut arena, Join::Left, None);
        let mut b = SigBuilder::new(&mut arena);
        let (t1, t2) = (b.input(20_000), b.input(20_001));
        let (a, c) = (b.add(s, t1), b.add(s, t2));
        let root = b.mul(a, c);
        let (_, stats) = simplify(&mut arena, root);
        // The second consumer finds no state: the first one took it.
        assert_eq!(stats.extended as i32, N - MIN_STATE_TERMS as i32 + 1);
    }

    #[test]
    #[should_panic(expected = "differs from the full path")]
    fn the_checker_rejects_a_fast_path_that_drops_a_term() {
        mutation::with(Mutation::DropNewTerm, || {
            let mut arena = TreeArena::new();
            let root = chain(&mut arena, Join::Left, None);
            simplify(&mut arena, root);
        });
    }

    #[test]
    #[should_panic(expected = "differs from the full path")]
    fn the_checker_rejects_a_fast_path_that_ignores_a_common_factor() {
        mutation::with(Mutation::SkipDivisorCheck, || {
            let mut arena = TreeArena::new();
            let root = with_shared_factor(&mut arena);
            simplify(&mut arena, root);
        });
    }
}
