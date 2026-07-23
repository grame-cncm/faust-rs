//! `contract` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::{
    RealType, SignalFirErrorCode, SignalFirOptions, compile_signals_to_fir_fastlane_with_ui,
};
use fir::{AccessType, FirBinOp, FirMatch, FirType, match_fir};
use signals::{BinOp, SigBuilder};
use tlib::TreeArena;
use ui::{ControlKind, UiProgram};

#[test]
fn non_empty_signal_list_returns_fir_module_root() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let i0 = b.input(0);
        let c0 = b.real(0.5);
        b.binop(BinOp::Mul, i0, c0)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("Step 1A should emit a module for valid top-level inputs");

    assert!(matches!(
        match_fir(&out.store, out.module),
        FirMatch::Module { .. }
    ));
    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let FirMatch::Block(decls) = match_fir(&out.store, functions) else {
        panic!("module functions block expected");
    };
    for required_name in [
        "metadata",
        "instanceConstants",
        "instanceResetUserInterface",
        "instanceClear",
        "buildUserInterface",
        "compute",
    ] {
        assert!(
            decls.iter().any(|id| {
                matches!(
                    match_fir(&out.store, *id),
                    FirMatch::DeclareFun { ref name, .. } if name == required_name
                )
            }),
            "section function `{required_name}` must exist in fast-lane module"
        );
    }
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
    // The output store emits an explicit FaustFloat cast around the internal
    // computation node; unwrap it to reach the actual BinOp.
    let inner = unwrap_output_cast(&out.store, stored_value);
    assert!(matches!(
        match_fir(&out.store, inner),
        FirMatch::BinOp {
            op: FirBinOp::Mul,
            ..
        }
    ));
}
#[test]
fn comparison_binops_lower_to_int32_boolean_values() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let i0 = b.input(0);
        let zero = b.real(0.0);
        b.binop(BinOp::Eq, i0, zero)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("comparison should lower through fast-lane");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
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
    assert!(matches!(
        match_fir(&out.store, inner),
        FirMatch::BinOp {
            op: FirBinOp::Eq,
            typ: FirType::Int32,
            ..
        }
    ));
}
#[test]
fn enable_with_checkbox_lowers_select2_condition_to_int32() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let gate = b.checkbox(0);
        b.enable(input, gate)
    };
    let ui = one_control_ui(ControlKind::Checkbox, "gate", None, false, false);
    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("checkbox-driven enable should lower through fast-lane");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
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
    let FirMatch::Select2 { cond, .. } = match_fir(&out.store, inner) else {
        panic!("enable should lower to FIR Select2");
    };
    assert_eq!(out.store.value_type(cond), Some(FirType::Int32));
}
#[test]
fn invalid_options_return_typed_error_code() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.input(0)
    };
    let err = compile_fastlane_without_ui(
        &arena,
        &[sig0],
        1,
        1,
        &SignalFirOptions {
            module_name: "".to_owned(),
            real_type: RealType::Float32,
            ..SignalFirOptions::default()
        },
    )
    .expect_err("empty module name should fail option validation");

    assert_eq!(err.code(), SignalFirErrorCode::InvalidOptions);
    assert_eq!(err.code().as_str(), "FRS-SFIR-0001");
}
#[test]
fn clocked_signal_family_returns_dedicated_error_code() {
    // Roadmap P0.1: the clocked family no longer falls into the generic
    // `FRS-SFIR-0004` bucket — it gets the dedicated `ClockedNotLowered`
    // rejection until the clock-domain lowering (P1-P3) lands.
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let i0 = b.input(0);
        b.upsampling(&[i0])
    };
    let err = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect_err("upsampling is outside current lowering slice");

    assert_eq!(err.code(), SignalFirErrorCode::ClockedNotLowered);
    assert_eq!(err.code().as_str(), "FRS-SFIR-0007");
}
#[test]
fn input_index_out_of_range_returns_typed_error_code() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.input(1)
    };
    let err = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect_err("input(1) is invalid when num_inputs=1");

    assert_eq!(err.code(), SignalFirErrorCode::InputIndexOutOfRange);
    assert_eq!(err.code().as_str(), "FRS-SFIR-0006");
}
#[test]
fn pow_min_max_and_unary_math_lower_to_fir_fun_calls() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let i0 = b.input(0);
        let s0 = b.sin(i0);
        let c0 = b.real(0.25);
        let c1 = b.real(0.5);
        let mx = b.max(c0, c1);
        b.pow(s0, mx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("pow/min/max/unary should be supported in Step 2B.1");

    let FirMatch::Module {
        globals, functions, ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module root expected");
    };
    let FirMatch::Block(globals_items) = match_fir(&out.store, globals) else {
        panic!("module globals block expected");
    };
    for expected in ["sin", "pow"] {
        assert!(
            globals_items.iter().any(|id| {
                matches!(
                    match_fir(&out.store, *id),
                    FirMatch::DeclareFun { ref name, body: None, .. } if name == expected
                )
            }),
            "globals should declare extern math prototype '{expected}'"
        );
    }
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    let store_value = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("compute should include one output store");
    // Unwrap the FaustFloat cast wrapping the output to reach the computation.
    let store_value = unwrap_output_cast(&out.store, store_value);
    let FirMatch::FunCall { name, args, .. } = match_fir(&out.store, store_value) else {
        panic!("top-level pow should lower to FIR fun call");
    };
    assert_eq!(name, "pow");
    assert_eq!(args.len(), 2);

    let FirMatch::FunCall { name: lhs_name, .. } = match_fir(&out.store, args[0]) else {
        panic!("lhs should lower to unary fun call");
    };
    assert_eq!(lhs_name, "sin");
    match match_fir(&out.store, args[1]) {
        FirMatch::FunCall { name: rhs_name, .. } => assert_eq!(rhs_name, "fmax"),
        FirMatch::Float32 { value, .. } => assert_eq!(value, 0.5),
        FirMatch::Float64 { value, .. } => assert_eq!(value, 0.5),
        _ => panic!("rhs should lower to fmax or the simplified constant 0.5"),
    }
}
#[test]
fn foreign_function_calls_lower_to_fir_fun_calls_and_prototypes() {
    let mut arena = TreeArena::new();
    let sig0 = {
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
        let args = arena.cons(input0, arena.nil());
        let mut b = SigBuilder::new(&mut arena);
        b.ffun(ff, args)
    };

    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("SIGFFUN should lower to FIR fun calls");

    let FirMatch::Module {
        globals, functions, ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module root expected");
    };
    let FirMatch::Block(globals_items) = match_fir(&out.store, globals) else {
        panic!("module globals block expected");
    };
    assert!(
        globals_items.iter().any(|id| {
            matches!(
                match_fir(&out.store, *id),
                FirMatch::DeclareFun { ref name, body: None, .. } if name == "isnanf"
            )
        }),
        "globals should declare extern foreign prototype 'isnanf'"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    let store_value = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("compute should include one output store");
    let store_value = unwrap_output_cast(&out.store, store_value);
    let FirMatch::FunCall { name, args, typ } = match_fir(&out.store, store_value) else {
        panic!("SIGFFUN should lower to a FIR fun call");
    };
    assert_eq!(name, "isnanf");
    assert_eq!(args.len(), 1);
    assert_eq!(typ, FirType::Int32);
}
#[test]
fn delay_prefix_select_and_cast_nodes_are_supported() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let z0 = b.real(0.0);
        let pre = b.prefix(z0, in0);
        let d1 = b.delay1(pre);
        let n1 = b.int(1);
        let delayed = b.delay(d1, n1);
        let as_int = b.int_cast(delayed);
        let as_float = b.float_cast(as_int);
        let c1 = b.real(1.0);
        let c0 = b.real(0.0);
        b.select2(c1, as_float, c0)
    };

    compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("Step 2B.2 should support delay/prefix/select/casts slice");
}
#[test]
fn foreign_var_count_is_rejected_under_execution_options() {
    // C++ `generateFVar` rejects `fFullCount` when gOneSample || gExtControl:
    // neither `control` nor `frame` supplies a block count (execution-options
    // port plan §3.5).
    use crate::signal_fir::{ControlRateMode, ProcessingApi};

    for (control, api) in [
        (ControlRateMode::External, ProcessingApi::Block),
        (ControlRateMode::InlinePerBlock, ProcessingApi::OneSample),
        (ControlRateMode::External, ProcessingApi::OneSample),
    ] {
        let mut arena = TreeArena::new();
        let sig0 = {
            let ty = arena.int(0);
            let name = arena.symbol("count");
            let file = arena.symbol("<math.h>");
            SigBuilder::new(&mut arena).fvar(ty, name, file)
        };
        let options = SignalFirOptions {
            control_rate_mode: control,
            processing_api: api,
            ..SignalFirOptions::default()
        };
        let err = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &options)
            .expect_err("foreign `count` must be rejected under -ec/-os");
        assert_eq!(err.code().as_str(), "FRS-SFIR-0009", "{control:?} {api:?}");
    }
}

#[test]
fn foreign_var_count_lowers_to_compute_funarg() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("count");
        let file = arena.symbol("<math.h>");
        SigBuilder::new(&mut arena).fvar(ty, name, file)
    };

    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("foreign `count` variable should lower via compute fun args");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable { value, .. }
                if matches!(
                    unwrap_output_cast(&out.store, value),
                    inner if matches!(
                        match_fir(&out.store, inner),
                        FirMatch::LoadVar {
                            ref name,
                            access: AccessType::FunArgs,
                            typ: FirType::Int32,
                        } if name == "count"
                    )
                )
        )),
        "output should store a load of compute(count, ...) fun arg"
    );
}
#[test]
fn left_shift_binop_lowers_to_int32_fir_shift() {
    let mut arena = TreeArena::default();
    let shifted = {
        let mut b = SigBuilder::new(&mut arena);
        let lhs = b.int(1);
        let rhs = b.int(3);
        b.binop(BinOp::Lsh, lhs, rhs)
    };

    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[shifted],
        0,
        1,
        &UiProgram::empty(),
        &SignalFirOptions::default(),
    )
    .expect("lsh should lower through the fast-lane");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    let store_value = stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("compute should include one output store");
    let store_value = unwrap_output_cast(&out.store, store_value);
    assert!(
        matches!(
            match_fir(&out.store, store_value),
            FirMatch::BinOp {
                op: FirBinOp::Lsh,
                typ: FirType::Int32,
                ..
            }
        ) || matches!(
            match_fir(&out.store, store_value),
            FirMatch::Int32 {
                value: 8,
                typ: FirType::Int32
            }
        ),
        "left shift should lower to an Int32 FIR Lsh binop or simplify to Int32(8)"
    );
}
#[test]
fn right_shift_binops_lower_to_int32_fir_shifts() {
    for (source_op, expected_op) in [(BinOp::ARsh, FirBinOp::ARsh), (BinOp::LRsh, FirBinOp::LRsh)] {
        let mut arena = TreeArena::default();
        let shifted = {
            let mut b = SigBuilder::new(&mut arena);
            let lhs = b.int(16);
            let rhs = b.int(2);
            b.binop(source_op, lhs, rhs)
        };

        let out = compile_signals_to_fir_fastlane_with_ui(
            &arena,
            &[shifted],
            0,
            1,
            &UiProgram::empty(),
            &SignalFirOptions::default(),
        )
        .expect("right shift should lower through the fast-lane");

        let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
            panic!("module root expected");
        };
        let loop_body = find_compute_loop_body(&out.store, functions);
        let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
            panic!("compute loop body block expected");
        };
        let store_value = stmts
            .iter()
            .find_map(|id| match match_fir(&out.store, *id) {
                FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
                _ => None,
            })
            .expect("compute should include one output store");
        let store_value = unwrap_output_cast(&out.store, store_value);
        let expected_value = match source_op {
            BinOp::ARsh => 4,
            BinOp::LRsh => 4,
            _ => unreachable!(),
        };
        assert!(
            matches!(
                match_fir(&out.store, store_value),
                FirMatch::BinOp {
                    op,
                    typ: FirType::Int32,
                    ..
                } if op == expected_op
            ) || matches!(
                match_fir(&out.store, store_value),
                FirMatch::Int32 {
                    value,
                    typ: FirType::Int32
                } if value == expected_value
            ),
            "right shift should lower to an Int32 FIR {expected_op:?} binop or simplify to Int32({expected_value})"
        );
    }
}
// ─── P2: `-ss` / `--scheduling-strategy` option plumbing ──────────────────────
//
// Vectorization port plan phase P2 threads `SchedulingStrategy` into
// `SignalFirOptions` without activating scheduling. These tests only check
// that the field is present, defaults to `DepthFirst` (matching `-ss 0` in
// scalar and vector modes alike), and round-trips through struct-update
// syntax like every other option in this struct.

#[test]
fn signal_fir_options_default_scheduling_strategy_is_depth_first() {
    assert_eq!(
        SignalFirOptions::default().scheduling_strategy,
        SchedulingStrategy::DepthFirst
    );
}
#[test]
fn signal_fir_options_scheduling_strategy_is_independent_of_compute_mode() {
    use crate::signal_fir::ComputeMode;

    // Selecting a non-default scheduling strategy must not perturb the
    // (still separate) compute-mode default, and vice versa.
    let options = SignalFirOptions {
        scheduling_strategy: SchedulingStrategy::ReverseBreadthFirst,
        ..SignalFirOptions::default()
    };
    assert_eq!(
        options.scheduling_strategy,
        SchedulingStrategy::ReverseBreadthFirst
    );
    assert_eq!(options.compute_mode, ComputeMode::Scalar);

    let vector_options = SignalFirOptions {
        compute_mode: ComputeMode::Vector {
            vec_size: 64,
            loop_variant: 1,
        },
        ..SignalFirOptions::default()
    };
    assert_eq!(
        vector_options.scheduling_strategy,
        SchedulingStrategy::DepthFirst
    );
}
