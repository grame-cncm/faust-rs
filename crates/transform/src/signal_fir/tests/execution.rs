//! Structural tests for the four execution shapes of the `-ec`/`-os` port
//! (execution-options port plan §7.1): classic block, external control,
//! one-sample, and the combined mode.
//!
//! Each test compiles `input(0) * hslider(0)` — one sample-rate input, one
//! block-rate control value — and asserts the emitted module shape: which
//! entry points exist, where the slow value lives, and that no stack local
//! crosses a function boundary (checked both structurally and through the
//! FIR verifier's scope analysis).

use super::fixtures::*;
use crate::signal_fir::{
    ControlRateMode, ProcessingApi, SignalFirOptions, compile_signals_to_fir_fastlane_with_ui,
};
use fir::checker::verify_fir_module;
use fir::{AccessType, FirId, FirMatch, FirStore, match_fir};
use signals::{BinOp, SigBuilder};
use tlib::TreeArena;
use ui::{ControlKind, ControlRange};

/// Compiles the slider×input fixture under the given execution options.
fn compile_slider_gain(options: &SignalFirOptions) -> crate::signal_fir::SignalFirOutput {
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let gain = b.hslider(0);
        b.binop(BinOp::Mul, input, gain)
    };
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
    compile_signals_to_fir_fastlane_with_ui(&arena, &[sig0], 1, 1, &ui, options)
        .expect("slider gain fixture must compile in every execution shape")
}

/// Returns the declared function bodies by name from the module.
fn function_bodies(store: &FirStore, module: FirId) -> Vec<(String, Option<FirId>)> {
    let FirMatch::Module { functions, .. } = match_fir(store, module) else {
        panic!("module root expected");
    };
    let FirMatch::Block(items) = match_fir(store, functions) else {
        panic!("functions block expected");
    };
    items
        .iter()
        .filter_map(|id| match match_fir(store, *id) {
            FirMatch::DeclareFun { name, body, .. } => Some((name, body)),
            _ => None,
        })
        .collect()
}

fn body_of(funs: &[(String, Option<FirId>)], name: &str) -> Option<FirId> {
    funs.iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, body)| *body)
}

/// Collects every node reachable from `root` (statement edges included).
fn reachable(store: &FirStore, root: FirId) -> Vec<FirId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        out.push(id);
        stack.extend(fir::fir_match_children(store, id));
    }
    out
}

fn contains_slow_stack_decl(store: &FirStore, root: FirId) -> bool {
    reachable(store, root).iter().any(|id| {
        matches!(
            match_fir(store, *id),
            FirMatch::DeclareVar { ref name, access: AccessType::Stack, .. }
                if name.starts_with("fSlow") || name.starts_with("iSlow")
        )
    })
}

fn contains_slow_struct_store(store: &FirStore, root: FirId) -> bool {
    reachable(store, root).iter().any(|id| {
        matches!(
            match_fir(store, *id),
            FirMatch::StoreVar { ref name, access: AccessType::Struct, .. }
                if name.starts_with("fSlow") || name.starts_with("iSlow")
        )
    })
}

fn assert_no_scope_errors(store: &FirStore, module: FirId) {
    let report = verify_fir_module(store, module);
    let scope_errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| matches!(d.code, "FIR-SC01" | "FIR-SC02" | "FIR-F08" | "FIR-F09"))
        .collect();
    assert!(
        scope_errors.is_empty(),
        "no stack local may cross a function boundary and the execution \
         entry-point contract must hold: {scope_errors:?}"
    );
}

#[test]
fn classic_shape_has_no_execution_entry_points() {
    let out = compile_slider_gain(&SignalFirOptions::default());
    let funs = function_bodies(&out.store, out.module);
    assert!(body_of(&funs, "control").is_none());
    assert!(funs.iter().all(|(n, _)| n != "frame"));
    let compute = body_of(&funs, "compute").expect("compute body");
    // The slow value stays a compute-local stack declaration.
    assert!(contains_slow_stack_decl(&out.store, compute));
    assert_no_scope_errors(&out.store, out.module);
}

#[test]
fn external_control_moves_slow_values_to_control_with_state_promotion() {
    let options = SignalFirOptions {
        control_rate_mode: ControlRateMode::External,
        ..SignalFirOptions::default()
    };
    let out = compile_slider_gain(&options);
    let funs = function_bodies(&out.store, out.module);
    let control = body_of(&funs, "control").expect("-ec must emit a control function");
    let compute = body_of(&funs, "compute").expect("compute body");
    assert!(funs.iter().all(|(n, _)| n != "frame"));
    // §4.4 promotion: the slow value is stored to DSP state in `control` and
    // never remains a stack local in either function.
    assert!(contains_slow_struct_store(&out.store, control));
    assert!(!contains_slow_stack_decl(&out.store, control));
    assert!(!contains_slow_stack_decl(&out.store, compute));
    assert!(!contains_slow_struct_store(&out.store, compute));
    // `compute` keeps the sample loop and the block I/O aliases.
    assert!(reachable(&out.store, compute).iter().any(|id| matches!(
        match_fir(&out.store, *id),
        FirMatch::DeclareVar { ref name, .. } if name == "output0"
    )));
    assert_no_scope_errors(&out.store, out.module);
}

#[test]
fn one_sample_shape_emits_frame_and_empty_compute() {
    let options = SignalFirOptions {
        processing_api: ProcessingApi::OneSample,
        ..SignalFirOptions::default()
    };
    let out = compile_slider_gain(&options);
    let funs = function_bodies(&out.store, out.module);
    assert!(body_of(&funs, "control").is_none());
    let frame = body_of(&funs, "frame").expect("-os must emit a frame function");
    let compute = body_of(&funs, "compute").expect("compute body");
    // §2.3: canonical compute is kept but emitted empty.
    assert!(matches!(
        match_fir(&out.store, compute),
        FirMatch::Block(stmts) if stmts.is_empty()
    ));
    let frame_nodes = reachable(&out.store, frame);
    // Direct flat-array channel I/O, no sample loop, no count, no aliases.
    assert!(frame_nodes.iter().any(|id| matches!(
        match_fir(&out.store, *id),
        FirMatch::LoadTable { ref name, access: AccessType::FunArgs, .. } if name == "inputs"
    )));
    assert!(frame_nodes.iter().any(|id| matches!(
        match_fir(&out.store, *id),
        FirMatch::StoreTable { ref name, access: AccessType::FunArgs, .. } if name == "outputs"
    )));
    assert!(frame_nodes.iter().all(|id| !matches!(
        match_fir(&out.store, *id),
        FirMatch::SimpleForLoop { .. } | FirMatch::ForLoop { .. }
    )));
    assert!(frame_nodes.iter().all(|id| !matches!(
        match_fir(&out.store, *id),
        FirMatch::LoadVar { ref name, .. } if name == "count" || name == "i0"
    )));
    // Without -ec, the slow value is recomputed inside frame (§2.4).
    assert!(contains_slow_stack_decl(&out.store, frame));
    assert_no_scope_errors(&out.store, out.module);
}

#[test]
fn combined_mode_splits_control_from_frame() {
    let options = SignalFirOptions {
        control_rate_mode: ControlRateMode::External,
        processing_api: ProcessingApi::OneSample,
        ..SignalFirOptions::default()
    };
    let out = compile_slider_gain(&options);
    let funs = function_bodies(&out.store, out.module);
    let control = body_of(&funs, "control").expect("control function");
    let frame = body_of(&funs, "frame").expect("frame function");
    let compute = body_of(&funs, "compute").expect("compute body");
    assert!(matches!(
        match_fir(&out.store, compute),
        FirMatch::Block(stmts) if stmts.is_empty()
    ));
    // Control work lives in `control` only: frame neither declares nor
    // stores slow values, it loads the promoted DSP state.
    assert!(contains_slow_struct_store(&out.store, control));
    assert!(!contains_slow_stack_decl(&out.store, frame));
    assert!(!contains_slow_struct_store(&out.store, frame));
    assert!(reachable(&out.store, frame).iter().any(|id| matches!(
        match_fir(&out.store, *id),
        FirMatch::LoadVar { ref name, access: AccessType::Struct, .. }
            if name.starts_with("fSlow")
    )));
    assert_no_scope_errors(&out.store, out.module);
}

#[test]
fn vector_external_control_promotes_snapshots_and_emits_control() {
    // Plan phase 5: -ec -vec. The certified vector pipeline must emit a
    // `control(dsp)` function holding the UI snapshot stores, and compute
    // must read the promoted DSP state instead of the host-mutated zones.
    use crate::signal_fir::ComputeMode;

    let options = SignalFirOptions {
        compute_mode: ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        },
        control_rate_mode: ControlRateMode::External,
        ..SignalFirOptions::default()
    };
    let out = compile_slider_gain(&options);
    assert_eq!(
        out.vector_pipeline_status,
        crate::signal_fir::VectorPipelineStatus::Certified,
        "the -ec vector path must stay certified: {:?}",
        out.vector_pipeline_detail
    );
    let funs = function_bodies(&out.store, out.module);
    let control = body_of(&funs, "control").expect("-ec -vec must emit a control function");
    let compute = body_of(&funs, "compute").expect("compute body");
    // The snapshot store lives in control (fSlow0 = cast(fHslider0)).
    assert!(contains_slow_struct_store(&out.store, control));
    // Compute must not read the UI zone directly anymore: every zone read
    // goes through the promoted snapshot.
    let compute_nodes = reachable(&out.store, compute);
    assert!(
        compute_nodes.iter().all(|id| !matches!(
            match_fir(&out.store, *id),
            FirMatch::LoadVar { ref name, .. } if name.starts_with("fHslider")
        )),
        "compute must not observe UI zones under external control"
    );
    assert!(compute_nodes.iter().any(|id| matches!(
        match_fir(&out.store, *id),
        FirMatch::LoadVar { ref name, access: AccessType::Struct, .. }
            if name.starts_with("fSlow") || name.starts_with("fVecControlTemp")
    )));
    assert_no_scope_errors(&out.store, out.module);
}

#[test]
fn vector_classic_mode_is_unchanged_by_the_ec_machinery() {
    use crate::signal_fir::ComputeMode;

    let options = SignalFirOptions {
        compute_mode: ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        },
        ..SignalFirOptions::default()
    };
    let out = compile_slider_gain(&options);
    assert_eq!(
        out.vector_pipeline_status,
        crate::signal_fir::VectorPipelineStatus::Certified
    );
    let funs = function_bodies(&out.store, out.module);
    assert!(body_of(&funs, "control").is_none());
    assert_no_scope_errors(&out.store, out.module);
}
