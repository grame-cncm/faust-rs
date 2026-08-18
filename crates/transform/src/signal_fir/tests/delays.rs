//! `delays` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_fir::{
    SignalFirErrorCode, SignalFirOptions, SignalFirRequest, compile_signals_to_fir_fastlane,
    delay::{DelayOptions, plan_delays},
};
use fir::{FirBinOp, FirMatch, FirType, match_fir};
use signals::{BinOp, SigBuilder};
use tlib::TreeArena;
use ui::{ControlKind, ControlRange};

#[test]
fn delay_plan_keeps_hash_consed_carrier_state_per_clock_context() {
    let mut arena = TreeArena::new();
    let (carried, clocked0, clocked1) = {
        let mut b = SigBuilder::new(&mut arena);
        let carried = b.int(1);
        let delayed = b.delay1(carried);
        let env0 = b.clock_env_token(0);
        let env1 = b.clock_env_token(1);
        (carried, b.clocked(env0, delayed), b.clocked(env1, delayed))
    };
    let plan = plan_delays(
        &arena,
        &std::collections::HashMap::new(),
        &[clocked0, clocked1],
        &DelayOptions::default(),
        None,
    )
    .expect("clock-context delay planning should succeed");

    assert_eq!(plan.lines.get(&(carried, Some(0))), Some(&1));
    assert_eq!(plan.lines.get(&(carried, Some(1))), Some(&1));
    assert_eq!(plan.lines.len(), 2, "sibling domains need distinct lines");
}
#[test]
fn delay_plan_keeps_wrapper_clock_state_in_the_parent_context() {
    let mut arena = TreeArena::new();
    let (clock_carried, payload_carried, wrapper) = {
        let mut b = SigBuilder::new(&mut arena);
        let clock_carried = b.int(1);
        let payload_carried = b.int(2);
        let clock = b.delay1(clock_carried);
        let payload = b.delay1(payload_carried);
        let env = b.clock_env_token(0);
        let guarded_clock = b.clocked(env, clock);
        let guarded_payload = b.clocked(env, payload);
        let held_payload = b.perm_var(guarded_payload);
        (
            clock_carried,
            payload_carried,
            b.on_demand(&[guarded_clock, held_payload]),
        )
    };
    let plan = plan_delays(
        &arena,
        &std::collections::HashMap::new(),
        &[wrapper],
        &DelayOptions::default(),
        None,
    )
    .expect("clock-wrapper delay planning should succeed");

    assert_eq!(plan.lines.get(&(clock_carried, None)), Some(&1));
    assert_eq!(plan.lines.get(&(payload_carried, Some(0))), Some(&1));
    assert_eq!(plan.lines.len(), 2);
}
#[test]
fn delay1_lowers_to_struct_state_declaration_and_update() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let i0 = b.input(0);
        b.delay1(i0)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("delay1 should lower with explicit state");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::DeclareVar { .. })),
        "delay state should create struct declaration"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    // With max_copy_delay >= 1 (default), Delay1 uses the Shift strategy which
    // writes to an array slot (StoreTable), not a named variable (StoreVar).
    assert!(
        stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::StoreTable { .. })),
        "delay state should create compute update store into array slot"
    );
}
#[test]
fn int_delay1_uses_int32_state_slot() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let one = b.int(1);
        b.delay1(one)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("integer delay1 should lower");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                typ: FirType::Int32,
                ..
            }
        )),
        "integer delay state should allocate an Int32 slot"
    );
}
#[test]
fn fixed_delay_two_uses_unrolled_shift_copies() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let two = b.int(2);
        b.delay(in0, two)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("delay 2 should lower");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let delay_name = struct_items
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, 3),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some(name),
            _ => None,
        })
        .expect("delay 2 should allocate one size-3 shift buffer");
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "delay 2 should not allocate fIOTA"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    let delay_stores = stmts
        .iter()
        .filter(|id| {
            matches!(
                match_fir(&out.store, **id),
                FirMatch::StoreTable { ref name, .. } if name == &delay_name
            )
        })
        .count();
    assert_eq!(
        delay_stores, 3,
        "delay 2 should emit one immediate write and two unrolled shift copies"
    );
    assert!(
        !stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::ForLoop { .. })),
        "delay 2 should not emit a shift loop"
    );
}
#[test]
fn fixed_delay_two_runs_shift_copies_after_output_store() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let two = b.int(2);
        b.delay(in0, two)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("delay 2 should lower");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let delay_name = struct_items
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, 3),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some(name),
            _ => None,
        })
        .expect("delay 2 should allocate one size-3 shift buffer");

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };

    let output_pos = stmts
        .iter()
        .position(|id| {
            matches!(
                match_fir(&out.store, *id),
                FirMatch::StoreTable { ref name, .. } if name == "output0"
            )
        })
        .expect("compute loop should include one output store");
    let delay_positions: Vec<usize> = stmts
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            if matches!(
                match_fir(&out.store, *id),
                FirMatch::StoreTable { ref name, .. } if name == &delay_name
            ) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        delay_positions.len(),
        3,
        "delay 2 should emit three delay stores"
    );
    assert!(
        delay_positions[0] < output_pos,
        "shift delay immediate write should occur before the output store"
    );
    assert!(
        delay_positions[1] > output_pos && delay_positions[2] > output_pos,
        "shift delay copy updates should occur after the output store"
    );
}
#[test]
fn fixed_delay_three_uses_shift_loop() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let three = b.int(3);
        b.delay(in0, three)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("delay 3 should lower");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, 4),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec")
        )),
        "delay 3 should allocate one size-4 shift buffer"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "delay 3 should not allocate fIOTA"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::ForLoop { .. })),
        "delay 3 should emit a reverse shift loop"
    );
}
#[test]
fn delay1_and_fixed_delay_share_one_prescanned_delay_line() {
    let mut arena = TreeArena::new();
    let (sig0, sig1) = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let delay1 = b.delay1(in0);
        let two = b.int(2);
        let delay2 = b.delay(in0, two);
        (delay1, delay2)
    };
    let out =
        compile_fastlane_without_ui(&arena, &[sig0, sig1], 1, 2, &SignalFirOptions::default())
            .expect("Delay1 and fixed delay should lower with one shared prescanned line");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };

    let delay_sizes: Vec<usize> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some(size),
            _ => None,
        })
        .collect();
    assert_eq!(
        delay_sizes,
        [3],
        "Delay1 and Delay(x, 2) should share one size-3 shift buffer"
    );
}
#[test]
fn fixed_delay_lowers_to_struct_array_and_iota_updates() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let n3 = b.int(3);
        b.delay(in0, n3)
    };
    // Force CircularPow2 strategy by setting max_copy_delay=0 so that even a
    // tiny delay uses the ring-buffer + fIOTA path that this test verifies.
    let out = compile_fastlane_without_ui(
        &arena,
        &[sig0],
        1,
        1,
        &SignalFirOptions {
            max_copy_delay: 0,
            ..SignalFirOptions::default()
        },
    )
    .expect("constant fixed delay should lower");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };

    let delay_decl = struct_items
        .iter()
        .find(|id| {
            matches!(
                match_fir(&out.store, **id),
                FirMatch::DeclareVar {
                    ref name,
                    typ: FirType::Array(_, 4),
                    ..
                } if name.starts_with("fVec") || name.starts_with("iVec")
            )
        })
        .copied()
        .expect("constant delay should allocate a size-4 delay line");
    let FirMatch::DeclareVar {
        name: delay_name,
        typ,
        ..
    } = match_fir(&out.store, delay_decl)
    else {
        panic!("delay declaration expected");
    };
    match typ {
        FirType::Array(inner, 4) => assert_eq!(*inner, FirType::Float32),
        other => panic!("unexpected delay declaration type: {other:?}"),
    }
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Int32,
                ..
            } if name == "fIOTA"
        )),
        "fixed delay should declare persistent fIOTA state"
    );

    let clear_body = find_decl_fun_body(&out.store, functions, "instanceClear");
    let FirMatch::Block(clear_stmts) = match_fir(&out.store, clear_body) else {
        panic!("instanceClear block expected");
    };
    assert!(
        clear_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name == "fIOTA"
        )),
        "instanceClear should reset fIOTA"
    );
    assert!(
        clear_stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::SimpleForLoop { .. })),
        "instanceClear should zero the delay-line array"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable { ref name, .. } if name == &delay_name
        )),
        "compute loop should write the current sample into the delay line"
    );
    let write_index = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, index, .. } if name == delay_name => Some(index),
            _ => None,
        })
        .expect("compute loop should include one delay-line write");
    let FirMatch::BinOp {
        op: FirBinOp::And,
        lhs,
        rhs,
        ..
    } = match_fir(&out.store, write_index)
    else {
        panic!("delay write index should be masked");
    };
    assert!(matches!(
        match_fir(&out.store, lhs),
        FirMatch::LoadVar { ref name, .. } if name == "fIOTA"
    ));
    assert!(matches!(
        match_fir(&out.store, rhs),
        FirMatch::Int32 { value: 3, .. }
    ));
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name == "fIOTA"
        )),
        "compute loop should increment fIOTA once per sample"
    );

    let stored_value = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("compute should include one output store");
    let inner = unwrap_output_cast(&out.store, stored_value);
    let FirMatch::LoadTable { name, index, .. } = match_fir(&out.store, inner) else {
        panic!("fixed delay output should lower to a delay-line read");
    };
    assert_eq!(name, delay_name);
    let FirMatch::BinOp {
        op: FirBinOp::And,
        lhs,
        rhs,
        ..
    } = match_fir(&out.store, index)
    else {
        panic!("delay index should be masked");
    };
    assert!(matches!(
        match_fir(&out.store, rhs),
        FirMatch::Int32 { value: 3, .. }
    ));
    assert!(matches!(
        match_fir(&out.store, lhs),
        FirMatch::BinOp {
            op: FirBinOp::Sub,
            ..
        }
    ));
}
#[test]
fn circular_delay_runs_iota_bump_after_output_store() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let three = b.int(3);
        b.delay(in0, three)
    };
    let out = compile_fastlane_without_ui(
        &arena,
        &[sig0],
        1,
        1,
        &SignalFirOptions {
            max_copy_delay: 0,
            ..SignalFirOptions::default()
        },
    )
    .expect("circular delay should lower");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };

    let output_pos = stmts
        .iter()
        .position(|id| {
            matches!(
                match_fir(&out.store, *id),
                FirMatch::StoreTable { ref name, .. } if name == "output0"
            )
        })
        .expect("compute loop should include one output store");
    let iota_bump_pos = stmts
        .iter()
        .position(|id| {
            matches!(
                match_fir(&out.store, *id),
                FirMatch::StoreVar { ref name, .. } if name == "fIOTA"
            )
        })
        .expect("compute loop should include one fIOTA bump");

    assert!(
        iota_bump_pos > output_pos,
        "circular delay sample-end update should run after the output store"
    );
}
#[test]
fn fixed_delay_at_mcd_boundary_uses_circular_pow2() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let four = b.int(4);
        b.delay(in0, four)
    };
    let out = compile_fastlane_without_ui(
        &arena,
        &[sig0],
        1,
        1,
        &SignalFirOptions {
            max_copy_delay: 4,
            ..SignalFirOptions::default()
        },
    )
    .expect("delay equal to mcd should lower");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, 8),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec")
        )),
        "delay equal to mcd should use a power-of-two ring buffer"
    );
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "delay equal to mcd should allocate fIOTA"
    );
}
#[test]
fn delay1_and_mcd_boundary_delay_share_circular_line() {
    let mut arena = TreeArena::new();
    let (sig0, sig1) = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let delay1 = b.delay1(in0);
        let four = b.int(4);
        let delay4 = b.delay(in0, four);
        (delay1, delay4)
    };
    let out = compile_fastlane_without_ui(
        &arena,
        &[sig0, sig1],
        1,
        2,
        &SignalFirOptions {
            max_copy_delay: 4,
            ..SignalFirOptions::default()
        },
    )
    .expect("Delay1 and Delay(x, mcd) should share a circular line");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, 8),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec")
        )),
        "shared line should use an 8-slot power-of-two buffer"
    );
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "shared mcd-boundary line should allocate fIOTA"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        !stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::ForLoop { .. })),
        "shared circular line should not emit a shift loop for Delay1"
    );
}
#[test]
fn fixed_delay_at_dlt_boundary_uses_if_wrapping() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let eight = b.int(8);
        b.delay(in0, eight)
    };
    let out = compile_fastlane_without_ui(
        &arena,
        &[sig0],
        1,
        1,
        &SignalFirOptions {
            max_copy_delay: 4,
            delay_line_threshold: 8,
            ..SignalFirOptions::default()
        },
    )
    .expect("delay equal to dlt should lower");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let counter_name = struct_items
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Int32,
                ..
            } if name.starts_with("fIdx") => Some(name),
            _ => None,
        })
        .expect("delay equal to dlt should declare an if-wrapping counter");
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, 9),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec")
        )),
        "delay equal to dlt should use an exact-size buffer"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "if-wrapping delay should not allocate fIOTA"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name == &counter_name
        )),
        "if-wrapping delay should update the per-line counter at end of sample"
    );

    let stored_value = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("compute should include one output store");
    let inner = unwrap_output_cast(&out.store, stored_value);
    let FirMatch::LoadTable { index, .. } = match_fir(&out.store, inner) else {
        panic!("if-wrapping output should lower to a delay-line read");
    };
    assert!(
        matches!(match_fir(&out.store, index), FirMatch::Select2 { .. }),
        "if-wrapping delay read should use a wrapped select2 index"
    );
}
#[test]
fn zero_delay_uses_fast_path_without_delay_resources() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let n0 = b.int(0);
        b.delay(in0, n0)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("zero delay should lower through fast path");

    let FirMatch::Module {
        dsp_struct,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "zero delay should not allocate fIOTA"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. }
                if name.starts_with("fVec") || name.starts_with("iVec")
        )),
        "zero delay should not allocate a delay line"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    let stored_value = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("compute should include one output store");
    let inner = unwrap_output_cast(&out.store, stored_value);
    assert!(
        matches!(
            match_fir(&out.store, inner),
            FirMatch::Cast { .. } | FirMatch::LoadTable { .. }
        ),
        "zero delay should lower to the carried value without delay-line readback"
    );
}
#[test]
fn variable_delay_with_audio_input_amount_uses_tinput_interval() {
    // `process = _ : @(_)` — audio input as delay amount.
    // TINPUT has interval(-1, 1), so hi=1 → delay line next_pow2(2) = 2.
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let amount = b.input(1);
        b.delay(in0, amount)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 2, 1, &SignalFirOptions::default())
        .expect("audio-input delay amount must be accepted");
    // Verify a 2-sample delay line was allocated.
    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("expected Module");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let delay_sizes: Vec<usize> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some(size),
            _ => None,
        })
        .collect();
    assert_eq!(delay_sizes, [2], "expected a single 2-sample delay line");
}
#[test]
fn variable_delay_with_strictly_negative_hi_is_rejected() {
    // A slider shifted so that its interval is entirely negative (hi < 0)
    // must be rejected — C++ `checkDelayInterval` rejects `hi() < 0`.
    // e.g. hslider("d",10,0,100,1) - 200  →  interval [-200, -100], hi=-100.
    let ui = one_control_ui(
        ControlKind::HSlider,
        "d",
        Some(ControlRange {
            init: 10.0,
            min: 0.0,
            max: 100.0,
            step: 1.0,
        }),
        false,
        false,
    );
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let slider = b.hslider(0);
        let offset = b.real(200.0);
        let shifted = b.binop(BinOp::Sub, slider, offset);
        b.delay(in0, shifted)
    };
    let err = compile_signals_to_fir_fastlane(&SignalFirRequest::new(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    ))
    .expect_err("slider with hi<0 interval must be rejected as delay amount");
    assert_eq!(err.code(), SignalFirErrorCode::UnsupportedSignalNode);
}
#[test]
fn variable_delay_with_slider_bound_lowers_to_interval_sized_delay_line() {
    // process = @(100) : @(hslider("Delay",10,0,100,1));
    // The outer @(100) is constant; the inner @(hslider(...)) is variable
    // with interval [0,100].  Both should lower: the slider delay uses a
    // delay line sized to next_power_of_two(101) = 128.
    let ui = one_control_ui(
        ControlKind::HSlider,
        "Delay",
        Some(ControlRange {
            init: 10.0,
            min: 0.0,
            max: 100.0,
            step: 1.0,
        }),
        false,
        false,
    );
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let n100 = b.int(100);
        let delayed_fixed = b.delay(in0, n100);
        let raw_slider = b.hslider(0);
        let slider_amount = b.int_cast(raw_slider);
        b.delay(delayed_fixed, slider_amount)
    };
    let out = compile_signals_to_fir_fastlane(&SignalFirRequest::new(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    ))
    .expect("variable delay with bounded slider should lower successfully");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    // Expect two delay-line DeclareVar(Array) entries: one for @(100)
    // (size=128) and one for the slider-bounded @(hslider) (size=128 too,
    // since next_power_of_two(101)=128).
    let delay_line_sizes: Vec<usize> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some(size),
            _ => None,
        })
        .collect();
    assert_eq!(
        delay_line_sizes.len(),
        2,
        "expected two delay-line arrays (one fixed @100, one slider-bounded), got {:?}",
        delay_line_sizes
    );
    let max_size = delay_line_sizes.iter().copied().max().unwrap_or(0);
    assert!(
        max_size >= 128,
        "slider delay line must be >= 128 samples (next_power_of_two(101)), got {max_size}"
    );
}
#[test]
fn variable_delay_with_zero_hi_interval_uses_zero_delay_passthrough() {
    // @(hslider("Delay1",10,0,100,1) - 100) → interval [-100, 0], hi=0.
    // C++ checkDelayInterval rejects only hi < 0, so hi==0 is accepted
    // and produces a zero-delay (passthrough).  Regression for the
    // `<= 0.0` vs `< 0.0` boundary condition in variable_delay_max_bound.
    let ui = one_control_ui(
        ControlKind::HSlider,
        "Delay1",
        Some(ControlRange {
            init: 10.0,
            min: 0.0,
            max: 100.0,
            step: 1.0,
        }),
        false,
        false,
    );
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let n100_fixed = b.int(100);
        let stage1 = b.delay(in0, n100_fixed);
        // hslider - 100  → interval [0,100] - 100 = [-100, 0], hi == 0
        let slider = b.hslider(0);
        let offset = b.real(100.0);
        let shifted = b.binop(BinOp::Sub, slider, offset);
        b.delay(stage1, shifted)
    };
    compile_signals_to_fir_fastlane(&SignalFirRequest::new(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    ))
    .expect("delay with hi=0 interval should lower as zero-delay (passthrough)");
}
