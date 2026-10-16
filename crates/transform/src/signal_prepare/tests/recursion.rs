//! `recursion` group of the signal_prepare tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use crate::signal_prepare::{SimpleSigType, prepare_signals_for_fir};
use signals::{BinOp, SigBuilder, SigMatch, dump_sig_readable, match_sig};
use tlib::{de_bruijn_rec, de_bruijn_ref, match_sym_rec, match_sym_ref};

#[test]
fn prepare_signals_for_fir_preserves_reverse_time_rec_projection_contract() {
    let mut arena = tlib::TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let cotangent = b.input(0);
        let feedback = b.proj(0, self_ref);
        let half = b.real(0.5);
        let transposed = b.mul(half, feedback);
        b.add(cotangent, transposed)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let reverse_group = b.reverse_time_rec(group);
        b.proj(0, reverse_group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("reverse-time recursion should prepare");

    let SigMatch::Proj(0, prepared_reverse_group) = match_sig(&prepared.arena, prepared.outputs[0])
    else {
        panic!("prepared output should remain a projection");
    };
    let SigMatch::ReverseTimeRec(prepared_group) =
        match_sig(&prepared.arena, prepared_reverse_group)
    else {
        panic!("projection target should remain ReverseTimeRec");
    };
    let (var, prepared_body_list) =
        match_sym_rec(&prepared.arena, prepared_group).expect("symbolic recursion expected");
    let prepared_body = prepared
        .arena
        .hd(prepared_body_list)
        .expect("prepared recursion body head");
    assert!(
        dump_sig_readable(&prepared.arena, prepared_body).contains("SIGPROJ"),
        "reverse body should still carry its feedback projection"
    );
    assert!(
        dump_sig_readable(&prepared.arena, prepared_body).contains("SIGINPUT"),
        "reverse body should still carry its cotangent input"
    );
    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Real));

    let SigMatch::BinOp(_, _, rhs) = match_sig(&prepared.arena, prepared_body) else {
        panic!("prepared reverse body should stay affine");
    };
    let SigMatch::BinOp(_, _, feedback) = match_sig(&prepared.arena, rhs) else {
        panic!("prepared reverse feedback should stay scaled");
    };
    let SigMatch::Proj(0, feedback_group) = match_sig(&prepared.arena, feedback) else {
        panic!("feedback edge should remain proj(0, symref(var))");
    };
    assert_eq!(match_sym_ref(&prepared.arena, feedback_group), Some(var));
}
#[test]
fn prepare_signals_for_fir_closes_unconstrained_recursion_to_int() {
    // A fully unconstrained self-recursion `x = x` carries no operation to widen
    // its type. The C++ `TREC` seed is `Int` (interval {0}) and the fixpoint
    // converges to it, so the reduced type must be `Int` — matching the canonical
    // typer, not the old fast-lane `Real` override (which has been removed).
    let mut arena = tlib::TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, self_ref)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("recursive typing should converge");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Int));
}
#[test]
fn prepare_signals_for_fir_keeps_integer_recursive_min_feedback_int() {
    let mut arena = tlib::TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let inc = b.int(1);
        let sum = b.add(prev, inc);
        let cap = b.int(3);
        b.min(sum, cap)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("recursive int min should prepare");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Int));
    let SigMatch::Proj(_, prepared_group) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("prepared output should stay a projection");
    };
    let (_, prepared_body_list) =
        match_sym_rec(&prepared.arena, prepared_group).expect("symbolic recursion expected");
    let prepared_body = prepared
        .arena
        .hd(prepared_body_list)
        .expect("prepared recursion body head");
    let SigMatch::Min(sum, cap) = match_sig(&prepared.arena, prepared_body) else {
        panic!("prepared body should stay SIGMIN");
    };
    assert_eq!(match_sig(&prepared.arena, cap), SigMatch::Int(3));
    let SigMatch::BinOp(_, lhs, rhs) = match_sig(&prepared.arena, sum) else {
        panic!("prepared min lhs should stay integer addition");
    };
    let (prev, inc) = feedback_and_increment(&prepared.arena, lhs, rhs);
    assert!(
        !matches!(match_sig(&prepared.arena, prev), SigMatch::FloatCast(_)),
        "integer recursive feedback should not be promoted to float before SIGMIN"
    );
    assert_eq!(match_sig(&prepared.arena, inc), SigMatch::Int(1));
}
#[test]
fn prepare_signals_for_fir_keeps_integer_recursive_abs_feedback_int() {
    let mut arena = tlib::TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let inc = b.int(1);
        let sum = b.add(prev, inc);
        b.abs(sum)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("recursive int abs should prepare");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Int));
    let SigMatch::Proj(_, prepared_group) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("prepared output should stay a projection");
    };
    let (_, prepared_body_list) =
        match_sym_rec(&prepared.arena, prepared_group).expect("symbolic recursion expected");
    let prepared_body = prepared
        .arena
        .hd(prepared_body_list)
        .expect("prepared recursion body head");
    let SigMatch::Abs(sum) = match_sig(&prepared.arena, prepared_body) else {
        panic!("prepared body should stay SIGABS");
    };
    let SigMatch::BinOp(_, lhs, rhs) = match_sig(&prepared.arena, sum) else {
        panic!("prepared abs operand should stay integer addition");
    };
    let (prev, inc) = feedback_and_increment(&prepared.arena, lhs, rhs);
    assert!(
        !matches!(match_sig(&prepared.arena, prev), SigMatch::FloatCast(_)),
        "integer recursive feedback should not be promoted to float before SIGABS"
    );
    assert_eq!(match_sig(&prepared.arena, inc), SigMatch::Int(1));
}

/// The operands of `prev + 1`, in either order: normalization joins the
/// state term `prev` last, so the sum is `1 + prev` (DIFF-BEH-023).
fn feedback_and_increment(
    arena: &tlib::TreeArena,
    lhs: signals::SigId,
    rhs: signals::SigId,
) -> (signals::SigId, signals::SigId) {
    if matches!(match_sig(arena, lhs), SigMatch::Int(_)) {
        (rhs, lhs)
    } else {
        (lhs, rhs)
    }
}
#[test]
fn recursive_fixpoint_recomputes_body_types_after_real_widening() {
    let mut arena = tlib::TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let counter_body = {
        let mut b = SigBuilder::new(&mut arena);
        let counter_proj = b.proj(0, self_ref);
        let counter_prev = b.delay1(counter_proj);
        let one = b.int(1);
        b.binop(BinOp::Add, counter_prev, one)
    };
    let amp_body = {
        let mut b = SigBuilder::new(&mut arena);
        let amp_proj = b.proj(1, self_ref);
        let amp_prev = b.delay1(amp_proj);
        let half = b.real(0.5);
        b.binop(BinOp::Add, amp_prev, half)
    };
    let gated_body = {
        let mut b = SigBuilder::new(&mut arena);
        let amp_proj = b.proj(1, self_ref);
        let counter_proj = b.proj(0, self_ref);
        let amp_prev = b.delay1(amp_proj);
        let counter_prev = b.delay1(counter_proj);
        let period = b.int(128);
        let zero = b.int(0);
        let one = b.int(1);
        let rem = b.binop(BinOp::Rem, counter_prev, period);
        let eq = b.binop(BinOp::Eq, rem, zero);
        let gate = b.binop(BinOp::Sub, one, eq);
        b.binop(BinOp::Mul, amp_prev, gate)
    };
    let nil = arena.nil();
    let tail2 = arena.cons(gated_body, nil);
    let tail1 = arena.cons(amp_body, tail2);
    let body_list = arena.cons(counter_body, tail1);
    let group = de_bruijn_rec(&mut arena, body_list);
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(2, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("recursive real widening should converge");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Real));
}
