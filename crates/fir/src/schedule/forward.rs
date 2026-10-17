//! W3: forward stored struct scalars to later loads (producer, design note §3).
//!
//! After `fRec0[0] = fRecBody6;`, a later `fRec0[0]` in the same straight-line
//! region reads `fRecBody6` instead of memory, as long as nothing in between
//! may write `fRec0[0]`. Only values that are already a local assigned once in
//! `compute`, or a constant, are forwarded: no variable is created.

use std::collections::HashMap;

use super::effects::{Addr, effects_of, table_addr};
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, match_fir};

/// Value forwarded for each available address, indexed by name so that a
/// write only touches the entries of the field or array it writes.
#[derive(Clone, Default)]
pub(crate) struct Available {
    /// `None`: the scalar field; `Some(k)`: element `k` of the array.
    by_name: HashMap<String, HashMap<Option<i32>, FirId>>,
}

impl Available {
    fn key(addr: &Addr) -> Option<(&str, Option<i32>)> {
        match addr {
            Addr::Field(x) => Some((x, None)),
            Addr::Elem(x, k) => Some((x, Some(*k))),
            _ => None,
        }
    }

    fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    fn clear(&mut self) {
        self.by_name.clear();
    }

    fn get(&self, addr: &Addr) -> Option<FirId> {
        let (name, slot) = Self::key(addr)?;
        self.by_name.get(name)?.get(&slot).copied()
    }

    fn insert(&mut self, addr: &Addr, value: FirId) {
        if let Some((name, slot)) = Self::key(addr) {
            self.by_name
                .entry(name.to_owned())
                .or_default()
                .insert(slot, value);
        }
    }

    /// Drops the entries a write of `addr` may change (`Addr::conflicts`).
    fn invalidate(&mut self, addr: &Addr) {
        match addr {
            Addr::All => self.clear(),
            Addr::Field(x) => {
                self.by_name.remove(x);
            }
            Addr::Elem(x, k) => {
                if let Some(slots) = self.by_name.get_mut(x) {
                    slots.remove(&None);
                    slots.remove(&Some(*k));
                    if slots.is_empty() {
                        self.by_name.remove(x);
                    }
                }
            }
            Addr::Local(_) | Addr::Stack => {}
        }
    }
}

/// Number of writes of each local in a function body (declarations, stores,
/// loop variables, tees).
pub(crate) fn local_write_counts(store: &FirStore, id: FirId, out: &mut HashMap<String, usize>) {
    let mut bump = |name: String| *out.entry(name).or_default() += 1;
    match match_fir(store, id) {
        FirMatch::DeclareVar {
            name,
            access: AccessType::Stack | AccessType::Loop,
            ..
        }
        | FirMatch::StoreVar {
            name,
            access: AccessType::Stack | AccessType::Loop,
            ..
        }
        | FirMatch::TeeVar {
            name,
            access: AccessType::Stack | AccessType::Loop,
            ..
        } => bump(name),
        FirMatch::SimpleForLoop { var, .. } | FirMatch::ForLoop { var, .. } => bump(var),
        _ => {}
    }
    for child in crate::child_ids(&match_fir(store, id)) {
        local_write_counts(store, child, out);
    }
}

pub(crate) struct Forwarder<'a> {
    store: &'a mut FirStore,
    writes: &'a HashMap<String, usize>,
    pub(crate) forwarded: usize,
}

impl<'a> Forwarder<'a> {
    pub(crate) fn new(store: &'a mut FirStore, writes: &'a HashMap<String, usize>) -> Self {
        Self {
            store,
            writes,
            forwarded: 0,
        }
    }

    /// A value that may stand for a stored address: a constant, or a local
    /// assigned once.
    fn forwardable(&self, value: FirId) -> bool {
        match match_fir(self.store, value) {
            FirMatch::Bool { .. }
            | FirMatch::Int32 { .. }
            | FirMatch::Int64 { .. }
            | FirMatch::Float32 { .. }
            | FirMatch::Float64 { .. } => true,
            FirMatch::LoadVar {
                name,
                access: AccessType::Stack,
                ..
            } => self.writes.get(&name) == Some(&1),
            _ => false,
        }
    }

    /// `value` for a load of type `typ`, if the types agree.
    fn substitute(
        &self,
        available: &Available,
        addr: &Addr,
        typ: &crate::FirType,
    ) -> Option<FirId> {
        let value = available.get(addr)?;
        (self.store.value_type(value).as_ref() == Some(typ)).then_some(value)
    }

    fn invalidate(available: &mut Available, writes: &[Addr]) {
        for w in writes {
            if available.is_empty() {
                return;
            }
            available.invalidate(w);
        }
    }

    /// Rewrites the loads of `id` that read an available address.
    fn value(&mut self, id: FirId, available: &Available) -> FirId {
        if available.is_empty() {
            return id;
        }
        match match_fir(self.store, id) {
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct | AccessType::Static,
                typ,
            } => match self.substitute(available, &Addr::Field(name), &typ) {
                Some(value) => {
                    self.forwarded += 1;
                    value
                }
                None => id,
            },
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                let new_index = self.value(index, available);
                if matches!(access, AccessType::Struct | AccessType::Static) {
                    let addr = table_addr(self.store, &name, new_index);
                    if matches!(addr, Addr::Elem(..))
                        && let Some(value) = self.substitute(available, &addr, &typ)
                    {
                        self.forwarded += 1;
                        return value;
                    }
                }
                if new_index == index {
                    return id;
                }
                FirBuilder::new(self.store).load_table(name, access, new_index, typ)
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                let (l, r) = (self.value(lhs, available), self.value(rhs, available));
                if (l, r) == (lhs, rhs) {
                    return id;
                }
                FirBuilder::new(self.store).binop(op, l, r, typ)
            }
            FirMatch::Neg { value, typ } => {
                let v = self.value(value, available);
                if v == value {
                    return id;
                }
                FirBuilder::new(self.store).neg(v, typ)
            }
            FirMatch::Cast { typ, value } => {
                let v = self.value(value, available);
                if v == value {
                    return id;
                }
                FirBuilder::new(self.store).cast(typ, v)
            }
            FirMatch::Bitcast { typ, value } => {
                let v = self.value(value, available);
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
                let c = self.value(cond, available);
                let t = self.value(then_value, available);
                let e = self.value(else_value, available);
                if (c, t, e) == (cond, then_value, else_value) {
                    return id;
                }
                FirBuilder::new(self.store).select2(c, t, e, typ)
            }
            FirMatch::FunCall { name, args, typ } => {
                let new_args: Vec<FirId> = args.iter().map(|&a| self.value(a, available)).collect();
                if new_args == args {
                    return id;
                }
                FirBuilder::new(self.store).fun_call(name, &new_args, typ)
            }
            _ => id,
        }
    }

    /// Rewrites a block's statements in order, threading `available`.
    pub(crate) fn block(&mut self, id: FirId, available: &mut Available) -> FirId {
        let FirMatch::Block(items) = match_fir(self.store, id) else {
            return self.stmt(id, available);
        };
        let new_items: Vec<FirId> = items.iter().map(|&s| self.stmt(s, available)).collect();
        if new_items == items {
            return id;
        }
        FirBuilder::new(self.store).block(&new_items)
    }

    fn arm(&mut self, id: FirId, available: &Available) -> FirId {
        let mut copy = available.clone();
        self.block(id, &mut copy)
    }

    fn stmt(&mut self, id: FirId, available: &mut Available) -> FirId {
        // A struct/static store's address and forwardable value, recorded after
        // the generic invalidation below.
        let mut record: Option<(Addr, FirId)> = None;
        let new_id = match match_fir(self.store, id) {
            // A nested block is a scope: a local declared in it may not stand
            // for a store after it, so it gets a copy, like an arm.
            FirMatch::Block(_) => self.arm(id, available),
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init: Some(init),
            } => {
                let v = self.value(init, available);
                if v == init {
                    id
                } else {
                    FirBuilder::new(self.store).declare_var(name, typ, access, Some(v))
                }
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                let v = self.value(value, available);
                let new_id = if v == value {
                    id
                } else {
                    FirBuilder::new(self.store).store_var(name.clone(), access, v)
                };
                if matches!(access, AccessType::Struct | AccessType::Static) && self.forwardable(v)
                {
                    record = Some((Addr::Field(name), v));
                }
                new_id
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                let i = self.value(index, available);
                let v = self.value(value, available);
                let new_id = if (i, v) == (index, value) {
                    id
                } else {
                    FirBuilder::new(self.store).store_table(name.clone(), access, i, v)
                };
                if matches!(access, AccessType::Struct | AccessType::Static) {
                    let addr = table_addr(self.store, &name, i);
                    if matches!(addr, Addr::Elem(..)) && self.forwardable(v) {
                        record = Some((addr, v));
                    }
                }
                new_id
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.value(cond, available);
                let t = self.arm(then_block, available);
                let e = else_block.map(|e| self.arm(e, available));
                if (c, t, e) == (cond, then_block, else_block) {
                    id
                } else {
                    FirBuilder::new(self.store).if_(c, t, e)
                }
            }
            FirMatch::Control { cond, stmt } => {
                let c = self.value(cond, available);
                let s = self.arm(stmt, available);
                if (c, s) == (cond, stmt) {
                    id
                } else {
                    FirBuilder::new(self.store).control(c, s)
                }
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                let c = self.value(cond, available);
                let new_cases: Vec<(i64, FirId)> = cases
                    .iter()
                    .map(|&(k, b)| (k, self.arm(b, available)))
                    .collect();
                let d = default.map(|d| self.arm(d, available));
                if c == cond && new_cases == cases && d == default {
                    id
                } else {
                    FirBuilder::new(self.store).switch(c, &new_cases, d)
                }
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => {
                let u = self.value(upper, available);
                let b = self.block(body, &mut Available::default());
                if (u, b) == (upper, body) {
                    id
                } else {
                    FirBuilder::new(self.store).simple_for_loop(var, u, b, is_reverse)
                }
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => {
                let i = self.value(init, available);
                let b = self.block(body, &mut Available::default());
                if (i, b) == (init, body) {
                    id
                } else {
                    FirBuilder::new(self.store).for_loop(var, i, end, step, b, is_reverse)
                }
            }
            FirMatch::WhileLoop { cond, body } => {
                let b = self.block(body, &mut Available::default());
                if b == body {
                    id
                } else {
                    FirBuilder::new(self.store).while_loop(cond, b)
                }
            }
            FirMatch::Drop(value) => {
                let v = self.value(value, available);
                if v == value {
                    id
                } else {
                    FirBuilder::new(self.store).drop_(v)
                }
            }
            FirMatch::DeclareVar { init: None, .. }
            | FirMatch::Label(_)
            | FirMatch::NullStatement => {
                return id;
            }
            _ => {
                available.clear();
                return id;
            }
        };
        let writes = effects_of(self.store, new_id).writes;
        Self::invalidate(available, &writes);
        if let Some((addr, value)) = record {
            available.insert(&addr, value);
        }
        new_id
    }
}

#[cfg(test)]
mod available_tests {
    use super::{Addr, Available};
    use crate::{FirBuilder, FirStore};

    /// `Available::invalidate(w)` drops exactly the addresses `w` conflicts with.
    #[test]
    fn invalidate_agrees_with_conflicts() {
        let mut store = FirStore::new();
        let value = FirBuilder::new(&mut store).float64(1.0);
        let addrs = [
            Addr::Field("a".into()),
            Addr::Field("b".into()),
            Addr::Elem("a".into(), 0),
            Addr::Elem("a".into(), 1),
            Addr::Elem("b".into(), 0),
        ];
        let writes = [
            Addr::Field("a".into()),
            Addr::Elem("a".into(), 0),
            Addr::Elem("b".into(), 1),
            Addr::Local("a".into()),
            Addr::Stack,
            Addr::All,
        ];
        for w in &writes {
            let mut available = Available::default();
            for addr in &addrs {
                available.insert(addr, value);
            }
            available.invalidate(w);
            for addr in &addrs {
                assert_eq!(
                    available.get(addr).is_none(),
                    w.conflicts(addr),
                    "write {w:?}, address {addr:?}"
                );
            }
        }
    }
}
