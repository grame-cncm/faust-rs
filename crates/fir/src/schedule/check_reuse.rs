//! Independent checker of W7 (design note
//! `porting/fir-local-reuse-w7-design-2026-10-17-en.md` §3).
//!
//! Walks the body before and after W7 in parallel and keeps, for each slot, the
//! variable whose value it holds on every path. A read of `x` through slot `s`
//! is accepted only where `s` holds `x`: every write of `x` is a write of `s`,
//! so the last write of `s` is then the last write of `x`. Nothing here uses
//! the producer's numbering, intervals or loop rule.

use std::collections::{HashMap, HashSet};

use crate::{AccessType, FirId, FirMatch, FirStore, FirType, match_fir};

/// For each slot, the variable it holds on every path; a slot missing from
/// the map holds no known variable.
type Holds = HashMap<String, String>;

fn join(a: &Holds, b: &Holds) -> Holds {
    a.iter()
        .filter(|(slot, var)| b.get(*slot) == Some(*var))
        .map(|(s, v)| (s.clone(), v.clone()))
        .collect()
}

/// Every variable or table name used under `id`.
fn names_in(store: &FirStore, id: FirId, out: &mut HashSet<String>) {
    let node = match_fir(store, id);
    match &node {
        FirMatch::LoadVar { name, .. }
        | FirMatch::LoadVarAddress { name, .. }
        | FirMatch::TeeVar { name, .. }
        | FirMatch::DeclareVar { name, .. }
        | FirMatch::StoreVar { name, .. }
        | FirMatch::LoadTable { name, .. }
        | FirMatch::StoreTable { name, .. }
        | FirMatch::DeclareTable { name, .. }
        | FirMatch::ShiftArrayVar { name, .. } => {
            out.insert(name.clone());
        }
        FirMatch::SimpleForLoop { var, .. } | FirMatch::ForLoop { var, .. } => {
            out.insert(var.clone());
        }
        FirMatch::IteratorForLoop { iterators, .. } => out.extend(iterators.iter().cloned()),
        _ => {}
    }
    for child in crate::fir_match_children(store, id) {
        names_in(store, child, out);
    }
}

/// The zero constant of `typ` a declaration without initializer becomes.
fn is_zero_of(store: &FirStore, id: FirId, typ: &FirType) -> bool {
    match (match_fir(store, id), typ) {
        (FirMatch::Float64 { value, .. }, FirType::Float64) => value.to_bits() == 0,
        (FirMatch::Float32 { value, .. }, FirType::Float32) => value.to_bits() == 0,
        (FirMatch::Int32 { value, .. }, FirType::Int32) => value == 0,
        (FirMatch::Int64 { value, .. }, FirType::Int64) => value == 0,
        _ => false,
    }
}

struct ReuseCheck<'a> {
    store: &'a FirStore,
    /// Slot name → type.
    slots: HashMap<String, FirType>,
    /// Variable → slot.
    renamed: HashMap<String, String>,
    /// Names seen under their own name.
    kept: HashSet<String>,
    /// Subtrees already found unchanged.
    seen: HashSet<FirId>,
}

impl ReuseCheck<'_> {
    fn bind(&mut self, var: &str, slot: &str, typ: &FirType) -> Result<(), String> {
        let slot_typ = self
            .slots
            .get(slot)
            .ok_or_else(|| format!("`{var}` renamed to `{slot}`, which is not a slot"))?;
        if slot_typ != typ {
            return Err(format!(
                "`{var}` ({typ:?}) given slot `{slot}` ({slot_typ:?})"
            ));
        }
        if self.kept.contains(var) {
            return Err(format!("`{var}` is renamed and also kept"));
        }
        match self.renamed.get(var) {
            Some(s) if s != slot => Err(format!("`{var}` renamed to both `{s}` and `{slot}`")),
            Some(_) => Ok(()),
            None => {
                self.renamed.insert(var.to_owned(), slot.to_owned());
                Ok(())
            }
        }
    }

    fn keep(&mut self, var: &str) -> Result<(), String> {
        if self.slots.contains_key(var) {
            return Err(format!("slot `{var}` used by the original program"));
        }
        if self.renamed.contains_key(var) {
            return Err(format!("`{var}` is renamed and also kept"));
        }
        self.kept.insert(var.to_owned());
        Ok(())
    }

    /// An unchanged subtree: its stack variables keep their names.
    fn unchanged(&mut self, id: FirId) -> Result<(), String> {
        if !self.seen.insert(id) {
            return Ok(());
        }
        let mut names = HashSet::new();
        names_in(self.store, id, &mut names);
        for n in names {
            self.keep(&n)?;
        }
        Ok(())
    }

    /// `after` computes what `before` computes, its renamed reads seeing their
    /// own variable (when `check`).
    fn value(&mut self, b: FirId, a: FirId, holds: &Holds, check: bool) -> Result<(), String> {
        if b == a {
            return self.unchanged(b);
        }
        let s = self.store;
        match (match_fir(s, b), match_fir(s, a)) {
            (
                FirMatch::LoadVar {
                    name: x,
                    access: AccessType::Stack,
                    typ,
                },
                FirMatch::LoadVar {
                    name: slot,
                    access: AccessType::Stack,
                    typ: typ2,
                },
            ) if typ == typ2 => {
                self.bind(&x, &slot, &typ)?;
                if check && holds.get(&slot) != Some(&x) {
                    return Err(format!(
                        "read of `{x}` through `{slot}`, which holds {:?} there",
                        holds.get(&slot)
                    ));
                }
                Ok(())
            }
            (
                FirMatch::LoadTable {
                    name,
                    access,
                    index,
                    typ,
                },
                FirMatch::LoadTable {
                    name: n2,
                    access: a2,
                    index: i2,
                    typ: t2,
                },
            ) if (name.as_str(), access, &typ) == (n2.as_str(), a2, &t2) => {
                self.value(index, i2, holds, check)
            }
            (
                FirMatch::BinOp { op, lhs, rhs, typ },
                FirMatch::BinOp {
                    op: o2,
                    lhs: l2,
                    rhs: r2,
                    typ: t2,
                },
            ) if op == o2 && typ == t2 => {
                self.value(lhs, l2, holds, check)?;
                self.value(rhs, r2, holds, check)
            }
            (FirMatch::Neg { value, typ }, FirMatch::Neg { value: v2, typ: t2 })
            | (FirMatch::Cast { typ, value }, FirMatch::Cast { typ: t2, value: v2 })
            | (FirMatch::Bitcast { typ, value }, FirMatch::Bitcast { typ: t2, value: v2 })
                if typ == t2 =>
            {
                self.value(value, v2, holds, check)
            }
            (
                FirMatch::Select2 {
                    cond,
                    then_value,
                    else_value,
                    typ,
                },
                FirMatch::Select2 {
                    cond: c2,
                    then_value: t2,
                    else_value: e2,
                    typ: ty2,
                },
            ) if typ == ty2 => {
                self.value(cond, c2, holds, check)?;
                self.value(then_value, t2, holds, check)?;
                self.value(else_value, e2, holds, check)
            }
            (
                FirMatch::FunCall { name, args, typ },
                FirMatch::FunCall {
                    name: n2,
                    args: a2,
                    typ: t2,
                },
            ) if name == n2 && typ == t2 && args.len() == a2.len() => {
                for (x, y) in args.iter().zip(&a2) {
                    self.value(*x, *y, holds, check)?;
                }
                Ok(())
            }
            (x, y) => Err(format!("values differ: {x:?} vs {y:?}")),
        }
    }

    fn block(&mut self, b: FirId, a: FirId, holds: &mut Holds, check: bool) -> Result<(), String> {
        self.stmt(b, a, holds, check)
    }

    /// A loop: its header and body from the fixed point of the slots' contents
    /// at its head, then checked once from there; the loop leaves from its head.
    fn looped(
        &mut self,
        headers: &[(FirId, FirId)],
        body: (FirId, FirId),
        holds: &mut Holds,
        check: bool,
    ) -> Result<(), String> {
        let entry = holds.clone();
        let mut head = entry.clone();
        loop {
            let mut state = head.clone();
            for &(x, y) in headers {
                self.value(x, y, &state, false)?;
            }
            self.block(body.0, body.1, &mut state, false)?;
            let next = join(&entry, &state);
            if next == head {
                break;
            }
            head = next;
        }
        let mut state = head.clone();
        for &(x, y) in headers {
            self.value(x, y, &state, check)?;
        }
        self.block(body.0, body.1, &mut state, check)?;
        *holds = head;
        Ok(())
    }

    fn stmt(&mut self, b: FirId, a: FirId, holds: &mut Holds, check: bool) -> Result<(), String> {
        // An untouched statement mentions no slot (slots are fresh names), so
        // it leaves the slots' contents as they are.
        if b == a {
            return self.unchanged(b);
        }
        let s = self.store;
        match (match_fir(s, b), match_fir(s, a)) {
            (FirMatch::Block(xs), FirMatch::Block(ys)) => {
                if xs.len() != ys.len() {
                    return Err(format!("block of {} items became {}", xs.len(), ys.len()));
                }
                for (x, y) in xs.iter().zip(&ys) {
                    self.stmt(*x, *y, holds, check)?;
                }
                Ok(())
            }
            (
                FirMatch::DeclareVar {
                    name,
                    typ,
                    access: AccessType::Stack,
                    init,
                },
                FirMatch::StoreVar {
                    name: slot,
                    access: AccessType::Stack,
                    value,
                },
            ) => {
                match init {
                    Some(init) => self.value(init, value, holds, check)?,
                    None if is_zero_of(s, value, &typ) => {}
                    None => {
                        return Err(format!("`{name}` declared without value, stored non-zero"));
                    }
                }
                self.bind(&name, &slot, &typ)?;
                holds.insert(slot, name);
                Ok(())
            }
            (
                FirMatch::DeclareVar {
                    name,
                    typ,
                    access,
                    init,
                },
                FirMatch::DeclareVar {
                    name: n2,
                    typ: t2,
                    access: a2,
                    init: i2,
                },
            ) if (&name, &typ, access) == (&n2, &t2, a2) => {
                match (init, i2) {
                    (Some(x), Some(y)) => self.value(x, y, holds, check)?,
                    (None, None) => {}
                    _ => return Err(format!("declaration of `{name}` changed its initializer")),
                }
                self.keep(&name)
            }
            (
                FirMatch::StoreVar {
                    name,
                    access,
                    value,
                },
                FirMatch::StoreVar {
                    name: n2,
                    access: a2,
                    value: v2,
                },
            ) if access == a2 => {
                self.value(value, v2, holds, check)?;
                if name == n2 {
                    if access == AccessType::Stack {
                        self.keep(&name)?;
                    }
                } else if access == AccessType::Stack {
                    let typ = self
                        .slots
                        .get(&n2)
                        .cloned()
                        .ok_or_else(|| format!("store to `{name}` became a store to `{n2}`"))?;
                    self.bind(&name, &n2, &typ)?;
                    holds.insert(n2, name);
                } else {
                    return Err(format!("store to `{name}` became a store to `{n2}`"));
                }
                Ok(())
            }
            (
                FirMatch::StoreTable {
                    name,
                    access,
                    index,
                    value,
                },
                FirMatch::StoreTable {
                    name: n2,
                    access: a2,
                    index: i2,
                    value: v2,
                },
            ) if (&name, access) == (&n2, a2) => {
                self.value(index, i2, holds, check)?;
                self.value(value, v2, holds, check)
            }
            (
                FirMatch::If {
                    cond,
                    then_block,
                    else_block,
                },
                FirMatch::If {
                    cond: c2,
                    then_block: t2,
                    else_block: e2,
                },
            ) => {
                self.value(cond, c2, holds, check)?;
                let mut then_holds = holds.clone();
                self.block(then_block, t2, &mut then_holds, check)?;
                let mut else_holds = holds.clone();
                match (else_block, e2) {
                    (Some(x), Some(y)) => self.block(x, y, &mut else_holds, check)?,
                    (None, None) => {}
                    _ => return Err("an else arm appeared or vanished".into()),
                }
                *holds = join(&then_holds, &else_holds);
                Ok(())
            }
            (FirMatch::Control { cond, stmt }, FirMatch::Control { cond: c2, stmt: s2 }) => {
                self.value(cond, c2, holds, check)?;
                let mut arm = holds.clone();
                self.block(stmt, s2, &mut arm, check)?;
                *holds = join(holds, &arm);
                Ok(())
            }
            (
                FirMatch::Switch {
                    cond,
                    cases,
                    default,
                },
                FirMatch::Switch {
                    cond: c2,
                    cases: k2,
                    default: d2,
                },
            ) => {
                self.value(cond, c2, holds, check)?;
                if cases.len() != k2.len() || cases.iter().zip(&k2).any(|(x, y)| x.0 != y.0) {
                    return Err("switch cases changed".into());
                }
                let mut joined: Option<Holds> = None;
                let mut arms: Vec<(FirId, FirId)> =
                    cases.iter().zip(&k2).map(|(x, y)| (x.1, y.1)).collect();
                match (default, d2) {
                    (Some(x), Some(y)) => arms.push((x, y)),
                    (None, None) => joined = Some(holds.clone()),
                    _ => return Err("switch default changed".into()),
                }
                for (x, y) in arms {
                    let mut arm = holds.clone();
                    self.block(x, y, &mut arm, check)?;
                    joined = Some(match joined {
                        Some(j) => join(&j, &arm),
                        None => arm,
                    });
                }
                *holds = joined.unwrap_or_default();
                Ok(())
            }
            (
                FirMatch::SimpleForLoop {
                    var,
                    upper,
                    body,
                    is_reverse,
                },
                FirMatch::SimpleForLoop {
                    var: v2,
                    upper: u2,
                    body: b2,
                    is_reverse: r2,
                },
            ) if (&var, is_reverse) == (&v2, r2) => {
                self.keep(&var)?;
                self.looped(&[(upper, u2)], (body, b2), holds, check)
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
                FirMatch::ForLoop {
                    var: v2,
                    init: i2,
                    end: e2,
                    step: s2,
                    body: b2,
                    is_reverse: r2,
                },
            ) if (&var, is_reverse) == (&v2, r2) => {
                self.keep(&var)?;
                self.value(init, i2, holds, check)?;
                self.looped(&[(end, e2), (step, s2)], (body, b2), holds, check)
            }
            (FirMatch::WhileLoop { cond, body }, FirMatch::WhileLoop { cond: c2, body: b2 }) => {
                self.looped(&[(cond, c2)], (body, b2), holds, check)
            }
            (FirMatch::Drop(x), FirMatch::Drop(y)) => self.value(x, y, holds, check),
            (x, y) => Err(format!("statements differ: {x:?} vs {y:?}")),
        }
    }
}

/// Accepts `after` as W7 of `before` (both `compute` bodies in `store`).
pub(crate) fn check_reuse(store: &FirStore, before: FirId, after: FirId) -> Result<(), String> {
    let (FirMatch::Block(xs), FirMatch::Block(ys)) =
        (match_fir(store, before), match_fir(store, after))
    else {
        return Err("compute bodies are not blocks".into());
    };
    if ys.len() < xs.len() {
        return Err("statements were dropped".into());
    }
    let (decls, rest) = ys.split_at(ys.len() - xs.len());
    let mut used = HashSet::new();
    names_in(store, before, &mut used);
    let mut slots = HashMap::new();
    for &d in decls {
        let FirMatch::DeclareVar {
            name,
            typ,
            access: AccessType::Stack,
            init: None,
        } = match_fir(store, d)
        else {
            return Err("an added statement is not a slot declaration".into());
        };
        if !matches!(
            typ,
            FirType::Float64 | FirType::Float32 | FirType::Int32 | FirType::Int64
        ) {
            return Err(format!("slot `{name}` of type {typ:?}"));
        }
        if used.contains(&name) {
            return Err(format!(
                "slot `{name}` already names something in the program"
            ));
        }
        if slots.insert(name.clone(), typ).is_some() {
            return Err(format!("slot `{name}` declared twice"));
        }
    }
    let mut checker = ReuseCheck {
        store,
        slots,
        renamed: HashMap::new(),
        kept: HashSet::new(),
        seen: HashSet::new(),
    };
    let mut holds = Holds::new();
    for (x, y) in xs.iter().zip(rest) {
        checker.stmt(*x, *y, &mut holds, true)?;
    }
    Ok(())
}
