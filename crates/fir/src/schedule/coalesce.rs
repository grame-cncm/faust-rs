//! W7: stack scalars of `compute` whose lifetimes do not overlap share one
//! variable (producer, design note `porting/fir-local-reuse-w7-design-2026-10-17-en.md` §2).
//!
//! Each shared variable, a *slot* (`fReuse0`, `iReuse3`...), is declared once
//! at the top of `compute`; `DeclareVar x = e` becomes `StoreVar slot = e`, and
//! the loads and stores of `x` use the slot. One wasm local, interpreter heap
//! cell or Cranelift variable then serves many FIR values: JavaScriptCore
//! (Safari) gives every wasm local its own stack slot, and tens of thousands of
//! them overflow the 512 KB stack of an AudioWorklet thread.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

/// Slot name prefix of each candidate type.
fn slot_prefix(typ: &FirType) -> Option<&'static str> {
    match typ {
        FirType::Float64 => Some("fReuse"),
        FirType::Float32 => Some("fReusef"),
        FirType::Int32 => Some("iReuse"),
        FirType::Int64 => Some("lReuse"),
        _ => None,
    }
}

/// Whether `name` looks like a slot: such names are never candidates, so a
/// second run of the pass leaves its own output alone.
fn is_slot_like(name: &str) -> bool {
    ["fReuse", "iReuse", "lReuse"]
        .iter()
        .any(|p| name.starts_with(p))
}

#[derive(Default)]
struct Var {
    typ: Option<FirType>,
    /// Two declarations with different types, or a mention the rewriter does
    /// not handle.
    excluded: bool,
    first_decl: Option<usize>,
    last: usize,
}

/// Program-order numbering of `compute` and the facts the intervals need.
#[derive(Default)]
struct Analysis {
    pos: usize,
    index: HashMap<String, usize>,
    vars: Vec<Var>,
    /// Every name the body uses, for fresh slot names.
    names: HashSet<String>,
    /// Open loops (indices into `loops`) around the current position.
    open: Vec<usize>,
    /// `(start, end)` of every loop, `end` set when it closes.
    loops: Vec<(usize, usize)>,
    /// Per (variable, loop): whether its first mention in the loop declares it.
    first_in_loop: HashMap<(usize, usize), bool>,
}

impl Analysis {
    fn var(&mut self, name: &str) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        self.names.insert(name.to_owned());
        self.vars.push(Var::default());
        self.index.insert(name.to_owned(), self.vars.len() - 1);
        self.vars.len() - 1
    }

    fn exclude(&mut self, name: &str) {
        let i = self.var(name);
        self.vars[i].excluded = true;
    }

    fn mention(&mut self, name: &str, decl: Option<&FirType>) {
        let i = self.var(name);
        let pos = self.pos;
        let v = &mut self.vars[i];
        v.last = v.last.max(pos);
        if let Some(typ) = decl {
            if v.first_decl.is_none() {
                v.first_decl = Some(pos);
            }
            match &v.typ {
                None => v.typ = Some(typ.clone()),
                Some(t) if t != typ => v.excluded = true,
                Some(_) => {}
            }
        }
        for &l in &self.open {
            self.first_in_loop.entry((i, l)).or_insert(decl.is_some());
        }
    }

    /// Every variable mentioned anywhere under `id` is excluded: the rewriter
    /// leaves that subtree as it is.
    fn exclude_subtree(&mut self, store: &FirStore, id: FirId) {
        match match_fir(store, id) {
            FirMatch::LoadVar { name, .. }
            | FirMatch::LoadVarAddress { name, .. }
            | FirMatch::TeeVar { name, .. }
            | FirMatch::DeclareVar { name, .. }
            | FirMatch::StoreVar { name, .. }
            | FirMatch::LoadTable { name, .. }
            | FirMatch::StoreTable { name, .. }
            | FirMatch::ShiftArrayVar { name, .. } => self.exclude(&name),
            FirMatch::SimpleForLoop { var, .. } | FirMatch::ForLoop { var, .. } => {
                self.exclude(&var)
            }
            FirMatch::IteratorForLoop { iterators, .. } => {
                for it in iterators {
                    self.exclude(&it);
                }
            }
            _ => {}
        }
        for child in crate::child_ids(&match_fir(store, id)) {
            self.exclude_subtree(store, child);
        }
    }

    fn value(&mut self, store: &FirStore, id: FirId) {
        match match_fir(store, id) {
            FirMatch::LoadVar { name, access, .. } => {
                if access == AccessType::Stack {
                    self.mention(&name, None);
                } else {
                    self.exclude(&name);
                }
            }
            FirMatch::LoadTable { name, index, .. } => {
                self.exclude(&name);
                self.value(store, index);
            }
            FirMatch::BinOp { lhs, rhs, .. } => {
                self.value(store, lhs);
                self.value(store, rhs);
            }
            FirMatch::Neg { value, .. }
            | FirMatch::Cast { value, .. }
            | FirMatch::Bitcast { value, .. } => self.value(store, value),
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => {
                self.value(store, cond);
                self.value(store, then_value);
                self.value(store, else_value);
            }
            FirMatch::FunCall { args, .. } => {
                for a in args {
                    self.value(store, a);
                }
            }
            FirMatch::Bool { .. }
            | FirMatch::Int32 { .. }
            | FirMatch::Int64 { .. }
            | FirMatch::Float32 { .. }
            | FirMatch::Float64 { .. } => {}
            _ => self.exclude_subtree(store, id),
        }
    }

    fn open_loop(&mut self) {
        self.loops.push((self.pos, 0));
        self.open.push(self.loops.len() - 1);
    }

    fn close_loop(&mut self) {
        let l = self.open.pop().expect("an open loop");
        self.loops[l].1 = self.pos;
        self.pos += 1;
    }

    fn stmt(&mut self, store: &FirStore, id: FirId) {
        match match_fir(store, id) {
            FirMatch::Block(items) => {
                for item in items {
                    self.stmt(store, item);
                }
            }
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            } => {
                if let Some(init) = init {
                    self.value(store, init);
                }
                if access == AccessType::Stack && slot_prefix(&typ).is_some() {
                    self.mention(&name, Some(&typ));
                } else {
                    self.exclude(&name);
                }
                self.pos += 1;
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                self.value(store, value);
                if access == AccessType::Stack {
                    self.mention(&name, None);
                } else {
                    self.exclude(&name);
                }
                self.pos += 1;
            }
            FirMatch::StoreTable {
                name, index, value, ..
            } => {
                self.exclude(&name);
                self.value(store, index);
                self.value(store, value);
                self.pos += 1;
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.value(store, cond);
                self.pos += 1;
                self.stmt(store, then_block);
                if let Some(e) = else_block {
                    self.stmt(store, e);
                }
            }
            FirMatch::Control { cond, stmt } => {
                self.value(store, cond);
                self.pos += 1;
                self.stmt(store, stmt);
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.value(store, cond);
                self.pos += 1;
                for (_, b) in cases {
                    self.stmt(store, b);
                }
                if let Some(d) = default {
                    self.stmt(store, d);
                }
            }
            // A loop's header (bound, step, condition) is evaluated at every
            // iteration: it is numbered inside the loop.
            FirMatch::SimpleForLoop {
                var, upper, body, ..
            } => {
                self.exclude(&var);
                self.open_loop();
                self.value(store, upper);
                self.pos += 1;
                self.stmt(store, body);
                self.close_loop();
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                ..
            } => {
                self.exclude(&var);
                self.value(store, init);
                self.pos += 1;
                self.open_loop();
                self.value(store, end);
                self.value(store, step);
                self.pos += 1;
                self.stmt(store, body);
                self.close_loop();
            }
            FirMatch::WhileLoop { cond, body } => {
                self.open_loop();
                self.value(store, cond);
                self.pos += 1;
                self.stmt(store, body);
                self.close_loop();
            }
            FirMatch::Drop(value) => {
                self.value(store, value);
                self.pos += 1;
            }
            FirMatch::Label(_) | FirMatch::NullStatement => {}
            _ => {
                self.exclude_subtree(store, id);
                self.pos += 1;
            }
        }
    }

    /// The live interval of each candidate: `(start, end, name, type)`.
    fn intervals(&self) -> Vec<(usize, usize, String, FirType)> {
        let mut span: Vec<Option<(usize, usize)>> = self
            .vars
            .iter()
            .map(|v| v.first_decl.map(|s| (s, v.last)))
            .collect();
        for (&(i, l), &first_is_decl) in &self.first_in_loop {
            let (ls, le) = self.loops[l];
            if let Some((s, e)) = &mut span[i]
                && (*s < ls || !first_is_decl)
            {
                *s = (*s).min(ls);
                *e = (*e).max(le);
            }
        }
        let mut names: Vec<(&String, &usize)> = self.index.iter().collect();
        names.sort_by_key(|(_, i)| **i);
        names
            .into_iter()
            .filter_map(|(name, &i)| {
                let v = &self.vars[i];
                let (s, e) = span[i]?;
                let typ = v.typ.clone()?;
                let candidate = !v.excluded
                    && !name.starts_with("input")
                    && !name.starts_with("output")
                    && !is_slot_like(name);
                candidate.then(|| (s, e, name.clone(), typ))
            })
            .collect()
    }
}

/// Slot of each renamed variable, and the slots (name, type) in creation order.
type Assignment = (HashMap<String, String>, Vec<(String, FirType)>);

/// Per type: the slots in use, by the end of their current interval, and the
/// free ones.
#[derive(Default)]
struct Pool {
    active: BinaryHeap<Reverse<(usize, usize)>>,
    free: Vec<usize>,
}

/// Assigns slots by linear scan, per type: a slot is reused once the interval
/// that held it ended strictly before the new one starts. Returns the slot of
/// each variable and the slots in creation order.
fn assign(
    mut intervals: Vec<(usize, usize, String, FirType)>,
    names: &HashSet<String>,
) -> Option<Assignment> {
    intervals.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    let mut pools: HashMap<&'static str, Pool> = HashMap::new();
    let mut slots: Vec<(String, FirType)> = Vec::new();
    let mut map = HashMap::new();
    for (s, e, name, typ) in intervals {
        let prefix = slot_prefix(&typ)?;
        let pool = pools.entry(prefix).or_default();
        while let Some(Reverse((end, slot))) = pool.active.peek().copied() {
            if end >= s {
                break;
            }
            pool.active.pop();
            pool.free.push(slot);
        }
        let slot = match pool.free.pop() {
            Some(slot) => slot,
            None => {
                let k = slots.iter().filter(|(_, t)| *t == typ).count();
                let slot_name = format!("{prefix}{k}");
                if names.contains(&slot_name) {
                    return None;
                }
                slots.push((slot_name, typ.clone()));
                slots.len() - 1
            }
        };
        pool.active.push(Reverse((e, slot)));
        map.insert(name, slots[slot].0.clone());
    }
    Some((map, slots))
}

/// Rewrites the mentions of the renamed variables.
struct Renamer<'a> {
    store: &'a mut FirStore,
    map: &'a HashMap<String, String>,
}

impl Renamer<'_> {
    fn zero(&mut self, typ: &FirType) -> FirId {
        let mut b = FirBuilder::new(self.store);
        match typ {
            FirType::Float64 => b.float64(0.0),
            FirType::Float32 => b.float32(0.0),
            FirType::Int64 => b.int64(0),
            _ => b.int32(0),
        }
    }

    fn value(&mut self, id: FirId) -> FirId {
        match match_fir(self.store, id) {
            FirMatch::LoadVar {
                name,
                access: AccessType::Stack,
                typ,
            } => match self.map.get(&name) {
                Some(slot) => {
                    FirBuilder::new(self.store).load_var(slot.clone(), AccessType::Stack, typ)
                }
                None => id,
            },
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                let i = self.value(index);
                if i == index {
                    return id;
                }
                FirBuilder::new(self.store).load_table(name, access, i, typ)
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                let (l, r) = (self.value(lhs), self.value(rhs));
                if (l, r) == (lhs, rhs) {
                    return id;
                }
                FirBuilder::new(self.store).binop(op, l, r, typ)
            }
            FirMatch::Neg { value, typ } => {
                let v = self.value(value);
                if v == value {
                    return id;
                }
                FirBuilder::new(self.store).neg(v, typ)
            }
            FirMatch::Cast { typ, value } => {
                let v = self.value(value);
                if v == value {
                    return id;
                }
                FirBuilder::new(self.store).cast(typ, v)
            }
            FirMatch::Bitcast { typ, value } => {
                let v = self.value(value);
                if v == value {
                    return id;
                }
                FirBuilder::new(self.store).bitcast(typ, v)
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                let c = self.value(cond);
                let t = self.value(then_value);
                let e = self.value(else_value);
                if (c, t, e) == (cond, then_value, else_value) {
                    return id;
                }
                FirBuilder::new(self.store).select2(c, t, e, typ)
            }
            FirMatch::FunCall { name, args, typ } => {
                let new_args: Vec<FirId> = args.iter().map(|&a| self.value(a)).collect();
                if new_args == args {
                    return id;
                }
                FirBuilder::new(self.store).fun_call(name, &new_args, typ)
            }
            _ => id,
        }
    }

    fn opt(&mut self, id: Option<FirId>) -> Option<FirId> {
        id.map(|i| self.stmt(i))
    }

    fn stmt(&mut self, id: FirId) -> FirId {
        match match_fir(self.store, id) {
            FirMatch::Block(items) => {
                let new_items: Vec<FirId> = items.iter().map(|&s| self.stmt(s)).collect();
                if new_items == items {
                    return id;
                }
                FirBuilder::new(self.store).block(&new_items)
            }
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            } => {
                let new_init = init.map(|i| self.value(i));
                if access == AccessType::Stack
                    && let Some(slot) = self.map.get(&name)
                {
                    let value = match new_init {
                        Some(v) => v,
                        None => self.zero(&typ),
                    };
                    return FirBuilder::new(self.store).store_var(
                        slot.clone(),
                        AccessType::Stack,
                        value,
                    );
                }
                if new_init == init {
                    return id;
                }
                FirBuilder::new(self.store).declare_var(name, typ, access, new_init)
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                let v = self.value(value);
                let target = match (access, self.map.get(&name)) {
                    (AccessType::Stack, Some(slot)) => slot.clone(),
                    _ if v == value => return id,
                    _ => name,
                };
                FirBuilder::new(self.store).store_var(target, access, v)
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                let (i, v) = (self.value(index), self.value(value));
                if (i, v) == (index, value) {
                    return id;
                }
                FirBuilder::new(self.store).store_table(name, access, i, v)
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.value(cond);
                let t = self.stmt(then_block);
                let e = self.opt(else_block);
                if (c, t, e) == (cond, then_block, else_block) {
                    return id;
                }
                FirBuilder::new(self.store).if_(c, t, e)
            }
            FirMatch::Control { cond, stmt } => {
                let (c, s) = (self.value(cond), self.stmt(stmt));
                if (c, s) == (cond, stmt) {
                    return id;
                }
                FirBuilder::new(self.store).control(c, s)
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                let c = self.value(cond);
                let new_cases: Vec<(i64, FirId)> =
                    cases.iter().map(|&(k, b)| (k, self.stmt(b))).collect();
                let d = self.opt(default);
                if c == cond && new_cases == cases && d == default {
                    return id;
                }
                FirBuilder::new(self.store).switch(c, &new_cases, d)
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => {
                let (u, b) = (self.value(upper), self.stmt(body));
                if (u, b) == (upper, body) {
                    return id;
                }
                FirBuilder::new(self.store).simple_for_loop(var, u, b, is_reverse)
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => {
                let (i, e, s) = (self.value(init), self.value(end), self.value(step));
                let b = self.stmt(body);
                if (i, e, s, b) == (init, end, step, body) {
                    return id;
                }
                FirBuilder::new(self.store).for_loop(var, i, e, s, b, is_reverse)
            }
            FirMatch::WhileLoop { cond, body } => {
                let (c, b) = (self.value(cond), self.stmt(body));
                if (c, b) == (cond, body) {
                    return id;
                }
                FirBuilder::new(self.store).while_loop(c, b)
            }
            FirMatch::Drop(value) => {
                let v = self.value(value);
                if v == value {
                    return id;
                }
                FirBuilder::new(self.store).drop_(v)
            }
            _ => id,
        }
    }
}

/// W7 on `compute`'s body `body`: the rewritten body (slot declarations first),
/// or `None` when no variable is renamed.
pub(crate) fn reuse_locals(store: &mut FirStore, body: FirId) -> Option<FirId> {
    let FirMatch::Block(_) = match_fir(store, body) else {
        return None;
    };
    let mut analysis = Analysis::default();
    analysis.stmt(store, body);
    let intervals = analysis.intervals();
    if intervals.is_empty() {
        return None;
    }
    let (map, slots) = assign(intervals, &analysis.names)?;
    let renamed = Renamer {
        store: &mut *store,
        map: &map,
    }
    .stmt(body);
    let FirMatch::Block(items) = match_fir(store, renamed) else {
        return None;
    };
    let mut b = FirBuilder::new(store);
    let mut new_items: Vec<FirId> = slots
        .into_iter()
        .map(|(name, typ)| b.declare_var(name, typ, AccessType::Stack, None))
        .collect();
    new_items.extend(items);
    Some(b.block(&new_items))
}
