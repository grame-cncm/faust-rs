//! `ui_tables` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_fir::{
    SignalFirOptions, compile_signals_to_fir_fastlane_with_ui, siggen::interpret_generator_for_test,
};
use fir::{AccessType, FirMatch, FirType, match_fir};
use signals::{BinOp, SigBuilder};
use tlib::TreeArena;
use ui::{ControlKind, ControlRange};

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
            } if name.starts_with('i')
        )),
        "integer generated tables should declare Int32 element type and take the \
         upstream `i` type prefix (§5.6)"
    );
}
