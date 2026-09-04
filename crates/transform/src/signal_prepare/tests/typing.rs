//! `typing` group of the signal_prepare tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_prepare::{
    SimpleSigType, prepare_signals_for_fir, prepare_signals_for_fir_verified_with_origins,
};
use signals::{BinOp, SigBuilder, SigMatch, dump_sig_readable, match_sig};

#[test]
fn prepare_signals_for_fir_records_reduced_numeric_types() {
    let mut arena = tlib::TreeArena::new();
    let outputs = {
        let mut b = SigBuilder::new(&mut arena);
        let v0 = b.int(1);
        let v1 = b.int(2);
        let v2 = b.int(3);
        let waveform = b.waveform(&[v0, v1, v2]);
        let input = b.input(0);
        let read = b.rdtbl(waveform, input);
        let selector = b.int(1);
        let zero = b.real(0.0);
        let mix = b.select2(selector, read, zero);
        vec![waveform, read, mix]
    };

    let prepared = prepare_signals_for_fir(&arena, &outputs, &ui::UiProgram::empty())
        .expect("simple numeric typing should work");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Int));
    assert_eq!(prepared.ty(prepared.outputs[1]), Some(SimpleSigType::Int));
    assert_eq!(prepared.ty(prepared.outputs[2]), Some(SimpleSigType::Real));
}

#[test]
fn preparation_remaps_and_inherits_signal_origins_across_rewrites() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut builder = signals::SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let one = builder.int(1);
        builder.delay(input, one)
    };
    let mut origins = propagate::SignalOrigins::default();
    origins.record(output, output);

    let prepared = prepare_signals_for_fir_verified_with_origins(
        &arena,
        &[output],
        &ui::UiProgram::empty(),
        &origins,
        &crate::signal_prepare::PrepareOptions::default(),
    )
    .expect("provenance-aware preparation should succeed");

    let prepared_output = prepared.outputs()[0];
    assert!(matches!(
        signals::match_sig(prepared.arena(), prepared_output),
        signals::SigMatch::Delay1(_)
    ));
    assert_eq!(prepared.origins().origins_for(prepared_output), &[output]);
}
#[test]
fn prepare_signals_for_fir_keeps_sampling_frequency_in_delay_amounts_after_simplify() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let sr = b.fconst(ty, name, file);
        let half = b.real(0.5);
        let sr_real = b.float_cast(sr);
        let scaled = b.mul(half, sr_real);
        let divisor = b.real(440.0);
        let ratio = b.div(scaled, divisor);
        let amount = b.int_cast(ratio);
        b.delay(input, amount)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("delay amount using fSamplingFreq should prepare");

    let SigMatch::Delay(_, amount) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("prepared output should stay a delay");
    };
    assert!(
        subtree_contains_fconst(&prepared.arena, amount),
        "delay amount should keep its fSamplingFreq dependency after fast-lane simplify, got {}",
        dump_sig_readable(&prepared.arena, amount)
    );
}
#[test]
fn prepare_signals_for_fir_promotes_delay_amounts_to_int() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let amount = b.real(1.5);
        b.delay(input, amount)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("delay promotion should succeed");

    match match_sig(&prepared.arena, prepared.outputs[0]) {
        SigMatch::Delay1(inner) => {
            assert_eq!(match_sig(&prepared.arena, inner), SigMatch::Input(0));
        }
        SigMatch::Delay(_, amount) => match match_sig(&prepared.arena, amount) {
            SigMatch::IntCast(inner) => {
                assert_eq!(match_sig(&prepared.arena, inner), SigMatch::Real(1.5));
            }
            SigMatch::Int(1) => {}
            _ => panic!("delay amount should stay as IntCast(real(1.5)) or simplify to Int(1)"),
        },
        _ => panic!("promoted output should stay a delay-family node"),
    }
}
#[test]
fn prepare_signals_for_fir_promotes_select2_selector_and_mixed_branches() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let selector = b.input(0);
        let then_value = b.int(1);
        let else_value = b.input(1);
        b.select2(selector, then_value, else_value)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("select2 promotion should succeed");

    let SigMatch::Select2(selector, then_value, else_value) =
        match_sig(&prepared.arena, prepared.outputs[0])
    else {
        panic!("promoted output should stay SIGSELECT2");
    };
    let SigMatch::IntCast(selector_inner) = match_sig(&prepared.arena, selector) else {
        panic!("select2 selector should be promoted to SIGINTCAST");
    };
    assert_eq!(
        match_sig(&prepared.arena, selector_inner),
        SigMatch::Input(0)
    );
    assert_eq!(
        match_sig(&prepared.arena, then_value),
        SigMatch::Real(1.0),
        "mixed-typed branch should be promoted to real"
    );
    assert_eq!(match_sig(&prepared.arena, else_value), SigMatch::Input(1));
    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Real));
}
#[test]
fn prepare_signals_for_fir_recovers_shared_select2_selector_from_float_context() {
    let mut arena = tlib::TreeArena::new();
    let (arith_out, select_out) = {
        let mut b = SigBuilder::new(&mut arena);
        let input0 = b.input(0);
        let half = b.real(0.5);
        let cmp = b.lt(input0, half);
        let input1 = b.input(1);
        let arith = b.add(cmp, input1);
        let one = b.int(1);
        let zero = b.int(0);
        let sel = b.select2(cmp, one, zero);
        (arith, sel)
    };

    let prepared =
        prepare_signals_for_fir(&arena, &[arith_out, select_out], &ui::UiProgram::empty())
            .expect("shared comparison should promote in both arithmetic and select2 contexts");

    let SigMatch::Select2(selector, _, _) = match_sig(&prepared.arena, prepared.outputs[1]) else {
        panic!("second output should stay SIGSELECT2");
    };
    assert!(
        matches!(match_sig(&prepared.arena, selector), SigMatch::IntCast(_))
            || matches!(
                match_sig(&prepared.arena, selector),
                SigMatch::BinOp(
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne,
                    _,
                    _
                )
            ),
        "shared select2 selector should stay in an integer comparison domain"
    );
}
#[test]
fn prepare_signals_for_fir_recovers_shared_delay_amount_from_float_context() {
    let mut arena = tlib::TreeArena::new();
    let (arith_out, delay_out) = {
        let mut b = SigBuilder::new(&mut arena);
        let input0 = b.input(0);
        let half = b.real(0.5);
        let cmp = b.lt(input0, half);
        let input1 = b.input(1);
        let arith = b.add(cmp, input1);
        let input2 = b.input(2);
        let delay = b.delay(input2, cmp);
        (arith, delay)
    };

    let prepared =
        prepare_signals_for_fir(&arena, &[arith_out, delay_out], &ui::UiProgram::empty())
            .expect("shared comparison should promote in both arithmetic and delay contexts");

    let SigMatch::Delay(_, amount) = match_sig(&prepared.arena, prepared.outputs[1]) else {
        panic!("second output should stay SIGDELAY");
    };
    assert!(
        matches!(match_sig(&prepared.arena, amount), SigMatch::IntCast(_))
            || matches!(
                match_sig(&prepared.arena, amount),
                SigMatch::BinOp(
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne,
                    _,
                    _
                )
            ),
        "shared delay amount should stay in an integer comparison domain"
    );
}
#[test]
fn prepare_signals_for_fir_recovers_shared_rdtbl_index_from_float_context() {
    let mut arena = tlib::TreeArena::new();
    let (arith_out, table_out) = {
        let mut b = SigBuilder::new(&mut arena);
        let input0 = b.input(0);
        let half = b.real(0.5);
        let cmp = b.lt(input0, half);
        let input1 = b.input(1);
        let arith = b.add(cmp, input1);
        let v0 = b.real(0.0);
        let v1 = b.real(1.0);
        let waveform = b.waveform(&[v0, v1]);
        let table = b.rdtbl(waveform, cmp);
        (arith, table)
    };

    let prepared =
        prepare_signals_for_fir(&arena, &[arith_out, table_out], &ui::UiProgram::empty())
            .expect("shared comparison should promote in both arithmetic and rdtbl contexts");

    let SigMatch::RdTbl(_, index) = match_sig(&prepared.arena, prepared.outputs[1]) else {
        panic!("second output should stay SIGRDTBL");
    };
    assert!(
        matches!(match_sig(&prepared.arena, index), SigMatch::IntCast(_))
            || matches!(
                match_sig(&prepared.arena, index),
                SigMatch::BinOp(
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne,
                    _,
                    _
                )
            ),
        "shared table-read index should stay in an integer comparison domain"
    );
}
#[test]
fn prepare_signals_for_fir_recovers_shared_wrtbl_write_signal_from_float_context() {
    let mut arena = tlib::TreeArena::new();
    let (arith_out, table_out) = {
        let mut b = SigBuilder::new(&mut arena);
        let input0 = b.input(0);
        let half = b.real(0.5);
        let cmp = b.lt(input0, half);
        let input1 = b.input(1);
        let arith = b.add(cmp, input1);
        let size = b.int(8);
        let generator = b.int(0);
        let write_index = b.int(1);
        let table = b.wrtbl(size, generator, write_index, cmp);
        (arith, table)
    };

    let prepared =
        prepare_signals_for_fir(&arena, &[arith_out, table_out], &ui::UiProgram::empty())
            .expect("shared comparison should promote in both arithmetic and wrtbl contexts");

    let SigMatch::WrTbl(_, _, _, write_signal) = match_sig(&prepared.arena, prepared.outputs[1])
    else {
        panic!("second output should stay SIGWRTBL");
    };
    assert!(
        matches!(
            match_sig(&prepared.arena, write_signal),
            SigMatch::IntCast(_)
        ) || matches!(
            match_sig(&prepared.arena, write_signal),
            SigMatch::BinOp(
                BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne,
                _,
                _
            )
        ),
        "shared wrtbl write signal should stay in an integer comparison domain"
    );
}
#[test]
fn prepare_signals_for_fir_recovers_shared_zero_pad_amount_from_float_context() {
    let mut arena = tlib::TreeArena::new();
    let (arith_out, padded_out) = {
        let mut b = SigBuilder::new(&mut arena);
        let input0 = b.input(0);
        let half = b.real(0.5);
        let cmp = b.lt(input0, half);
        let input1 = b.input(1);
        let arith = b.add(cmp, input1);
        let input2 = b.input(2);
        let padded = b.zero_pad(input2, cmp);
        (arith, padded)
    };

    let prepared =
        prepare_signals_for_fir(&arena, &[arith_out, padded_out], &ui::UiProgram::empty())
            .expect("shared comparison should promote in both arithmetic and zero_pad contexts");

    let SigMatch::ZeroPad(_, amount) = match_sig(&prepared.arena, prepared.outputs[1]) else {
        panic!("second output should stay SIGZEROPAD");
    };
    assert!(
        matches!(match_sig(&prepared.arena, amount), SigMatch::IntCast(_))
            || matches!(
                match_sig(&prepared.arena, amount),
                SigMatch::BinOp(
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne,
                    _,
                    _
                )
            ),
        "shared zero-pad amount should stay in an integer comparison domain"
    );
}
#[test]
fn prepare_signals_for_fir_promotes_table_read_index_to_int() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let v0 = b.real(0.0);
        let v1 = b.real(1.0);
        let waveform = b.waveform(&[v0, v1]);
        let index = b.input(0);
        b.rdtbl(waveform, index)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("table promotion should succeed");

    let SigMatch::RdTbl(_, index) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("promoted output should stay SIGRDTBL");
    };
    // The audio-input index interval ([-1, 1]) cannot prove the access
    // in-bounds, so the check-table pass (step 2.10b) wraps the promoted
    // index in the full clamp `max(0, min(index, size-1))`.
    let SigMatch::Max(zero, inner) = match_sig(&prepared.arena, index) else {
        panic!("unprovable table read index should be clamped by -ct");
    };
    assert_eq!(match_sig(&prepared.arena, zero), SigMatch::Int(0));
    let SigMatch::Min(cast, upper) = match_sig(&prepared.arena, inner) else {
        panic!("clamp should be max(0, min(index, size-1))");
    };
    assert_eq!(match_sig(&prepared.arena, upper), SigMatch::Int(1));
    let SigMatch::IntCast(raw) = match_sig(&prepared.arena, cast) else {
        panic!("table read index should be promoted to SIGINTCAST");
    };
    assert_eq!(match_sig(&prepared.arena, raw), SigMatch::Input(0));
}
#[test]
fn prepare_signals_for_fir_promotes_real_mul_operands_before_binop() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let gate_init = b.int(0);
        let gate_next = b.int(1);
        let gate = b.prefix(gate_init, gate_next);
        let carrier_init = b.real(0.0);
        let carrier_next = b.real(0.5);
        let carrier = b.prefix(carrier_init, carrier_next);
        let inner = b.binop(BinOp::Mul, carrier, gate);
        b.binop(BinOp::Mul, inner, gate)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("mixed real/int multiplication should prepare");

    let SigMatch::BinOp(BinOp::Mul, left, right) = match_sig(&prepared.arena, prepared.outputs[0])
    else {
        panic!("prepared output should stay SIGBINOP(Mul, ...)");
    };
    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Real));
    assert_eq!(prepared.ty(left), Some(SimpleSigType::Real));
    assert!(
        matches!(
            prepared.ty(right),
            Some(SimpleSigType::Int) | Some(SimpleSigType::Real)
        ),
        "outer multiplication rhs should stay typed after simplification"
    );

    assert!(
        prepared.ty(left).is_some(),
        "simplified lhs subtree should stay typed for FIR lowering"
    );
}
#[test]
fn prepare_signals_for_fir_uses_foreign_function_return_type() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let ty_int = arena.int(0);
        let ty_real = arena.int(1);
        let incfile = arena.symbol("<math.h>");
        let libfile = arena.symbol("\"\"");
        let name_f32 = arena.symbol("isnanf");
        let name_f64 = arena.symbol("isnan");
        let name_f80 = arena.symbol("isnanl");
        let name_fx = arena.symbol("isnanfx");
        let nil = arena.nil();
        let names = {
            let tail0 = arena.cons(name_fx, nil);
            let tail1 = arena.cons(name_f80, tail0);
            let tail2 = arena.cons(name_f64, tail1);
            arena.cons(name_f32, tail2)
        };
        let arg_types = arena.cons(ty_real, nil);
        let payload = arena.cons(names, arg_types);
        let signature = arena.cons(ty_int, payload);
        let ff_tag = arena.intern_tag("FFUN");
        let ff = arena.intern(tlib::NodeKind::Tag(ff_tag), &[signature, incfile, libfile]);
        let input0 = {
            let mut b = SigBuilder::new(&mut arena);
            b.input(0)
        };
        let args = arena.cons(input0, nil);
        let mut b = SigBuilder::new(&mut arena);
        b.ffun(ff, args)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("foreign function result type should prepare");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Int));
}

// C++ regression f81491f8 (2026-09-04): `fmod(int, int)` typed int made the
// WASM backend call `$fmodf` on i32 operands. In faust-rs the promotion pass
// casts both `fmod` operands to real by primitive (`promote_real_binary`),
// independently of the node's nature, and the promotion invariant checker
// refuses a prepared `fmod` whose operands or result are not real.
#[test]
fn prepare_signals_for_fir_promotes_fmod_int_operands_to_real() {
    let mut arena = tlib::TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let x = b.int_cast(input);
        let y = b.int(1000);
        b.fmod(x, y)
    };

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("fmod on integer operands should prepare");

    let SigMatch::Fmod(left, right) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!(
            "prepared output should stay SIGFMOD, got {}",
            dump_sig_readable(&prepared.arena, prepared.outputs[0])
        );
    };
    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Real));
    assert_eq!(prepared.ty(left), Some(SimpleSigType::Real));
    assert_eq!(prepared.ty(right), Some(SimpleSigType::Real));
    assert!(
        matches!(match_sig(&prepared.arena, left), SigMatch::FloatCast(_)),
        "int() operand must be wrapped in an explicit float cast, got {}",
        dump_sig_readable(&prepared.arena, left)
    );
    assert!(
        matches!(
            match_sig(&prepared.arena, right),
            SigMatch::FloatCast(_) | SigMatch::Real(_)
        ),
        "integer literal operand must become real, got {}",
        dump_sig_readable(&prepared.arena, right)
    );
}

/// The `so.loop` / `it.raise_modulo` shape: an int-seeded recursion wrapped by
/// `fmod`. The recursive state must come out real, never an int accumulator
/// feeding the C `fmod`.
#[test]
fn prepare_signals_for_fir_keeps_fmod_recursion_real() {
    let mut arena = tlib::TreeArena::new();
    let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let prev = b.proj(0, self_ref);
        let one = b.int(1);
        let next = b.binop(BinOp::Add, prev, one);
        let modulus = b.int(1000);
        b.fmod(next, modulus)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = tlib::de_bruijn_rec(&mut arena, body_list);
    let output = SigBuilder::new(&mut arena).proj(0, group);

    let prepared = prepare_signals_for_fir(&arena, &[output], &ui::UiProgram::empty())
        .expect("int-seeded fmod recursion should prepare");

    assert_eq!(prepared.ty(prepared.outputs[0]), Some(SimpleSigType::Real));

    let SigMatch::Proj(0, prepared_group) = match_sig(&prepared.arena, prepared.outputs[0]) else {
        panic!("prepared output should remain a projection");
    };
    let (_, prepared_body_list) =
        tlib::match_sym_rec(&prepared.arena, prepared_group).expect("symbolic recursion expected");
    let prepared_body = prepared
        .arena
        .hd(prepared_body_list)
        .expect("prepared recursion body head");
    let SigMatch::Fmod(left, right) = match_sig(&prepared.arena, prepared_body) else {
        panic!(
            "recursion body should stay SIGFMOD, got {}",
            dump_sig_readable(&prepared.arena, prepared_body)
        );
    };
    assert_eq!(prepared.ty(prepared_body), Some(SimpleSigType::Real));
    assert_eq!(prepared.ty(left), Some(SimpleSigType::Real));
    assert_eq!(prepared.ty(right), Some(SimpleSigType::Real));
}
