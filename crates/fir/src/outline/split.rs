//! Phase 2 of the outlining pass: long straight-line blocks cut into runs
//! (`porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §3.4).
//!
//! A function still above [`super::OutlinePolicy::split_budget`] after
//! phase 1 has its dominant block — found by descending into the largest
//! nested block of an item holding more than half of the current block — cut
//! into consecutive runs of at most `budget` statements. Each run becomes a
//! function called in its place, with the parameters of phase 1. An item the
//! analysis refuses is a barrier: it stays in the caller and ends the run.
//!
//! A local declared at the top level of a run and read after it escapes. It
//! goes through a per-type spill array declared by the caller
//! (`<method>Spill<k>`, a local `Array(T, n)` without initialiser, passed by
//! reference): the defining run ends with `spill[k] = e`, every later read of
//! `e` becomes `spill[k]`. Only a single-assignment local may escape —
//! declared once in the block with an initialiser, of a scalar type, never
//! stored into, tee'd, addressed or shifted — so the slot always holds the
//! value of `e`. An item declaring a local that may not escape (stored into
//! later, passed to an outlined function, an array) and used after it stays
//! in the caller, where the runs reach it as a parameter.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::{
    Added, Binding, Content, Env, FreeUse, Mode, Mutation, Param, Pass, block_items, content_items,
    is_local_access, is_scalar, resolve,
};
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

/// The infix of a spill array's name: `<method>Spill<k>`.
const SPILL_INFIX: &str = "Spill";
/// At most this many splits of one function (each makes progress or stops).
const MAX_SPLITS: usize = 64;

/// Which nested block of a statement a path step enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Then,
    Else,
    Body,
    Case(usize),
    Default,
}

/// One step from a block to a nested block: the item, then the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    index: usize,
    slot: Slot,
}

/// Statements of a block, nested ones included (as phase 1 counts them).
pub(super) fn block_size(store: &FirStore, block: FirId) -> usize {
    block_items(store, block)
        .into_iter()
        .map(|s| stmt_size(store, s))
        .sum()
}

pub(super) fn stmt_size(store: &FirStore, stmt: FirId) -> usize {
    let nested: usize = nested_blocks(store, stmt)
        .into_iter()
        .map(|(_, b)| block_size(store, b))
        .sum();
    match match_fir(store, stmt) {
        FirMatch::Block(_) => block_size(store, stmt),
        FirMatch::Control { stmt: inner, .. } => 1 + stmt_size(store, inner),
        _ => 1 + nested,
    }
}

/// The blocks nested directly in a statement, with the slot of each.
fn nested_blocks(store: &FirStore, stmt: FirId) -> Vec<(Slot, FirId)> {
    match match_fir(store, stmt) {
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => {
            let mut out = vec![(Slot::Then, then_block)];
            out.extend(else_block.map(|b| (Slot::Else, b)));
            out
        }
        FirMatch::ForLoop { body, .. }
        | FirMatch::SimpleForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. } => vec![(Slot::Body, body)],
        FirMatch::Switch { cases, default, .. } => {
            let mut out: Vec<_> = cases
                .iter()
                .enumerate()
                .map(|(k, &(_, b))| (Slot::Case(k), b))
                .collect();
            out.extend(default.map(|b| (Slot::Default, b)));
            out
        }
        _ => Vec::new(),
    }
}

/// How one item of the split block uses each local name, scoping ignored:
/// `true` when it stores into it, tees, addresses or shifts it, or passes it
/// to a function the pass created (such an argument must name the variable,
/// so it cannot become a spill slot read). Counts the declarations of each
/// name in `declared`.
fn names_used(
    store: &FirStore,
    id: FirId,
    added: &HashSet<String>,
    out: &mut HashMap<String, bool>,
    declared: &mut HashMap<String, usize>,
) {
    let used = match match_fir(store, id) {
        FirMatch::LoadVar { name, access, .. } | FirMatch::LoadTable { name, access, .. } => {
            Some((name, access, false))
        }
        FirMatch::StoreVar { name, access, .. }
        | FirMatch::StoreTable { name, access, .. }
        | FirMatch::TeeVar { name, access, .. }
        | FirMatch::LoadVarAddress { name, access, .. }
        | FirMatch::ShiftArrayVar { name, access, .. } => Some((name, access, true)),
        FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } => {
            *declared.entry(name).or_insert(0) += 1;
            None
        }
        FirMatch::ForLoop { var, .. } | FirMatch::SimpleForLoop { var, .. } => {
            *declared.entry(var).or_insert(0) += 1;
            None
        }
        FirMatch::FunCall { name, args, .. } if added.contains(&name) => {
            for arg in args {
                if let FirMatch::LoadVar { name, access, .. } = match_fir(store, arg)
                    && is_local_access(access)
                {
                    out.insert(name, true);
                }
            }
            None
        }
        _ => None,
    };
    if let Some((name, access, mutated)) = used
        && is_local_access(access)
    {
        *out.entry(name).or_insert(false) |= mutated;
    }
    for &child in store.arena.children(id).unwrap_or(&[]) {
        names_used(store, child, added, out, declared);
    }
}

/// What the cut selection knows of the split block's items.
struct Items {
    sizes: Vec<usize>,
    tops: Vec<Option<(String, FirType, bool)>>,
    barrier: Vec<bool>,
    /// Last item that uses each name.
    last_use: HashMap<String, usize>,
    /// Single-assignment status of each top-level declaration's name.
    single: Vec<bool>,
}

impl Items {
    /// Cuts the items into runs of at most `budget` statements between
    /// barriers. A run from `s` ends at a cut in the second half of its
    /// budget where the fewest locals cross; a cut that a non-single-
    /// assignment local would cross is not taken (unless `any_cut`).
    fn runs(&self, budget: usize, any_cut: bool) -> Vec<(usize, usize)> {
        let n = self.sizes.len();
        let mut runs = Vec::new();
        let mut i = 0;
        while i < n {
            if self.barrier[i] {
                i += 1;
                continue;
            }
            let seg_end = (i..n).find(|&k| self.barrier[k]).unwrap_or(n);
            let mut s = i;
            while s < seg_end {
                match self.cut(s, seg_end, budget, any_cut) {
                    Some(p) => {
                        runs.push((s, p));
                        s = p;
                    }
                    None => s += 1,
                }
            }
            i = seg_end;
        }
        runs
    }

    /// The end of the run starting at `s` (module doc), or `None` if no cut
    /// up to `seg_end` is allowed.
    fn cut(&self, s: usize, seg_end: usize, budget: usize, any_cut: bool) -> Option<usize> {
        // Locals crossing the cut before item `p`, and those of them that
        // may not cross; `dying[u]` the ones whose last use is item `u`.
        let (mut live, mut bad) = (0usize, 0usize);
        let mut dying: HashMap<usize, (usize, usize)> = HashMap::new();
        let mut cum = 0;
        let mut best: Option<(usize, usize)> = None;
        for p in s + 1..=seg_end {
            cum += self.sizes[p - 1];
            if let Some((name, _, _)) = &self.tops[p - 1]
                && let Some(&u) = self.last_use.get(name)
                && u >= p
            {
                let not_single = usize::from(!self.single[p - 1]);
                live += 1;
                bad += not_single;
                let d = dying.entry(u).or_insert((0, 0));
                d.0 += 1;
                d.1 += not_single;
            }
            if let Some((l, b)) = dying.remove(&(p - 1)) {
                live -= l;
                bad -= b;
            }
            let allowed = any_cut || bad == 0;
            if cum > budget && p > s + 1 {
                if best.is_some() {
                    break;
                }
                if allowed {
                    return Some(p);
                }
                continue;
            }
            if allowed && (cum * 2 >= budget || p == seg_end) && best.is_none_or(|(c, _)| live <= c)
            {
                best = Some((live, p));
            }
        }
        best.map(|(_, p)| p)
    }
}

/// The top-level declaration an item makes, if any: name, type, whether it
/// has an initialiser.
fn top_declaration(store: &FirStore, item: FirId) -> Option<(String, FirType, bool)> {
    match match_fir(store, item) {
        FirMatch::DeclareVar {
            name, typ, init, ..
        } => Some((name, typ, init.is_some())),
        FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } => Some((
            name,
            FirType::Array(Box::new(elem_type), values.len()),
            true,
        )),
        _ => None,
    }
}

fn declare_top(store: &FirStore, env: &mut Env, item: FirId) {
    if let Some((name, typ, _)) = top_declaration(store, item) {
        env.declare(name, Binding::Var(typ));
    }
}

/// One run chosen for outlining, before its function is built.
struct Run {
    start: usize,
    end: usize,
    params: Vec<Param>,
    /// Escapes of earlier runs it reads: name.
    reads: Vec<String>,
    /// Its own escapes, in declaration order: name, type.
    escapes: Vec<(String, FirType)>,
}

/// Turns accesses to escaped locals into accesses to their spill slots.
struct SlotRewriter<'a> {
    store: &'a mut FirStore,
    /// name → (spill array, slot, array type).
    slots: &'a HashMap<String, (String, i32, FirType)>,
    access: AccessType,
    memo: HashMap<FirId, FirId>,
    /// `SpillWrongSlot`: the first slot read reads a neighbouring slot.
    shift_first: bool,
    shifted: bool,
}

impl SlotRewriter<'_> {
    fn rewrite(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, typ }
                if is_local_access(access) && self.slots.contains_key(&name) =>
            {
                let (spill, k, _) = self.slots[&name].clone();
                let k = if self.shift_first && !self.shifted {
                    self.shifted = true;
                    if k > 0 { k - 1 } else { k + 1 }
                } else {
                    k
                };
                let mut b = FirBuilder::new(self.store);
                let index = b.int32(k);
                b.load_table(spill, self.access, index, typ)
            }
            _ => match self.store.arena.children(id).map(<[FirId]>::to_vec) {
                Some(children) if !children.is_empty() => {
                    let mut changed = false;
                    let mut new = Vec::with_capacity(children.len());
                    for c in children {
                        let n = self.rewrite(c);
                        changed |= n != c;
                        new.push(n);
                    }
                    match self.store.arena.kind(id).cloned() {
                        Some(kind) if changed => self.store.arena.intern(kind, &new),
                        _ => id,
                    }
                }
                _ => id,
            },
        };
        self.memo.insert(id, out);
        out
    }
}

impl Pass {
    fn fresh_spill(&mut self, root: &str) -> String {
        loop {
            let name = format!("{root}{SPILL_INFIX}{}", self.next_spill);
            self.next_spill += 1;
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }

    /// Phase 2 on one function: while it is above the split budget, split
    /// its dominant block.
    pub(super) fn split_large(
        &mut self,
        fname: &str,
        root: &str,
        base: &Env,
        mut body: FirId,
    ) -> FirId {
        for _ in 0..MAX_SPLITS {
            if block_size(&self.store, body) <= self.policy.split_budget {
                break;
            }
            let path = self.split_path(body);
            let mut env = base.clone();
            let new = self.split_along(fname, root, &mut env, body, &path);
            if new == body {
                break;
            }
            body = new;
        }
        body
    }

    /// The path to the block to split: from the body, into the largest
    /// nested block of an item holding more than half of the current block.
    fn split_path(&self, body: FirId) -> Vec<Step> {
        let mut path = Vec::new();
        let mut block = body;
        loop {
            let items = block_items(&self.store, block);
            let total = block_size(&self.store, block);
            let Some((index, size)) = items
                .iter()
                .enumerate()
                .map(|(i, &s)| (i, stmt_size(&self.store, s)))
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
            else {
                return path;
            };
            if size * 2 <= total {
                return path;
            }
            let Some((slot, child)) = nested_blocks(&self.store, items[index])
                .into_iter()
                .max_by_key(|&(_, b)| block_size(&self.store, b))
            else {
                return path;
            };
            path.push(Step { index, slot });
            block = child;
        }
    }

    /// `block` with the block at the end of `path` split; `env` holds what
    /// is visible at `block`.
    fn split_along(
        &mut self,
        fname: &str,
        root: &str,
        env: &mut Env,
        block: FirId,
        path: &[Step],
    ) -> FirId {
        env.push();
        let items = block_items(&self.store, block);
        let new_items = match path.split_first() {
            None => self.split_items(fname, root, env, &items),
            Some((step, rest)) => {
                for &item in &items[..step.index] {
                    declare_top(&self.store, env, item);
                }
                let item = items[step.index];
                let new = self.descend(fname, root, env, item, step.slot, rest);
                let mut new_items = items.clone();
                new_items[step.index] = new;
                new_items
            }
        };
        env.pop();
        if new_items == items {
            block
        } else {
            FirBuilder::new(&mut self.store).block(&new_items)
        }
    }

    fn descend(
        &mut self,
        fname: &str,
        root: &str,
        env: &mut Env,
        item: FirId,
        slot: Slot,
        rest: &[Step],
    ) -> FirId {
        match (match_fir(&self.store, item), slot) {
            (
                FirMatch::If {
                    cond,
                    then_block,
                    else_block,
                },
                Slot::Then | Slot::Else,
            ) => {
                if slot == Slot::Then {
                    let new = self.split_along(fname, root, env, then_block, rest);
                    FirBuilder::new(&mut self.store).if_(cond, new, else_block)
                } else {
                    let Some(else_block) = else_block else {
                        return item;
                    };
                    let new = self.split_along(fname, root, env, else_block, rest);
                    FirBuilder::new(&mut self.store).if_(cond, then_block, Some(new))
                }
            }
            (
                FirMatch::SimpleForLoop {
                    var,
                    upper,
                    body,
                    is_reverse,
                },
                Slot::Body,
            ) => {
                let typ = self.store.value_type(upper).unwrap_or(FirType::Int32);
                env.push();
                env.declare(var.clone(), Binding::Var(typ));
                let new = self.split_along(fname, root, env, body, rest);
                env.pop();
                FirBuilder::new(&mut self.store).simple_for_loop(var, upper, new, is_reverse)
            }
            (
                FirMatch::ForLoop {
                    var,
                    init,
                    end,
                    step,
                    body,
                    is_reverse,
                },
                Slot::Body,
            ) => {
                env.push();
                declare_top(&self.store, env, init);
                let new = self.split_along(fname, root, env, body, rest);
                env.pop();
                FirBuilder::new(&mut self.store).for_loop(var, init, end, step, new, is_reverse)
            }
            (FirMatch::WhileLoop { cond, body }, Slot::Body) => {
                let new = self.split_along(fname, root, env, body, rest);
                FirBuilder::new(&mut self.store).while_loop(cond, new)
            }
            (
                FirMatch::Switch {
                    cond,
                    cases,
                    default,
                },
                Slot::Case(_) | Slot::Default,
            ) => {
                let mut cases = cases;
                let mut default = default;
                match slot {
                    Slot::Case(k) => {
                        let new = self.split_along(fname, root, env, cases[k].1, rest);
                        cases[k].1 = new;
                    }
                    _ => {
                        let Some(d) = default else {
                            return item;
                        };
                        default = Some(self.split_along(fname, root, env, d, rest));
                    }
                }
                FirBuilder::new(&mut self.store).switch(cond, &cases, default)
            }
            _ => item,
        }
    }

    /// Sizes, declarations, barriers and uses of the items of the block to
    /// split; `env` holds what is visible at the block.
    fn scan_items(&self, env: &Env, items: &[FirId]) -> Items {
        let sizes: Vec<usize> = items.iter().map(|&i| stmt_size(&self.store, i)).collect();
        let tops: Vec<Option<(String, FirType, bool)>> = items
            .iter()
            .map(|&i| top_declaration(&self.store, i))
            .collect();
        let mut declared: HashMap<String, usize> = HashMap::new();
        let mut mutated: HashSet<String> = HashSet::new();
        let mut last_use: HashMap<String, usize> = HashMap::new();
        let mut barrier = Vec::with_capacity(items.len());
        let mut env_i = env.clone();
        for (i, &item) in items.iter().enumerate() {
            let mut used = HashMap::new();
            names_used(
                &self.store,
                item,
                &self.added_names,
                &mut used,
                &mut declared,
            );
            for (name, m) in used {
                if m {
                    mutated.insert(name.clone());
                }
                last_use.insert(name, i);
            }
            let c: Content = content_items(&self.store, &self.added_names, &[item]);
            barrier.push(match c {
                Err(_) => true,
                Ok(uses) => resolve(&env_i, &uses).is_err(),
            });
            declare_top(&self.store, &mut env_i, item);
        }
        let single: Vec<bool> = tops
            .iter()
            .map(|top| {
                top.as_ref().is_some_and(|(name, typ, has_init)| {
                    *has_init
                        && is_scalar(typ)
                        && declared.get(name) == Some(&1)
                        && !mutated.contains(name)
                })
            })
            .collect();
        // An item declaring a local that may not escape, and that something
        // after it uses, stays in the caller: the runs then reach that local
        // as a parameter, and no cut has to avoid it.
        let any_cut = self.mutation == Mutation::SpillMutable;
        for (i, top) in tops.iter().enumerate() {
            if let Some((name, _, _)) = top
                && !any_cut
                && !single[i]
                && last_use.get(name).is_some_and(|&u| u > i)
            {
                barrier[i] = true;
            }
        }
        Items {
            sizes,
            tops,
            barrier,
            last_use,
            single,
        }
    }

    /// The items of one block, its long straight-line stretches cut into
    /// runs moved into functions (module doc).
    fn split_items(
        &mut self,
        fname: &str,
        root: &str,
        env: &mut Env,
        items: &[FirId],
    ) -> Vec<FirId> {
        let n = items.len();
        let info = self.scan_items(env, items);
        let runs = info.runs(
            self.policy.budget.max(1),
            self.mutation == Mutation::SpillMutable,
        );
        let Items {
            sizes,
            tops,
            last_use,
            ..
        } = info;

        // Each run: outlined if large enough and its analysis resolves.
        let min = self.policy.min_block.max(2);
        let mut escape_of: HashMap<String, FirType> = HashMap::new();
        let mut chosen: Vec<Run> = Vec::new();
        let mut env_s = env.clone();
        let mut next_item = 0;
        for (s, p) in runs {
            for &item in &items[next_item..s] {
                declare_top(&self.store, &mut env_s, item);
            }
            next_item = s;
            let size: usize = sizes[s..p].iter().sum();
            if size < min {
                continue;
            }
            let Ok(uses) = content_items(&self.store, &self.added_names, &items[s..p]) else {
                continue;
            };
            let (reads, rest): (Vec<_>, Vec<_>) = uses
                .into_iter()
                .partition(|(name, _)| escape_of.contains_key(name));
            let rest: BTreeMap<String, FreeUse> = rest.into_iter().collect();
            if reads.iter().any(|(_, u)| u.stores || u.at_zero || u.other) {
                continue;
            }
            let Ok(params) = resolve(&env_s, &rest) else {
                continue;
            };
            let mut escapes = Vec::new();
            for top in &tops[s..p] {
                if let Some((name, typ, _)) = top
                    && last_use.get(name).is_some_and(|&u| u >= p)
                {
                    escapes.push((name.clone(), typ.clone()));
                }
            }
            for (name, typ) in &escapes {
                escape_of.insert(name.clone(), typ.clone());
            }
            chosen.push(Run {
                start: s,
                end: p,
                params,
                reads: reads.into_iter().map(|(name, _)| name).collect(),
                escapes,
            });
        }
        if chosen.is_empty() {
            return items.to_vec();
        }

        // Spill slots, per element type, in declaration order.
        let mut spill_of_type: Vec<(FirType, String, i32)> = Vec::new();
        let mut slots: HashMap<String, (String, i32, FirType)> = HashMap::new();
        for run in &chosen {
            for (name, typ) in &run.escapes {
                let pos = match spill_of_type.iter().position(|(t, _, _)| t == typ) {
                    Some(pos) => pos,
                    None => {
                        let spill = self.fresh_spill(root);
                        spill_of_type.push((typ.clone(), spill, 0));
                        spill_of_type.len() - 1
                    }
                };
                let (_, spill, count) = &mut spill_of_type[pos];
                slots.insert(name.clone(), (spill.clone(), *count, typ.clone()));
                *count += 1;
            }
        }
        let array_of = |spill: &str| -> (FirType, usize) {
            let (t, _, n) = spill_of_type
                .iter()
                .find(|(_, s, _)| s == spill)
                .expect("spill");
            (t.clone(), usize::try_from(*n).expect("slot count"))
        };

        // Build the runs' functions and the calls.
        let mut calls: Vec<(usize, usize, FirId)> = Vec::new();
        for run in &chosen {
            let mut spills: Vec<String> = run
                .reads
                .iter()
                .chain(run.escapes.iter().map(|(n, _)| n))
                .map(|n| slots[n].0.clone())
                .collect();
            spills.sort();
            spills.dedup();
            let mut params = run.params.clone();
            for spill in &spills {
                let (typ, n) = array_of(spill);
                params.push(Param {
                    name: spill.clone(),
                    typ,
                    mode: Mode::Spill(n),
                    access: AccessType::Stack,
                });
            }
            params.sort_by(|a, b| a.name.cmp(&b.name));
            let name = self.fresh_name(root);
            let block = FirBuilder::new(&mut self.store).block(&items[run.start..run.end]);
            let body = self.callee_body(block, &params);
            let read_slots: HashMap<String, (String, i32, FirType)> = run
                .reads
                .iter()
                .map(|n| (n.clone(), slots[n].clone()))
                .collect();
            let shift = self.mutation == Mutation::SpillWrongSlot
                && !self.mutated
                && !read_slots.is_empty();
            let mut rw = SlotRewriter {
                store: &mut self.store,
                slots: &read_slots,
                access: AccessType::FunArgs,
                memo: HashMap::new(),
                shift_first: shift,
                shifted: false,
            };
            let body = rw.rewrite(body);
            if rw.shifted {
                self.mutated = true;
            }
            let mut body_items = block_items(&self.store, body);
            for (e, typ) in &run.escapes {
                let (spill, k, _) = slots[e].clone();
                let mut b = FirBuilder::new(&mut self.store);
                let index = b.int32(k);
                let value = b.load_var(e.clone(), AccessType::Stack, typ.clone());
                body_items.push(b.store_table(spill, AccessType::FunArgs, index, value));
            }
            let body = FirBuilder::new(&mut self.store).block(&body_items);
            let call = self.call_stmt(&name, &params);
            calls.push((run.start, run.end, call));
            self.queue.push_back(self.added.len());
            self.added.push(Added {
                name,
                root: root.to_owned(),
                parent: fname.to_owned(),
                params,
                body,
                statements: sizes[run.start..run.end].iter().sum(),
                is_run: true,
            });
        }
        if calls.len() >= 2 && self.mutate(Mutation::SpillEarly) {
            let (a, b) = (calls[0].2, calls[1].2);
            calls[0].2 = b;
            calls[1].2 = a;
        }

        // The caller: spill declarations before the first call, calls in
        // place of the runs, later reads of escapes through the slots.
        let mut out = Vec::with_capacity(n);
        let mut k = 0;
        let mut defined: HashMap<String, (String, i32, FirType)> = HashMap::new();
        let mut i = 0;
        while i < n {
            if k < calls.len() && calls[k].0 == i {
                if k == 0 {
                    for (typ, spill, count) in &spill_of_type {
                        let n = usize::try_from(*count).expect("slot count");
                        let arr = FirType::Array(Box::new(typ.clone()), n);
                        out.push(FirBuilder::new(&mut self.store).declare_var(
                            spill.clone(),
                            arr,
                            AccessType::Stack,
                            None,
                        ));
                    }
                }
                out.push(calls[k].2);
                for (e, _) in &chosen[k].escapes {
                    defined.insert(e.clone(), slots[e].clone());
                }
                i = calls[k].1;
                k += 1;
                continue;
            }
            let item = items[i];
            if defined.is_empty() {
                out.push(item);
            } else {
                let mut rw = SlotRewriter {
                    store: &mut self.store,
                    slots: &defined,
                    access: AccessType::Stack,
                    memo: HashMap::new(),
                    shift_first: false,
                    shifted: false,
                };
                out.push(rw.rewrite(item));
            }
            i += 1;
        }
        out
    }
}
