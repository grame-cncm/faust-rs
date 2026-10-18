//! Pre-simplification pass that detects structurally isomorphic `SYMREC` groups
//! and unifies them to a single canonical representative.
//!
//! This pass enables downstream algebraic simplification (e.g., `x - x → 0`)
//! across signal trees produced by independent constructions of the same
//! recursion (for example a hand-written one and a library-expanded one).
//!
//! Exposes `merge_isomorphic_symrec_groups` which must run after `de_bruijn_to_sym`
//! and prior to algebraic simplification passes.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use signals::SigId;
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};

/// Returns a unique sentinel hole token used to track self references in `SYMREC`.
fn gen_hole(arena: &mut TreeArena) -> SigId {
    arena.symbol("__rec_hole__")
}

/// Merges structurally isomorphic `SYMREC` groups into canonical representatives.
///
/// Converts duplicate `SYMREC` groups generating identical output lists
/// (after masking recursive loops within their bodies) into references
/// corresponding to a single, stable canonical `SYMREC`.
pub fn merge_isomorphic_symrec_groups(arena: &mut TreeArena, outputs: &[SigId]) -> Vec<SigId> {
    merge_with(arena, outputs, true)
}

/// [`merge_isomorphic_symrec_groups`], opening each group with the pruned walk
/// of [`open_symrec`] when `pruned` (the production path) or with the full
/// walk otherwise (the reference the tests compare it with).
fn merge_with(arena: &mut TreeArena, outputs: &[SigId], pruned: bool) -> Vec<SigId> {
    let mut current_outputs = outputs.to_vec();

    loop {
        let mut visited = HashSet::new();
        let mut symrecs = Vec::new();
        for &out in &current_outputs {
            collect_symrec_nodes(arena, out, &mut visited, &mut symrecs);
        }

        if symrecs.len() < 2 {
            break;
        }

        let signatures = open_signatures(arena, &current_outputs, &symrecs, pruned);

        let mut signature_groups: HashMap<SigId, Vec<SigId>> = HashMap::new();
        for (rec, sig_id) in signatures {
            signature_groups.entry(sig_id).or_default().push(rec);
        }

        let (rec_map, ref_map) = build_symrec_substitution(arena, &signature_groups);

        if rec_map.is_empty() {
            break;
        }

        let mut cache = HashMap::new();
        current_outputs = current_outputs
            .iter()
            .map(|&root| apply_symrec_substitution(arena, root, &rec_map, &ref_map, &mut cache))
            .collect();
    }

    current_outputs
}

/// The opened signature of each group of `symrecs` (reachable from
/// `outputs`): its body list with its own references replaced by a hole.
fn open_signatures(
    arena: &mut TreeArena,
    outputs: &[SigId],
    symrecs: &[SigId],
    pruned: bool,
) -> HashMap<SigId, SigId> {
    let hole = gen_hole(arena);
    let scope = pruned.then(|| FreeVars::new(arena, outputs));
    let mut signatures = HashMap::new();
    for &rec in symrecs {
        if let Some((var, body_list)) = match_sym_rec(arena, rec) {
            // Pruning is exact only for a variable bound by this group alone
            // (see `open_symrec`); any other keeps the full walk.
            let pruning = scope.as_ref().filter(|scope| scope.binds_once(var));
            let mut cache = HashMap::new();
            let opened = open_symrec(arena, body_list, var, hole, pruning, &mut cache);
            signatures.insert(rec, opened);
        }
    }
    signatures
}

/// Recursively identify all reachable `SYMREC` nodes across the tree.
fn collect_symrec_nodes(
    arena: &TreeArena,
    sig: SigId,
    visited: &mut HashSet<SigId>,
    symrecs: &mut Vec<SigId>,
) {
    if !visited.insert(sig) {
        return;
    }

    if let Some((_, body)) = match_sym_rec(arena, sig) {
        symrecs.push(sig);
        if let Some(items) = list_to_vec(arena, body) {
            for item in items {
                collect_symrec_nodes(arena, item, visited, symrecs);
            }
        }
    } else if let Some(node) = arena.node(sig) {
        for &child in node.children.as_slice() {
            collect_symrec_nodes(arena, child, visited, symrecs);
        }
    }
}

/// Free symbolic-recursion variables of the nodes reachable from the outputs,
/// and how many distinct `SYMREC` nodes bind each variable there.
///
/// `free(t)` holds the `v` such that some `SYMREF(v)` is reachable from `t`
/// through no `SYMREC(v, _)`: `{v}` for a reference to `v`,
/// `free(body) \ {v}` for `SYMREC(v, body)`, the union of the children
/// otherwise. A set is as large as the recursion nesting around its node
/// (usually 0 to 2 variables), sorted, and shared with a child holding the
/// same set.
///
/// Built once per round of [`merge_isomorphic_symrec_groups`] and dropped
/// with it; nodes are immutable, so a set never goes stale within a round.
struct FreeVars {
    sets: HashMap<SigId, Rc<[SigId]>>,
    binders: HashMap<SigId, usize>,
    empty: Rc<[SigId]>,
}

impl FreeVars {
    fn new(arena: &TreeArena, roots: &[SigId]) -> Self {
        let mut scope = Self {
            sets: HashMap::new(),
            binders: HashMap::new(),
            empty: Rc::from([]),
        };
        for &root in roots {
            scope.compute(arena, root);
        }
        scope
    }

    fn compute(&mut self, arena: &TreeArena, sig: SigId) -> Rc<[SigId]> {
        if let Some(set) = self.sets.get(&sig) {
            return Rc::clone(set);
        }
        let children: Vec<SigId> = arena
            .node(sig)
            .map_or_else(Vec::new, |node| node.children.as_slice().to_vec());
        let mut set = Rc::clone(&self.empty);
        for child in children {
            let child_set = self.compute(arena, child);
            set = union(&set, &child_set);
        }
        if let Some(var) = match_sym_ref(arena, sig) {
            set = union(&set, &Rc::from([var]));
        } else if let Some((var, _)) = match_sym_rec(arena, sig) {
            *self.binders.entry(var).or_default() += 1;
            if set.binary_search(&var).is_ok() {
                set = set.iter().copied().filter(|&v| v != var).collect();
            }
        }
        self.sets.insert(sig, Rc::clone(&set));
        set
    }

    /// Whether `var` may occur free in `sig` (`true` for an unknown node).
    fn may_contain(&self, sig: SigId, var: SigId) -> bool {
        self.sets
            .get(&sig)
            .is_none_or(|set| set.binary_search(&var).is_ok())
    }

    /// Whether exactly one reachable `SYMREC` node binds `var`.
    fn binds_once(&self, var: SigId) -> bool {
        self.binders.get(&var) == Some(&1)
    }
}

/// Sorted union of two sorted variable sets, sharing `a` or `b` when one
/// contains the other.
fn union(a: &Rc<[SigId]>, b: &Rc<[SigId]>) -> Rc<[SigId]> {
    let contains_all = |x: &[SigId], y: &[SigId]| y.iter().all(|v| x.binary_search(v).is_ok());
    if contains_all(a, b) {
        return Rc::clone(a);
    }
    if contains_all(b, a) {
        return Rc::clone(b);
    }
    let mut merged: Vec<SigId> = a.iter().chain(b.iter()).copied().collect();
    merged.sort_unstable();
    merged.dedup();
    Rc::from(merged)
}

/// Normalizes a `SYMREC` by tracking references bound to `var` as `hole`s.
///
/// With `pruning`, a subtree in which `var` is not free is returned as is.
/// That is exact when `var` is bound by a single `SYMREC` node, the one being
/// opened: that node cannot be reached from its own body (a child's id is
/// smaller than its parent's), so every `SYMREF(var)` reachable from the body
/// is free where it is reached, and a subtree without one opens to itself,
/// every `arena.intern` of the walk meeting an existing node. The pruned walk
/// therefore returns the same signature and creates the same nodes in the same
/// order, while visiting only the nodes that depend on `var` instead of the
/// whole subgraph below the body (other recursions, tables, controls).
fn open_symrec(
    arena: &mut TreeArena,
    sig: SigId,
    var: SigId,
    hole: SigId,
    pruning: Option<&FreeVars>,
    cache: &mut HashMap<SigId, SigId>,
) -> SigId {
    if pruning.is_some_and(|scope| !scope.may_contain(sig, var)) {
        return sig;
    }
    if let Some(&cached) = cache.get(&sig) {
        return cached;
    }
    #[cfg(test)]
    open_visits::record();

    let result = if match_sym_ref(arena, sig) == Some(var) {
        hole
    } else if let Some(node) = arena.node(sig) {
        let kind = node.kind.clone();
        let children = node.children.as_slice().to_vec();

        let mut new_children = Vec::with_capacity(children.len());
        for &c in &children {
            new_children.push(open_symrec(arena, c, var, hole, pruning, cache));
        }
        arena.intern(kind, &new_children)
    } else {
        sig
    };

    cache.insert(sig, result);
    result
}

/// Counts the nodes [`open_symrec`] rebuilds on the current thread, so tests
/// can bound the work of a merge.
#[cfg(test)]
mod open_visits {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn record() {
        COUNT.with(|count| count.set(count.get() + 1));
    }

    /// Visits since the last call on this thread.
    pub(super) fn take() -> u64 {
        COUNT.with(|count| count.replace(0))
    }
}

/// Picks canonical delegates mapping each duplicate node.
fn build_symrec_substitution(
    arena: &mut TreeArena,
    groups: &HashMap<SigId, Vec<SigId>>,
) -> (HashMap<SigId, SigId>, HashMap<SigId, SigId>) {
    let mut rec_map = HashMap::new();
    let mut ref_map = HashMap::new();

    for members in groups.values() {
        if members.len() > 1 {
            let mut sorted = members.clone();
            sorted.sort_by_key(|id| id.as_u32());
            let canon = sorted[0];

            let canon_var = match_sym_rec(arena, canon).unwrap().0;
            let canon_ref = tlib::sym_ref(arena, canon_var);

            for &dup in &sorted[1..] {
                rec_map.insert(dup, canon);

                let dup_var = match_sym_rec(arena, dup).unwrap().0;
                let dup_ref = tlib::sym_ref(arena, dup_var);
                ref_map.insert(dup_ref, canon_ref);
            }
        }
    }

    (rec_map, ref_map)
}

/// Replaces non-canonical identifiers consistently referencing one source of truth.
fn apply_symrec_substitution(
    arena: &mut TreeArena,
    sig: SigId,
    rec_map: &HashMap<SigId, SigId>,
    ref_map: &HashMap<SigId, SigId>,
    cache: &mut HashMap<SigId, SigId>,
) -> SigId {
    if let Some(&cached) = cache.get(&sig) {
        return cached;
    }

    if let Some(&canon_rec) = rec_map.get(&sig) {
        cache.insert(sig, canon_rec);
        return canon_rec;
    }

    if let Some(&canon_ref) = ref_map.get(&sig) {
        cache.insert(sig, canon_ref);
        return canon_ref;
    }

    let result = if let Some(node) = arena.node(sig) {
        let kind = node.kind.clone();
        let children = node.children.as_slice().to_vec();

        let mut new_children = Vec::with_capacity(children.len());
        for c in children {
            new_children.push(apply_symrec_substitution(arena, c, rec_map, ref_map, cache));
        }
        arena.intern(kind, &new_children)
    } else {
        sig
    };

    cache.insert(sig, result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use signals::{SigBuilder, SigMatch, match_sig};
    use std::collections::{HashMap, HashSet};
    use tlib::{TreeArena, list_to_vec, sym_rec, sym_ref, vec_to_list};

    fn arena() -> TreeArena {
        TreeArena::new()
    }

    fn make_single_rec(
        a: &mut TreeArena,
        name: &str,
        build_body: impl Fn(&mut SigBuilder, SigId) -> SigId,
    ) -> SigId {
        let var = a.symbol(name);
        let r = sym_ref(a, var);
        let mut b = SigBuilder::new(a);
        let body = build_body(&mut b, r);
        let list = vec_to_list(a, &[body]);
        sym_rec(a, var, list)
    }

    #[test]
    fn merge_identical_single_output_symrecs() {
        let mut a = arena();
        let rec0 = make_single_rec(&mut a, "W0", |b, rf| {
            let c = b.int(1);
            b.sub(c, rf)
        });
        let rec2 = make_single_rec(&mut a, "W2", |b, rf| {
            let c = b.int(1);
            b.sub(c, rf)
        });

        let outputs = vec![rec0, rec2];
        let merged = merge_isomorphic_symrec_groups(&mut a, &outputs);

        assert_eq!(
            merged[0], merged[1],
            "Isomorphic SYMRECs must merge to canonical representative"
        );
    }

    #[test]
    fn merge_identical_multi_output_symrecs() {
        let mut a = arena();
        let build = |a: &mut TreeArena, name: &str| {
            let var = a.symbol(name);
            let r = sym_ref(a, var);
            let mut b = SigBuilder::new(a);
            let b1 = {
                let c = b.int(1);
                b.add(r, c)
            };
            let b2 = {
                let c = b.int(2);
                b.mul(r, c)
            };
            let list = vec_to_list(a, &[b1, b2]);
            sym_rec(a, var, list)
        };

        let rec0 = build(&mut a, "W0");
        let rec2 = build(&mut a, "W2");

        let outputs = vec![rec0, rec2];
        let merged = merge_isomorphic_symrec_groups(&mut a, &outputs);

        assert_eq!(merged[0], merged[1]);
    }

    #[test]
    fn merge_does_not_unify_distinct_symrecs() {
        let mut a = arena();
        let rec0 = make_single_rec(&mut a, "W0", |b, rf| {
            let c = b.int(1);
            b.add(rf, c)
        });
        let rec2 = make_single_rec(&mut a, "W2", |b, rf| {
            let c = b.int(1);
            b.mul(rf, c)
        });

        let outputs = vec![rec0, rec2];
        let merged = merge_isomorphic_symrec_groups(&mut a, &outputs);

        assert_ne!(merged[0], merged[1]);
    }

    #[test]
    fn merge_followed_by_simplify_gives_zero() {
        let mut a = arena();
        let rec0 = make_single_rec(&mut a, "W0", |b, rf| {
            let c = b.int(1);
            b.add(rf, c)
        });
        let rec2 = make_single_rec(&mut a, "W2", |b, rf| {
            let c = b.int(1);
            b.add(rf, c)
        });

        let mut b = SigBuilder::new(&mut a);
        let proj0 = b.proj(0, rec0);
        let proj2 = b.proj(0, rec2);
        let sub = b.sub(proj0, proj2);

        let merged_outputs = merge_isomorphic_symrec_groups(&mut a, &[sub]);
        let merged_sub = merged_outputs[0];

        // After merging, proj0 and proj2 refer to the same SYMREC group.
        let simplified = crate::simplify::simplify_const(&mut a, merged_sub);

        match match_sig(&a, simplified) {
            SigMatch::Int(0) => {}
            _ => panic!("Expected subtraction of merged isomorphic groups to simplify to 0"),
        }
    }

    #[test]
    fn merge_nested_symrec_groups() {
        let mut a = arena();
        let inner1 = make_single_rec(&mut a, "W10", |b, rf| {
            let c = b.int(1);
            b.add(rf, c)
        });
        let inner2 = make_single_rec(&mut a, "W12", |b, rf| {
            let c = b.int(1);
            b.add(rf, c)
        });

        let outer1 = make_single_rec(&mut a, "W0", |b, rf| b.mul(inner1, rf));
        let outer2 = make_single_rec(&mut a, "W2", |b, rf| b.mul(inner2, rf));

        let outputs = vec![outer1, outer2];
        let merged = merge_isomorphic_symrec_groups(&mut a, &outputs);

        assert_eq!(merged[0], merged[1]);
    }

    #[test]
    fn merge_is_idempotent() {
        let mut a = arena();
        let rec0 = make_single_rec(&mut a, "W0", |b, rf| {
            let c = b.int(1);
            b.sub(c, rf)
        });
        let rec2 = make_single_rec(&mut a, "W2", |b, rf| {
            let c = b.int(1);
            b.sub(c, rf)
        });

        let outputs = vec![rec0, rec2];
        let merged1 = merge_isomorphic_symrec_groups(&mut a, &outputs);
        let merged2 = merge_isomorphic_symrec_groups(&mut a, &merged1);

        assert_eq!(merged1, merged2);
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

    /// Random recursion forest shapes: groups nested in bodies and reading
    /// the variables of the groups around them, groups shared between bodies,
    /// a closed subgraph shared by all, and now and then a group rebinding an
    /// enclosing variable (two `SYMREC` nodes binding one variable).
    struct ForestGen<'g> {
        rng: Rng,
        prefix: &'g str,
        fresh: usize,
        scope: Vec<SigId>,
        built: Vec<SigId>,
        pool: Vec<SigId>,
    }

    impl ForestGen<'_> {
        fn group(&mut self, a: &mut TreeArena, depth: usize) -> SigId {
            let var = if !self.scope.is_empty() && self.rng.below(4) == 0 {
                self.scope[self.rng.below(self.scope.len())]
            } else {
                self.fresh += 1;
                a.symbol(format!("{}{}", self.prefix, self.fresh))
            };
            self.scope.push(var);
            let own = sym_ref(a, var);
            let first = self.expr(a, depth);
            let mut outputs = vec![SigBuilder::new(a).add(own, first)];
            if self.rng.below(3) == 0 {
                outputs.push(self.expr(a, depth));
            }
            self.scope.pop();
            let list = vec_to_list(a, &outputs);
            let group = sym_rec(a, var, list);
            self.built.push(group);
            group
        }

        fn expr(&mut self, a: &mut TreeArena, depth: usize) -> SigId {
            match self.rng.below(7) {
                0 | 1 => self.pool[self.rng.below(self.pool.len())],
                2 => {
                    let var = self.scope[self.rng.below(self.scope.len())];
                    sym_ref(a, var)
                }
                3 if depth > 0 => {
                    let (x, y) = (self.expr(a, depth - 1), self.expr(a, depth - 1));
                    SigBuilder::new(a).add(x, y)
                }
                4 if depth > 0 => {
                    let group = self.group(a, depth - 1);
                    SigBuilder::new(a).proj(0, group)
                }
                5 if !self.built.is_empty() => {
                    let group = self.built[self.rng.below(self.built.len())];
                    SigBuilder::new(a).proj(0, group)
                }
                _ => {
                    let x = self.expr(a, depth.saturating_sub(1));
                    let k = self.pool[0];
                    SigBuilder::new(a).mul(x, k)
                }
            }
        }
    }

    /// A forest drawn from `seed`: the same shape twice under two variable
    /// prefixes (so isomorphic groups exist at every nesting level), plus a
    /// third shape of its own. Two arenas built from one seed hold the same
    /// ids.
    fn random_forest(a: &mut TreeArena, seed: u64) -> Vec<SigId> {
        let mut pool: Vec<SigId> = (0..3).map(|i| SigBuilder::new(a).input(i)).collect();
        let mut closed = pool[0];
        for i in 0..20 {
            let mut b = SigBuilder::new(a);
            let k = b.int(i);
            let product = b.mul(closed, pool[1]);
            closed = b.add(product, k);
        }
        pool.push(closed);
        let mut roots = Vec::new();
        for (prefix, shape_seed) in [("A", seed), ("B", seed), ("C", seed ^ 0x5555)] {
            let mut generator = ForestGen {
                rng: Rng(shape_seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
                prefix,
                fresh: 0,
                scope: Vec::new(),
                built: Vec::new(),
                pool: pool.clone(),
            };
            let group = generator.group(a, 4);
            roots.push(SigBuilder::new(a).proj(0, group));
        }
        let difference = SigBuilder::new(a).sub(roots[0], roots[1]);
        roots.push(difference);
        roots
    }

    fn reachable_groups(a: &TreeArena, roots: &[SigId]) -> Vec<SigId> {
        let mut visited = HashSet::new();
        let mut groups = Vec::new();
        for &root in roots {
            collect_symrec_nodes(a, root, &mut visited, &mut groups);
        }
        groups
    }

    #[test]
    fn pruned_opening_matches_the_full_walk() {
        // The pruned walk must give every group the signature of the full
        // walk and create the same nodes in the same order, so that two
        // arenas built alike stay identical; so must the whole merge.
        let (mut shadowed, mut merging) = (0, 0);
        for seed in 1..=300u64 {
            let mut pruned = arena();
            let mut full = arena();
            let roots = random_forest(&mut pruned, seed);
            assert_eq!(random_forest(&mut full, seed), roots);
            let groups = reachable_groups(&pruned, &roots);

            let signatures = open_signatures(&mut pruned, &roots, &groups, true);
            let reference = open_signatures(&mut full, &roots, &groups, false);
            assert_eq!(signatures, reference, "seed {seed}");
            assert_eq!(pruned.len(), full.len(), "seed {seed}");

            let merged = merge_with(&mut pruned, &roots, true);
            assert_eq!(merged, merge_with(&mut full, &roots, false), "seed {seed}");
            assert_eq!(pruned.len(), full.len(), "seed {seed}");

            let scope = FreeVars::new(&pruned, &roots);
            if scope.binders.values().any(|&count| count > 1) {
                shadowed += 1;
            }
            if merged != roots {
                merging += 1;
            }
        }
        assert!(
            shadowed > 40,
            "too few forests rebinding a variable: {shadowed}"
        );
        assert!(merging > 200, "too few forests with a merge: {merging}");
    }

    #[test]
    fn opening_cost_is_linear_in_nested_and_sibling_groups() {
        // A chain of N groups, each body reading the previous group, and N
        // sibling groups reading one closed subgraph of M nodes. The full walk
        // re-opens everything below each body: Σ k over the chain plus N·M
        // for the siblings. The pruned walk visits the few nodes of each body
        // that depend on its own variable.
        const N: i32 = 200;
        const M: i32 = 200;
        let mut a = arena();
        let x = SigBuilder::new(&mut a).input(0);
        let mut closed = x;
        for i in 0..M {
            let mut b = SigBuilder::new(&mut a);
            let k = b.int(i);
            closed = b.add(closed, k);
        }
        let mut chain = closed;
        let mut siblings = Vec::new();
        for k in 0..N {
            let link = make_single_rec(&mut a, &format!("C{k}"), |b, rf| b.add(rf, chain));
            chain = SigBuilder::new(&mut a).proj(0, link);
            let sibling = make_single_rec(&mut a, &format!("S{k}"), |b, rf| {
                let product = b.mul(rf, closed);
                let c = b.int(k);
                b.add(product, c)
            });
            siblings.push(SigBuilder::new(&mut a).proj(0, sibling));
        }
        let mut roots = vec![chain];
        roots.extend(siblings);
        let groups = reachable_groups(&a, &roots);
        assert_eq!(groups.len(), 2 * N as usize);

        open_visits::take();
        let signatures = open_signatures(&mut a, &roots, &groups, true);
        let pruned = open_visits::take();
        let reference = open_signatures(&mut a, &roots, &groups, false);
        let full = open_visits::take();
        assert_eq!(signatures, reference);
        assert!(
            pruned <= 8 * groups.len() as u64,
            "{pruned} visits for {} groups",
            groups.len()
        );
        assert!(full >= (N * M) as u64, "the full walk visited only {full}");
    }

    #[test]
    fn open_symrec_replaces_only_own_symref() {
        let mut a = arena();
        let var1 = a.symbol("W1");
        let ref1 = sym_ref(&mut a, var1);

        let hole = a.symbol("__hole__");
        let rec = make_single_rec(&mut a, "W0", |b, rf| b.add(rf, ref1));
        let (var0, body_list) = tlib::match_sym_rec(&a, rec).unwrap();

        let mut cache = HashMap::new();
        let opened = super::open_symrec(&mut a, body_list, var0, hole, None, &mut cache);

        let bodies = list_to_vec(&a, opened).unwrap();
        match match_sig(&a, bodies[0]) {
            SigMatch::BinOp(_, left, right) => {
                assert_eq!(left, hole, "Own symref should be replaced by hole");
                assert_eq!(right, ref1, "Other symref should be untouched");
            }
            _ => panic!("Expected BinOp"),
        }
    }
}
