//! Integration tests for `signal_pipeline`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use std::path::PathBuf;

use compiler::Compiler;
use parser::CompilationMetadataKey;
use signals::{BinOp, SigMatch, match_sig};
use tlib::{TreeArena, TreeId, check_de_bruijn_coherence};
use ui::{ControlKind, UiGroupKind, UiMatch, match_ui};

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn compile_corpus(file: &str) -> compiler::SignalCompileOutput {
    let compiler = Compiler::new();
    let path = corpus_path(file);
    compiler
        .compile_file_default_to_signals(&path)
        .unwrap_or_else(|e| panic!("failed to compile {} to signals: {e}", path.display()))
}

fn compile_inline(name: &str, source: &str) -> compiler::SignalCompileOutput {
    Compiler::new()
        .compile_source_to_signals(name, source)
        .unwrap_or_else(|e| panic!("failed to compile {name} to signals: {e}"))
}

fn assert_outputs_de_bruijn_coherent(out: &compiler::SignalCompileOutput) {
    for &sig in &out.signals {
        check_de_bruijn_coherence(&out.parse.state.arena, sig).unwrap_or_else(|e| {
            panic!(
                "output signal {} is not De Bruijn coherent: {e}",
                sig.as_u32()
            )
        });
    }
}

fn expect_ui_group(
    out: &compiler::SignalCompileOutput,
    node: ui::UiId,
    expected_kind: UiGroupKind,
    expected_label: &str,
) -> Vec<ui::UiId> {
    let UiMatch::Group {
        kind,
        label,
        metadata: _,
        children,
    } = match_ui(&out.ui.arena, node)
    else {
        panic!("expected UI group node");
    };
    assert_eq!(kind, expected_kind);
    assert_eq!(label, expected_label);
    children
}

#[test]
fn corpus_passthrough_maps_to_input_signal() {
    let out = compile_corpus("rep_01_passthrough.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Input(0)
    );
}

#[test]
fn corpus_gain_bias_lowers_to_add_mul_and_constant() {
    let out = compile_corpus("rep_02_gain_bias.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);

    let SigMatch::BinOp(BinOp::Add, lhs, rhs) = match_sig(&out.parse.state.arena, out.signals[0])
    else {
        panic!("rep_02 should lower to one add signal");
    };
    assert_mul_input_const(&out.parse.state.arena, lhs, 0);
    assert!(matches!(
        match_sig(&out.parse.state.arena, rhs),
        SigMatch::Real(_) | SigMatch::Int(_)
    ));
}

#[test]
fn corpus_operator_precedence_structure_is_stable() {
    let out = compile_corpus("rep_21_operator_precedence.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);

    // The evaluator constant-folds `1 + 2 * 3 - 4` to `3`.
    assert!(
        matches!(
            match_sig(&out.parse.state.arena, out.signals[0]),
            SigMatch::Int(3)
        ),
        "rep_21 should constant-fold to int(3)"
    );
}

#[test]
fn corpus_feedback_simple_exposes_recursive_projection() {
    let out = compile_corpus("rep_23_feedback_simple.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_basic_expands_pipeline_outputs() {
    let out = compile_corpus("fad_basic.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "f");

    let SigMatch::Sin(primal_input) = match_sig(&out.parse.state.arena, out.signals[0]) else {
        panic!("fad_basic primal output should be sin(f)");
    };
    assert!(matches!(
        match_sig(&out.parse.state.arena, primal_input),
        SigMatch::HSlider(_)
    ));
}

#[test]
fn corpus_fad_multi_seed_emits_one_tangent_per_seed_output() {
    // fad(f*g : sin, (f, g)): the seed box `(f, g)` has 2 outputs, so the
    // fad node bundles two independent differentiation variables.
    // Expected layout: [sin(f*g), cos(f*g)*g, cos(f*g)*f] → 3 outputs.
    let out = compile_corpus("fad_multi_seed.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 3);
    assert_eq!(out.signals.len(), 3);
    assert_eq!(out.ui.controls.len(), 2);
    assert_eq!(out.ui.controls[0].label, "f");
    assert_eq!(out.ui.controls[1].label, "g");
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Sin(_)
    ));
}

#[test]
fn corpus_fad_lambda_recursive_seed_shares_recursion_across_primal_and_tangent() {
    // Regression: seed and body reach the same recursive phi_gen sub-term
    // through a lambda parameter. A buggy `de_bruijn_to_sym` pair allocated
    // two fresh recursion names, forking the shared sub-term and emitting a
    // phantom second recursion in the generated code. This test only asserts
    // that the pipeline accepts the DSP end-to-end with the expected lane
    // count; the structural sharing is exercised by the downstream codegen
    // regression (one `fRec` shared between primal and tangent).
    //
    // Runs on a dedicated thread with a larger stack because the DSP drags
    // `stdfaust.lib` and produces a deep evaluation tree that overflows the
    // default debug-build thread stack.
    let out = std::thread::Builder::new()
        .name("fad-lambda-recursive-seed".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| compile_corpus("fad_lambda_recursive_seed.dsp"))
        .expect("spawn worker")
        .join()
        .expect("worker thread should finish");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert!(out.ui.controls.is_empty());
}

#[test]
fn corpus_fad_nested_on_recursive_seed_emits_three_lanes_one_recursion() {
    // Regression: nested fad(fad(eq,phi),phi) where phi is a recursive
    // accumulator. A prior bug forked phi into two or three SYMREC nodes via
    // fresh-name drift in de_bruijn_to_sym, emitting phantom recursion slots
    // and a stuck-at-zero iRec that silenced audio. The fix defers the single
    // de_bruijn_to_sym conversion to signal_prepare so all outputs share one
    // Converter memo and one recursion name.
    let out = compile_corpus("fad_nested_on_recursive_seed.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 3); // pos, vel, acc
    assert_eq!(out.signals.len(), 3);
    assert!(out.ui.controls.is_empty());
}

#[test]
fn corpus_fad_tanh_ffun_produces_non_zero_tangent() {
    // fad(ma.tanh(p), p): tanh is an external ffunction (SigMatch::FFun).
    // The FAD pass must recognise the "tanhf"/"tanh"/"tanhl" name family and
    // apply d/dp tanh(x) = (1 - tanh²(x)) · x'.
    // Expected: 2 outputs (primal + tangent); tangent must not be constant 0.
    //
    // Runs on a 64 MiB thread because stdfaust.lib produces a deep evaluation
    // tree that overflows the default debug-build thread stack.
    let out = std::thread::Builder::new()
        .name("fad-tanh".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| compile_corpus("fad_tanh.dsp"))
        .expect("spawn worker")
        .join()
        .expect("worker thread should finish");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "p");

    // Primal must be an FFun node (tanh applied to the slider).
    assert!(
        matches!(
            signals::match_sig(&out.parse.state.arena, out.signals[0]),
            signals::SigMatch::FFun(_, _)
        ),
        "primal of fad(tanh(p), p) should be SIGFFUN"
    );
    // Tangent must not be the constant 0 — it encodes (1 − tanh²(p)).
    assert!(
        !matches!(
            signals::match_sig(&out.parse.state.arena, out.signals[1]),
            signals::SigMatch::Real(v) if v == 0.0
        ),
        "tangent of tanh(p) w.r.t. p must not be constant zero"
    );
}

/// Asserts that `fad(<hyperbolic>(p), p)` emits 2 signals where the tangent is
/// a non-zero `SIGFFUN` expression (the primal is also `SIGFFUN`).
/// Runs on a 64 MiB thread because stdfaust.lib causes a deep evaluation tree.
fn assert_fad_hyperbolic(dsp: &'static str) {
    let out = std::thread::Builder::new()
        .name(format!("fad-{dsp}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || compile_corpus(dsp))
        .expect("spawn worker")
        .join()
        .expect("worker thread should finish");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(
        out.process_arity.outputs, 2,
        "{dsp}: must emit [primal, tangent]"
    );
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(
        !matches!(
            signals::match_sig(&out.parse.state.arena, out.signals[1]),
            signals::SigMatch::Real(v) if v == 0.0
        ),
        "{dsp}: tangent must not be constant zero"
    );
}

#[test]
fn corpus_fad_sinh_ffun_produces_non_zero_tangent() {
    assert_fad_hyperbolic("fad_sinh.dsp");
}

#[test]
fn corpus_fad_cosh_ffun_produces_non_zero_tangent() {
    assert_fad_hyperbolic("fad_cosh.dsp");
}

#[test]
fn corpus_fad_atanh_ffun_produces_non_zero_tangent() {
    assert_fad_hyperbolic("fad_atanh.dsp");
}

#[test]
fn corpus_fad_asinh_ffun_produces_non_zero_tangent() {
    assert_fad_hyperbolic("fad_asinh.dsp");
}

#[test]
fn corpus_fad_acosh_ffun_produces_non_zero_tangent() {
    assert_fad_hyperbolic("fad_acosh.dsp");
}

#[test]
fn corpus_fad_product_emits_one_tangent_per_seed() {
    // fad(f * g, f): differentiates wrt f only → primal + 1 tangent = 2 signals
    let out = compile_corpus("fad_product.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
    assert_eq!(out.ui.controls[0].label, "f");
    assert_eq!(out.ui.controls[1].label, "g");
}

#[test]
fn corpus_fad_waveform_index_basic_emits_symmetric_lookup_slope_tangent() {
    let out = compile_corpus("fad_waveform_index_basic.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "k");
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::RdTbl(_, _)
    ));
    assert!(
        !matches!(
            match_sig(&out.parse.state.arena, out.signals[1]),
            SigMatch::Real(v) if v == 0.0
        ),
        "read-only waveform lookup tangent must not be constant zero"
    );
}

#[test]
fn corpus_fad_rdtbl_index_basic_emits_symmetric_lookup_slope_tangent() {
    let out = compile_corpus("fad_rdtbl_index_basic.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "k");
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::RdTbl(_, _)
    ));
    assert!(
        !matches!(
            match_sig(&out.parse.state.arena, out.signals[1]),
            SigMatch::Real(v) if v == 0.0
        ),
        "read-only generator table tangent must not be constant zero"
    );
}

#[test]
fn corpus_fad_rwtable_index_keeps_zero_tangent() {
    let out = compile_corpus("fad_rwtable_index_zero_tangent.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::RdTbl(_, _)
    ));
    assert!(
        matches!(
            match_sig(&out.parse.state.arena, out.signals[1]),
            SigMatch::Real(v) if v == 0.0
        ),
        "writable-table lookup tangent should remain zero in phase 1"
    );
}

#[test]
fn corpus_fad_recursive_waveform_index_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_waveform_index.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "step");
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::RdTbl(_, _)
    ));
    assert!(
        !matches!(
            match_sig(&out.parse.state.arena, out.signals[1]),
            SigMatch::Real(v) if v == 0.0
        ),
        "recursive read-index tangent must not be constant zero"
    );
}

#[test]
fn corpus_fad_recursive_compiles_through_full_signal_pipeline() {
    // fad(fb : +~*(g), fb): differentiates wrt fb → primal + 1 tangent = 2 signals
    let out = compile_corpus("fad_recursive.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_branch_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_branch.dsp");
    // +~(fad(*(g))): 1 audio input, expanded outputs
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    // Rec exposes primal + FAD tangent from feedback branch: 1 primal + 1 tangent for "g"
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    // Primal output is a recursive projection
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
    // Tangent output is also a recursive projection (tangent propagates through recursion)
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_left_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_left.dsp");
    // fad(+)~*(g): 1 audio input, expanded outputs
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    // 1 primal + 1 tangent for "g"
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_recursive_both_compiles_through_full_signal_pipeline() {
    // fad(+, g)~fad(*(g), g): 1 audio input, 2 FAD nodes both wrt g
    let out = compile_corpus("fad_recursive_both.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.ui.controls.len(), 1);
    // Rec has 2 FAD nodes → outputs * (1 + 2) = 1 * 3 = 3 signals
    assert_eq!(out.process_arity.outputs, 3);
    assert_eq!(out.signals.len(), 3);
}

#[test]
fn corpus_fad_recursive_deep_right_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_deep_right.dsp");
    // +~vgroup("fb", fad(*(g))): FAD nested inside vgroup inside feedback
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_recursive_deep_left_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_deep_left.dsp");
    // vgroup("sum", fad(+))~*(g): FAD nested inside vgroup in left branch
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_recursive_deep_both_compiles_through_full_signal_pipeline() {
    // vgroup("sum", fad(+, g))~vgroup("fb", fad(*(g), g)): 2 FAD nodes → 3 outputs
    let out = compile_corpus("fad_recursive_deep_both.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 3);
    assert_eq!(out.signals.len(), 3);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_gradient_host_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_gradient_host.dsp");
    // fad(error): error = (input*g - input*target)^2, 1 control → 2 signals
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_triple_chain_compiles_through_full_signal_pipeline() {
    // fad(a * b * c, a): differentiates wrt a only → primal + 1 tangent = 2 signals
    let out = compile_corpus("fad_triple_chain.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 3);
}

#[test]
fn corpus_fad_trig_composition_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_trig_composition.dsp");
    // fad(f : sin : cos): 0 inputs, 1 control → 2 signals
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_division_compiles_through_full_signal_pipeline() {
    // fad(num / den, num): differentiates wrt num only → 2 signals
    let out = compile_corpus("fad_division.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
}

#[test]
fn corpus_fad_power_compiles_through_full_signal_pipeline() {
    // fad(base ^ exp, base): differentiates wrt base only → 2 signals
    let out = compile_corpus("fad_power.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
}

#[test]
fn corpus_fad_delay_variable_compiles_through_full_signal_pipeline() {
    // fad(_ * g : @(d), g): differentiates wrt g only → 1 input, 2 signals
    let out = compile_corpus("fad_delay_variable.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
}

#[test]
fn corpus_fad_recursive_multi_control_compiles_through_full_signal_pipeline() {
    // fad(+ ~ *(fb) : *(vol), fb): differentiates wrt fb only → 1 input, 2 signals
    let out = compile_corpus("fad_recursive_multi_control.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
}

#[test]
fn corpus_fad_recursive_delay_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_delay.dsp");
    // fad(+ ~ (@(128) : *(fb))): 1 input, 1 control → 2 signals
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn corpus_fad_recursive_parametric_self_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_parametric_self.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    let SigMatch::Proj(_, primal_group) = match_sig(&out.parse.state.arena, out.signals[0]) else {
        panic!("fad_recursive_parametric_self primal output should be a recursive projection");
    };
    let SigMatch::Proj(_, tangent_group) = match_sig(&out.parse.state.arena, out.signals[1]) else {
        panic!("fad_recursive_parametric_self tangent output should be a recursive projection");
    };
    assert_eq!(
        primal_group, tangent_group,
        "unified FAD recursion should project primal and tangent from the same recursive group"
    );
}

#[test]
fn corpus_fad_recursive_nested_parametric_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_nested_parametric.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_triple_nested_parametric_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_triple_nested_parametric.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_multi_output_parametric_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_multi_output_parametric.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 4);
    assert_eq!(out.signals.len(), 4);
    assert_eq!(out.ui.controls.len(), 1);
    for sig in &out.signals {
        assert!(matches!(
            match_sig(&out.parse.state.arena, *sig),
            SigMatch::Proj(_, _)
        ));
    }
}

#[test]
fn corpus_fad_recursive_multi_seed_shared_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_multi_seed_shared.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 3);
    assert_eq!(out.signals.len(), 3);
    assert_eq!(out.ui.controls.len(), 2);

    let mut groups = Vec::new();
    for sig in &out.signals {
        let SigMatch::Proj(_, group) = match_sig(&out.parse.state.arena, *sig) else {
            panic!("fad_recursive_multi_seed_shared outputs should all be recursive projections");
        };
        groups.push(group);
    }

    assert!(
        groups.windows(2).all(|window| window[0] == window[1]),
        "unified multi-seed FAD recursion should project every output from the same recursive group"
    );
}

#[test]
fn corpus_fad_recursive_mutual_multi_seed_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_mutual_multi_seed.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 6);
    assert_eq!(out.signals.len(), 6);
    assert_eq!(out.ui.controls.len(), 2);
    for sig in &out.signals {
        assert!(matches!(
            match_sig(&out.parse.state.arena, *sig),
            SigMatch::Proj(_, _)
        ));
    }
}

#[test]
fn corpus_fad_recursive_mutual_crossed_parametric_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_mutual_crossed_parametric.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 4);
    assert_eq!(out.signals.len(), 4);
    assert_eq!(out.ui.controls.len(), 1);
    for sig in &out.signals {
        assert!(matches!(
            match_sig(&out.parse.state.arena, *sig),
            SigMatch::Proj(_, _)
        ));
    }
}

#[test]
fn corpus_fad_recursive_nested_mutual_parametric_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_nested_mutual_parametric.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_local_projection_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_local_projection.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 2);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_pair_reduction_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_pair_reduction.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_multi_local_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_multi_local.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 2);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_recursive_multilane_local_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_recursive_multilane_local.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_recursive_fad_tangent_projection_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_recursive_fad_tangent_projection.dsp",
        r#"
        process = step ~ _
        with {
            target = hslider("Target", 0, -1, 1, 0.01);
            lr = hslider("LR", 0.05, 0.0001, 0.5, 0.0001);
            step(prev) = prev - lr * grad
            with {
                loss = (prev - target) ^ 2;
                grad = fad(loss, prev) : !, _;
            };
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 2);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_recursive_fad_nested_inner_rec_back_edge_is_debruijn_coherent() {
    let out = compile_inline(
        "inline_recursive_fad_nested_inner_rec_back_edge.dsp",
        r#"
        process = step ~ _
        with {
            target = hslider("Target", 0.25, -1, 1, 0.01);
            step(prev) = prev + 0.01 * grad + 0.001 * inner
            with {
                inner = (+(prev) : *(0.5)) ~ _;
                loss = (inner + prev - target) ^ 2;
                grad = fad(loss, prev) : !, _;
            };
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_outputs_de_bruijn_coherent(&out);
}

#[test]
fn inline_recursive_fad_alternate_local_projection_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_recursive_fad_alternate_local_projection.dsp",
        r#"
        process = step ~ _
        with {
            target = hslider("Target", 0, -1, 1, 0.01);
            step(prev) = fad((prev - target) ^ 2, prev) : _, !;
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_recursive_fad_pair_reduction_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_recursive_fad_pair_reduction.dsp",
        r#"
        process = step ~ _
        with {
            target = hslider("Target", 0, -1, 1, 0.01);
            step(prev) = fad((prev - target) ^ 2, prev) : (_, _) : +;
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_recursive_multiple_local_fads_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_recursive_multiple_local_fads.dsp",
        r#"
        process = step ~ _
        with {
            target = hslider("Target", 0, -1, 1, 0.01);
            lr = hslider("LR", 0.05, 0.0001, 0.5, 0.0001);
            step(prev) = prev - lr * (g1 + g2)
            with {
                g1 = fad((prev - target) ^ 2, prev) : !, _;
                g2 = fad((prev + target) ^ 2, prev) : !, _;
            };
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 2);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_recursive_fad_under_vgroup_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_recursive_fad_under_vgroup.dsp",
        r#"
        process = step ~ _
        with {
            target = hslider("Target", 0, -1, 1, 0.01);
            lr = hslider("LR", 0.05, 0.0001, 0.5, 0.0001);
            step(prev) = prev - lr * grad
            with {
                grad = vgroup("g", fad((prev - target) ^ 2, prev)) : !, _;
            };
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 2);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_recursive_fad_with_derived_seed_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_recursive_fad_with_derived_seed.dsp",
        r#"
        process = step ~ _
        with {
            step(prev) = grad
            with {
                seed = abs(prev);
                grad = fad(seed * seed, seed) : !, _;
            };
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(out.ui.controls.len(), 0);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn inline_multilane_recursive_fad_local_projection_compiles_through_full_signal_pipeline() {
    let out = compile_inline(
        "inline_multilane_recursive_fad_local_projection.dsp",
        r#"
        process = step ~ (_, _)
        with {
            target = hslider("Target", 0, -1, 1, 0.01);
            step(a, b) = (fad((a - target) ^ 2, a) : !, _, b);
        };
        "#,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Proj(_, _)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Proj(_, _)
    ));
}

#[test]
fn corpus_fad_minmax_compiles_through_full_signal_pipeline() {
    // fad(min(a, b), a): differentiates wrt a only → 0 inputs, 2 signals
    let out = compile_corpus("fad_minmax.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 2);
}

#[test]
fn corpus_fad_select2_compiles_through_full_signal_pipeline() {
    // fad(select2(sel, a, b), a): differentiates wrt a only → 0 inputs, 2 signals
    let out = compile_corpus("fad_select2.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 3);
}

#[test]
fn corpus_fad_math_chain_compiles_through_full_signal_pipeline() {
    let out = compile_corpus("fad_math_chain.dsp");
    // fad(x : exp : log : sqrt : abs): 0 inputs, 1 control → 2 signals
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn inline_fad_seq_with_identity_pair_compiles() {
    // fad(*(g), g) : (_,_): fad produces primal + tangent = 2 outputs → matches (_,_) inputs.
    let source = r#"process = fad(*(hslider("g", 0.5, 0, 1, 0.01)), hslider("g", 0.5, 0, 1, 0.01)) : (_,_);"#;
    let out = compile_inline("fad_seq_identity.dsp", source);
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);
    assert_eq!(out.ui.controls.len(), 1);
}

#[test]
fn inline_fad_body_uses_body_input_slice_when_seed_bus_is_wider() {
    let source = r#"process = fad(*, (_,_,_));"#;
    let out = compile_inline("fad_wide_seed_bus.dsp", source);
    assert_eq!(out.process_arity.inputs, 3);
    assert_eq!(out.process_arity.outputs, 4);
    assert_eq!(out.signals.len(), 4);
}

#[test]
fn inline_rad_body_uses_body_input_slice_when_seed_bus_is_wider() {
    let source = r#"process = rad(*, (_,_,_));"#;
    let out = compile_inline("rad_wide_seed_bus.dsp", source);
    assert_eq!(out.process_arity.inputs, 3);
    assert_eq!(out.process_arity.outputs, 4);
    assert_eq!(out.signals.len(), 4);
}

#[test]
fn inline_partial_mul_with_trigger_argument_compiles_to_signal_mul() {
    let source = r#"
upfront(x) = (x-x') > 0.0;
decay(n,x) = x - (x>0.0)/n;
release(n) = + ~ decay(n);
trigger(n) = upfront : release(n) : >(0.0);
process = *(button("play") : trigger(128));
"#;

    let out = compile_inline("partial_mul_trigger.dsp", source);
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);

    let SigMatch::BinOp(BinOp::Mul, lhs, rhs) = match_sig(&out.parse.state.arena, out.signals[0])
    else {
        panic!("partial mul trigger repro should lower to one mul signal");
    };
    assert!(
        matches!(match_sig(&out.parse.state.arena, lhs), SigMatch::Input(0))
            || matches!(match_sig(&out.parse.state.arena, rhs), SigMatch::Input(0)),
        "one mul operand should be the implicit input wire"
    );
}

#[test]
fn corpus_two_in_two_out_ui_lowers_to_input_slider_muls() {
    let out = compile_corpus("rep_10_two_in_two_out_ui.dsp");
    assert_eq!(out.process_arity.inputs, 2);
    assert_eq!(out.process_arity.outputs, 2);
    assert_eq!(out.signals.len(), 2);

    assert_mul_input_ui(&out.parse.state.arena, out.signals[0], 0);
    assert_mul_input_ui(&out.parse.state.arena, out.signals[1], 1);
}

#[test]
fn corpus_parallel_mix_lowers_to_sum_of_two_scaled_inputs() {
    let out = compile_corpus("rep_22_parallel_mix.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);

    let SigMatch::BinOp(BinOp::Add, lhs, rhs) = match_sig(&out.parse.state.arena, out.signals[0])
    else {
        panic!("rep_22 should lower to one add signal");
    };
    assert_mul_input_const(&out.parse.state.arena, lhs, 0);
    assert_mul_input_const(&out.parse.state.arena, rhs, 0);
}

#[test]
fn corpus_environment_waveform_combines_access_and_waveform_outputs() {
    let out = compile_corpus("rep_20_environment_waveform.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 3);
    assert_eq!(out.signals.len(), 3);

    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Input(0)
    );
    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[1]),
        SigMatch::Int(3)
    );

    let SigMatch::Waveform(values) = match_sig(&out.parse.state.arena, out.signals[2]) else {
        panic!("rep_20 third output should be waveform");
    };
    assert_eq!(values.len(), 3);
    assert!(matches!(
        match_sig(&out.parse.state.arena, values[0]),
        SigMatch::Int(1)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, values[1]),
        SigMatch::Int(-2)
    ));
    assert!(matches!(
        match_sig(&out.parse.state.arena, values[2]),
        SigMatch::Real(_)
    ));
}

#[test]
fn corpus_nonlinear_clip_lowers_to_min_max_shape() {
    let out = compile_corpus("rep_07_nonlinear_clip.dsp");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);

    let SigMatch::Max(lo, inner) = match_sig(&out.parse.state.arena, out.signals[0]) else {
        panic!("rep_07 should lower to max(lo, ...)");
    };
    assert!(matches!(
        match_sig(&out.parse.state.arena, lo),
        SigMatch::Real(_)
    ));
    let SigMatch::Min(hi, mul) = match_sig(&out.parse.state.arena, inner) else {
        panic!("rep_07 inner should lower to min(hi, ...)");
    };
    assert!(matches!(
        match_sig(&out.parse.state.arena, hi),
        SigMatch::Real(_)
    ));
    assert_mul_input_const(&out.parse.state.arena, mul, 0);
}

#[test]
fn corpus_primitive_family_includes_pow_output() {
    let out = compile_corpus("rep_19_primitive_family.dsp");
    assert_eq!(out.process_arity.outputs, 10);
    assert_eq!(out.signals.len(), 10);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Pow(_, _)
    ));
}

#[test]
fn corpus_extended_primitives_cover_unary_and_binary_signal_nodes() {
    let out = compile_corpus("rep_31_extended_primitives.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 20);
    assert_eq!(out.signals.len(), 20);

    let got: Vec<SigMatch<'_>> = out
        .signals
        .iter()
        .copied()
        .map(|s| match_sig(&out.parse.state.arena, s))
        .collect();

    assert!(matches!(got[0], SigMatch::Acos(_)));
    assert!(matches!(got[1], SigMatch::Asin(_)));
    assert!(matches!(got[2], SigMatch::Atan(_)));
    assert!(matches!(got[3], SigMatch::Atan2(_, _)));
    assert!(matches!(got[4], SigMatch::Cos(_)));
    assert!(matches!(got[5], SigMatch::Sin(_)));
    assert!(matches!(got[6], SigMatch::Tan(_)));
    assert!(matches!(got[7], SigMatch::Exp(_)));
    assert!(matches!(got[8], SigMatch::Log(_)));
    assert!(matches!(got[9], SigMatch::Log10(_)));
    assert!(matches!(got[10], SigMatch::Sqrt(_)));
    assert!(matches!(got[11], SigMatch::Abs(_)));
    assert!(matches!(got[12], SigMatch::Min(_, _)));
    assert!(matches!(got[13], SigMatch::Max(_, _)));
    assert!(matches!(got[14], SigMatch::Fmod(_, _)));
    assert!(matches!(got[15], SigMatch::Remainder(_, _)));
    assert!(matches!(got[16], SigMatch::Floor(_)));
    assert!(matches!(got[17], SigMatch::Ceil(_)));
    assert!(matches!(got[18], SigMatch::Rint(_)));
    assert!(matches!(got[19], SigMatch::Round(_)));
}

#[test]
fn corpus_higher_order_named_direct_apply_lowers_checkbox_ui() {
    let out = compile_corpus("rep_58_higher_order_named_direct_apply.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Checkbox(_)
    ));
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Horizontal, "top");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].kind, ControlKind::Checkbox);
    assert_eq!(out.ui.controls[0].label, "c");
}

#[test]
fn corpus_higher_order_named_argument_apply_lowers_checkbox_ui() {
    let out = compile_corpus("rep_59_higher_order_named_argument_apply.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert!(matches!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Checkbox(_)
    ));
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Horizontal, "top");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].kind, ControlKind::Checkbox);
    assert_eq!(out.ui.controls[0].label, "c");
}

#[test]
fn inline_empty_root_group_uses_source_stem_in_ui_program() {
    let out = compile_inline(
        "empty_root_group.dsp",
        r#"process = vgroup("", checkbox("c"));"#,
    );
    let root_children =
        expect_ui_group(&out, out.ui.root, UiGroupKind::Vertical, "empty_root_group");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
}

#[test]
fn inline_empty_root_group_prefers_declared_name_metadata() {
    let out = compile_inline(
        "ignored_root_name.dsp",
        "declare name \"main\";\nprocess = vgroup(\"\", checkbox(\"c\"));\n",
    );
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Vertical, "main");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
}

#[test]
fn corpus_group_label_substitution_reaches_compiler_ui_output() {
    let out = compile_corpus("rep_52_eval_label_group_path_subst.dsp");
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Vertical, "main3");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].kind, ControlKind::HSlider);
    assert_eq!(out.ui.controls[0].label, "gain");
}

#[test]
fn corpus_sine_phasor_lowers_to_gain_times_sin_of_feedback_phase() {
    let out = compile_corpus("rep_38_sine_phasor.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, out.signals.len());
    assert!(
        !out.signals.is_empty(),
        "rep_38 should expose at least one output signal"
    );

    for sig in &out.signals {
        let SigMatch::BinOp(BinOp::Mul, gain, carrier) = match_sig(&out.parse.state.arena, *sig)
        else {
            panic!("rep_38 should lower each output to gain * sin(phase)");
        };
        assert!(matches!(
            match_sig(&out.parse.state.arena, gain),
            SigMatch::HSlider(_) | SigMatch::VSlider(_) | SigMatch::NumEntry(_) | SigMatch::Real(_)
        ));
        assert!(matches!(
            match_sig(&out.parse.state.arena, carrier),
            SigMatch::Sin(_)
        ));
    }
}

#[test]
fn corpus_metadata_master_is_exposed_in_compilation_output() {
    let out = compile_corpus("rep_40_metadata_master.dsp");
    let values = out
        .compilation_metadata
        .entries()
        .get(&CompilationMetadataKey::global("name"))
        .expect("master metadata key should exist");
    assert!(values.contains("main"));
}

#[test]
fn corpus_metadata_import_is_prefixed_like_cpp() {
    let out = compile_corpus("rep_41_metadata_import.dsp");
    let imported = corpus_path("metadata/imported_meta.dsp")
        .canonicalize()
        .expect("imported metadata file should canonicalize");
    let values = out
        .compilation_metadata
        .entries()
        .get(&CompilationMetadataKey::scoped(
            imported.to_string_lossy().into_owned(),
            "author",
        ))
        .expect("imported metadata key should exist");
    assert!(values.contains("imported-author"));
}

#[test]
fn corpus_component_metadata_is_aggregated_through_eval_loading() {
    let out = compile_corpus("rep_42_component_metadata.dsp");
    let child = corpus_path("metadata/component_child.dsp")
        .canonicalize()
        .expect("component child should canonicalize");
    let values = out
        .compilation_metadata
        .entries()
        .get(&CompilationMetadataKey::scoped(
            child.to_string_lossy().into_owned(),
            "author",
        ))
        .expect("component metadata key should exist");
    assert!(values.contains("component-author"));
}

#[test]
fn corpus_library_metadata_is_aggregated_through_eval_loading() {
    let out = compile_corpus("rep_43_library_metadata.dsp");
    let child = corpus_path("metadata/library_child.dsp")
        .canonicalize()
        .expect("library child should canonicalize");
    let values = out
        .compilation_metadata
        .entries()
        .get(&CompilationMetadataKey::scoped(
            child.to_string_lossy().into_owned(),
            "author",
        ))
        .expect("library metadata key should exist");
    assert!(values.contains("library-author"));
}

#[test]
fn corpus_modulation_wrappers_without_matching_widgets_reduce_to_identity() {
    for file in [
        "rep_32_modulation_single.dsp",
        "rep_33_modulation_chain.dsp",
    ] {
        let out = compile_corpus(file);
        assert_eq!(out.process_arity.inputs, 1, "{file} should keep 1 input");
        assert_eq!(out.process_arity.outputs, 1, "{file} should keep 1 output");
        assert_eq!(out.signals.len(), 1, "{file} should lower to one signal");
        assert_eq!(
            match_sig(&out.parse.state.arena, out.signals[0]),
            SigMatch::Input(0),
            "{file} should reduce to input(0)"
        );
    }
}

#[test]
fn closure_captured_case_results_keep_distinct_environments() {
    let source = r#"
make(x) = case { (0) => x; };
process = make(1)(0) + make(2)(0);
"#;
    let out = compile_inline(
        "closure_captured_case_results_keep_distinct_environments.dsp",
        source,
    );
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);

    // The evaluator constant-folds `1 + 2` to `3`.
    // The important invariant is that the two `make` calls produce distinct
    // captured values (1 and 2) rather than aliasing, which is verified by
    // the final folded result being 3 (not 2 or 4).
    assert!(
        matches!(
            match_sig(&out.parse.state.arena, out.signals[0]),
            SigMatch::Int(3)
        ),
        "captured case results should fold to int(3)"
    );
}

#[test]
fn corpus_eval_label_widget_substitution_reaches_ui_controls() {
    let out = compile_corpus("rep_51_eval_label_widget_subst.dsp");
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "gain3");
}

#[test]
fn corpus_eval_label_group_path_substitution_reaches_ui_controls() {
    let out = compile_corpus("rep_52_eval_label_group_path_subst.dsp");
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "gain");
}

#[test]
fn corpus_eval_label_modulation_target_substitution_matches_ui_control() {
    let out = compile_corpus("rep_53_eval_label_modulation_target_subst.dsp");
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "gain3");
}

#[test]
fn corpus_metadata_bearing_widget_label_splits_display_label_and_ui_metadata() {
    let out = compile_corpus("rep_56_noise_smoo_slider.dsp");
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "gain");
    assert_eq!(
        out.ui.controls[0].metadata,
        vec![("style".to_owned(), "knob".to_owned())]
    );
}

#[test]
fn corpus_relative_widget_path_rebases_to_parent_group() {
    let out = compile_corpus("rep_60_ui_relative_widget_path.dsp");
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Horizontal, "Foo");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].kind, ControlKind::HSlider);
    assert_eq!(out.ui.controls[0].label, "volume");
}

#[test]
fn corpus_typed_widget_path_creates_typed_group() {
    let out = compile_corpus("rep_61_ui_typed_widget_path.dsp");
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Horizontal, "Oscillator");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].kind, ControlKind::HSlider);
    assert_eq!(out.ui.controls[0].label, "freq");
}

#[test]
fn corpus_relative_widget_path_metadata_survives_rebase() {
    let out = compile_corpus("rep_62_ui_relative_widget_path_metadata.dsp");
    let root_children = expect_ui_group(&out, out.ui.root, UiGroupKind::Horizontal, "Foo");
    assert_eq!(root_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, root_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "gain");
    assert_eq!(
        out.ui.controls[0].metadata,
        vec![("style".to_owned(), "knob".to_owned())]
    );
}

#[test]
fn corpus_relative_group_label_rebases_explicit_group_to_parent() {
    let out = compile_corpus("rep_63_ui_relative_group_rebase.dsp");
    let root_children = expect_ui_group(
        &out,
        out.ui.root,
        UiGroupKind::Vertical,
        "rep_63_ui_relative_group_rebase",
    );
    assert_eq!(root_children.len(), 2);
    // Root-forest order keeps insertion order (the `../` rebase is a Rust-side
    // extension with no C++ reference; lexicographic ordering applies only to
    // children within a group).
    assert!(expect_ui_group(&out, root_children[0], UiGroupKind::Horizontal, "Foo").is_empty());
    let bar_children = expect_ui_group(&out, root_children[1], UiGroupKind::Vertical, "Bar");
    assert_eq!(bar_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, bar_children[0]),
        UiMatch::InputControl(0)
    );
    assert_eq!(out.ui.controls.len(), 1);
    assert_eq!(out.ui.controls[0].label, "gain");
}

#[test]
fn corpus_relative_group_label_clamps_navigation_at_root() {
    let out = compile_corpus("rep_64_ui_relative_group_root_clamp.dsp");
    let root_children = expect_ui_group(
        &out,
        out.ui.root,
        UiGroupKind::Vertical,
        "rep_64_ui_relative_group_root_clamp",
    );
    assert_eq!(root_children.len(), 2);
    // Root-forest order keeps insertion order (the `../` rebase is a Rust-side
    // extension with no C++ reference; lexicographic ordering applies only to
    // children within a group).
    assert!(expect_ui_group(&out, root_children[0], UiGroupKind::Horizontal, "Foo").is_empty());
    let bar_children = expect_ui_group(&out, root_children[1], UiGroupKind::Vertical, "Bar");
    assert_eq!(bar_children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, bar_children[0]),
        UiMatch::InputControl(0)
    );
}

#[test]
fn inline_group_label_metadata_splits_display_label_and_group_metadata() {
    let out = compile_inline(
        "group_metadata",
        "process = hgroup(\"top [tooltip:hello]\", checkbox(\"c\"));\n",
    );
    let UiMatch::Group {
        kind,
        label,
        metadata,
        children,
    } = match_ui(&out.ui.arena, out.ui.root)
    else {
        panic!("expected root grouped UI");
    };
    assert_eq!(kind, UiGroupKind::Horizontal);
    assert_eq!(label, "top");
    assert_eq!(metadata, vec![("tooltip".to_owned(), "hello".to_owned())]);
    assert_eq!(children.len(), 1);
    assert_eq!(
        match_ui(&out.ui.arena, children[0]),
        UiMatch::InputControl(0)
    );
}

/// Regression test for rep_72: float literal patterns must not be coerced to
/// integers during argument simplification.
///
/// `foo2(1.0) = 456;` stores a `float_bits(1.0)` constant transition.
/// The old `simplify_pattern` fast-path converted `Real(1.0)` → `Int(1)`,
/// so the TreeId equality check `int(1) == float_bits(1.0)` failed.
#[test]
fn corpus_float_literal_pattern_matching_compiles_like_cpp() {
    let out = compile_corpus("rep_72_float_literal_pattern.dsp");
    // 4 outputs, 0 inputs — all four pattern functions are constants.
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 4);
    // Every output must simplify to a numeric constant.
    for sig in &out.signals {
        assert!(
            matches!(
                match_sig(&out.parse.state.arena, *sig),
                SigMatch::Int(_) | SigMatch::Real(_)
            ),
            "each output of rep_72 should be a numeric constant, got {:?}",
            match_sig(&out.parse.state.arena, *sig)
        );
    }
}

/// Regression test for rep_73: `patternSimplification` must fold `max`/`min`
/// expressions via full signal propagation, not just literal arithmetic.
///
/// `f(max(1, min(6, 4)))` — `max(1, min(6, 4))` must be reduced to `boxInt(4)`
/// at automaton construction time so the rule `f(4) = 40` matches.
/// The old `pattern_simplification` only handled direct literal arithmetic and
/// could not fold xtended functions like `max`/`min`.
#[test]
fn corpus_pattern_max_min_fold_compiles_like_cpp() {
    let out = compile_corpus("rep_73_pattern_max_min_fold.dsp");
    assert_eq!(out.process_arity.inputs, 0);
    assert_eq!(out.process_arity.outputs, 1);
    assert!(
        matches!(
            match_sig(&out.parse.state.arena, out.signals[0]),
            SigMatch::Int(40)
        ),
        "process should be SigInt(40), got {:?}",
        match_sig(&out.parse.state.arena, out.signals[0])
    );
}

fn assert_mul_input_const(arena: &TreeArena, sig: TreeId, expected_input: i32) {
    let SigMatch::BinOp(BinOp::Mul, a, b) = match_sig(arena, sig) else {
        panic!("branch should be Mul");
    };
    let am = match_sig(arena, a);
    let bm = match_sig(arena, b);
    let ok = matches!(
        (am, bm),
        (SigMatch::Input(i), SigMatch::Real(_))
            | (SigMatch::Real(_), SigMatch::Input(i))
            | (SigMatch::Input(i), SigMatch::Int(_))
            | (SigMatch::Int(_), SigMatch::Input(i))
            if i == expected_input
    );
    assert!(
        ok,
        "mul branch should combine input({expected_input}) with constant"
    );
}

fn assert_mul_input_ui(arena: &TreeArena, sig: TreeId, expected_input: i32) {
    let SigMatch::BinOp(BinOp::Mul, a, b) = match_sig(arena, sig) else {
        panic!("branch should be Mul");
    };
    let am = match_sig(arena, a);
    let bm = match_sig(arena, b);
    let ok = matches!(
        (am, bm),
        (SigMatch::Input(i), SigMatch::HSlider(_))
            | (SigMatch::HSlider(_), SigMatch::Input(i))
            if i == expected_input
    );
    assert!(
        ok,
        "mul branch should combine input({expected_input}) with hslider"
    );
}
