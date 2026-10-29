//! `recursion` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_fir::SignalFirOptions;
use crate::signal_prepare::{SimpleSigType, prepare_signals_for_fir};
use fir::{AccessType, FirMatch, FirType, match_fir};
use signals::{BinOp, SigBuilder};
use tlib::{TreeArena, de_bruijn_rec, de_bruijn_ref, match_sym_rec};
use ui::UiProgram;

#[test]
fn rec_proj_lowers_without_placeholder_nodes() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let c0 = b.real(0.1);
        // `~` feeds back through an explicit one-sample delay.
        let back_reference = b.proj(0, self_ref);
        let feedback = b.delay1(back_reference);
        // body = input(0) + 0.1 + feedback  (uses recursion so proj is emitted)
        let sum = b.binop(BinOp::Add, in0, c0);
        b.add(sum, feedback)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("rec/proj signal should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        1,
        1,
        &SignalFirOptions::default(),
    )
    .expect("Step 2C.2 should support rec/proj real lowering");

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
                typ: FirType::Float32 | FirType::Float64,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fRec")
        )),
        "simple unary rec/proj should allocate one scalar recursion state field"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, _),
                ..
            }
                if name.starts_with("fRec") || name.starts_with("iRec")
        )),
        "simple unary rec/proj should not allocate an array-backed recursion carrier"
    );
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("fRecCur")
        )),
        "simple unary rec/proj should materialize the current sample as a stack-local binding"
    );
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fRec")
        )),
        "simple unary rec/proj should finalize the scalar recursion state after output"
    );
    assert!(
        stmts
            .iter()
            .all(|id| !matches!(match_fir(&out.store, *id), FirMatch::Cast { .. })),
        "rec/proj lowering should not need cast wrappers around recursive array accesses"
    );
}
#[test]
fn reverse_time_rec_projection_lowers_to_reverse_sample_loop() {
    let mut arena = TreeArena::new();
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
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let reverse_group = b.reverse_time_rec(group);
        b.proj(0, reverse_group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("reverse-time rec/proj signal should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        1,
        1,
        &SignalFirOptions::default(),
    )
    .expect("ReverseTimeRec projection should lower through fast-lane");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    assert!(
        find_compute_simple_loop_reverse_flag(&out.store, functions),
        "ReverseTimeRec outputs should select a reverse sample loop"
    );

    let compute_body = find_decl_fun_body(&out.store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(&out.store, compute_body) else {
        panic!("compute block expected");
    };
    let reset_pos = stmts
        .iter()
        .position(|id| {
            matches!(
                match_fir(&out.store, *id),
                FirMatch::StoreVar {
                    ref name,
                    access: AccessType::Struct,
                    ..
                } if name.starts_with("fRec")
            )
        })
        .expect("ReverseTimeRec scalar carrier should reset in compute preamble");
    let loop_pos = stmts
        .iter()
        .position(|id| matches!(match_fir(&out.store, *id), FirMatch::SimpleForLoop { .. }))
        .expect("compute should contain a sample loop");
    assert!(
        reset_pos < loop_pos,
        "ReverseTimeRec carrier reset must run before the reverse sample loop"
    );
}
#[test]
fn mixed_forward_and_reverse_time_outputs_lower_to_split_sample_loops() {
    let mut arena = TreeArena::new();
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
    let (forward, reverse) = {
        let mut b = SigBuilder::new(&mut arena);
        let forward = b.input(0);
        let reverse_group = b.reverse_time_rec(group);
        let reverse = b.proj(0, reverse_group);
        (forward, reverse)
    };

    let prepared = prepare_signals_for_fir(&arena, &[forward, reverse], &UiProgram::empty())
        .expect("mixed forward/reverse output bundle should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        1,
        2,
        &SignalFirOptions::default(),
    )
    .expect("mixed forward/reverse output bundle should lower through split loops");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let compute_body = find_decl_fun_body(&out.store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(&out.store, compute_body) else {
        panic!("compute block expected");
    };
    let loop_directions: Vec<bool> = stmts
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::SimpleForLoop { is_reverse, .. } => Some(is_reverse),
            _ => None,
        })
        .collect();
    assert_eq!(
        loop_directions,
        vec![false, true],
        "mixed bundles should run forward primals before reverse adjoints"
    );
}
#[test]
fn recursive_feedback_delay1_reuses_single_scalar_recursion_state() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let inc = b.real(0.25);
        b.add(prev, inc)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("feedback group should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        0,
        1,
        &SignalFirOptions::default(),
    )
    .expect("feedback delay1 should lower through recursion array slots");

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
    assert_eq!(
        struct_items
            .iter()
            .filter(|id| matches!(
                match_fir(&out.store, **id),
                FirMatch::DeclareVar {
                    ref name,
                    typ: FirType::Float32 | FirType::Float64,
                    access: AccessType::Struct,
                    ..
                } if name.starts_with("fRec")
            ))
            .count(),
        1,
        "simple feedback recurrence should use one scalar recursion state field"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                typ: FirType::Array(_, _),
                ..
            } if name.starts_with("fRec") || name.starts_with("iRec")
        )),
        "simple feedback recurrence should not allocate array-backed recursion storage"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "simple feedback recurrence should not allocate fIOTA"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert_eq!(
        stmts
            .iter()
            .filter(|id| matches!(
                match_fir(&out.store, **id),
                FirMatch::StoreVar {
                    ref name,
                    access: AccessType::Struct,
                    ..
                } if name.starts_with("fRec")
            ))
            .count(),
        1,
        "simple feedback recurrence should commit one scalar state update"
    );
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("fRecCur")
        )),
        "simple feedback recurrence should expose the current sample through a stack-local binding"
    );
    assert!(
        stmts
            .iter()
            .all(|id| !matches!(match_fir(&out.store, *id), FirMatch::Cast { .. })),
        "feedback delay1 recursion reuse should not insert cast wrappers"
    );
}
#[test]
fn multi_output_recursive_group_stays_two_slot_array_backed() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let back_reference0 = b.proj(0, self_ref);
        let feedback0 = b.delay1(back_reference0);
        b.binop(BinOp::Add, in0, feedback0)
    };
    let body1 = {
        let mut b = SigBuilder::new(&mut arena);
        let in1 = b.input(1);
        let back_reference1 = b.proj(1, self_ref);
        let feedback1 = b.delay1(back_reference1);
        b.binop(BinOp::Add, in1, feedback1)
    };
    let tail = arena.cons(body1, arena.nil());
    let body_list = arena.cons(body0, tail);
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = SigBuilder::new(&mut arena).proj(0, group);

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("multi-output feedback group should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        2,
        1,
        &SignalFirOptions::default(),
    )
    .expect("multi-output feedback group should lower");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items
            .iter()
            .filter(|id| matches!(
                match_fir(&out.store, **id),
                FirMatch::DeclareVar {
                    ref name,
                    typ: FirType::Array(_, 2),
                    ..
                } if name.starts_with("fRec")
            ))
            .count()
            >= 2,
        "multi-output recursion should stay on the two-slot array-backed path"
    );
}
#[test]
fn delay_analysis_attributes_nested_delay1_chain_to_recursion_output() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev1 = b.delay1(feedback);
        let prev2 = b.delay1(prev1);
        let inc = b.real(0.25);
        b.add(prev2, inc)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("nested feedback group should prepare");
    let delay = analyze_delays_for_prepared(&prepared);
    let signals::SigMatch::Proj(index, prepared_group) =
        signals::match_sig(prepared.arena(), prepared.outputs()[0])
    else {
        panic!("prepared output should stay a recursion projection");
    };
    let (var, _bodies) = match_sym_rec(prepared.arena(), prepared_group)
        .expect("prepared projection should target a symbolic recursion group");
    let analysis = delay
        .rec_output_analysis_in_context(var.as_u32(), index as usize, None)
        .expect("nested delay1 chain should be attributed to the recursion output");
    assert_eq!(
        analysis.max_delay, 2,
        "Delay1(Delay1(Proj)) should attribute total delay 2 to the recursion carrier"
    );
    assert_eq!(
        analysis.delay_count, 1,
        "the recursion output should observe one delayed access chain in this fixture"
    );
}
#[test]
fn delay_analysis_attributes_fixed_delay_over_feedback_delay1_to_recursion_output() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let ten = b.int(10);
        let delayed = b.delay(prev, ten);
        let input = b.input(0);
        b.add(delayed, input)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("delayed feedback group should prepare");
    let delay = analyze_delays_for_prepared(&prepared);
    let signals::SigMatch::Proj(index, prepared_group) =
        signals::match_sig(prepared.arena(), prepared.outputs()[0])
    else {
        panic!("prepared output should stay a recursion projection");
    };
    let (var, _bodies) = match_sym_rec(prepared.arena(), prepared_group)
        .expect("prepared projection should target a symbolic recursion group");
    let analysis = delay
        .rec_output_analysis_in_context(var.as_u32(), index as usize, None)
        .expect("Delay(Delay1(Proj), 10) should be attributed to the recursion output");
    assert_eq!(
        analysis.max_delay, 11,
        "Delay(Delay1(Proj), 10) should attribute total delay 11 to the recursion carrier"
    );
    assert_eq!(
        analysis.delay_count, 1,
        "the recursion output should observe one delayed access chain in this fixture"
    );
}
#[test]
fn nested_feedback_delay1_chain_reuses_one_recursion_carrier() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev1 = b.delay1(feedback);
        let prev2 = b.delay1(prev1);
        let inc = b.real(0.25);
        b.add(prev2, inc)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("nested feedback group should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        0,
        1,
        &SignalFirOptions::default(),
    )
    .expect("nested feedback delay chain should lower through one recursion carrier");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let rec_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fRec") => Some((name, size)),
            _ => None,
        })
        .collect();
    let delay_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some((name, size)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rec_arrays.len(),
        1,
        "nested feedback delay chain should use exactly one recursion carrier"
    );
    assert!(
        rec_arrays[0].1 >= 3,
        "nested feedback delay chain should upsize the recursion carrier to hold two delayed reads"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "small upsized recursion carrier should stay on the exact-shift strategy"
    );
    assert!(
        delay_arrays.is_empty(),
        "nested feedback delay chain should not allocate auxiliary delay vectors"
    );
}
#[test]
fn fixed_delay_over_feedback_chain_reuses_one_recursion_carrier() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let ten = b.int(10);
        let delayed = b.delay(prev, ten);
        let input = b.input(0);
        b.add(delayed, input)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("delayed feedback group should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        1,
        1,
        &SignalFirOptions::default(),
    )
    .expect("Delay(Delay1(Proj), 10) should lower through one recursion carrier");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let rec_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fRec") => Some((name, size)),
            _ => None,
        })
        .collect();
    let delay_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some((name, size)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rec_arrays.len(),
        1,
        "fixed delay over recursion feedback should use exactly one recursion carrier"
    );
    assert!(
        rec_arrays[0].1 >= 12,
        "fixed delay over recursion feedback should upsize the recursion carrier for delay 11"
    );
    assert!(
        delay_arrays.is_empty(),
        "fixed delay over recursion feedback should not allocate auxiliary delay vectors"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "small fixed delay over recursion feedback should stay on the exact-shift strategy"
    );
}
#[test]
fn top_level_recursion_projection_delay_chain_reuses_one_recursion_carrier() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let input = b.input(0);
        b.add(prev, input)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let current = b.proj(0, group);
        let prev1 = b.delay1(current);
        let prev2 = b.delay1(prev1);
        let sum1 = b.add(current, prev1);
        b.add(sum1, prev2)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("top-level delayed recursion projection should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        1,
        1,
        &SignalFirOptions::default(),
    )
    .expect("top-level delayed recursion projection should lower through one recursion carrier");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let rec_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fRec") => Some((name, size)),
            _ => None,
        })
        .collect();
    let delay_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fVec") || name.starts_with("iVec") => Some((name, size)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rec_arrays.len(),
        1,
        "top-level delayed recursion projection should use exactly one recursion carrier"
    );
    assert!(
        rec_arrays[0].1 >= 3,
        "top-level delayed recursion projection should upsize the recursion carrier for two delayed reads"
    );
    assert!(
        delay_arrays.is_empty(),
        "top-level delayed recursion projection should not allocate auxiliary delay vectors"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "small top-level delayed recursion projection should stay on the exact-shift strategy"
    );
}
#[test]
fn nested_feedback_delay_chain_of_three_uses_shift_loop_before_mcd_boundary() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev1 = b.delay1(feedback);
        let prev2 = b.delay1(prev1);
        let prev3 = b.delay1(prev2);
        let inc = b.real(0.25);
        b.add(prev3, inc)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("nested feedback group should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        0,
        1,
        &SignalFirOptions::default(),
    )
    .expect("nested feedback delay chain of three should lower through one recursion carrier");

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
    let rec_arrays: Vec<_> = struct_items
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(_, size),
                ..
            } if name.starts_with("fRec") => Some((name, size)),
            _ => None,
        })
        .collect();
    assert_eq!(rec_arrays.len(), 1, "expected one recursion carrier");
    assert_eq!(
        rec_arrays[0].1, 4,
        "three delayed reads should allocate an exact size-4 recursion carrier"
    );
    assert!(
        !struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "delay chain below max_copy_delay should not allocate fIOTA"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::ForLoop { .. })),
        "exact-shift recursion carrier of size 4 should emit a reverse shift loop like Faust C++"
    );
}
#[test]
fn large_feedback_delay_chain_uses_circular_recursion_carrier_past_copy_threshold() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let ten = b.int(10);
        let delayed = b.delay(prev, ten);
        let input = b.input(0);
        b.add(delayed, input)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("delayed feedback group should prepare");
    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        1,
        1,
        &SignalFirOptions {
            max_copy_delay: 4,
            ..SignalFirOptions::default()
        },
    )
    .expect("large delayed feedback should use the circular recursion carrier past mcd");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    assert!(
        struct_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name == "fIOTA"
        )),
        "feedback delay chain beyond max_copy_delay should fall back to the circular recursion strategy"
    );
}
#[test]
fn integer_recursive_min_lowers_to_int_recursion_and_min_i_call() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let one = b.int(1);
        let sum = b.add(prev, one);
        let three = b.int(3);
        b.min(sum, three)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("integer recursion should prepare");
    assert_eq!(prepared.ty(prepared.outputs()[0]), Some(SimpleSigType::Int));

    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        0,
        1,
        &SignalFirOptions::default(),
    )
    .expect("integer min recursion should lower");

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
                typ: FirType::Int32,
                access: AccessType::Struct,
                ..
            } if name.starts_with("iRec")
        )),
        "simple integer recursion should allocate one scalar Int32 recursion state field"
    );

    let dump = fir::dump_fir(&out.store, out.module);
    assert!(
        !dump.contains("name=fmin") && !dump.contains("name=fminf"),
        "integer min recursion should not call floating-point fmin helpers"
    );
    assert!(
        dump.contains("min_i"),
        "integer min recursion should stay an explicit integer min_i function call"
    );
}
#[test]
fn integer_recursive_abs_lowers_to_int_recursion_and_abs_call() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let feedback = b.proj(0, self_ref);
        let prev = b.delay1(feedback);
        let one = b.int(1);
        let sum = b.add(prev, one);
        b.abs(sum)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.proj(0, group)
    };

    let prepared = prepare_signals_for_fir(&arena, &[sig0], &UiProgram::empty())
        .expect("integer abs recursion should prepare");
    assert_eq!(prepared.ty(prepared.outputs()[0]), Some(SimpleSigType::Int));

    let out = compile_fastlane_without_ui(
        prepared.arena(),
        prepared.outputs(),
        0,
        1,
        &SignalFirOptions::default(),
    )
    .expect("integer abs recursion should lower");

    let dump = fir::dump_fir(&out.store, out.module);
    assert!(
        !dump.contains("name=fabs") && !dump.contains("name=fabsf"),
        "integer abs recursion should not call floating-point fabs helpers"
    );
    assert!(
        dump.contains("abs"),
        "integer abs recursion should stay an explicit integer abs function call"
    );
}
