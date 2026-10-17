//! Tests of the W7 producer and of its independent checker, with the mutations
//! of the design note §3 that the checker must reject.

use super::{check_reuse::check_reuse, coalesce::reuse_locals};
use crate::{AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

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
    fn local(&mut self, name: &str) -> FirId {
        self.b().load_var(name, AccessType::Stack, FirType::Float64)
    }
    fn decl(&mut self, name: &str, v: f64) -> FirId {
        let init = self.num(v);
        self.b()
            .declare_var(name, FirType::Float64, AccessType::Stack, Some(init))
    }
    fn slot(&mut self, name: &str) -> FirId {
        self.b()
            .declare_var(name, FirType::Float64, AccessType::Stack, None)
    }
    fn set(&mut self, name: &str, v: f64) -> FirId {
        let v = self.num(v);
        self.b().store_var(name, AccessType::Stack, v)
    }
    /// `fRec0[k] = value`.
    fn out(&mut self, k: i32, value: FirId) -> FirId {
        let i = self.b().int32(k);
        self.b().store_table("fRec0", AccessType::Struct, i, value)
    }
    fn add(&mut self, a: FirId, b: FirId) -> FirId {
        self.b().binop(FirBinOp::Add, a, b, FirType::Float64)
    }
    fn block(&mut self, items: &[FirId]) -> FirId {
        self.b().block(items)
    }
    fn for_loop(&mut self, body: &[FirId]) -> FirId {
        let n = self.b().int32(4);
        let body = self.block(body);
        self.b().simple_for_loop("i0", n, body, false)
    }
    fn if_(&mut self, then: &[FirId]) -> FirId {
        let c = self
            .b()
            .load_var("iCond", AccessType::Struct, FirType::Int32);
        let t = self.block(then);
        self.b().if_(c, t, None)
    }

    /// W7 on `body`, which the checker must accept.
    fn reuse(&mut self, body: FirId) -> FirId {
        let out = reuse_locals(&mut self.s, body).expect("W7 renames something");
        check_reuse(&self.s, body, out).expect("the checker accepts W7's output");
        out
    }

    /// The slot of every renamed stack variable read or stored in `body`, in
    /// order of appearance.
    fn slots_used(&self, body: FirId) -> Vec<String> {
        let FirMatch::Block(items) = match_fir(&self.s, body) else {
            panic!("a block");
        };
        items
            .iter()
            .filter_map(|&i| match match_fir(&self.s, i) {
                FirMatch::DeclareVar {
                    name, init: None, ..
                } => Some(name),
                _ => None,
            })
            .collect()
    }
}

// ─── W7 producer ─────────────────────────────────────────────────────────────

/// Two temporaries used one after the other share a slot.
#[test]
fn disjoint_temporaries_share_a_slot() {
    let mut f = Fir::new();
    let da = f.decl("fTemp0", 1.0);
    let a = f.local("fTemp0");
    let ua = f.out(0, a);
    let db = f.decl("fTemp1", 2.0);
    let b = f.local("fTemp1");
    let ub = f.out(1, b);
    let body = f.block(&[da, ua, db, ub]);
    let out = f.reuse(body);
    assert_eq!(f.slots_used(out), vec!["fReuse0".to_owned()]);
}

/// Two temporaries live at once get two slots.
#[test]
fn overlapping_temporaries_get_two_slots() {
    let mut f = Fir::new();
    let da = f.decl("fTemp0", 1.0);
    let db = f.decl("fTemp1", 2.0);
    let (a, b) = (f.local("fTemp0"), f.local("fTemp1"));
    let sum = f.add(a, b);
    let u = f.out(0, sum);
    let body = f.block(&[da, db, u]);
    let out = f.reuse(body);
    assert_eq!(f.slots_used(out).len(), 2);
}

/// A variable declared before a loop and read in it lives through the whole
/// loop: a temporary of the loop body cannot take its slot, one after the loop
/// can take the body's.
#[test]
fn a_variable_read_in_a_loop_lives_through_it() {
    let mut f = Fir::new();
    let da = f.decl("fSlow0", 1.0);
    let db = f.decl("fTemp0", 2.0);
    let (a, b) = (f.local("fSlow0"), f.local("fTemp0"));
    let sum = f.add(a, b);
    let u = f.out(0, sum);
    let lp = f.for_loop(&[db, u]);
    let dc = f.decl("fTemp1", 3.0);
    let c = f.local("fTemp1");
    let uc = f.out(1, c);
    let body = f.block(&[da, lp, dc, uc]);
    let out = f.reuse(body);
    assert_eq!(f.slots_used(out).len(), 2);
}

/// A variable assigned in both arms of an `if` and read after it keeps its slot.
#[test]
fn a_variable_set_in_arms_keeps_its_slot() {
    let mut f = Fir::new();
    let dx = f.slot("fTemp0");
    let sx = f.set("fTemp0", 1.0);
    let arm = f.if_(&[sx]);
    let x = f.local("fTemp0");
    let u = f.out(0, x);
    let dy = f.decl("fTemp1", 2.0);
    let y = f.local("fTemp1");
    let uy = f.out(1, y);
    let body = f.block(&[dx, arm, u, dy, uy]);
    let out = f.reuse(body);
    assert_eq!(f.slots_used(out).len(), 1);
}

/// Different types never share; the buffer pseudo-variables are not renamed;
/// a second run changes nothing.
#[test]
fn types_names_and_idempotence() {
    let mut f = Fir::new();
    let da = f.decl("fTemp0", 1.0);
    let a = f.local("fTemp0");
    let ua = f.out(0, a);
    let seven = f.b().int32(7);
    let di = f
        .b()
        .declare_var("iTemp0", FirType::Int32, AccessType::Stack, Some(seven));
    let i = f.b().load_var("iTemp0", AccessType::Stack, FirType::Int32);
    let fi = f.b().cast(FirType::Float64, i);
    let ui = f.out(1, fi);
    let din = f.decl("input0", 0.0);
    let body = f.block(&[da, ua, di, ui, din]);
    let out = f.reuse(body);
    let mut slots = f.slots_used(out);
    slots.sort();
    assert_eq!(slots, vec!["fReuse0".to_owned(), "iReuse0".to_owned()]);
    assert_eq!(reuse_locals(&mut f.s, out), None, "W7 is idempotent");
}

/// Slots are numbered per type, in creation order, however the types
/// interleave: three `Float64` and two `Int32` values, all live at once,
/// declared in alternation.
#[test]
fn slots_are_numbered_per_type() {
    let mut f = Fir::new();
    let mut items = Vec::new();
    for k in 0..3 {
        items.push(f.decl(&format!("fTemp{k}"), f64::from(k)));
        if k < 2 {
            let v = f.b().int32(k);
            let d = f.b().declare_var(
                format!("iTemp{k}"),
                FirType::Int32,
                AccessType::Stack,
                Some(v),
            );
            items.push(d);
        }
    }
    for k in 0..3 {
        let r = f.local(&format!("fTemp{k}"));
        items.push(f.out(k, r));
    }
    for k in 0..2 {
        let i = f
            .b()
            .load_var(format!("iTemp{k}"), AccessType::Stack, FirType::Int32);
        let r = f.b().cast(FirType::Float64, i);
        items.push(f.out(3 + k, r));
    }
    let body = f.block(&items);
    let out = f.reuse(body);
    assert_eq!(
        f.slots_used(out),
        ["fReuse0", "iReuse0", "fReuse1", "iReuse1", "fReuse2"]
    );
}

// ─── Mutations the checker must reject ───────────────────────────────────────

/// The checker rejects `after`, for the reason containing `why`.
fn rejects(f: &Fir, before: FirId, after: FirId, why: &str) {
    let reason = check_reuse(&f.s, before, after).expect_err("the checker rejects the mutation");
    assert!(
        reason.contains(why),
        "rejected for another reason: {reason}"
    );
}

/// `fTemp0 = 1; fTemp1 = 2; fRec0[0] = fTemp0 + fTemp1;` before W7.
fn two_live(f: &mut Fir) -> FirId {
    let da = f.decl("fTemp0", 1.0);
    let db = f.decl("fTemp1", 2.0);
    let (a, b) = (f.local("fTemp0"), f.local("fTemp1"));
    let sum = f.add(a, b);
    let u = f.out(0, sum);
    f.block(&[da, db, u])
}

#[test]
fn mutation_two_live_variables_on_one_slot() {
    let mut f = Fir::new();
    let before = two_live(&mut f);
    let d = f.slot("fReuse0");
    let (s1, s2) = (f.set("fReuse0", 1.0), f.set("fReuse0", 2.0));
    let (r1, r2) = (f.local("fReuse0"), f.local("fReuse0"));
    let sum = f.add(r1, r2);
    let u = f.out(0, sum);
    let after = f.block(&[d, s1, s2, u]);
    rejects(&f, before, after, "which holds Some(\"fTemp1\")");
}

#[test]
fn mutation_loop_temporary_on_the_slot_of_a_value_read_in_the_loop() {
    let mut f = Fir::new();
    let da = f.decl("fSlow0", 1.0);
    let db = f.decl("fTemp0", 2.0);
    let (a, b) = (f.local("fSlow0"), f.local("fTemp0"));
    let sum = f.add(a, b);
    let u = f.out(0, sum);
    let lp = f.for_loop(&[db, u]);
    let before = f.block(&[da, lp]);

    let d = f.slot("fReuse0");
    let s1 = f.set("fReuse0", 1.0);
    let s2 = f.set("fReuse0", 2.0);
    let (r1, r2) = (f.local("fReuse0"), f.local("fReuse0"));
    let sum2 = f.add(r1, r2);
    let u2 = f.out(0, sum2);
    let lp2 = f.for_loop(&[s2, u2]);
    let after = f.block(&[d, s1, lp2]);
    rejects(&f, before, after, "read of `fSlow0` through `fReuse0`");
}

#[test]
fn mutation_slot_holding_another_variable_after_an_arm() {
    let mut f = Fir::new();
    let da = f.decl("fTemp0", 1.0);
    let db = f.decl("fTemp1", 2.0);
    let b = f.local("fTemp1");
    let ub = f.out(1, b);
    let arm = f.if_(&[db, ub]);
    let a = f.local("fTemp0");
    let ua = f.out(0, a);
    let before = f.block(&[da, arm, ua]);

    let d = f.slot("fReuse0");
    let s1 = f.set("fReuse0", 1.0);
    let s2 = f.set("fReuse0", 2.0);
    let r = f.local("fReuse0");
    let ub2 = f.out(1, r);
    let arm2 = f.if_(&[s2, ub2]);
    let ua2 = f.out(0, r);
    let after = f.block(&[d, s1, arm2, ua2]);
    rejects(
        &f,
        before,
        after,
        "read of `fTemp0` through `fReuse0`, which holds None",
    );
}

#[test]
fn mutation_renaming_is_not_a_function() {
    let mut f = Fir::new();
    let da = f.decl("fTemp0", 1.0);
    let a = f.local("fTemp0");
    let ua = f.out(0, a);
    let before = f.block(&[da, ua]);
    let (d0, d1) = (f.slot("fReuse0"), f.slot("fReuse1"));
    let s = f.set("fReuse0", 1.0);
    let r = f.local("fReuse1");
    let u = f.out(0, r);
    let after = f.block(&[d0, d1, s, u]);
    rejects(&f, before, after, "renamed to both");
}

#[test]
fn mutation_slot_of_another_type() {
    let mut f = Fir::new();
    let da = f.decl("fTemp0", 1.0);
    let a = f.local("fTemp0");
    let ua = f.out(0, a);
    let before = f.block(&[da, ua]);
    let d = f
        .b()
        .declare_var("iReuse0", FirType::Int32, AccessType::Stack, None);
    let s = f.set("iReuse0", 1.0);
    let r = f.local("iReuse0");
    let u = f.out(0, r);
    let after = f.block(&[d, s, u]);
    rejects(&f, before, after, "given slot `iReuse0`");
}

#[test]
fn mutation_slot_name_already_used() {
    let mut f = Fir::new();
    let before = two_live(&mut f);
    // `fTemp1` as the slot of `fTemp0`: the program already uses that name.
    let d = f.slot("fTemp1");
    let s = f.set("fTemp1", 1.0);
    let db = f.decl("fTemp1", 2.0);
    let (a, b) = (f.local("fTemp1"), f.local("fTemp1"));
    let sum = f.add(a, b);
    let u = f.out(0, sum);
    let after = f.block(&[d, s, db, u]);
    rejects(&f, before, after, "already names something");
}

#[test]
fn mutation_lost_or_extra_statement() {
    let mut f = Fir::new();
    let before = two_live(&mut f);
    let out = reuse_locals(&mut f.s, before).expect("W7 renames");
    let FirMatch::Block(mut items) = match_fir(&f.s, out) else {
        panic!("a block");
    };
    let mut lost = items.clone();
    lost.pop();
    let lost = f.block(&lost);
    rejects(&f, before, lost, "statements differ");
    let extra = f.set("fReuse0", 3.0);
    items.insert(1, extra);
    let extra = f.block(&items);
    rejects(&f, before, extra, "not a slot declaration");
}
