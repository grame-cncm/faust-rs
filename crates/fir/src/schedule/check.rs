//! Independent checker of the W2/W3 producers (design note §5).
//!
//! Written separately from `effects`, `forward` and `sink`: its own location
//! model, purity rule, conflict test and tree walks. It accepts a producer's
//! output only when it can re-derive, from the trees themselves, that every
//! forwarded load reads the value last stored at its address and that every
//! reordering only moves a pure declaration later, past statements that do not
//! touch what it reads or defines.

use std::collections::HashMap;

use super::sink::BlockOrder;
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirMathOp, FirStore, FirType, match_fir};

/// A location, in the checker's own terms.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Loc {
    /// A struct/static scalar or a whole struct/static array.
    Slot(String),
    /// One constant element of a struct/static array.
    Cell(String, i32),
    /// A scalar local (stack, loop, argument).
    Var(String),
    /// The stack tables and audio buffers, as one region.
    Heap,
    /// Anything.
    Any,
}

/// Whether writing `w` may change a read of `r`. `Known::forget` implements it
/// by name; the test below checks that both agree.
#[cfg(test)]
fn hits(w: &Loc, r: &Loc) -> bool {
    match (w, r) {
        (Loc::Any, _) | (_, Loc::Any) => true,
        (Loc::Heap, Loc::Heap) => true,
        (Loc::Var(a), Loc::Var(b)) => a == b,
        (Loc::Slot(a), Loc::Slot(b)) => a == b,
        (Loc::Slot(a), Loc::Cell(b, _)) | (Loc::Cell(a, _), Loc::Slot(b)) => a == b,
        (Loc::Cell(a, i), Loc::Cell(b, j)) => a == b && i == j,
        _ => false,
    }
}

#[derive(Default)]
struct Touch {
    reads: Vec<Loc>,
    writes: Vec<Loc>,
    impure: bool,
}

/// Mathematical functions the checker accepts as pure.
fn pure_call(name: &str) -> bool {
    if FirMathOp::from_symbol(name).is_some() {
        return true;
    }
    const PURE: [&str; 21] = [
        "abs",
        "max_i",
        "min_i",
        "acosh",
        "acoshf",
        "asinh",
        "asinhf",
        "atanh",
        "atanhf",
        "cosh",
        "coshf",
        "sinh",
        "sinhf",
        "tanh",
        "tanhf",
        "copysign",
        "copysignf",
        "isinf",
        "isinff",
        "isnan",
        "isnanf",
    ];
    PURE.contains(&name)
}

fn cell_or_slot(store: &FirStore, name: &str, index: FirId) -> Loc {
    if let FirMatch::Int32 { value, .. } = match_fir(store, index) {
        Loc::Cell(name.to_owned(), value)
    } else {
        Loc::Slot(name.to_owned())
    }
}

/// Reads of a value; marks `impure` (and a write of `Any`) when it has effects.
fn touch_value(store: &FirStore, id: FirId, t: &mut Touch) {
    let node = match_fir(store, id);
    match &node {
        FirMatch::Bool { .. }
        | FirMatch::Int32 { .. }
        | FirMatch::Int64 { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. }
        | FirMatch::NullValue { .. } => {}
        FirMatch::LoadVar { name, access, .. } => match access {
            AccessType::Struct | AccessType::Static => t.reads.push(Loc::Slot(name.clone())),
            AccessType::Stack | AccessType::Loop | AccessType::FunArgs => {
                t.reads.push(Loc::Var(name.clone()));
            }
            AccessType::Global => {
                t.impure = true;
                t.writes.push(Loc::Any);
            }
        },
        FirMatch::LoadTable {
            name,
            access,
            index,
            ..
        } => {
            touch_value(store, *index, t);
            match access {
                AccessType::Struct | AccessType::Static => {
                    t.reads.push(cell_or_slot(store, name, *index));
                }
                AccessType::Stack => {
                    t.reads.push(Loc::Var(name.clone()));
                    t.reads.push(Loc::Heap);
                }
                AccessType::FunArgs => {}
                AccessType::Loop | AccessType::Global => {
                    t.impure = true;
                    t.writes.push(Loc::Any);
                }
            }
        }
        FirMatch::BinOp { .. }
        | FirMatch::Neg { .. }
        | FirMatch::Cast { .. }
        | FirMatch::Bitcast { .. }
        | FirMatch::Select2 { .. } => {
            for child in crate::child_ids(&node) {
                touch_value(store, child, t);
            }
        }
        FirMatch::FunCall { name, args, .. } => {
            if !pure_call(name) {
                t.impure = true;
                t.writes.push(Loc::Any);
            }
            for &arg in args {
                touch_value(store, arg, t);
            }
        }
        _ => {
            t.impure = true;
            t.writes.push(Loc::Any);
        }
    }
}

/// Reads and writes of a statement and its subtree.
fn touch_stmt(store: &FirStore, id: FirId, t: &mut Touch) {
    let node = match_fir(store, id);
    match &node {
        FirMatch::DeclareVar {
            name,
            access: AccessType::Stack | AccessType::Loop,
            init,
            ..
        } => {
            if let Some(init) = init {
                touch_value(store, *init, t);
            }
            t.writes.push(Loc::Var(name.clone()));
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            touch_value(store, *value, t);
            t.writes.push(match access {
                AccessType::Struct | AccessType::Static => Loc::Slot(name.clone()),
                AccessType::Stack | AccessType::Loop => Loc::Var(name.clone()),
                _ => Loc::Any,
            });
        }
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => {
            touch_value(store, *index, t);
            touch_value(store, *value, t);
            match access {
                AccessType::Struct | AccessType::Static => {
                    t.writes.push(cell_or_slot(store, name, *index));
                }
                AccessType::Stack => {
                    t.reads.push(Loc::Var(name.clone()));
                    t.writes.push(Loc::Heap);
                }
                _ => t.writes.push(Loc::Any),
            }
        }
        FirMatch::Block(items) => {
            for &item in items {
                touch_stmt(store, item, t);
            }
        }
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            touch_value(store, *cond, t);
            touch_stmt(store, *then_block, t);
            if let Some(e) = else_block {
                touch_stmt(store, *e, t);
            }
        }
        FirMatch::Control { cond, stmt } => {
            touch_value(store, *cond, t);
            touch_stmt(store, *stmt, t);
        }
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            touch_value(store, *cond, t);
            for (_, b) in cases {
                touch_stmt(store, *b, t);
            }
            if let Some(d) = default {
                touch_stmt(store, *d, t);
            }
        }
        FirMatch::SimpleForLoop {
            var, upper, body, ..
        } => {
            touch_value(store, *upper, t);
            t.reads.push(Loc::Var(var.clone()));
            t.writes.push(Loc::Var(var.clone()));
            touch_stmt(store, *body, t);
        }
        FirMatch::ForLoop {
            var,
            init,
            end,
            step,
            body,
            ..
        } => {
            for v in [*init, *end, *step] {
                touch_value(store, v, t);
            }
            t.reads.push(Loc::Var(var.clone()));
            t.writes.push(Loc::Var(var.clone()));
            touch_stmt(store, *body, t);
        }
        FirMatch::WhileLoop { cond, body } => {
            touch_value(store, *cond, t);
            touch_stmt(store, *body, t);
        }
        FirMatch::Drop(value) => touch_value(store, *value, t),
        FirMatch::Label(_) | FirMatch::NullStatement => {}
        _ => {
            t.impure = true;
            t.writes.push(Loc::Any);
        }
    }
}

fn touch(store: &FirStore, id: FirId) -> Touch {
    let mut t = Touch::default();
    touch_stmt(store, id, &mut t);
    t
}

/// How many times each local is assigned in `id` (declarations, stores, tees,
/// loop variables).
fn count_assignments(store: &FirStore, id: FirId, out: &mut HashMap<String, usize>) {
    let node = match_fir(store, id);
    let assigned = match &node {
        FirMatch::DeclareVar { name, access, .. }
        | FirMatch::StoreVar { name, access, .. }
        | FirMatch::TeeVar { name, access, .. }
            if matches!(access, AccessType::Stack | AccessType::Loop) =>
        {
            Some(name.clone())
        }
        FirMatch::SimpleForLoop { var, .. } | FirMatch::ForLoop { var, .. } => Some(var.clone()),
        _ => None,
    };
    if let Some(name) = assigned {
        *out.entry(name).or_default() += 1;
    }
    for child in crate::child_ids(&node) {
        count_assignments(store, child, out);
    }
}

// ─── W3 ──────────────────────────────────────────────────────────────────────

struct ForwardCheck<'a> {
    store: &'a FirStore,
    assignments: HashMap<String, usize>,
}

/// Values the checker knows at struct locations: scalars and whole arrays by
/// name, constant cells by name then index.
#[derive(Clone, Default)]
struct Known {
    slots: HashMap<String, FirId>,
    cells: HashMap<String, HashMap<i32, FirId>>,
}

impl Known {
    fn get(&self, loc: &Loc) -> Option<FirId> {
        match loc {
            Loc::Slot(x) => self.slots.get(x).copied(),
            Loc::Cell(x, k) => self.cells.get(x)?.get(k).copied(),
            _ => None,
        }
    }

    fn insert(&mut self, loc: Loc, value: FirId) {
        match loc {
            Loc::Slot(x) => {
                self.slots.insert(x, value);
            }
            Loc::Cell(x, k) => {
                self.cells.entry(x).or_default().insert(k, value);
            }
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.slots.clear();
        self.cells.clear();
    }

    /// Forgets what a write of `w` may change (`hits`).
    fn forget(&mut self, w: &Loc) {
        match w {
            Loc::Any => self.clear(),
            Loc::Slot(x) => {
                self.slots.remove(x);
                self.cells.remove(x);
            }
            Loc::Cell(x, k) => {
                self.slots.remove(x);
                if let Some(cells) = self.cells.get_mut(x) {
                    cells.remove(k);
                }
            }
            Loc::Var(_) | Loc::Heap => {}
        }
    }
}

impl ForwardCheck<'_> {
    /// A value that may stand for its address: a constant, or a local assigned
    /// exactly once in the function.
    fn stands_in(&self, value: FirId) -> bool {
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
            } => self.assignments.get(&name) == Some(&1),
            _ => false,
        }
    }

    /// Whether `b` computes what `a` computes, given the values `known` at the
    /// addresses loaded.
    fn same_value(&self, a: FirId, b: FirId, known: &Known) -> Result<(), String> {
        if a == b {
            return Ok(());
        }
        let (na, nb) = (match_fir(self.store, a), match_fir(self.store, b));
        // A forwarded load: `a` reads an address whose known value is `b`.
        let loaded = match &na {
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct | AccessType::Static,
                ..
            } => Some(Loc::Slot(name.clone())),
            FirMatch::LoadTable {
                name,
                access: AccessType::Struct | AccessType::Static,
                index,
                ..
            } => match cell_or_slot(self.store, name, *index) {
                cell @ Loc::Cell(..) => Some(cell),
                _ => None,
            },
            _ => None,
        };
        if let Some(loc) = loaded
            && known.get(&loc) == Some(b)
        {
            return if self.stands_in(b) && self.store.value_type(a) == self.store.value_type(b) {
                Ok(())
            } else {
                Err(format!("forwarded value {b:?} cannot stand for {loc:?}"))
            };
        }
        // Otherwise the same node with equivalent children.
        let same_shape = match (&na, &nb) {
            (
                FirMatch::LoadTable {
                    name: x,
                    access: p,
                    typ: s,
                    ..
                },
                FirMatch::LoadTable {
                    name: y,
                    access: q,
                    typ: t,
                    ..
                },
            ) => x == y && p == q && s == t,
            (FirMatch::BinOp { op: x, typ: s, .. }, FirMatch::BinOp { op: y, typ: t, .. }) => {
                x == y && s == t
            }
            (FirMatch::Neg { typ: s, .. }, FirMatch::Neg { typ: t, .. })
            | (FirMatch::Cast { typ: s, .. }, FirMatch::Cast { typ: t, .. })
            | (FirMatch::Bitcast { typ: s, .. }, FirMatch::Bitcast { typ: t, .. })
            | (FirMatch::Select2 { typ: s, .. }, FirMatch::Select2 { typ: t, .. }) => s == t,
            (
                FirMatch::FunCall {
                    name: x,
                    typ: s,
                    args: u,
                },
                FirMatch::FunCall {
                    name: y,
                    typ: t,
                    args: v,
                },
            ) => x == y && s == t && u.len() == v.len(),
            _ => false,
        };
        if !same_shape {
            return Err(format!("value {b:?} differs from {a:?}"));
        }
        let (ca, cb) = (crate::child_ids(&na), crate::child_ids(&nb));
        if ca.len() != cb.len() {
            return Err(format!("value {b:?} differs from {a:?}"));
        }
        for (x, y) in ca.into_iter().zip(cb) {
            self.same_value(x, y, known)?;
        }
        Ok(())
    }

    fn forget(known: &mut Known, writes: &[Loc]) {
        for w in writes {
            known.forget(w);
        }
    }

    /// Checks the statements of `a` (original) against `b` (forwarded).
    fn block(&self, a: FirId, b: FirId, known: &mut Known) -> Result<(), String> {
        let (FirMatch::Block(xs), FirMatch::Block(ys)) =
            (match_fir(self.store, a), match_fir(self.store, b))
        else {
            return self.stmt(a, b, known);
        };
        if xs.len() != ys.len() {
            return Err("a forwarded block changed its length".to_owned());
        }
        for (x, y) in xs.into_iter().zip(ys) {
            self.stmt(x, y, known)?;
        }
        Ok(())
    }

    fn arm(&self, a: FirId, b: FirId, known: &Known) -> Result<(), String> {
        self.block(a, b, &mut known.clone())
    }

    fn opt_arm(&self, a: Option<FirId>, b: Option<FirId>, known: &Known) -> Result<(), String> {
        match (a, b) {
            (None, None) => Ok(()),
            (Some(a), Some(b)) => self.arm(a, b, known),
            _ => Err("an else arm appeared or disappeared".to_owned()),
        }
    }

    fn stmt(&self, a: FirId, b: FirId, known: &mut Known) -> Result<(), String> {
        let mut stored: Option<(Loc, FirId)> = None;
        match (match_fir(self.store, a), match_fir(self.store, b)) {
            (FirMatch::Block(_), FirMatch::Block(_)) => self.arm(a, b, known)?,
            (
                FirMatch::DeclareVar {
                    name: n,
                    typ: s,
                    access: p,
                    init: x,
                },
                FirMatch::DeclareVar {
                    name: m,
                    typ: t,
                    access: q,
                    init: y,
                },
            ) if n == m && s == t && p == q => match (x, y) {
                (None, None) => {}
                (Some(x), Some(y)) => self.same_value(x, y, known)?,
                _ => return Err(format!("declaration of `{n}` changed")),
            },
            (
                FirMatch::StoreVar {
                    name: n,
                    access: p,
                    value: x,
                },
                FirMatch::StoreVar {
                    name: m,
                    access: q,
                    value: y,
                },
            ) if n == m && p == q => {
                self.same_value(x, y, known)?;
                if matches!(p, AccessType::Struct | AccessType::Static) && self.stands_in(y) {
                    stored = Some((Loc::Slot(n), y));
                }
            }
            (
                FirMatch::StoreTable {
                    name: n,
                    access: p,
                    index: i,
                    value: x,
                },
                FirMatch::StoreTable {
                    name: m,
                    access: q,
                    index: j,
                    value: y,
                },
            ) if n == m && p == q => {
                self.same_value(i, j, known)?;
                self.same_value(x, y, known)?;
                if matches!(p, AccessType::Struct | AccessType::Static) && self.stands_in(y) {
                    let loc = cell_or_slot(self.store, &n, j);
                    if matches!(loc, Loc::Cell(..)) {
                        stored = Some((loc, y));
                    }
                }
            }
            (
                FirMatch::If {
                    cond: c,
                    then_block: t,
                    else_block: e,
                },
                FirMatch::If {
                    cond: d,
                    then_block: u,
                    else_block: f,
                },
            ) => {
                self.same_value(c, d, known)?;
                self.arm(t, u, known)?;
                self.opt_arm(e, f, known)?;
            }
            (FirMatch::Control { cond: c, stmt: s }, FirMatch::Control { cond: d, stmt: t }) => {
                self.same_value(c, d, known)?;
                self.arm(s, t, known)?;
            }
            (
                FirMatch::Switch {
                    cond: c,
                    cases: xs,
                    default: e,
                },
                FirMatch::Switch {
                    cond: d,
                    cases: ys,
                    default: f,
                },
            ) => {
                self.same_value(c, d, known)?;
                if xs.len() != ys.len() {
                    return Err("switch cases changed".to_owned());
                }
                for ((k, x), (l, y)) in xs.into_iter().zip(ys) {
                    if k != l {
                        return Err("switch case labels changed".to_owned());
                    }
                    self.arm(x, y, known)?;
                }
                self.opt_arm(e, f, known)?;
            }
            (
                FirMatch::SimpleForLoop {
                    var: v,
                    upper: x,
                    body: s,
                    is_reverse: r,
                },
                FirMatch::SimpleForLoop {
                    var: w,
                    upper: y,
                    body: t,
                    is_reverse: q,
                },
            ) if v == w && r == q => {
                self.same_value(x, y, known)?;
                self.block(s, t, &mut Known::default())?;
            }
            (
                FirMatch::ForLoop {
                    var: v,
                    init: x,
                    end: e,
                    step: p,
                    body: s,
                    is_reverse: r,
                },
                FirMatch::ForLoop {
                    var: w,
                    init: y,
                    end: f,
                    step: q,
                    body: t,
                    is_reverse: u,
                },
            ) if v == w && r == u && e == f && p == q => {
                self.same_value(x, y, known)?;
                self.block(s, t, &mut Known::default())?;
            }
            (
                FirMatch::WhileLoop { cond: c, body: s },
                FirMatch::WhileLoop { cond: d, body: t },
            ) if c == d => {
                self.block(s, t, &mut Known::default())?;
            }
            (FirMatch::Drop(x), FirMatch::Drop(y)) => self.same_value(x, y, known)?,
            _ if a == b => {}
            _ => return Err(format!("statement {b:?} differs from {a:?}")),
        }
        let t = touch(self.store, b);
        if t.impure {
            known.clear();
        } else {
            Self::forget(known, &t.writes);
        }
        if let Some((loc, value)) = stored {
            known.insert(loc, value);
        }
        Ok(())
    }
}

/// Checks a W3 rewrite of a function body: `forwarded` must compute what
/// `original` computes, statement by statement, with each differing load read
/// from the value last stored at its address.
pub(crate) fn check_forward(
    store: &FirStore,
    original: FirId,
    forwarded: FirId,
) -> Result<(), String> {
    let mut assignments = HashMap::new();
    count_assignments(store, forwarded, &mut assignments);
    let checker = ForwardCheck { store, assignments };
    checker.block(original, forwarded, &mut Known::default())
}

// ─── W2 ──────────────────────────────────────────────────────────────────────

/// The local a statement may be moved for: a non-array stack declaration with a
/// pure initializer.
fn sinkable(store: &FirStore, id: FirId) -> Option<String> {
    if let FirMatch::DeclareVar {
        name,
        access: AccessType::Stack,
        typ,
        init: Some(init),
    } = match_fir(store, id)
        && !matches!(typ, FirType::Array(..))
    {
        let mut t = Touch::default();
        touch_value(store, init, &mut t);
        return (!t.impure).then_some(name);
    }
    None
}

/// First entry of the sorted `list` greater than `i`.
fn next_after(list: Option<&Vec<usize>>, i: usize) -> Option<usize> {
    let list = list?;
    list.get(list.partition_point(|&k| k <= i)).copied()
}

/// Checks one block's order (design note §5, W2 points 1 to 4).
///
/// Items that write memory, the heap or `Any` are never movable, so they keep
/// their relative order (checked first); the first such writer after an item is
/// therefore the earliest one emitted, and is the only one to compare with.
pub(crate) fn check_order(
    store: &FirStore,
    items: &[FirId],
    order: &[usize],
) -> Result<(), String> {
    let n = items.len();
    if order.len() != n {
        return Err("the order is not a permutation".to_owned());
    }
    let mut pos = vec![usize::MAX; n];
    for (p, &i) in order.iter().enumerate() {
        if i >= n || pos[i] != usize::MAX {
            return Err("the order is not a permutation".to_owned());
        }
        pos[i] = p;
    }
    let touches: Vec<Touch> = items.iter().map(|&id| touch(store, id)).collect();

    // Writers and readers per location, in item order.
    let mut var_writers: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut var_readers: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut slot_any: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut slot_whole: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut cells: HashMap<(&str, i32), Vec<usize>> = HashMap::new();
    let mut heap: Vec<usize> = Vec::new();
    let mut any: Vec<usize> = Vec::new();
    for (k, t) in touches.iter().enumerate() {
        for w in &t.writes {
            let list = match w {
                Loc::Var(x) => var_writers.entry(x).or_default(),
                Loc::Slot(x) => {
                    slot_whole.entry(x).or_default().push(k);
                    slot_any.entry(x).or_default()
                }
                Loc::Cell(x, c) => {
                    cells.entry((x, *c)).or_default().push(k);
                    slot_any.entry(x).or_default()
                }
                Loc::Heap => &mut heap,
                Loc::Any => &mut any,
            };
            list.push(k);
        }
        for r in &t.reads {
            if let Loc::Var(x) = r {
                var_readers.entry(x).or_default().push(k);
            }
        }
    }
    for list in var_writers
        .values_mut()
        .chain(var_readers.values_mut())
        .chain(slot_any.values_mut())
        .chain(slot_whole.values_mut())
        .chain(cells.values_mut())
        .chain([&mut heap, &mut any])
    {
        list.dedup();
    }
    let movable: Vec<Option<String>> = items
        .iter()
        .map(|&id| {
            sinkable(store, id).filter(|x| var_writers.get(x.as_str()).map(Vec::len) == Some(1))
        })
        .collect();

    // Fixed items keep their order; nothing rises above a fixed predecessor.
    let mut last_fixed: Option<usize> = None;
    for i in 0..n {
        if let Some(f) = last_fixed
            && pos[f] > pos[i]
        {
            return Err(format!("item {i} was moved above item {f}"));
        }
        if movable[i].is_none() {
            last_fixed = Some(i);
        }
    }

    // An item emitted after a later item moved down: it must be movable, its
    // readers must all follow it, and no writer of what it reads may precede it.
    let mut min_after = vec![usize::MAX; n + 1];
    for i in (0..n).rev() {
        min_after[i] = min_after[i + 1].min(pos[i]);
    }
    for i in 0..n {
        if min_after[i + 1] > pos[i] {
            continue;
        }
        let Some(var) = &movable[i] else {
            return Err(format!("item {i} cannot move"));
        };
        if let Some(readers) = var_readers.get(var.as_str())
            && let Some(&k) = readers.iter().find(|&&k| k > i && pos[k] < pos[i])
        {
            return Err(format!(
                "item {i} was moved after item {k}, which reads `{var}`"
            ));
        }
        let FirMatch::DeclareVar {
            init: Some(init), ..
        } = match_fir(store, items[i])
        else {
            return Err(format!("item {i} cannot move"));
        };
        let mut t = Touch::default();
        touch_value(store, init, &mut t);
        let mut first: Vec<Option<usize>> = vec![next_after(Some(&any), i)];
        for r in &t.reads {
            match r {
                Loc::Var(x) => {
                    if let Some(writers) = var_writers.get(x.as_str()) {
                        first.extend(writers.iter().filter(|&&k| k > i).map(|&k| Some(k)));
                    }
                }
                Loc::Slot(x) => first.push(next_after(slot_any.get(x.as_str()), i)),
                Loc::Cell(x, c) => {
                    first.push(next_after(slot_whole.get(x.as_str()), i));
                    first.push(next_after(cells.get(&(x.as_str(), *c)), i));
                }
                Loc::Heap => first.push(next_after(Some(&heap), i)),
                Loc::Any => return Err(format!("item {i} cannot move")),
            }
        }
        if let Some(k) = first.into_iter().flatten().find(|&k| pos[k] < pos[i]) {
            return Err(format!(
                "item {i} was moved after item {k}, which writes what it reads"
            ));
        }
    }
    Ok(())
}

/// Rebuilds the scheduled body from the forwarded body and the recorded orders,
/// with the checker's own traversal.
fn reassemble(store: &mut FirStore, id: FirId, orders: &HashMap<Vec<FirId>, Vec<usize>>) -> FirId {
    match match_fir(store, id) {
        FirMatch::Block(items) => {
            let items: Vec<FirId> = items
                .iter()
                .map(|&s| reassemble(store, s, orders))
                .collect();
            let emitted: Vec<FirId> = match orders.get(&items) {
                Some(order) => order.iter().map(|&i| items[i]).collect(),
                None => items,
            };
            FirBuilder::new(store).block(&emitted)
        }
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let t = reassemble(store, then_block, orders);
            let e = else_block.map(|e| reassemble(store, e, orders));
            FirBuilder::new(store).if_(cond, t, e)
        }
        FirMatch::Control { cond, stmt } => {
            let s = reassemble(store, stmt, orders);
            FirBuilder::new(store).control(cond, s)
        }
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            let cases: Vec<(i64, FirId)> = cases
                .iter()
                .map(|&(k, b)| (k, reassemble(store, b, orders)))
                .collect();
            let d = default.map(|d| reassemble(store, d, orders));
            FirBuilder::new(store).switch(cond, &cases, d)
        }
        FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } => {
            let b = reassemble(store, body, orders);
            FirBuilder::new(store).simple_for_loop(var, upper, b, is_reverse)
        }
        FirMatch::ForLoop {
            var,
            init,
            end,
            step,
            body,
            is_reverse,
        } => {
            let b = reassemble(store, body, orders);
            FirBuilder::new(store).for_loop(var, init, end, step, b, is_reverse)
        }
        FirMatch::WhileLoop { cond, body } => {
            let b = reassemble(store, body, orders);
            FirBuilder::new(store).while_loop(cond, b)
        }
        _ => id,
    }
}

/// Checks a W2 rewrite: every recorded order, and that the orders applied to
/// `before` give exactly `after`.
pub(crate) fn check_sink(
    store: &mut FirStore,
    before: FirId,
    after: FirId,
    orders: &[BlockOrder],
) -> Result<(), String> {
    let mut by_items = HashMap::with_capacity(orders.len());
    for record in orders {
        check_order(store, &record.items, &record.order)?;
        by_items.insert(record.items.clone(), record.order.clone());
    }
    if reassemble(store, before, &by_items) != after {
        return Err("the scheduled body is not the recorded orders applied".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod known_tests {
    use super::{Known, Loc, hits};
    use crate::{FirBuilder, FirStore};

    /// `Known::forget(w)` forgets exactly the locations `hits(w, _)` names.
    #[test]
    fn forget_agrees_with_hits() {
        let mut store = FirStore::new();
        let value = FirBuilder::new(&mut store).float64(1.0);
        let locs = [
            Loc::Slot("a".into()),
            Loc::Slot("b".into()),
            Loc::Cell("a".into(), 0),
            Loc::Cell("a".into(), 1),
            Loc::Cell("b".into(), 0),
        ];
        let writes = [
            Loc::Slot("a".into()),
            Loc::Cell("a".into(), 0),
            Loc::Cell("b".into(), 1),
            Loc::Var("a".into()),
            Loc::Heap,
            Loc::Any,
        ];
        for w in &writes {
            let mut known = Known::default();
            for loc in &locs {
                known.insert(loc.clone(), value);
            }
            known.forget(w);
            for loc in &locs {
                assert_eq!(
                    known.get(loc).is_none(),
                    hits(w, loc),
                    "write {w:?}, location {loc:?}"
                );
            }
        }
    }
}
