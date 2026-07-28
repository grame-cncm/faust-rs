//! Shared test DSL for the `signal_fir` test suite: forest builders,
//! FIR navigation helpers, and the UI fixture used across the groups.

use crate::signal_fir::{
    SignalFirOptions, compile_signals_to_fir_fastlane_with_ui,
    delay::{DelayManager, DelayOptions, plan_delays},
};
use fir::{FirMatch, FirType, match_fir};
use tlib::TreeArena;
use ui::{ControlKind, ControlRange, ControlSpec, UiBuilder, UiProgram, UiRootOrigin};

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
pub(super) fn unwrap_output_cast(store: &fir::FirStore, id: fir::FirId) -> fir::FirId {
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
pub(super) fn find_decl_fun_body(
    store: &fir::FirStore,
    functions: fir::FirId,
    target: &str,
) -> fir::FirId {
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
pub(super) fn find_compute_loop_body(store: &fir::FirStore, functions: fir::FirId) -> fir::FirId {
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
pub(super) fn find_compute_simple_loop_reverse_flag(
    store: &fir::FirStore,
    functions: fir::FirId,
) -> bool {
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
pub(super) fn compile_fastlane_without_ui(
    arena: &TreeArena,
    signals: &[signals::SigId],
    num_inputs: usize,
    num_outputs: usize,
    options: &SignalFirOptions,
) -> Result<crate::signal_fir::SignalFirOutput, crate::signal_fir::SignalFirError> {
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
pub(super) fn analyze_delays_for_prepared(
    prepared: &crate::signal_prepare::PreparedSignals,
) -> DelayManager {
    let mut delay = DelayManager::new(DelayOptions::default());
    let plan = plan_delays(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
        &delay.options(),
        None,
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
pub(super) fn one_control_ui(
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
            source_node: None,
        }],
        root_origin: UiRootOrigin::Synthesized,
        emit_ui: true,
    }
}
// ── Variability-driven statement placement (Phase 1) ────────────────────

/// Helper: returns the compute body block as a `Vec<FirId>`.
pub(super) fn find_compute_body_stmts(
    store: &fir::FirStore,
    functions: fir::FirId,
) -> Vec<fir::FirId> {
    let body = find_decl_fun_body(store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(store, body) else {
        panic!("compute body block expected");
    };
    stmts
}
/// Helper: returns the `instanceConstants` body block as a `Vec<FirId>`.
pub(super) fn find_instance_constants_stmts(
    store: &fir::FirStore,
    functions: fir::FirId,
) -> Vec<fir::FirId> {
    let body = find_decl_fun_body(store, functions, "instanceConstants");
    let FirMatch::Block(stmts) = match_fir(store, body) else {
        panic!("instanceConstants body block expected");
    };
    stmts
}
