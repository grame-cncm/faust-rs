//! FIR-dump structural tests for the clocked reference programs (roadmap
//! P3.1 checklist: "structure matches the plan §2.4 captured C++ for the three
//! reference programs").
//!
//! These lock the **emission shape** of the guarded blocks — complementing the
//! numeric differential (`cpp_clocked_differential.rs`, which validates
//! *behavior* against the C++ branch). We assert stable, clocked-specific
//! markers in the faust-rs-generated C++ rather than exact text, so the tests
//! survive variable-suffix churn:
//!
//! - `fPerm`      — the sample-and-hold field a clocked block writes for its
//!   held outputs (only clocked lowering emits it);
//! - `if (`       — the boolean-OD / DS firing guard (`CodeIFblock`);
//! - `lOd`        — the counted inner loop variable (integer OD / upsampling);
//! - `fDSCounter` — the per-domain downsampling modulo counter;
//! - `fIOTA_d`    — the per-domain circular cursor for in-block delay state.

use codegen::backends::cpp::CppOptions;
use compiler::{Compiler, FirVerifyOptions, SchedulingStrategy, SignalFirLane};

fn compile_cpp(name: &str, source: &str) -> String {
    Compiler::new()
        .with_fir_verify_options(FirVerifyOptions {
            enabled: true,
            strict: false,
        })
        .compile_source_to_cpp_with_lane(
            name,
            source,
            &CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{name} C++ compilation failed: {e}"))
}

/// Returns the body of the generated `compute` method.
fn compute_body(cpp: &str) -> String {
    let start = cpp
        .find("void compute(")
        .unwrap_or_else(|| panic!("no compute() in generated C++:\n{cpp}"));
    // Take from `compute(` to the end of the class — enough to inspect the loop.
    cpp[start..].to_owned()
}

#[test]
fn sibling_ondemand_regions_do_not_reuse_local_scheduled_temporaries() {
    // Reduced form of `ondemand(os.osc(...)), ondemand(os.square(...))`.
    // Both phases contain the same hash-consed startup delay, but each guarded
    // region must materialize and update it in its own lexical/domain context.
    let source = r#"
        decimal(x) = x - floor(x);
        startup = 1 - (1');
        phase(inc) = (select2(startup, +(inc), 0) : decimal) ~ _;
        process = (button("a") : ondemand(phase(0.01))),
                  (button("b") : ondemand(phase(0.02)));
    "#;
    let strategies = [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ];

    for strategy in strategies {
        Compiler::new()
            .with_scheduling_strategy(strategy)
            .compile_source_to_cpp_with_lane(
                "sibling_ondemand_phases.dsp",
                source,
                &CppOptions::default(),
                SignalFirLane::TransformFastLane,
            )
            .unwrap_or_else(|error| {
                panic!("sibling ondemand lowering failed under {strategy:?}: {error}")
            });
    }
}

#[test]
fn shared_stateful_payload_gets_independent_state_per_sibling_domain() {
    // Reduced form of `ondemand_18_toggle_morph.dsp`: the exact same
    // hash-consed oscillator is consumed by two sibling `ondemand` instances.
    // C++ gives each occurrence its own startup delay and recursion carrier;
    // signal-only state keys instead leak the first sibling's `fRecCur*` local
    // into the second sibling and couple their fire-time histories.
    let source = r#"
        decimal(x) = x - floor(x);
        startup = 1 - (1');
        phase = (select2(startup, +(0.01), 0) : decimal) ~ _;
        process = (button("a") : ondemand(phase)),
                  (button("b") : ondemand(phase));
    "#;

    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        let cpp = Compiler::new()
            .with_scheduling_strategy(strategy)
            .compile_source_to_cpp_with_lane(
                "sibling_shared_state.dsp",
                source,
                &CppOptions::default(),
                SignalFirLane::TransformFastLane,
            )
            .unwrap_or_else(|error| {
                panic!("shared sibling state lowering failed under {strategy:?}: {error}")
            });
        let body = compute_body(&cpp);
        assert!(
            cpp.contains("_d0") && cpp.contains("_d1"),
            "sibling domains must own distinct persistent state:\n{cpp}"
        );
        assert!(
            body.contains("fRecCur"),
            "the reduced oscillator must exercise scalar recursion:\n{body}"
        );
    }
}

#[test]
fn boolean_ondemand_emits_guarded_if_with_hold_field() {
    let cpp = compile_cpp(
        "struct_od_bool",
        r#"process = (((_ % 2) == 0), _) : ondemand(+ ~ _);"#,
    );
    let body = compute_body(&cpp);
    assert!(
        body.contains("if ("),
        "boolean OD must emit an `if` guard:\n{body}"
    );
    assert!(
        body.contains("fPerm"),
        "boolean OD must hold its output in an fPerm field:\n{body}"
    );
    // The clock stays a plain per-sample expression; no counted loop.
    assert!(
        !body.contains("lOd"),
        "boolean OD must not emit a counted inner loop:\n{body}"
    );
}

#[test]
fn ondemand_with_circular_delay_emits_per_domain_cursor() {
    // @(20) forces CircularPow2 storage; inside a block it must use the
    // per-domain fIOTA_d cursor (roadmap P3 slice 4), not the global fIOTA.
    let cpp = compile_cpp(
        "struct_od_delay",
        r#"process = (((_ % 2) == 0), _) : ondemand(_ <: _, @(20) :> +);"#,
    );
    let body = compute_body(&cpp);
    assert!(body.contains("if ("), "guarded block expected:\n{body}");
    assert!(
        body.contains("fIOTA_d"),
        "in-block circular delay must use a per-domain fIOTA_d cursor:\n{body}"
    );
    // The global fIOTA must not be the one advancing this block's state.
    assert!(
        !body.contains("fIOTA ="),
        "the global fIOTA must not advance in-block state here:\n{body}"
    );
}

#[test]
fn held_domain_free_payload_state_is_inside_guarded_block() {
    // The recursion does not read a domain-internal input, so clock-env
    // inference can assign it an ancestor environment. C++ still emits the
    // whole held `Clocked(env, payload)` body in the guarded block and only
    // reads the hold field outside.
    let cpp = compile_cpp(
        "struct_od_domain_free",
        r#"process = (((_ % 2) == 0), (_ : !)) : ondemand(1 : (+ ~ _));"#,
    );
    let body = compute_body(&cpp);
    let guard = body
        .find("if (")
        .unwrap_or_else(|| panic!("guarded block expected:\n{body}"));
    let rec_update = body[guard..]
        .find("iRec")
        .map(|offset| guard + offset)
        .unwrap_or_else(|| panic!("recursive state update must be inside the guard:\n{body}"));
    let output_store = body[rec_update..]
        .find("output0")
        .map(|offset| rec_update + offset)
        .unwrap_or_else(|| panic!("output store expected after guarded payload:\n{body}"));
    assert!(
        rec_update < output_store,
        "recursive payload state must update inside the guard before the held output read:\n{body}"
    );
}

#[test]
fn upsampling_emits_counted_inner_loop() {
    // Consuming an outer input exercises ZeroPad lowering, including the
    // counted `lOd` iterator access checked by the FIR verifier above.
    let cpp = compile_cpp("struct_us", r#"process = (2, _) : upsampling(+ ~ _);"#);
    let body = compute_body(&cpp);
    assert!(
        body.contains("for (") && body.contains("lOd"),
        "upsampling must emit a counted inner `for` loop:\n{body}"
    );
    assert!(
        body.contains("fPerm"),
        "upsampling must hold its output in an fPerm field:\n{body}"
    );
}

#[test]
fn downsampling_emits_modulo_counter_guard() {
    let cpp = compile_cpp("struct_ds", r#"process = (2, _) : downsampling(+ ~ _);"#);
    let body = compute_body(&cpp);
    assert!(
        body.contains("fDSCounter"),
        "downsampling must emit a per-domain fDSCounter:\n{body}"
    );
    assert!(
        body.contains("if (") && body.contains('%'),
        "downsampling must guard on the counter and advance it modulo the clock:\n{body}"
    );
}

/// A `Konst` value held by a wrapper (`1.0 / SR` below) is emitted inside the
/// guarded block, at fire time, although every parent edge it has in the
/// signal DAG is `Konst`. Placement must therefore hoist it into persistent
/// storage and keep the constants it reads out of `instanceConstants()`'s
/// stack: `fPerm = 1.0f / fConst0` with `float fConst0` local to init was
/// `FRS-FIR-0001 use of undeclared variable` on every backend (found through
/// `op.on_change(\(t).(t / ma.SR))` under `fad`, 2026-09-18).
#[test]
fn konst_payload_of_a_guarded_block_reads_persistent_constants() {
    // `compile_cpp` runs the FIR verifier: an undeclared variable is an
    // error and fails the compilation whether or not warnings are fatal.
    let cpp = compile_cpp(
        "od_konst_payload",
        r#"SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));
process = (_, 1.0) : ondemand(\(a, b).(a / SR, b / SR))(button("c"));"#,
    );
    let body = compute_body(&cpp);
    assert!(
        !body.contains("/ fConst"),
        "the guarded block must not divide by an init-local constant:\n{body}"
    );
    assert!(
        body.contains("= fConst") || body.contains("* fConst"),
        "the held constant is read from a persistent fConst field:\n{body}"
    );
}
