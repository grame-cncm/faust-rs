//! `verify` group of the signal_prepare tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use crate::signal_prepare::{SignalPrepareError, SimpleSigType, prepare_signals_for_fir};
use signals::{BinOp, BlockRevPolicy, SigBuilder, SigMatch, match_sig};
use tlib::{de_bruijn_rec, de_bruijn_ref};
use ui::{ControlKind, ControlSpec};

#[test]
fn prepared_signals_verify_rejects_missing_reduced_type_entry() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        b.input(0)
    };

    let mut prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("baseline preparation should succeed");
    prepared.types.remove(&prepared.outputs[0]);

    let err = prepared
        .verify(&ui::UiProgram::empty())
        .expect_err("missing reduced type should fail verification");
    let SignalPrepareError::Validation(message) = err else {
        panic!("expected validation error");
    };
    assert!(message.contains("missing reduced type annotation"));
}
#[test]
fn prepared_signals_verify_rejects_out_of_range_recursive_projection() {
    let mut arena = tlib::TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        b.delay1(feedback)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let mut prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("baseline preparation should succeed");
    let SigMatch::Proj(_, prepared_group) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("prepared output should stay a projection");
    };
    let old_output = prepared.outputs[0];
    let old_reduced = prepared.ty(old_output).expect("baseline reduced type");
    let old_full = prepared
        .sig_ty(old_output)
        .cloned()
        .expect("baseline full type");
    let bad_output = {
        let mut b = SigBuilder::new(&mut prepared.arena);
        b.proj(3, prepared_group)
    };
    prepared.outputs[0] = bad_output;
    prepared.types.insert(bad_output, old_reduced);
    prepared.sig_types.insert(bad_output, old_full);

    let err = prepared
        .verify(&ui::UiProgram::empty())
        .expect_err("out-of-range recursive projection should fail verification");
    let SignalPrepareError::Validation(message) = err else {
        panic!("expected validation error");
    };
    assert!(message.contains("out of range"));
}
#[test]
fn prepared_signals_verify_rejects_block_reverse_ad_proj_out_of_range() {
    // The carrier exposes M + N outputs (1 primal + 2 seeds = 3 here).
    // A `Proj(7, _)` must be caught as out-of-range by the verifier.
    let mut arena = tlib::TreeArena::new();
    let primal_proj = {
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let y = b.input(1);
        let primal = b.add(x, y);
        let one = b.real(1.0);
        let carrier = b.block_reverse_ad(&[primal], &[x, y], &[one], BlockRevPolicy::TapeFull);
        b.proj(0, carrier)
    };

    let mut prepared = prepare_signals_for_fir(&arena, &[primal_proj], &ui::UiProgram::empty())
        .expect("baseline preparation should succeed");
    let SigMatch::Proj(_, prepared_group) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("prepared output should remain a Proj");
    };
    let old_output = prepared.outputs[0];
    let old_reduced = prepared.ty(old_output).expect("baseline reduced type");
    let old_full = prepared
        .sig_ty(old_output)
        .cloned()
        .expect("baseline full type");
    let bad_output = {
        let mut b = SigBuilder::new(&mut prepared.arena);
        b.proj(7, prepared_group)
    };
    prepared.outputs[0] = bad_output;
    prepared.types.insert(bad_output, old_reduced);
    prepared.sig_types.insert(bad_output, old_full);

    let err = prepared
        .verify(&ui::UiProgram::empty())
        .expect_err("out-of-range BlockReverseAD projection should fail verification");
    let SignalPrepareError::Validation(message) = err else {
        panic!("expected validation error");
    };
    assert!(
        message.contains("out of range for BlockReverseAD"),
        "unexpected message: {message}"
    );
}
#[test]
fn prepared_signals_verify_rejects_missing_ui_control_reference() {
    let mut signal_arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut signal_arena);
        b.button(0)
    };

    let mut ui = ui::UiProgram::empty();
    ui.controls.push(ControlSpec {
        id: 0,
        kind: ControlKind::Button,
        label: "button".to_owned(),
        metadata: Vec::new(),
        range: None,
        source_node: None,
    });

    let prepared = prepare_signals_for_fir(&signal_arena, &[output], &ui)
        .expect("baseline preparation with matching UI should succeed");
    let err = prepared
        .verify(&ui::UiProgram::empty())
        .expect_err("missing UI control registry entry should fail verification");
    let SignalPrepareError::Validation(message) = err else {
        panic!("expected validation error");
    };
    assert!(message.contains("missing UI control id 0"));
}
// ── Promotion-invariant / one-sample-delay boundary checks ────────────────────
//
// These exercise `verify_promotion_invariant` directly: the public `prepare`
// path always establishes `P`/`D1`, so a violation can only be constructed by
// handing the checker a deliberately inconsistent (arena, reduced-type) pair.

#[test]
fn verify_promotion_invariant_rejects_non_canonical_one_sample_delay() {
    use std::collections::HashMap;
    let mut arena = tlib::TreeArena::new();
    let (delay, value, one) = {
        let mut b = SigBuilder::new(&mut arena);
        let v = b.input(0);
        let one = b.int(1);
        let d = b.delay(v, one);
        (d, v, one)
    };
    let mut types = HashMap::new();
    types.insert(value, SimpleSigType::Real);
    types.insert(one, SimpleSigType::Int);
    types.insert(delay, SimpleSigType::Real);

    let err = crate::signal_prepare::verify_promotion_invariant(&arena, &types, delay)
        .expect_err("Delay(_, 1) must be rejected as a non-canonical one-sample delay (D1)");
    assert!(matches!(err, SignalPrepareError::Validation(_)));
}
#[test]
fn verify_promotion_invariant_rejects_mixed_domain_arithmetic() {
    use std::collections::HashMap;
    let mut arena = tlib::TreeArena::new();
    let (node, lhs, rhs) = {
        let mut b = SigBuilder::new(&mut arena);
        let l = b.real(1.0);
        let r = b.int(2);
        let n = b.binop(BinOp::Add, l, r);
        (n, l, r)
    };
    // A real-result Add whose right operand is still Int with no FloatCast —
    // exactly the unpromoted shape `P` forbids.
    let mut types = HashMap::new();
    types.insert(lhs, SimpleSigType::Real);
    types.insert(rhs, SimpleSigType::Int);
    types.insert(node, SimpleSigType::Real);

    let err = crate::signal_prepare::verify_promotion_invariant(&arena, &types, node)
        .expect_err("arithmetic BinOp with a non-promoted Int operand must be rejected (P)");
    assert!(matches!(err, SignalPrepareError::Validation(_)));
}
#[test]
fn verify_promotion_invariant_accepts_consistent_arithmetic() {
    use std::collections::HashMap;
    let mut arena = tlib::TreeArena::new();
    let (node, lhs, rhs) = {
        let mut b = SigBuilder::new(&mut arena);
        let l = b.real(1.0);
        let r = b.real(2.0);
        let n = b.binop(BinOp::Add, l, r);
        (n, l, r)
    };
    let mut types = HashMap::new();
    types.insert(lhs, SimpleSigType::Real);
    types.insert(rhs, SimpleSigType::Real);
    types.insert(node, SimpleSigType::Real);

    crate::signal_prepare::verify_promotion_invariant(&arena, &types, node)
        .expect("a domain-consistent real Add must satisfy the promotion invariant");
}
