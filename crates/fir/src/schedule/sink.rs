//! W2: sink reads and pure temporaries to their first use (producer, design
//! note §4).
//!
//! `double fTemp36 = fRec2[1];` at the top of the sample loop is emitted just
//! before the first statement that reads `fTemp36`, or before the first one that
//! may write `fRec2[1]`, whichever comes first. A JIT that keeps loads in program
//! order (V8's TurboFan for wasm) no longer keeps the value live, and spilled,
//! across the loop body.

use std::collections::HashMap;

use super::effects::{Addr, Effects, effects_of, value_effects};
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

/// One block's decision: its items (after its nested blocks were scheduled)
/// and the order they are emitted in.
pub(crate) struct BlockOrder {
    pub(crate) items: Vec<FirId>,
    pub(crate) order: Vec<usize>,
}

/// Per-address sorted lists of the items that write it.
#[derive(Default)]
struct WriterIndex {
    local: HashMap<String, Vec<usize>>,
    whole: HashMap<String, Vec<usize>>,
    elem: HashMap<(String, i32), Vec<usize>>,
    any_elem: HashMap<String, Vec<usize>>,
    stack: Vec<usize>,
    all: Vec<usize>,
}

fn push_once(list: &mut Vec<usize>, i: usize) {
    if list.last() != Some(&i) {
        list.push(i);
    }
}

/// First entry of the sorted `list` greater than `i`.
fn first_after(list: Option<&Vec<usize>>, i: usize) -> Option<usize> {
    let list = list?;
    list.get(list.partition_point(|&k| k <= i)).copied()
}

impl WriterIndex {
    fn add(&mut self, i: usize, addr: &Addr) {
        match addr {
            Addr::Local(x) => push_once(self.local.entry(x.clone()).or_default(), i),
            Addr::Field(x) => push_once(self.whole.entry(x.clone()).or_default(), i),
            Addr::Elem(x, k) => {
                push_once(self.elem.entry((x.clone(), *k)).or_default(), i);
                push_once(self.any_elem.entry(x.clone()).or_default(), i);
            }
            Addr::Stack => push_once(&mut self.stack, i),
            Addr::All => push_once(&mut self.all, i),
        }
    }

    /// First item after `i` whose writes conflict with a read of `addr`.
    fn first_conflict_after(&self, addr: &Addr, i: usize) -> Option<usize> {
        let found = match addr {
            Addr::Local(x) => [first_after(self.local.get(x), i), None],
            Addr::Field(x) => [
                first_after(self.whole.get(x), i),
                first_after(self.any_elem.get(x), i),
            ],
            Addr::Elem(x, k) => [
                first_after(self.whole.get(x), i),
                first_after(self.elem.get(&(x.clone(), *k)), i),
            ],
            Addr::Stack => [first_after(Some(&self.stack), i), None],
            Addr::All => [Some(i + 1), None],
        };
        found
            .into_iter()
            .flatten()
            .chain(first_after(Some(&self.all), i))
            .min()
    }
}

/// The local item `id` defines, if it is movable in principle: a non-array stack
/// `DeclareVar` with a pure initializer.
fn movable_decl(store: &FirStore, id: FirId) -> Option<String> {
    let FirMatch::DeclareVar {
        name,
        access: AccessType::Stack,
        typ,
        init: Some(init),
    } = match_fir(store, id)
    else {
        return None;
    };
    if matches!(typ, FirType::Array(..)) {
        return None;
    }
    value_effects(store, init, &mut Effects::default()).then_some(name)
}

/// Solid statements left between a sunk declaration and its first use.
///
/// Sinking a load right next to its use put its latency on the critical path
/// of serial code: the faust-coopiano voice (a Newton-solved hammer) took 25 %
/// more cycles per sample in V8 with no slack. Measured on 2026-10-17 (slope
/// method, cycles per sample, base = FIR order): voice 5,200–5,460 base, 4,850–
/// 5,030 at 8; light body 27,400–27,800 base, 17,300–18,500 at 8; imaged body
/// 45,400–46,300 base, 41,400–42,800 at 8. A slack of 4 or 16 measured the same
/// within noise.
pub(crate) const SLACK: usize = 8;

/// The W2 order of a block's items, or `None` when no item moves. A declaration
/// stops `slack` solid items before its anchor, and stays when that is not past
/// any solid item.
pub(crate) fn sink_order(store: &FirStore, items: &[FirId], slack: usize) -> Option<Vec<usize>> {
    let n = items.len();
    if n < 3 {
        return None;
    }
    let effects: Vec<Effects> = items.iter().map(|&id| effects_of(store, id)).collect();
    let mut writers = WriterIndex::default();
    let mut readers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, eff) in effects.iter().enumerate() {
        for addr in &eff.writes {
            writers.add(i, addr);
        }
        for addr in &eff.reads {
            if let Addr::Local(x) = addr {
                push_once(readers.entry(x.as_str()).or_default(), i);
            }
        }
    }
    let defined: Vec<Option<String>> = items
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let name = movable_decl(store, id)?;
            // Its local must have no other writer in the block.
            (writers.local.get(&name).map(Vec::as_slice) == Some(&[i][..])).then_some(name)
        })
        .collect();

    // `solid[k]`: items before `k` that are neither movable nor inert (labels,
    // empty statements). Passing only movable or inert items gains nothing, and
    // would make the pass swap such items back and forth when run again.
    let mut solid = vec![0usize; n + 1];
    for k in 0..n {
        let inert = matches!(
            match_fir(store, items[k]),
            FirMatch::Label(_) | FirMatch::NullStatement
        );
        solid[k + 1] = solid[k] + usize::from(defined[k].is_none() && !inert);
    }

    let solid_list: Vec<usize> = (0..n).filter(|&k| solid[k + 1] > solid[k]).collect();
    // `anchor[i]`: the item `i` is emitted just before (itself when it stays).
    let mut anchor: Vec<usize> = (0..n).collect();
    let mut moved = false;
    for i in (0..n).rev() {
        let Some(name) = &defined[i] else {
            continue;
        };
        let mut target = usize::MAX;
        if let Some(users) = readers.get(name.as_str()) {
            for &u in users.iter().filter(|&&u| u > i) {
                target = target.min(anchor[u]);
            }
        }
        for addr in &effects[i].reads {
            if let Some(j) = writers.first_conflict_after(addr, i) {
                target = target.min(j);
            }
        }
        if let Some(j) = first_after(Some(&writers.all), i) {
            target = target.min(j);
        }
        // Stop `slack` solid items before the anchor: the load keeps some
        // distance from its use (see `SLACK`).
        if target != usize::MAX && slack > 0 {
            let p = solid_list.partition_point(|&k| k < target);
            target = match p.checked_sub(slack) {
                Some(q) if solid_list[q] > i => solid_list[q],
                _ => usize::MAX,
            };
        }
        if target != usize::MAX && target > i + 1 && solid[target] > solid[i + 1] {
            anchor[i] = target;
            moved = true;
        }
    }
    if !moved {
        return None;
    }
    let mut before: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &a) in anchor.iter().enumerate() {
        if a != i {
            before.entry(a).or_default().push(i);
        }
    }
    let mut order = Vec::with_capacity(n);
    for (j, &a) in anchor.iter().enumerate() {
        if a != j {
            continue;
        }
        if let Some(list) = before.get(&j) {
            order.extend(list.iter().copied());
        }
        order.push(j);
    }
    // Items anchored before their own successor do not move.
    order
        .iter()
        .enumerate()
        .any(|(p, &i)| p != i)
        .then_some(order)
}

/// Schedules every block of a function body, innermost first, and records the
/// decisions.
pub(crate) struct Sinker<'a> {
    store: &'a mut FirStore,
    slack: usize,
    pub(crate) orders: Vec<BlockOrder>,
}

impl<'a> Sinker<'a> {
    pub(crate) fn new(store: &'a mut FirStore, slack: usize) -> Self {
        Self {
            store,
            slack,
            orders: Vec::new(),
        }
    }

    /// Schedules block `id` (its nested blocks first).
    pub(crate) fn block(&mut self, id: FirId) -> FirId {
        let FirMatch::Block(items) = match_fir(self.store, id) else {
            return self.stmt(id);
        };
        let items: Vec<FirId> = items.iter().map(|&s| self.stmt(s)).collect();
        let order = sink_order(self.store, &items, self.slack);
        let emitted: Vec<FirId> = match &order {
            Some(order) => order.iter().map(|&i| items[i]).collect(),
            None => items.clone(),
        };
        if let Some(order) = order {
            self.orders.push(BlockOrder { items, order });
        }
        FirBuilder::new(self.store).block(&emitted)
    }

    fn stmt(&mut self, id: FirId) -> FirId {
        match match_fir(self.store, id) {
            FirMatch::Block(_) => self.block(id),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let t = self.block(then_block);
                let e = else_block.map(|e| self.block(e));
                FirBuilder::new(self.store).if_(cond, t, e)
            }
            FirMatch::Control { cond, stmt } => {
                let s = self.block(stmt);
                FirBuilder::new(self.store).control(cond, s)
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                let cases: Vec<(i64, FirId)> =
                    cases.iter().map(|&(k, b)| (k, self.block(b))).collect();
                let d = default.map(|d| self.block(d));
                FirBuilder::new(self.store).switch(cond, &cases, d)
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => {
                let b = self.block(body);
                FirBuilder::new(self.store).simple_for_loop(var, upper, b, is_reverse)
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => {
                let b = self.block(body);
                FirBuilder::new(self.store).for_loop(var, init, end, step, b, is_reverse)
            }
            FirMatch::WhileLoop { cond, body } => {
                let b = self.block(body);
                FirBuilder::new(self.store).while_loop(cond, b)
            }
            _ => id,
        }
    }
}
