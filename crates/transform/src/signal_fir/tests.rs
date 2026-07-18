//! Test suite for the `signal_fir` fast-lane lowering pass.
//!
//! # Structure
//!
//! Each `#[test]` function follows the same three-step pattern:
//!
//! 1. **Build a signal forest** — use [`SigBuilder`] and, where needed,
//!    [`de_bruijn_rec`] / [`de_bruijn_ref`] to construct the input signal tree
//!    directly in a [`TreeArena`].
//! 2. **Lower to FIR** — call [`compile_fastlane_without_ui`] (or the full
//!    entry point for UI tests) and unwrap the [`SignalFirOutput`].
//! 3. **Assert on the FIR tree** — navigate to the relevant node with
//!    [`find_compute_loop_body`] / [`find_decl_fun_body`], strip the mandatory
//!    output cast with [`unwrap_output_cast`], then pattern-match with
//!    [`match_fir`].
//!
//! The private helpers below form a minimal test DSL that keeps the
//! assertion-focused body of each test free from boilerplate traversal code.

use super::{
    RealType, SignalFirErrorCode, SignalFirOptions, compile_signals_to_fir_fastlane_with_ui,
    delay::{DelayManager, DelayOptions, plan_delays},
    siggen::interpret_generator_for_test,
};
use crate::schedule::SchedulingStrategy;
use fir::{AccessType, FirBinOp, FirMatch, FirType, match_fir};
use signals::{BinOp, BlockRevPolicy, SigBuilder};

use tlib::{TreeArena, de_bruijn_rec, de_bruijn_ref, match_sym_rec};
use ui::{ControlKind, ControlRange, ControlSpec, UiBuilder, UiProgram, UiRootOrigin};

use crate::signal_prepare::{SimpleSigType, prepare_signals_for_fir};

// ── FIR tree navigation ──────────────────────────────────────────────────────

/// Peels off a `Cast(FaustFloat, inner)` wrapper if present, returning the
/// inner node unchanged if no such wrapper exists.
///
/// The lowering pass always stores output samples through an explicit
/// `Cast(FaustFloat, …)` regardless of the internal real type, so that
/// the generated C always writes `float` (or `double`) to the output
/// buffer regardless of the internal computation type.
///
/// Tests that want to assert on the *computation* result — rather than the
/// cast that writes it to the buffer — should call this first to peel the
/// wrapper and reach the actual expression node.
fn unwrap_output_cast(store: &fir::FirStore, id: fir::FirId) -> fir::FirId {
    match match_fir(store, id) {
        FirMatch::Cast {
            typ: FirType::FaustFloat,
            value,
        } => value,
        _ => id,
    }
}

/// Locates a named `DeclareFun` in a FIR functions block and returns its body.
///
/// `functions` must be a `FirMatch::Block` of `DeclareFun` nodes (the
/// top-level functions block of a generated FIR module). Panics with a
/// descriptive message if the block or the named function cannot be found,
/// or if the matching declaration has no body.
///
/// Used by [`find_compute_loop_body`] and directly by tests that need to
/// inspect generated functions other than `compute` (e.g. `init`,
/// `instanceInit`, `getNumInputs`).
fn find_decl_fun_body(store: &fir::FirStore, functions: fir::FirId, target: &str) -> fir::FirId {
    let FirMatch::Block(decls) = match_fir(store, functions) else {
        panic!("functions block expected");
    };
    let fun = decls
        .iter()
        .copied()
        .find(|id| {
            matches!(
                match_fir(store, *id),
                FirMatch::DeclareFun { ref name, .. } if name == target
            )
        })
        .unwrap_or_else(|| panic!("function `{target}` expected"));
    let FirMatch::DeclareFun {
        body: Some(body), ..
    } = match_fir(store, fun)
    else {
        panic!("declare fun with body expected for `{target}`");
    };
    body
}

/// Returns the body block of the sample loop inside the generated `compute`
/// function.
///
/// Every compiled DSP produces a `compute(count, inputs, outputs)` function
/// whose body contains exactly one sample-processing for-loop
/// (`SimpleForLoop` or `ForLoop`). This helper navigates past the function
/// declaration and loop header to return the loop body directly, so that
/// tests can pattern-match on individual statements (assignments, stores,
/// calls) without repeating the traversal.
///
/// Panics if the `compute` function or its sample loop is absent.
fn find_compute_loop_body(store: &fir::FirStore, functions: fir::FirId) -> fir::FirId {
    let compute_body = find_decl_fun_body(store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(store, compute_body) else {
        panic!("compute block expected");
    };
    stmts
        .iter()
        .find_map(|id| match match_fir(store, *id) {
            FirMatch::SimpleForLoop { body, .. } | FirMatch::ForLoop { body, .. } => Some(body),
            _ => None,
        })
        .unwrap_or_else(|| panic!("compute should contain an explicit sample loop"))
}

fn find_compute_simple_loop_reverse_flag(store: &fir::FirStore, functions: fir::FirId) -> bool {
    let compute_body = find_decl_fun_body(store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(store, compute_body) else {
        panic!("compute block expected");
    };
    stmts
        .iter()
        .find_map(|id| match match_fir(store, *id) {
            FirMatch::SimpleForLoop { is_reverse, .. } => Some(is_reverse),
            _ => None,
        })
        .unwrap_or_else(|| panic!("compute should contain an explicit simple sample loop"))
}

// ── Compilation entry-point wrappers ─────────────────────────────────────────

/// Runs the full fast-lane lowering pipeline with an empty UI program.
///
/// Most signal-level tests are not concerned with UI widget lowering.
/// This wrapper passes an empty [`UiProgram`] so those tests do not need
/// to construct one explicitly, reducing per-test boilerplate.
fn compile_fastlane_without_ui(
    arena: &TreeArena,
    signals: &[signals::SigId],
    num_inputs: usize,
    num_outputs: usize,
    options: &SignalFirOptions,
) -> Result<super::SignalFirOutput, super::SignalFirError> {
    let empty_ui = UiProgram::empty();
    compile_signals_to_fir_fastlane_with_ui(
        arena,
        signals,
        num_inputs,
        num_outputs,
        &empty_ui,
        options,
    )
}

fn analyze_delays_for_prepared(prepared: &crate::signal_prepare::PreparedSignals) -> DelayManager {
    let mut delay = DelayManager::new(DelayOptions::default());
    let plan = plan_delays(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
        &delay.options(),
    )
    .expect("delay planning should succeed on prepared signals");
    delay.set_rec_output_analysis(plan.rec_outputs);
    delay
}

// ── UI fixture builders ───────────────────────────────────────────────────────

/// Builds a minimal [`UiProgram`] containing exactly one control.
///
/// The generated program has a single top-level `vgroup("")` containing one
/// leaf node whose slot index is `0`. The `ControlSpec` at that slot is
/// filled with `kind`, `label`, and `range` as provided.
///
/// The three boolean flags select the leaf node type:
/// - `soundfile = true` → `UiBuilder::soundfile(0)` (takes precedence)
/// - `output = true` → `UiBuilder::output_control(0)` (bargraph)
/// - otherwise → `UiBuilder::input_control(0)` (slider / button / etc.)
///
/// Used by tests that exercise the UI lowering path (bargraphs, sliders,
/// soundfiles) without needing a full hand-crafted `UiProgram`.
fn one_control_ui(
    kind: ControlKind,
    label: &str,
    range: Option<ControlRange>,
    output: bool,
    soundfile: bool,
) -> UiProgram {
    let mut arena = TreeArena::new();
    let leaf = {
        let mut b = UiBuilder::new(&mut arena);
        if soundfile {
            b.soundfile(0)
        } else if output {
            b.output_control(0)
        } else {
            b.input_control(0)
        }
    };
    let root = UiBuilder::new(&mut arena).vgroup("", &[leaf]);
    UiProgram {
        arena,
        root,
        controls: vec![ControlSpec {
            id: 0,
            kind,
            label: label.to_owned(),
            metadata: Vec::new(),
            range,
        }],
        root_origin: UiRootOrigin::Synthesized,
        emit_ui: true,
    }
}

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
fn section_routing_places_ui_and_state_resets_in_distinct_functions() {
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HSlider,
        "gain",
        Some(ControlRange {
            init: 0.2,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }),
        false,
        false,
    );
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let slider = b.hslider(0);
        let delayed = b.delay1(slider);
        let in0 = b.input(0);
        b.binop(BinOp::Add, delayed, in0)
    };
    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("sectioned module should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let reset_body = find_decl_fun_body(&out.store, functions, "instanceResetUserInterface");
    let clear_body = find_decl_fun_body(&out.store, functions, "instanceClear");

    let FirMatch::Block(reset_stmts) = match_fir(&out.store, reset_body) else {
        panic!("reset body block expected");
    };
    let FirMatch::Block(clear_stmts) = match_fir(&out.store, clear_body) else {
        panic!("clear body block expected");
    };

    assert!(
        reset_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. }
                if name.starts_with("fHslider")
                    || name.starts_with("fVslider")
                    || name.starts_with("fEntry")
                    || name.starts_with("fButton")
                    || name.starts_with("fCheckbox")
        )),
        "UI zone init should be emitted in instanceResetUserInterface"
    );
    assert!(
        clear_stmts.iter().any(|id| {
            let m = match_fir(&out.store, *id);
            match m {
                FirMatch::StoreVar { ref name, .. }
                    if name.starts_with("fRec")
                        || name.starts_with("iRec")
                        || name.starts_with("fVec")
                        || name.starts_with("iVec") =>
                {
                    true
                }
                FirMatch::SimpleForLoop { body, .. } => {
                    // Circular-buffer state uses a loop to clear the 2-element array
                    if let FirMatch::Block(inner) = match_fir(&out.store, body) {
                        inner.iter().any(|sid| {
                            matches!(
                                match_fir(&out.store, *sid),
                                FirMatch::StoreTable { ref name, .. }
                                    if name.starts_with("fRec")
                                        || name.starts_with("iRec")
                                        || name.starts_with("fVec")
                                        || name.starts_with("iVec")
                            )
                        })
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }),
        "signal state init should be emitted in instanceClear"
    );
}

#[test]
fn ui_only_slider_still_emits_reset_init() {
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HSlider,
        "gain",
        Some(ControlRange {
            init: 0.2,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }),
        false,
        false,
    );
    let sig0 = SigBuilder::new(&mut arena).real(0.0);

    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        0,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("UI-only slider should still compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let reset_body = find_decl_fun_body(&out.store, functions, "instanceResetUserInterface");
    let build_ui_body = find_decl_fun_body(&out.store, functions, "buildUserInterface");

    let FirMatch::Block(reset_stmts) = match_fir(&out.store, reset_body) else {
        panic!("reset body block expected");
    };
    let FirMatch::Block(ui_stmts) = match_fir(&out.store, build_ui_body) else {
        panic!("buildUserInterface body block expected");
    };

    assert!(
        reset_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name == "fHslider0"
        )),
        "UI-only controls must still be initialized in instanceResetUserInterface"
    );
    assert!(
        ui_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::AddSlider { ref var, .. } if var == "fHslider0"
        )),
        "buildUserInterface should still expose the UI-only slider"
    );
}

#[test]
fn section_routing_places_table_initialization_in_instance_constants() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let size = b.int(4);
        let init = b.real(0.5);
        let ridx = b.input(0);
        b.read_only_table(size, init, ridx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("table section routing should compile");

    let FirMatch::Module {
        functions,
        static_decls,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module root expected");
    };
    let clear_body = find_decl_fun_body(&out.store, functions, "instanceClear");

    let FirMatch::Block(clear_stmts) = match_fir(&out.store, clear_body) else {
        panic!("clear body block expected");
    };

    // With static table declarations, table data is embedded inline at file scope
    // rather than initialized via StoreTable in instanceConstants.
    let FirMatch::Block(static_items) = match_fir(&out.store, static_decls) else {
        panic!("static_decls block expected");
    };
    assert!(
        static_items
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::DeclareTable { .. })),
        "table declaration should be emitted in static_decls (compile-time constant)"
    );
    assert!(
        !clear_stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::StoreTable { .. })),
        "instanceClear should not contain table initialization stores"
    );
}

#[test]
fn bargraph_emits_runtime_zone_store_in_compute() {
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HBargraph,
        "level",
        Some(ControlRange {
            init: 0.0,
            min: -60.0,
            max: 6.0,
            step: 0.0,
        }),
        true,
        false,
    );
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        b.hbargraph(0, in0)
    };
    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("bargraph signal should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let compute_loop_body = find_compute_loop_body(&out.store, functions);
    let ui_body = find_decl_fun_body(&out.store, functions, "buildUserInterface");

    let FirMatch::Block(compute_stmts) = match_fir(&out.store, compute_loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        compute_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name.starts_with("fHbargraph")
        )),
        "bargraph should emit runtime zone store in compute body"
    );

    let FirMatch::Block(ui_stmts) = match_fir(&out.store, ui_body) else {
        panic!("buildUserInterface body block expected");
    };
    assert!(
        ui_stmts
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::AddBargraph { .. })),
        "bargraph should be declared in buildUserInterface"
    );
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
fn waveform_and_rdtbl_lower_to_fir_table_nodes() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let v0 = b.real(1.0);
        let v1 = b.real(-2.0);
        let v2 = b.real(3.5);
        let table = b.waveform(&[v0, v1, v2]);
        let idx = b.input(0);
        b.rdtbl(table, idx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("Step 2G should support waveform+rdtbl table lowering");

    let FirMatch::Module {
        static_decls,
        functions,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(static_items) = match_fir(&out.store, static_decls) else {
        panic!("static_decls block expected");
    };
    assert!(
        static_items
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::DeclareTable { .. })),
        "Step 2G should allocate waveform table in static_decls (file-scope constant)"
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
    // Unwrap the FaustFloat cast wrapping the output to reach the LoadTable.
    let inner = unwrap_output_cast(&out.store, stored_value);
    assert!(
        matches!(match_fir(&out.store, inner), FirMatch::LoadTable { .. }),
        "rdtbl output should lower to FIR table read"
    );
}

#[test]
fn wrtbl_readonly_generator_constant_lowers_to_declared_table() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let size = b.int(8);
        let init = b.real(0.25);
        let ridx = b.input(0);
        b.read_only_table(size, init, ridx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("Step 2H should support readonly wrtbl with constant generator");

    let FirMatch::Module { static_decls, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(static_items) = match_fir(&out.store, static_decls) else {
        panic!("static_decls block expected");
    };
    let table = static_items
        .iter()
        .copied()
        .find(|id| matches!(match_fir(&out.store, *id), FirMatch::DeclareTable { .. }))
        .expect("readonly wrtbl should declare one table");
    let FirMatch::DeclareTable { values, .. } = match_fir(&out.store, table) else {
        panic!("declare table expected");
    };
    assert_eq!(values.len(), 8, "table must use requested constant size");
}

#[test]
fn wrtbl_runtime_write_emits_store_table_update() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let size = b.int(4);
        let init = b.real(0.0);
        let widx = b.input(0);
        let wsig = b.input(1);
        let ridx = b.input(0);
        b.write_read_table(size, init, widx, wsig, ridx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 2, 1, &SignalFirOptions::default())
        .expect("Step 2H should support wrtbl runtime write/read shape");

    let FirMatch::Module {
        dsp_struct,
        functions,
        static_decls,
        ..
    } = match_fir(&out.store, out.module)
    else {
        panic!("module expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };
    let struct_table = struct_items
        .iter()
        .copied()
        .find(|id| {
            matches!(
                match_fir(&out.store, *id),
                FirMatch::DeclareTable {
                    access: AccessType::Struct,
                    ..
                }
            )
        })
        .expect("runtime wrtbl should allocate one mutable struct table");
    let FirMatch::DeclareTable {
        values, elem_type, ..
    } = match_fir(&out.store, struct_table)
    else {
        panic!("struct table declaration expected");
    };
    assert_eq!(
        values.len(),
        4,
        "mutable wrtbl table should keep requested size"
    );
    assert_eq!(
        elem_type,
        FirType::Float32,
        "runtime wrtbl table should keep real element type"
    );

    let FirMatch::Block(static_items) = match_fir(&out.store, static_decls) else {
        panic!("static_decls block expected");
    };
    assert!(
        !static_items
            .iter()
            .any(|id| matches!(match_fir(&out.store, *id), FirMatch::DeclareTable { .. })),
        "runtime wrtbl should not be lowered as a static table"
    );

    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(stmts) = match_fir(&out.store, loop_body) else {
        panic!("compute loop body block expected");
    };
    assert!(
        stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable {
                access: AccessType::Struct,
                ..
            }
        )),
        "runtime wrtbl should emit FIR store_table update in compute body"
    );
}

#[test]
fn wrtbl_generator_expansion_preserves_integer_table_types() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let size = {
            let mut b = SigBuilder::new(&mut arena);
            b.int(5)
        };
        let init = {
            let mut b = SigBuilder::new(&mut arena);
            let real = b.real(1.75);
            b.int_cast(real)
        };
        let ridx = {
            let mut b = SigBuilder::new(&mut arena);
            b.input(0)
        };
        let mut b = SigBuilder::new(&mut arena);
        b.read_only_table(size, init, ridx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("readonly wrtbl with computed int generator should lower");

    let FirMatch::Module { static_decls, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(static_items) = match_fir(&out.store, static_decls) else {
        panic!("static_decls block expected");
    };
    let table = static_items
        .iter()
        .copied()
        .find(|id| matches!(match_fir(&out.store, *id), FirMatch::DeclareTable { .. }))
        .expect("readonly wrtbl should declare one static table");
    let FirMatch::DeclareTable {
        elem_type, values, ..
    } = match_fir(&out.store, table)
    else {
        panic!("declare table expected");
    };
    assert_eq!(
        elem_type,
        FirType::Int32,
        "computed integer generator should keep Int32 table type"
    );
    assert!(
        values
            .iter()
            .all(|id| matches!(match_fir(&out.store, *id), FirMatch::Int32 { .. })),
        "computed integer generator should lower to Int32 FIR literals"
    );
}

#[test]
fn siggen_interpreter_delay1_uses_previous_step_for_non_recursive_sources() {
    let mut arena = TreeArena::new();
    let generator = {
        let mut b = SigBuilder::new(&mut arena);
        let v0 = b.real(1.0);
        let v1 = b.real(2.0);
        let v2 = b.real(3.0);
        let v3 = b.real(4.0);
        let waveform = b.waveform(&[v0, v1, v2, v3]);
        b.delay1(waveform)
    };

    let values =
        interpret_generator_for_test(&arena, generator, 4).expect("generator should interpret");
    assert_eq!(values, vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn rec_proj_lowers_without_placeholder_nodes() {
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let c0 = b.real(0.1);
        let feedback = b.proj(0, self_ref);
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
        let feedback0 = b.proj(0, self_ref);
        b.binop(BinOp::Add, in0, feedback0)
    };
    let body1 = {
        let mut b = SigBuilder::new(&mut arena);
        let in1 = b.input(1);
        let feedback1 = b.proj(1, self_ref);
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
        .rec_output_analysis(var.as_u32(), index as usize)
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
        .rec_output_analysis(var.as_u32(), index as usize)
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
    let err = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
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
    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
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
    compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("delay with hi=0 interval should lower as zero-delay (passthrough)");
}

#[test]
fn int_waveform_declares_int32_table() {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let v0 = b.int(1);
        let v1 = b.int(2);
        let v2 = b.int(3);
        let table = b.waveform(&[v0, v1, v2]);
        let idx = b.int(0);
        b.rdtbl(table, idx)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("integer waveform should lower");

    let FirMatch::Module { static_decls, .. } = match_fir(&out.store, out.module) else {
        panic!("module expected");
    };
    let FirMatch::Block(static_items) = match_fir(&out.store, static_decls) else {
        panic!("static_decls block expected");
    };
    assert!(
        static_items.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareTable {
                name,
                elem_type: FirType::Int32,
                ..
            } if name.starts_with("iTbl")
        )),
        "integer waveform tables should declare Int32 element type and use the iTbl prefix"
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

// ── Variability-driven statement placement (Phase 1) ────────────────────

/// Helper: returns the compute body block as a `Vec<FirId>`.
fn find_compute_body_stmts(store: &fir::FirStore, functions: fir::FirId) -> Vec<fir::FirId> {
    let body = find_decl_fun_body(store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(store, body) else {
        panic!("compute body block expected");
    };
    stmts
}

/// Helper: returns the `instanceConstants` body block as a `Vec<FirId>`.
fn find_instance_constants_stmts(store: &fir::FirStore, functions: fir::FirId) -> Vec<fir::FirId> {
    let body = find_decl_fun_body(store, functions, "instanceConstants");
    let FirMatch::Block(stmts) = match_fir(store, body) else {
        panic!("instanceConstants body block expected");
    };
    stmts
}

#[test]
fn slider_cast_hoisted_to_control_as_fslow() {
    // Faust: process = hslider("gain", 0.5, 0, 1, 0.01) * _;
    // Expected: fSlow0 = float(fHslider0) in compute preamble,
    //           fSlow0 * input used in sample loop.
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HSlider,
        "gain",
        Some(ControlRange {
            init: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }),
        false,
        false,
    );
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let slider = b.hslider(0);
        let in0 = b.input(0);
        b.binop(BinOp::Mul, slider, in0)
    };
    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("slider*input should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let compute_stmts = find_compute_body_stmts(&out.store, functions);

    // There should be a DeclareVar(fSlow*, Stack) in the compute preamble.
    let has_fslow = compute_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("fSlow")
        )
    });
    assert!(
        has_fslow,
        "block-rate slider cast should be hoisted to compute preamble as fSlow*"
    );

    // Inside the sample loop, the slider should appear as a LoadVar(fSlow*),
    // not as an inline Cast(Float32, LoadVar(fHslider*)).
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(loop_stmts) = match_fir(&out.store, loop_body) else {
        panic!("loop body block expected");
    };
    let output_store = loop_stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("output store expected");
    let inner = unwrap_output_cast(&out.store, output_store);

    // The Mul should have a LoadVar(fSlow*) as one operand.
    if let FirMatch::BinOp { lhs, rhs, .. } = match_fir(&out.store, inner) {
        let lhs_is_fslow = matches!(
            match_fir(&out.store, lhs),
            FirMatch::LoadVar { ref name, .. } if name.starts_with("fSlow")
        );
        let rhs_is_fslow = matches!(
            match_fir(&out.store, rhs),
            FirMatch::LoadVar { ref name, .. } if name.starts_with("fSlow")
        );
        assert!(
            lhs_is_fslow || rhs_is_fslow,
            "sample-loop Mul should reference a hoisted fSlow* variable"
        );
    }
}

#[test]
fn sample_rate_const_hoisted_to_instance_constants_as_fconst_struct() {
    // Faust: process = 2.0 * float(SR);
    // The `FloatCast(fSamplingFreq)` sub-expression should be hoisted to
    // instanceConstants as a fConst*.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0); // type tag for integer fconst
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let two = b.real(2.0);
        let sr = b.fconst(ty, name, file);
        let sr_float = b.float_cast(sr);
        b.binop(BinOp::Mul, two, sr_float)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("const SR expression should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    // instanceConstants should contain a StoreVar(fConst*, Struct, ...).
    let has_fconst = ic_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fConst")
        )
    });
    assert!(
        has_fconst,
        "init-time constant expression should be hoisted to instanceConstants as fConst*"
    );
}

#[test]
fn recursive_feedback_stays_in_sample_loop() {
    // Faust: process = + ~ _;   (recursive feedback: Samp variability)
    // The recursion load/store must remain in the sample loop body.
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let feedback = b.proj(0, self_ref);
        b.binop(BinOp::Add, in0, feedback)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = SigBuilder::new(&mut arena).proj(0, group);
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("recursive feedback should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(loop_stmts) = match_fir(&out.store, loop_body) else {
        panic!("loop body block expected");
    };

    // The sample loop should still contain the output store (not empty).
    assert!(
        loop_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable { ref name, .. } if name == "output0"
        )),
        "recursive feedback output should remain in the sample loop"
    );

    // The recursive state update should be in the sample loop (not hoisted).
    let has_rec_update = loop_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable { ref name, .. }
                if name.starts_with("fRec") || name.starts_with("iRec")
        ) || matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. }
                if name.starts_with("fRec") || name.starts_with("iRec")
        )
    });
    assert!(
        has_rec_update,
        "recursive state update should remain in the sample loop, not be hoisted"
    );
}

#[test]
fn shared_konst_subexpr_stays_stack_local_inside_instance_constants() {
    // Faust-like shape: let s = 2.0 * float(SR); process = s + sin(s);
    // The shared `s` subtree is Konst and referenced twice, but only within
    // `instanceConstants()`, so it should materialize as a stack-local fConst*.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let sr = b.fconst(ty, name, file);
        let s = {
            let sr_float = b.float_cast(sr);
            let two = b.real(2.0);
            b.binop(BinOp::Mul, two, sr_float)
        };
        let sin_s = b.sin(s);
        b.binop(BinOp::Add, s, sin_s)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("shared Konst subtree should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    assert!(
        ic_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("fConst")
        )),
        "shared Konst subtree used only during init should hoist to a stack-local fConst*"
    );
    assert!(
        ic_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fConst")
        )),
        "the escaping outer Konst result should still materialize as a persistent fConst*"
    );
}

#[test]
fn integer_konst_uses_iconst_prefix() {
    // Faust-like shape: process = SR + 1;
    // The root integer Konst expression escapes to compute via the output path,
    // so it should materialize as an iConst* struct field.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let sr = b.fconst(ty, name, file);
        let one = b.int(1);
        b.binop(BinOp::Add, sr, one)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("integer Konst expression should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    assert!(
        ic_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("iConst")
        )),
        "escaping integer Konst expression should hoist as an iConst* struct field"
    );
}

#[test]
fn trivial_constant_not_hoisted() {
    // Bare literal constants (e.g. `process = 0.5;`) are trivial and
    // should NOT be materialized into fConst* variables.
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.real(0.5)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("bare constant should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    let has_fconst = ic_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                ..
            } if name.starts_with("fConst")
        )
    });
    assert!(
        !has_fconst,
        "trivial literal constant should NOT be hoisted to a fConst* variable"
    );
}

#[test]
fn integer_block_subexpr_uses_islow_prefix() {
    // Faust-like shape: process = int(slider) + 1;
    // The root integer block-rate expression sits at the Block->Samp boundary,
    // so it should hoist as iSlow*.
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HSlider,
        "gain",
        Some(ControlRange {
            init: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }),
        false,
        false,
    );
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let slider = b.hslider(0);
        let int_slider = b.int_cast(slider);
        let one = b.int(1);
        b.binop(BinOp::Add, int_slider, one)
    };
    let out = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    )
    .expect("integer block-rate subtree should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let compute_stmts = find_compute_body_stmts(&out.store, functions);

    assert!(
        compute_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("iSlow")
        )),
        "shared integer block-rate subtree should hoist to an iSlow* local"
    );
}

#[test]
fn konst_sr_based_mul_hoisted_to_instance_constants() {
    // Faust: process = 2.0 * float(SR) * float(SR);
    // The multiplication of two non-trivial init-time expressions should
    // produce fConst* variables in instanceConstants.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let sr = b.fconst(ty, name, file);
        let sr_float = b.float_cast(sr);
        let two = b.real(2.0);
        let product = b.binop(BinOp::Mul, two, sr_float);
        // Multiply again so the outer Mul is non-trivial and Konst.
        b.binop(BinOp::Mul, product, sr_float)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("SR-based expression should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    // instanceConstants should contain at least one fConst* store.
    let has_fconst = ic_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fConst")
        )
    });
    assert!(
        has_fconst,
        "non-trivial init-time SR expression should be hoisted as fConst*"
    );
}

// ── BlockReverseAD golden FIR tests (Phase B3–B6) ────────────────────────────

/// A BRA carrier lowering must produce exactly two sample loops in `compute`:
/// one forward loop for the primal output and one **reverse** loop for the
/// gradient output.
///
/// Circuit: `process = rad(2 * x, x)` where x is a constant seed (real=2.0).
/// The primal output is `2 * 2.0 = 4.0` and the gradient w.r.t. x is `2.0`.
///
/// This test checks the structural FIR property: the compute function contains
/// two `SimpleForLoop` nodes, the second of which has `is_reverse = true`.
#[test]
fn bra_simple_linear_produces_forward_and_reverse_loops() {
    let mut arena = TreeArena::new();

    // body = 2.0 * x  where x = Real(2.0) (constant seed)
    let x = SigBuilder::new(&mut arena).real(2.0);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body = SigBuilder::new(&mut arena).binop(BinOp::Mul, two, x);
    let cot = SigBuilder::new(&mut arena).real(1.0);

    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body],
        &[x],
        &[cot],
        BlockRevPolicy::TapeFull,
    );

    // Proj(0, carrier) = primal output (forward loop)
    // Proj(1, carrier) = gradient output (reverse loop)
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("simple BRA should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };

    // Collect all SimpleForLoop nodes in compute.
    let compute_body = find_decl_fun_body(&out.store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(&out.store, compute_body) else {
        panic!("compute body block expected");
    };
    let loops: Vec<bool> = stmts
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::SimpleForLoop { is_reverse, .. } => Some(is_reverse),
            _ => None,
        })
        .collect();

    assert_eq!(
        loops.len(),
        2,
        "BRA should produce exactly two sample loops (forward + reverse), got {loops:?}"
    );
    assert!(
        !loops[0],
        "first loop should be forward (is_reverse = false)"
    );
    assert!(
        loops[1],
        "second loop should be reverse (is_reverse = true)"
    );
}

/// A BRA carrier with a `Delay1` in the body must declare a `fBraCarry*`
/// struct field for the anti-causal adjoint carry.
///
/// Circuit: `body = delay1(x)` where x is a constant seed.
/// The BRA backward rule for `Delay1(x)` allocates a carry variable
/// (`fBraCarry0`) to propagate `adj[x][n-1] += adj[Delay1(x)][n]`
/// across reverse-loop iterations.
#[test]
fn bra_delay1_seed_produces_carry_struct_field() {
    let mut arena = TreeArena::new();

    let x = SigBuilder::new(&mut arena).real(0.5);
    let body = SigBuilder::new(&mut arena).delay1(x);
    let cot = SigBuilder::new(&mut arena).real(1.0);

    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body],
        &[x],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("Delay1 BRA should compile");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };

    let has_bra_carry = struct_items.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name.starts_with("fBraCarry")
        )
    });
    assert!(
        has_bra_carry,
        "Delay1 in BRA body should produce a fBraCarry* struct field for the anti-causal carry"
    );
}

/// BRA adjoint carry variables must appear in `instanceClear` but NOT in
/// `instanceResetUserInterface`.
///
/// BRA carry variables (`fBraCarry*`) are internal DSP state, not UI-controlled
/// parameters.  They must be zeroed during `instanceClear` but must NOT appear
/// in `instanceResetUserInterface` (which is reserved for sliders, buttons, etc.).
/// They are also reset by `emit_bra_compute_resets` in the compute preamble
/// (TBPTT(BS,BS) truncation: adjoint carry = 0 at start of each block).
///
/// Before this fix, `ensure_bra_delay1_carry` passed `Some(zero)` to
/// `ensure_named_struct_var`, causing the carry to be registered as a reset-time
/// UI init.
#[test]
fn bra_carry_absent_from_instance_reset_user_interface() {
    let mut arena = TreeArena::new();

    // Build: Proj(0, Rec([p * Delay1(Proj(0, self_ref)) + 2]))
    let p = SigBuilder::new(&mut arena).real(0.5);
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let feedback = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, p, delayed);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body_expr = SigBuilder::new(&mut arena).binop(signals::BinOp::Add, two, feedback);
    let body_list = arena.cons(body_expr, arena.nil());
    let rec_group = de_bruijn_rec(&mut arena, body_list);
    let rec_out = SigBuilder::new(&mut arena).proj(0, rec_group);

    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[rec_out],
        &[p],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("recursive BRA (one-pole, no UI) should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };

    // `instanceResetUserInterface` must NOT contain any `fBraCarry*` store.
    let reset_body = find_decl_fun_body(&out.store, functions, "instanceResetUserInterface");
    let FirMatch::Block(reset_stmts) = match_fir(&out.store, reset_body) else {
        panic!("instanceResetUserInterface body block expected");
    };
    let has_bra_carry_in_reset = reset_stmts.iter().any(|id| {
        if let FirMatch::StoreVar { name, .. } = match_fir(&out.store, *id) {
            return name.starts_with("fBraCarry");
        }
        false
    });
    assert!(
        !has_bra_carry_in_reset,
        "fBraCarry* must NOT appear in instanceResetUserInterface; \
         BRA carries are DSP state, not UI state"
    );

    // `instanceClear` MUST contain the `fBraCarry*` zero-init.
    let clear_body = find_decl_fun_body(&out.store, functions, "instanceClear");
    let FirMatch::Block(clear_stmts) = match_fir(&out.store, clear_body) else {
        panic!("instanceClear body block expected");
    };
    let has_bra_carry_in_clear = clear_stmts.iter().any(|id| {
        if let FirMatch::StoreVar { name, .. } = match_fir(&out.store, *id) {
            return name.starts_with("fBraCarry");
        }
        false
    });
    assert!(
        has_bra_carry_in_clear,
        "fBraCarry* must appear in instanceClear for proper DSP reset"
    );
}

/// BRA primal SYMREC carrier must NOT be reset in the `compute()` preamble.
///
/// The SYMREC primal state (e.g. `fRec<N>`) is persistent DSP filter memory.
/// It must only be cleared in `instanceClear()` and must never be reset in
/// `compute()`, which would sabotage the filter continuity across host blocks.
///
/// Before this fix, `emit_reverse_time_rec_compute_resets` reset ALL entries in
/// `rec_array_by_group_index`, including SYMREC primal carriers.  The fix adds
/// `reverse_time_rec_group_ids` to `RecursionState` so that only `ReverseTimeRec`
/// adjoint carriers are zeroed in the compute preamble.
#[test]
fn bra_symrec_primal_carrier_absent_from_compute_preamble_resets() {
    let mut arena = TreeArena::new();

    // Same one-pole recursive circuit as above.
    let p = SigBuilder::new(&mut arena).real(0.5);
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let feedback = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, p, delayed);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body_expr = SigBuilder::new(&mut arena).binop(signals::BinOp::Add, two, feedback);
    let body_list = arena.cons(body_expr, arena.nil());
    let rec_group = de_bruijn_rec(&mut arena, body_list);
    let rec_out = SigBuilder::new(&mut arena).proj(0, rec_group);

    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[rec_out],
        &[p],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("recursive BRA (one-pole) should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };

    // The compute() body starts with a preamble of StoreVar (resets) followed by
    // the forward/reverse sample loops.  Extract all top-level StoreVar names.
    let compute_body = find_decl_fun_body(&out.store, functions, "compute");
    let FirMatch::Block(compute_stmts) = match_fir(&out.store, compute_body) else {
        panic!("compute body block expected");
    };
    // Collect all `fRec*` variable names reset in the compute preamble
    // (StoreVar statements before any loop).
    let mut rec_resets_in_compute: Vec<String> = Vec::new();
    for &id in compute_stmts.iter() {
        match match_fir(&out.store, id) {
            FirMatch::StoreVar { name, .. } if name.starts_with("fRec") => {
                rec_resets_in_compute.push(name.clone());
            }
            _ => {}
        }
    }
    assert!(
        rec_resets_in_compute.is_empty(),
        "SYMREC primal carrier(s) {:?} must NOT be reset in compute() preamble; \
         they are persistent DSP state that must survive across host compute() calls",
        rec_resets_in_compute
    );

    // BRA adjoint carry MUST appear in the compute preamble (TBPTT(BS,BS) reset).
    let has_bra_carry_reset_in_compute = compute_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name.starts_with("fBraCarry")
        )
    });
    assert!(
        has_bra_carry_reset_in_compute,
        "fBraCarry* adjoint carry must be reset in compute() preamble for TBPTT(BS,BS)"
    );
}

/// A BRA body containing a `FConst` node (e.g. `fSampleRate` pulled in via
/// `stdfaust.lib` → `ma.SR`) must compile without error.
///
/// Before the fix, `FConst` was not listed as a leaf in:
/// - `is_trivially_reverse_evaluable` — causing it to be mistakenly taped,
/// - `collect_bra_postorder` — reaching the postorder as an unknown node,
/// - `propagate_bra_adj` — triggering `[FRS-SFIR-0004] signal FConst(…) not
///   supported in BlockReverseAD backward pass (B6)`.
///
/// The BRA backward pass treats `FConst` (and `FVar`) as zero-gradient leaves:
/// they are external scalars with no differentiable children.
#[test]
fn bra_body_with_fconst_leaf_compiles() {
    let mut arena = TreeArena::new();

    // Build a body: seed * FConst("fSampleRate")
    // This represents e.g. `p * ma.SR` inside the BRA expression.
    let p = SigBuilder::new(&mut arena).real(0.5);
    let ty_node = SigBuilder::new(&mut arena).int(0); // type tag (int=0)
    // Use the recognized alias "fSamplingFreq" — lower_fconst maps it to fSampleRate.
    let name_node = arena.string_lit("fSamplingFreq");
    let file_node = arena.string_lit("<math.h>");
    let fc = SigBuilder::new(&mut arena).fconst(ty_node, name_node, file_node);
    let body_expr = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, p, fc);

    // BRA: differentiate body w.r.t. p
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body_expr],
        &[p],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    // Must not crash with "signal FConst(…) not supported in BlockReverseAD backward pass".
    compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
        .expect("BRA body with FConst leaf must compile successfully");
}

/// A BRA carrier wrapping a recursive circuit must declare a `fBraCarry*`
/// struct field for the TBPTT feedback carry.
///
/// Circuit: `process = rad(c : +~*(c), c)` — the Faust one-pole feedback
/// filter where `c` is both input and feedback coefficient.  In Signal IR
/// the recursive circuit becomes `Proj(0, SYMREC(var, Add(Input, Mul(c,
/// Delay1(Proj(0, SYMREF(var)))))))`.  The pre-scan in
/// `ensure_bra_backward_sweep` allocates a `fBraCarry*` struct field for the
/// `Delay1(Proj(0, SYMREF))` feedback node.
#[test]
fn bra_recursive_one_pole_produces_feedback_carry_struct_field() {
    let mut arena = TreeArena::new();

    // Build: Proj(0, Rec([Input(0) + c * Delay1(Proj(0, self_ref))]))
    let c = SigBuilder::new(&mut arena).real(0.5); // feedback coefficient seed
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let feedback = SigBuilder::new(&mut arena).binop(BinOp::Mul, c, delayed);
    let input0 = SigBuilder::new(&mut arena).input(0);
    let body_expr = SigBuilder::new(&mut arena).binop(BinOp::Add, input0, feedback);
    let body_list = arena.cons(body_expr, arena.nil());
    let rec_group = de_bruijn_rec(&mut arena, body_list);
    let rec_out = SigBuilder::new(&mut arena).proj(0, rec_group);

    // BRA: differentiate rec_out w.r.t. c
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[rec_out],
        &[c],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 1, 2, &SignalFirOptions::default())
            .expect("recursive BRA (one-pole) should compile");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };

    // A carry struct field for the recursive feedback (Delay1(Proj(0, SYMREF)))
    // must be present after Phase B6 support.
    let has_bra_carry = struct_items.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name.starts_with("fBraCarry")
        )
    });
    assert!(
        has_bra_carry,
        "recursive BRA (one-pole) should produce a fBraCarry* struct field \
         for the TBPTT feedback carry"
    );
}

/// Regression test: BRA body containing a `FloatCast(int_rec)` node (e.g.
/// `no.noise`'s integer LCG accumulator cast to float) must compile without
/// emitting invalid `BinOp(Float32, Int32)` FIR nodes in the backward sweep.
///
/// Before this fix, `propagate_bra_adj` propagated the Float32 adjoint into
/// integer-typed subtrees via the `FloatCast(x)` identity rule, causing the
/// FIR checker to flag `[FRS-FIR-0001] BinOp operands have incompatible types:
/// Float32 vs Int32` in `compute`.
///
/// The fix: stop gradient propagation at `FloatCast(x)` when `x` is
/// integer-typed (non-differentiable integer-to-float cast).
///
/// Circuit: `bra(FloatCast(int_rec) * seed, seed)` — the integer LCG
/// recursion `Proj(0, SYMREC(v, Add(Mul(Delay1(Proj(0, SYMREF(v))), 1103515245), 12345)))`
/// cast to float, then multiplied by a float seed.
#[test]
fn bra_body_with_integer_float_cast_compiles() {
    let mut arena = TreeArena::new();

    // Build an integer LCG recursion: iRec = 1103515245 * iRec + 12345
    // Proj(0, SYMREC(v, Add(Mul(Delay1(Proj(0, SYMREF(v))), Int(1103515245)), Int(12345))))
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let mult_const = SigBuilder::new(&mut arena).int(1103515245);
    let add_const = SigBuilder::new(&mut arena).int(12345);
    let lcg_mul = SigBuilder::new(&mut arena).binop(BinOp::Mul, delayed, mult_const);
    let lcg_body = SigBuilder::new(&mut arena).binop(BinOp::Add, lcg_mul, add_const);
    let lcg_body_list = arena.cons(lcg_body, arena.nil());
    let lcg_rec = de_bruijn_rec(&mut arena, lcg_body_list);
    let lcg_proj = SigBuilder::new(&mut arena).proj(0, lcg_rec); // Int32 output

    // Cast the integer LCG to float (mimics `no.noise`'s `float` primitive)
    let lcg_float = SigBuilder::new(&mut arena).float_cast(lcg_proj); // Float32

    // Multiply by a float seed: body = FloatCast(int_rec) * seed
    let seed = SigBuilder::new(&mut arena).real(0.5);
    let body_expr = SigBuilder::new(&mut arena).binop(BinOp::Mul, lcg_float, seed);

    // BRA: differentiate body w.r.t. seed
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body_expr],
        &[seed],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    // Before the fix this crashed with:
    //   [FRS-FIR-0001] BinOp operands have incompatible types: Float32 vs Int32
    // because the backward sweep propagated Float32 gradient into the Int32 LCG body.
    compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
        .expect("BRA body with FloatCast(int_rec) must compile without BinOp type mismatch");
}

// ── Lowering-coverage obligation (rewriting-calculus §8.2 / analysis W8) ──────
//
// The signal→FIR rewriting-calculus formalisation states the obligation
// `L_prep ⊆ dom(lower)`: every signal constructor that `signal_prepare::verify`
// accepts must be lowerable by `lower_signal`. These tests make the obligation
// executable. See `porting/signal-to-fir-rewriting-calculus-2026-06-20-en.md`.

use signals::{SigMatch, match_sig};

/// Whether the fast-lane `lower_signal` dispatcher handles a constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoweringCoverage {
    /// Has its own arm in `lower_signal` (lowered when encountered directly).
    Direct,
    /// No top-level arm, but reachable through a parent arm (currently `Proj`).
    ViaParent,
    /// Falls through to `other => UnsupportedSignalNode`.
    Unsupported,
    /// Has its own arm in `lower_signal` that returns the dedicated
    /// `ClockedNotLowered` (`FRS-SFIR-0007`) rejection: clocked machinery
    /// accepted by `signal_prepare` but awaiting the clock-domain back half
    /// (roadmap P1–P3).
    ClockedRejection,
}

/// Exhaustive classification of every `SigMatch` constructor against the
/// `lower_signal` dispatcher in `module/core_lowering.rs`.
///
/// This match intentionally has **no wildcard arm**: a newly added `SigMatch`
/// variant will fail to compile here until it is consciously classified, which
/// is the drift guard for the `L_prep ⊆ dom(lower)` coverage obligation. The
/// `Unsupported` arm is the executable record of the W8 gap (families that
/// `signal_prepare::verify` accepts but `lower_signal` does not handle).
fn lowering_coverage(m: &SigMatch<'_>) -> LoweringCoverage {
    use LoweringCoverage::{ClockedRejection, Direct, Unsupported, ViaParent};
    match m {
        SigMatch::Int(_)
        | SigMatch::Real(_)
        | SigMatch::Input(_)
        | SigMatch::Output(..)
        | SigMatch::Delay1(_)
        | SigMatch::Delay(..)
        | SigMatch::Prefix(..)
        | SigMatch::IntCast(_)
        | SigMatch::BitCast(_)
        | SigMatch::FloatCast(_)
        | SigMatch::Select2(..)
        | SigMatch::Proj(..)
        | SigMatch::BinOp(..)
        | SigMatch::Pow(..)
        | SigMatch::Min(..)
        | SigMatch::Max(..)
        | SigMatch::Acos(_)
        | SigMatch::Asin(_)
        | SigMatch::Atan(_)
        | SigMatch::Atan2(..)
        | SigMatch::Cos(_)
        | SigMatch::Sin(_)
        | SigMatch::Tan(_)
        | SigMatch::Exp(_)
        | SigMatch::Exp10(_)
        | SigMatch::Log(_)
        | SigMatch::Log10(_)
        | SigMatch::Sqrt(_)
        | SigMatch::Abs(_)
        | SigMatch::Fmod(..)
        | SigMatch::Remainder(..)
        | SigMatch::Floor(_)
        | SigMatch::Ceil(_)
        | SigMatch::Rint(_)
        | SigMatch::Round(_)
        | SigMatch::Lowest(_)
        | SigMatch::Highest(_)
        | SigMatch::FConst(..)
        | SigMatch::FVar(..)
        | SigMatch::FFun(..)
        | SigMatch::RdTbl(..)
        | SigMatch::WrTbl(..)
        | SigMatch::Waveform(_)
        | SigMatch::Button(_)
        | SigMatch::Checkbox(_)
        | SigMatch::VSlider(_)
        | SigMatch::HSlider(_)
        | SigMatch::NumEntry(_)
        | SigMatch::VBargraph(..)
        | SigMatch::HBargraph(..)
        | SigMatch::Attach(..)
        | SigMatch::Enable(..)
        | SigMatch::Control(..)
        | SigMatch::Soundfile(_)
        | SigMatch::SoundfileLength(..)
        | SigMatch::SoundfileRate(..)
        | SigMatch::SoundfileBuffer(..) => Direct,

        // Reverse-mode-AD carriers: no top-level arm; lowered through `Proj`.
        SigMatch::BlockReverseAD { .. } | SigMatch::ReverseTimeRec(_) => ViaParent,

        // Clocked machinery: dedicated structured rejection until the
        // clock-domain lowering (roadmap P1-P3) lands.
        SigMatch::TempVar(_)
        | SigMatch::PermVar(_)
        | SigMatch::Seq(..)
        | SigMatch::ZeroPad(..)
        | SigMatch::Clocked(..)
        | SigMatch::ClockEnvToken(_)
        | SigMatch::OnDemand(_)
        | SigMatch::Upsampling(_)
        | SigMatch::Downsampling(_) => ClockedRejection,

        // Accepted by `verify` but with no `lower_signal` arm — the W8 gap.
        SigMatch::Gen(_)
        | SigMatch::AssertBounds(..)
        | SigMatch::Fir(_)
        | SigMatch::Iir(_)
        // Pre-preparation / legacy forms that `verify` itself also rejects.
        | SigMatch::Unknown
        | SigMatch::Rec(_) => Unsupported,
    }
}

#[test]
fn lowering_coverage_classifies_sampled_families() {
    let mut arena = TreeArena::new();
    let (int_sig, binop_sig, sin_sig, gen_sig) = {
        let mut b = SigBuilder::new(&mut arena);
        let i = b.int(1);
        let c = b.real(0.5);
        let binop = b.binop(BinOp::Mul, i, c);
        let sin = b.sin(c);
        let r = b.real(1.0);
        let gen_node = b.generate(r);
        (i, binop, sin, gen_node)
    };
    assert_eq!(
        lowering_coverage(&match_sig(&arena, int_sig)),
        LoweringCoverage::Direct
    );
    assert_eq!(
        lowering_coverage(&match_sig(&arena, binop_sig)),
        LoweringCoverage::Direct
    );
    assert_eq!(
        lowering_coverage(&match_sig(&arena, sin_sig)),
        LoweringCoverage::Direct
    );
    assert_eq!(
        lowering_coverage(&match_sig(&arena, gen_sig)),
        LoweringCoverage::Unsupported
    );
}

#[test]
fn fastlane_lowers_representative_supported_families() {
    // A small program touching several Direct families: Input, Real, BinOp, Sin.
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let g = b.real(0.5);
        let scaled = b.binop(BinOp::Mul, x, g);
        b.sin(scaled)
    };
    compile_fastlane_without_ui(&arena, &[sig], 1, 1, &SignalFirOptions::default())
        .expect("supported Direct families must lower to a FIR module");
}

#[test]
fn fastlane_rejects_unsupported_family_with_typed_error() {
    // `Gen` is accepted by `signal_prepare::verify` but has no `lower_signal`
    // arm (the W8 coverage gap). Compilation must fail with the stable typed
    // code rather than silently emit a module. Whether the rejection happens at
    // preparation or at lowering, the surfaced code is `UnsupportedSignalNode`.
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        let r = b.real(1.0);
        b.generate(r)
    };
    let err = compile_fastlane_without_ui(&arena, &[sig], 0, 1, &SignalFirOptions::default())
        .expect_err("an unsupported signal family must not silently compile");
    assert_eq!(err.code(), SignalFirErrorCode::UnsupportedSignalNode);
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
    use super::ComputeMode;

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
