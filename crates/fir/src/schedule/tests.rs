//! Tests of the W2/W3 producers and of their independent checker, with the
//! mutations of the design note §5 that the checker must reject.

use std::collections::HashMap;

use super::{check, forward, sink};
use crate::{AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

fn f64t() -> FirType {
    FirType::Float64
}

/// A small FIR vocabulary: struct array elements, stack locals, stores.
struct Fir {
    s: FirStore,
}

impl Fir {
    fn new() -> Self {
        Self { s: FirStore::new() }
    }
    fn b(&mut self) -> FirBuilder<'_> {
        FirBuilder::new(&mut self.s)
    }
    fn num(&mut self, v: f64) -> FirId {
        self.b().float64(v)
    }
    fn cell(&mut self, name: &str, k: i32) -> FirId {
        let i = self.b().int32(k);
        self.b().load_table(name, AccessType::Struct, i, f64t())
    }
    fn local(&mut self, name: &str) -> FirId {
        self.b().load_var(name, AccessType::Stack, f64t())
    }
    fn decl(&mut self, name: &str, init: FirId) -> FirId {
        self.b()
            .declare_var(name, f64t(), AccessType::Stack, Some(init))
    }
    fn set_cell(&mut self, name: &str, k: i32, v: FirId) -> FirId {
        let i = self.b().int32(k);
        self.b().store_table(name, AccessType::Struct, i, v)
    }
    fn set_field(&mut self, name: &str, v: FirId) -> FirId {
        self.b().store_var(name, AccessType::Struct, v)
    }
    fn set_local(&mut self, name: &str, v: FirId) -> FirId {
        self.b().store_var(name, AccessType::Stack, v)
    }
    fn mul(&mut self, a: FirId, k: f64) -> FirId {
        let k = self.num(k);
        self.b().binop(FirBinOp::Mul, a, k, f64t())
    }
    /// A store to a dynamic index: writes the whole array.
    fn set_any_cell(&mut self, name: &str, v: FirId) -> FirId {
        let i = self.b().load_var("i0", AccessType::Loop, FirType::Int32);
        self.b().store_table(name, AccessType::Struct, i, v)
    }
    /// A foreign call: writes anything.
    fn foreign(&mut self) -> FirId {
        let call = self.b().fun_call("ffoo", &[], f64t());
        self.b().drop_(call)
    }
    fn block(&mut self, items: &[FirId]) -> FirId {
        self.b().block(items)
    }

    fn sink(&self, items: &[FirId]) -> Option<Vec<usize>> {
        sink::sink_order(&self.s, items, 0)
    }
    fn check(&self, items: &[FirId], order: &[usize]) -> Result<(), String> {
        check::check_order(&self.s, items, order)
    }
    fn forward(&mut self, body: FirId) -> (FirId, usize) {
        let mut writes = HashMap::new();
        forward::local_write_counts(&self.s, body, &mut writes);
        let mut fw = forward::Forwarder::new(&mut self.s, &writes);
        let out = fw.block(body, &mut forward::Available::default());
        (out, fw.forwarded)
    }
    fn check_forward(&self, original: FirId, forwarded: FirId) -> Result<(), String> {
        check::check_forward(&self.s, original, forwarded)
    }
}

// ─── W2 producer ─────────────────────────────────────────────────────────────

/// The resonator-bank shape: each state read sinks to the statement that uses
/// it, past the other mode's statements, and before the copy that overwrites it.
#[test]
fn reads_sink_to_their_first_use() {
    let mut f = Fir::new();
    let r0 = f.cell("fRec0", 1);
    let d0 = f.decl("fTemp0", r0);
    let r1 = f.cell("fRec1", 1);
    let d1 = f.decl("fTemp1", r1);
    let t0 = f.local("fTemp0");
    let v0 = f.mul(t0, 2.0);
    let x = f.set_cell("fRec0", 0, v0);
    let t1 = f.local("fTemp1");
    let v1 = f.mul(t1, 3.0);
    let y = f.set_cell("fRec1", 0, v1);
    let c0v = f.cell("fRec0", 0);
    let c0 = f.set_cell("fRec0", 1, c0v);
    let c1v = f.cell("fRec1", 0);
    let c1 = f.set_cell("fRec1", 1, c1v);
    let items = [d0, d1, x, y, c0, c1];
    let order = f.sink(&items).expect("the reads move");
    assert_eq!(order, vec![0, 2, 1, 3, 4, 5]);
    f.check(&items, &order)
        .expect("the checker accepts the producer's order");
}

/// A read stops before the first store to its element.
#[test]
fn a_read_stops_before_a_store_to_its_element() {
    let mut f = Fir::new();
    let r0 = f.cell("fRec0", 1);
    let d0 = f.decl("fTemp0", r0);
    let other = f.num(1.0);
    let unrelated = f.set_field("fOther", other);
    let zero = f.num(0.0);
    let w = f.set_cell("fRec0", 1, zero);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    let items = [d0, unrelated, w, u];
    let order = f.sink(&items).expect("the read moves");
    let at = |k: usize| order.iter().position(|&i| i == k).unwrap();
    assert!(
        at(0) < at(2),
        "the read stays before the store to fRec0[1]: {order:?}"
    );
    assert_eq!(order, vec![1, 0, 2, 3]);
    f.check(&items, &order).unwrap();
}

/// A store to another element of the same array is not a barrier.
#[test]
fn a_store_to_another_element_is_no_barrier() {
    let mut f = Fir::new();
    let r0 = f.cell("fRec0", 1);
    let d0 = f.decl("fTemp0", r0);
    let zero = f.num(0.0);
    let w = f.set_cell("fRec0", 0, zero);
    let one = f.num(1.0);
    let w2 = f.set_field("fOther", one);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    let items = [d0, w, w2, u];
    assert_eq!(f.sink(&items), Some(vec![1, 2, 0, 3]));
}

/// A declaration with a foreign call never moves.
#[test]
fn an_impure_declaration_stays() {
    let mut f = Fir::new();
    let call = f.b().fun_call("ffoo", &[], f64t());
    let d0 = f.decl("fTemp0", call);
    let one = f.num(1.0);
    let x = f.set_field("fOther", one);
    let y = f.set_field("fOther2", one);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    assert_eq!(f.sink(&[d0, x, y, u]), None);
}

// ─── W3 producer ─────────────────────────────────────────────────────────────

/// `fRec0[0] = fRecBody0;` then `fRec0[1] = fRec0[0];` and `fOut = fRec0[0];`
/// read `fRecBody0`.
#[test]
fn a_stored_local_is_forwarded() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let d = f.decl("fRecBody0", one);
    let body_val = f.local("fRecBody0");
    let s = f.set_cell("fRec0", 0, body_val);
    let l1 = f.cell("fRec0", 0);
    let c = f.set_cell("fRec0", 1, l1);
    let l2 = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l2);
    let body = f.block(&[d, s, c, o]);
    let (out, n) = f.forward(body);
    assert_eq!(n, 2);
    let FirMatch::Block(items) = match_fir(&f.s, out) else {
        panic!()
    };
    let FirMatch::StoreTable { value, .. } = match_fir(&f.s, items[2]) else {
        panic!()
    };
    assert_eq!(value, body_val);
    f.check_forward(body, out).unwrap();
}

/// A whole-array store in between stops the forward.
#[test]
fn no_forward_across_a_whole_array_store() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let d = f.decl("fRecBody0", one);
    let v = f.local("fRecBody0");
    let s = f.set_cell("fRec0", 0, v);
    let zero = f.num(0.0);
    let w = f.set_any_cell("fRec0", zero);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let body = f.block(&[d, s, w, o]);
    assert_eq!(f.forward(body).1, 0);
}

/// A store in an arm is not available after the `if`.
#[test]
fn no_forward_out_of_an_arm() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let d = f.decl("fRecBody0", one);
    let v = f.local("fRecBody0");
    let s = f.set_cell("fRec0", 0, v);
    let arm = f.block(&[s]);
    let cond = f.b().bool_(true);
    let iff = f.b().if_(cond, arm, None);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let body = f.block(&[d, iff, o]);
    assert_eq!(f.forward(body).1, 0);
}

/// A store before a loop is not available inside it.
#[test]
fn no_forward_into_a_loop() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let d = f.decl("fRecBody0", one);
    let v = f.local("fRecBody0");
    let s = f.set_cell("fRec0", 0, v);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let lbody = f.block(&[o]);
    let upper = f.b().int32(4);
    let lp = f.b().simple_for_loop("j", upper, lbody, false);
    let body = f.block(&[d, s, lp]);
    assert_eq!(f.forward(body).1, 0);
}

/// A local assigned twice is never forwarded.
#[test]
fn no_forward_of_a_local_assigned_twice() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let d = f.decl("fTemp0", one);
    let two = f.num(2.0);
    let again = f.set_local("fTemp0", two);
    let v = f.local("fTemp0");
    let s = f.set_cell("fRec0", 0, v);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let body = f.block(&[d, again, s, o]);
    assert_eq!(f.forward(body).1, 0);
}

// ─── W2 checker mutations ────────────────────────────────────────────────────

/// `[d0 = fRec0[1], w, u = fOut <- fTemp0]` with `w` given.
fn read_write_use(f: &mut Fir, w: FirId) -> [FirId; 3] {
    let r0 = f.cell("fRec0", 1);
    let d0 = f.decl("fTemp0", r0);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    [d0, w, u]
}

#[test]
fn mutation_read_past_a_store_to_its_element() {
    let mut f = Fir::new();
    let zero = f.num(0.0);
    let w = f.set_cell("fRec0", 1, zero);
    let items = read_write_use(&mut f, w);
    assert!(f.check(&items, &[1, 0, 2]).is_err());
}

#[test]
fn mutation_read_past_a_whole_array_store() {
    let mut f = Fir::new();
    let zero = f.num(0.0);
    let w = f.set_any_cell("fRec0", zero);
    let items = read_write_use(&mut f, w);
    assert!(f.check(&items, &[1, 0, 2]).is_err());
}

#[test]
fn mutation_read_past_a_foreign_call() {
    let mut f = Fir::new();
    let w = f.foreign();
    let items = read_write_use(&mut f, w);
    assert!(f.check(&items, &[1, 0, 2]).is_err());
}

#[test]
fn mutation_read_past_a_store_to_a_local_it_reads() {
    let mut f = Fir::new();
    let a = f.local("fTempA");
    let init = f.mul(a, 1.0);
    let d0 = f.decl("fTemp0", init);
    let two = f.num(2.0);
    let w = f.set_local("fTempA", two);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    assert!(f.check(&[d0, w, u], &[1, 0, 2]).is_err());
}

#[test]
fn mutation_declaration_after_its_reader() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let z = f.set_field("fOther", one);
    let items = read_write_use(&mut f, z);
    assert!(f.check(&items, &[1, 2, 0]).is_err());
}

#[test]
fn mutation_impure_declaration_moved() {
    let mut f = Fir::new();
    let call = f.b().fun_call("ffoo", &[], f64t());
    let d0 = f.decl("fTemp0", call);
    let one = f.num(1.0);
    let x = f.set_field("fOther", one);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    assert!(f.check(&[d0, x, u], &[1, 0, 2]).is_err());
}

#[test]
fn mutation_reassigned_declaration_moved() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let d0 = f.decl("fTemp0", one);
    let x = f.set_field("fOther", one);
    let two = f.num(2.0);
    let again = f.set_local("fTemp0", two);
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    assert!(f.check(&[d0, x, again, u], &[1, 0, 2, 3]).is_err());
}

#[test]
fn mutation_fixed_statements_swapped() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let a = f.set_field("fA", one);
    let b = f.set_field("fB", one);
    assert!(f.check(&[a, b], &[1, 0]).is_err());
}

#[test]
fn mutation_item_moved_earlier() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let x = f.set_field("fA", one);
    let d0 = f.decl("fTemp0", one);
    assert!(f.check(&[x, d0], &[1, 0]).is_err());
}

#[test]
fn mutation_not_a_permutation() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let z = f.set_field("fOther", one);
    let items = read_write_use(&mut f, z);
    assert!(f.check(&items, &[0, 0, 2]).is_err());
    assert!(f.check(&items, &[0, 1]).is_err());
}

#[test]
fn a_legal_sink_is_accepted() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let z = f.set_field("fOther", one);
    let items = read_write_use(&mut f, z);
    f.check(&items, &[1, 0, 2]).unwrap();
}

// ─── W3 checker mutations ────────────────────────────────────────────────────

/// `[decl a; decl b; fRec0[0] = a; mid; fOut = fRec0[0]]`, and the same with
/// the final load replaced by `replacement`.
fn forward_pair(f: &mut Fir, mid: FirId, replacement: FirId) -> (FirId, FirId) {
    let one = f.num(1.0);
    let da = f.decl("fA", one);
    let two = f.num(2.0);
    let db = f.decl("fB", two);
    let a = f.local("fA");
    let s = f.set_cell("fRec0", 0, a);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let o2 = f.set_field("fOut", replacement);
    (
        f.block(&[da, db, s, mid, o]),
        f.block(&[da, db, s, mid, o2]),
    )
}

#[test]
fn a_legal_forward_is_accepted() {
    let mut f = Fir::new();
    let mid = f.b().null_statement();
    let a = f.local("fA");
    let (orig, fwd) = forward_pair(&mut f, mid, a);
    f.check_forward(orig, fwd).unwrap();
}

#[test]
fn mutation_forward_across_a_conflicting_store() {
    let mut f = Fir::new();
    let b = f.local("fB");
    let mid = f.set_cell("fRec0", 0, b);
    let a = f.local("fA");
    let (orig, fwd) = forward_pair(&mut f, mid, a);
    assert!(f.check_forward(orig, fwd).is_err());
}

#[test]
fn mutation_forward_of_another_element() {
    let mut f = Fir::new();
    let mid = f.b().null_statement();
    let one = f.num(1.0);
    let da = f.decl("fA", one);
    let a = f.local("fA");
    let s = f.set_cell("fRec0", 0, a);
    let l = f.cell("fRec0", 1);
    let o = f.set_field("fOut", l);
    let o2 = f.set_field("fOut", a);
    let orig = f.block(&[da, s, mid, o]);
    let fwd = f.block(&[da, s, mid, o2]);
    assert!(f.check_forward(orig, fwd).is_err());
}

#[test]
fn mutation_forward_out_of_an_arm() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let da = f.decl("fA", one);
    let a = f.local("fA");
    let s = f.set_cell("fRec0", 0, a);
    let arm = f.block(&[s]);
    let cond = f.b().bool_(true);
    let iff = f.b().if_(cond, arm, None);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let o2 = f.set_field("fOut", a);
    let orig = f.block(&[da, iff, o]);
    let fwd = f.block(&[da, iff, o2]);
    assert!(f.check_forward(orig, fwd).is_err());
}

#[test]
fn mutation_forward_into_a_loop() {
    let mut f = Fir::new();
    let one = f.num(1.0);
    let da = f.decl("fA", one);
    let a = f.local("fA");
    let s = f.set_cell("fRec0", 0, a);
    let l = f.cell("fRec0", 0);
    let o = f.set_field("fOut", l);
    let o2 = f.set_field("fOut", a);
    let upper = f.b().int32(4);
    let b1 = f.block(&[o]);
    let b2 = f.block(&[o2]);
    let lp1 = f.b().simple_for_loop("j", upper, b1, false);
    let lp2 = f.b().simple_for_loop("j", upper, b2, false);
    let orig = f.block(&[da, s, lp1]);
    let fwd = f.block(&[da, s, lp2]);
    assert!(f.check_forward(orig, fwd).is_err());
}

#[test]
fn mutation_forward_of_a_local_assigned_twice() {
    let mut f = Fir::new();
    let two = f.num(2.0);
    let mid = f.set_local("fA", two);
    let a = f.local("fA");
    let (orig, fwd) = forward_pair(&mut f, mid, a);
    assert!(f.check_forward(orig, fwd).is_err());
}

#[test]
fn mutation_forward_across_a_foreign_call() {
    let mut f = Fir::new();
    let mid = f.foreign();
    let a = f.local("fA");
    let (orig, fwd) = forward_pair(&mut f, mid, a);
    assert!(f.check_forward(orig, fwd).is_err());
}

/// With a slack of 2, a read stops two solid statements before its use.
#[test]
fn a_read_stops_slack_statements_before_its_use() {
    let mut f = Fir::new();
    let r0 = f.cell("fRec0", 1);
    let d0 = f.decl("fTemp0", r0);
    let one = f.num(1.0);
    let s: Vec<FirId> = ["fA", "fB", "fC", "fD"]
        .iter()
        .map(|name| f.set_field(name, one))
        .collect();
    let t0 = f.local("fTemp0");
    let u = f.set_field("fOut", t0);
    let items = [d0, s[0], s[1], s[2], s[3], u];
    let order = sink::sink_order(&f.s, &items, 2).expect("the read moves");
    assert_eq!(order, vec![1, 2, 0, 3, 4, 5]);
    f.check(&items, &order).unwrap();
    // Moved by the slack only: nothing left to move.
    let moved: Vec<FirId> = order.iter().map(|&i| items[i]).collect();
    assert_eq!(sink::sink_order(&f.s, &moved, 2), None);
}
