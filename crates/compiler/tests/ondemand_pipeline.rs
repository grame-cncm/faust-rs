//! Integration tests for the roadmap P0 exit criteria (clocked wrappers ×
//! signal_prepare × AD diagnostics).
//!
//! Scope (roadmap P0, `porting/ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md` §6):
//! - P0.1: propagation of the three clocked wrappers produces a clocked
//!   signal graph and `prepare_signals_for_fir` accepts it (the clock-env
//!   child of `Clocked` is an opaque annotation, never a signal); `signal_fir`
//!   rejects the still-unlowered clocked nodes with the structured
//!   `FRS-SFIR-0007` "not lowered yet" error — never a panic or the generic
//!   `FRS-SFIR-0004`.
//! - P0.2: two structurally identical wrapper instances in different contexts
//!   get *distinct* clock domains (the C++ de Bruijn collision class).
//! - P0.4 / P5: `fad` across a boundary is now differentiated (FAD Phase B
//!   wrapper + block-augmentation rules) — `fad_inside_ondemand_*` and
//!   `fad_around_ondemand_*` check the gradients numerically; only a bare
//!   wrapper reached outside a `Seq` (defensive) still errors. `rad` across a
//!   boundary still fails loudly and names the clocked construct it rejects.

use std::collections::BTreeSet;

use compiler::Compiler;
use signals::{SigMatch, match_sig};
use tlib::TreeArena;
use transform::signal_fir::{SignalFirOptions, compile_signals_to_fir_fastlane_with_ui};
use transform::signal_prepare::prepare_signals_for_fir;

fn compile_inline(name: &str, source: &str) -> compiler::SignalCompileOutput {
    Compiler::new()
        .compile_source_to_signals(name, source)
        .unwrap_or_else(|e| panic!("failed to compile {name} to signals: {e}"))
}

fn prepare_inline(name: &str, source: &str) -> transform::signal_prepare::PreparedSignals {
    let out = compile_inline(name, source);
    prepare_signals_for_fir(&out.parse.state.arena, &out.signals, &out.ui)
        .unwrap_or_else(|e| panic!("{name} should pass signal_prepare: {e}"))
}

#[test]
fn ondemand_fixture_passes_signal_prepare() {
    let prepared = prepare_inline(
        "od_gate_times_two",
        r#"process = (button("gate"), _) : ondemand(*(2));"#,
    );
    assert_eq!(prepared.outputs().len(), 1);
}

#[test]
fn upsampling_fixture_passes_signal_prepare() {
    let prepared = prepare_inline(
        "us_gate_times_two",
        r#"process = (button("gate"), _) : upsampling(*(2));"#,
    );
    assert_eq!(prepared.outputs().len(), 1);
}

#[test]
fn downsampling_fixture_passes_signal_prepare() {
    let prepared = prepare_inline(
        "ds_gate_times_two",
        r#"process = (button("gate"), _) : downsampling(*(2));"#,
    );
    assert_eq!(prepared.outputs().len(), 1);
}

#[test]
fn ondemand_with_internal_state_passes_signal_prepare() {
    // Recursive state inside the on-demand block: exercises SYMREC bodies
    // under `Clocked` wrappers plus `TempVar` inputs.
    let prepared = prepare_inline(
        "od_counter",
        r#"process = (button("gate"), _) : ondemand(+ ~ _);"#,
    );
    assert_eq!(prepared.outputs().len(), 1);
}

// ── P0.1 — signal_fir clean structured rejection ─────────────────────────────

#[test]
fn ondemand_signal_fir_rejects_with_clocked_not_lowered() {
    // Cohabitation §4 row 1: `ondemand` alone must reach `signal_fir` and be
    // rejected there with the dedicated `FRS-SFIR-0007`, not the generic
    // `FRS-SFIR-0004`, and never a panic.
    let out = compile_inline(
        "od_gate_times_two_fir",
        r#"process = (button("gate"), _) : ondemand(*(2));"#,
    );
    let err = compile_signals_to_fir_fastlane_with_ui(
        &out.parse.state.arena,
        &out.signals,
        out.process_arity.inputs,
        out.process_arity.outputs,
        &out.ui,
        &SignalFirOptions::default(),
    )
    .expect_err("clocked graphs must be rejected until the P3 lowering lands");
    assert_eq!(err.code().as_str(), "FRS-SFIR-0007", "got: {err}");
}

// ── P0.2 — clock-domain instance uniqueness ──────────────────────────────────

/// Collects every `SIGCLOCKENV` token id reachable from `sig` (env child of
/// `Clocked` wrappers), without treating the token as a signal.
fn collect_clock_env_ids(
    arena: &TreeArena,
    sig: signals::SigId,
    visited: &mut BTreeSet<u32>,
    out: &mut BTreeSet<u32>,
) {
    if !visited.insert(sig.as_u32()) {
        return;
    }
    if let SigMatch::Clocked(env, y) = match_sig(arena, sig) {
        if let SigMatch::ClockEnvToken(id) = match_sig(arena, env) {
            out.insert(id);
        }
        collect_clock_env_ids(arena, y, visited, out);
        return;
    }
    if arena.is_nil(sig) {
        return;
    }
    let Some(node) = arena.node(sig) else {
        return;
    };
    for &child in node.children.as_slice() {
        collect_clock_env_ids(arena, child, visited, out);
    }
}

#[test]
fn structurally_identical_ondemand_instances_get_distinct_domains() {
    // The C++ de Bruijn collision class (plan §3.4): two structurally
    // identical `ondemand` applications in different contexts must yield
    // *distinct* clock-domain instances.
    let out = compile_inline(
        "od_twice",
        r#"od(x) = (button("gate"), x) : ondemand(*(2));
           process = _ <: od, od :> _;"#,
    );
    assert_eq!(
        out.clock_domains.len(),
        2,
        "each propagated wrapper instance must allocate its own domain"
    );
    let mut ids = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for &sig in &out.signals {
        collect_clock_env_ids(&out.parse.state.arena, sig, &mut visited, &mut ids);
    }
    assert_eq!(
        ids.len(),
        2,
        "both domain tokens must be reachable and distinct, got ids {ids:?}"
    );
}

// ── P1 — clock inference + hierarchical graph on the real pipeline ──────────

#[test]
fn clock_inference_and_hgraph_run_on_prepared_ondemand_program() {
    use transform::clk_env::annotate;
    use transform::hgraph::{GraphKey, audit_hgraph, build_hgraph, schedule};
    use transform::schedule::SchedulingStrategy;

    let out = compile_inline(
        "od_p1_end_to_end",
        r#"process = (button("gate"), _) : ondemand(*(2));"#,
    );
    let prepared = prepare_signals_for_fir(&out.parse.state.arena, &out.signals, &out.ui)
        .expect("P0 guarantees signal_prepare accepts the clocked graph");

    // The SIGCLOCKENV token ids survive the staging-arena clone, so the
    // propagation-owned domain table stays valid on the prepared forest.
    let envs = annotate(prepared.arena(), &out.clock_domains, prepared.outputs())
        .expect("prepared ondemand program must be well-clocked");

    // The output (Seq) lives at the audio rate; exactly one domain exists.
    assert_eq!(out.clock_domains.len(), 1);
    assert_eq!(envs.env(prepared.outputs()[0]), Some(None));

    let hgraph = build_hgraph(
        prepared.arena(),
        &out.clock_domains,
        &envs,
        prepared.outputs(),
        prepared.sig_types_map(),
    )
    .expect("hgraph builds on the prepared forest");
    audit_hgraph(&hgraph).expect("partition property");

    // Control (the button's Konst/Block-variability UI plumbing, plan §4.6)
    // + one top graph + one wrapper subgraph, all scheduled deterministically.
    assert_eq!(hgraph.graphs().len(), 3);
    let control = hgraph
        .graph(GraphKey::Control)
        .expect("this DSP has a top-level Konst/Block signal (the gate button)");
    assert!(
        !control.is_empty(),
        "Control must own at least the signal that triggered its creation"
    );
    transform::hgraph::audit_control_variability(&hgraph, prepared.sig_types_map())
        .expect("Control never owns a Samp-variability signal");
    let sched =
        schedule(&hgraph, SchedulingStrategy::DepthFirst).expect("per-domain graphs are acyclic");
    let top = sched.schedule(GraphKey::Top).expect("top schedule");
    assert!(!top.is_empty());
    let wrapper_key = hgraph
        .graphs()
        .iter()
        .find_map(|(k, _)| matches!(k, GraphKey::Wrapper(_)).then_some(*k))
        .expect("exactly one wrapper subgraph");
    let sub = sched.schedule(wrapper_key).expect("subgraph schedule");
    assert!(!sub.is_empty(), "the block body must be scheduled");
}

// ── P3 (slice 1) — boolean ondemand guarded-block lowering ──────────────────

/// Compiles a temp DSP source through the interpreter fast lane and runs it
/// with explicit per-channel inputs.
fn run_interp_with_inputs(stem: &str, source: &str, inputs: &[Vec<f32>]) -> Vec<Vec<f32>> {
    use std::io::Cursor;

    use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
    use compiler::SignalFirLane;

    let path =
        std::env::temp_dir().join(format!("faust-rs-odp3-{stem}-{}.dsp", std::process::id()));
    std::fs::write(&path, source)
        .unwrap_or_else(|e| panic!("failed to write temporary DSP {}: {e}", path.display()));
    let fbc = Compiler::new()
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{stem}: interp compilation failed: {e}"));
    let _ = std::fs::remove_file(&path);

    let frame_count = inputs.first().map_or(0, Vec::len);
    let mut reader = Cursor::new(fbc);
    let mut factory =
        read_fbc::<f32>(&mut reader).unwrap_or_else(|e| panic!("{stem}: fbc parse failed: {e}"));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);

    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative outputs");
    let input_slices: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
    let mut outputs = vec![vec![0.0_f32; frame_count]; num_outputs];
    let mut output_slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frame_count as i32, &input_slices, &mut output_slices)
        .unwrap_or_else(|e| panic!("{stem}: interp execution failed: {e}"));
    outputs
}

#[test]
fn boolean_ondemand_compiles_to_fir_through_clocked_entry() {
    use transform::signal_fir::compile_signals_to_fir_fastlane_clocked;

    let out = compile_inline(
        "od_bool_fir",
        r#"process = ((_ != 0), _) : ondemand(*(2));"#,
    );
    compile_signals_to_fir_fastlane_clocked(
        &out.parse.state.arena,
        &out.signals,
        out.process_arity.inputs,
        out.process_arity.outputs,
        &out.ui,
        &out.clock_domains,
        &SignalFirOptions::default(),
    )
    .expect("boolean ondemand must lower through the clocked entry (P3 slice 1)");
}

#[test]
fn boolean_ondemand_holds_and_fires_at_runtime() {
    // y[n] = 2*x[n] when clk[n] != 0, else hold y[n-1]; holds start at 0.
    let clk = vec![0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0];
    let x = vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0];
    let outputs = run_interp_with_inputs(
        "hold_fire",
        r#"process = ((_ != 0), _) : ondemand(*(2));"#,
        &[clk.clone(), x.clone()],
    );
    assert_eq!(outputs.len(), 1);
    let mut held = 0.0_f32;
    for n in 0..clk.len() {
        if clk[n] != 0.0 {
            held = 2.0 * x[n];
        }
        assert!(
            (outputs[0][n] - held).abs() < 1.0e-6,
            "frame {n}: expected {held}, got {}",
            outputs[0][n]
        );
    }
}

#[test]
fn boolean_ondemand_with_recursive_state_accumulates_on_fire_only() {
    // Inside the block: acc = acc' + x, advanced only when the clock fires.
    let clk = vec![1.0, 0.0, 1.0, 0.0, 0.0, 1.0];
    let x = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let outputs = run_interp_with_inputs(
        "acc_on_fire",
        r#"process = ((_ != 0), _) : ondemand(+ ~ _);"#,
        &[clk.clone(), x.clone()],
    );
    let mut acc = 0.0_f32;
    let mut held = 0.0_f32;
    for n in 0..clk.len() {
        if clk[n] != 0.0 {
            acc += x[n];
            held = acc;
        }
        assert!(
            (outputs[0][n] - held).abs() < 1.0e-6,
            "frame {n}: expected {held}, got {}",
            outputs[0][n]
        );
    }
}

#[test]
fn integer_ondemand_repeats_body_clock_times() {
    // Counted-loop OD: the body runs `clock` times per tick, and OD inputs
    // are *not* zero-padded (unlike US) — the accumulator adds the snapshot
    // input on every inner iteration: with x ≡ 1 and clock 3, the held
    // value after tick n is 3 * (n + 1).
    let ones = vec![1.0; 5];
    let outputs = run_interp_with_inputs(
        "int_od_accumulator",
        r#"process = (3, _) : ondemand(+ ~ _);"#,
        &[ones],
    );
    for (n, &value) in outputs[0].iter().enumerate() {
        let expected = 3.0 * (n as f32 + 1.0);
        assert!(
            (value - expected).abs() < 1.0e-6,
            "frame {n}: expected {expected}, got {value}"
        );
    }
}

#[test]
fn domain_free_body_state_advances_in_fire_time() {
    // Faust C++ emits the payload of `Clocked(env, value)` in the guarded
    // block. Even when clock-env inference gives a recursion group the
    // audio-rate least fixed point, state syntactically inside a clocked body
    // advances in fire time.
    let x = vec![0.0; 4];
    let outputs = run_interp_with_inputs(
        "hoisted_counter",
        r#"process = (2, (_ : !)) : upsampling(1 : (+ ~ _));"#,
        &[x],
    );
    for (n, &value) in outputs[0].iter().enumerate() {
        let expected = 2.0 * (n as f32 + 1.0);
        assert!(
            (value - expected).abs() < 1.0e-6,
            "frame {n}: expected {expected}, got {value}"
        );
    }
}

#[test]
fn boolean_ondemand_domain_free_state_advances_on_fire_only() {
    // Regression for `ondemand(os.osc(...))`: the oscillator phase recursion
    // does not read a domain-internal input, but it is still the payload of a
    // `Clocked` held value and must stay inside the guarded `if`.
    let clk = vec![0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0];
    let muted = vec![0.0; clk.len()];
    let outputs = run_interp_with_inputs(
        "od_domain_free_acc_on_fire",
        r#"process = ((_ != 0), (_ : !)) : ondemand(1 : (+ ~ _));"#,
        &[clk.clone(), muted],
    );
    let mut held = 0.0_f32;
    for (n, &clock) in clk.iter().enumerate() {
        if clock != 0.0 {
            held += 1.0;
        }
        assert!(
            (outputs[0][n] - held).abs() < 1.0e-6,
            "frame {n}: expected {held}, got {}",
            outputs[0][n]
        );
    }
}

#[test]
fn upsampling_zero_pads_inputs_to_the_last_inner_iteration() {
    // US factor 2 around an accumulator: per tick the body runs twice, the
    // zero-padded input contributes only on the last inner iteration, so the
    // held output is the plain running sum of x (not 2x).
    let x = vec![1.0, 2.0, 3.0, 4.0];
    let outputs = run_interp_with_inputs(
        "us_running_sum",
        r#"process = (2, _) : upsampling(+ ~ _);"#,
        std::slice::from_ref(&x),
    );
    let mut acc = 0.0_f32;
    for (n, &value) in outputs[0].iter().enumerate() {
        acc += x[n];
        assert!(
            (value - acc).abs() < 1.0e-6,
            "frame {n}: expected {acc}, got {value}"
        );
    }
}

#[test]
fn downsampling_fires_once_every_clock_ticks() {
    // DS factor 2 around an accumulator fed with ones: fires at ticks
    // 0, 2, 4, … → held output 1, 1, 2, 2, 3, 3.
    let ones = vec![1.0; 6];
    let outputs = run_interp_with_inputs(
        "ds_accumulator",
        r#"process = (2, _) : downsampling(+ ~ _);"#,
        &[ones],
    );
    let expected = [1.0, 1.0, 2.0, 2.0, 3.0, 3.0];
    for (n, (&value, &want)) in outputs[0].iter().zip(expected.iter()).enumerate() {
        assert!(
            (value - want).abs() < 1.0e-6,
            "frame {n}: expected {want}, got {value}"
        );
    }
}

#[test]
fn inner_circular_delay_line_matches_unclocked_when_always_firing() {
    // @(17) > max_copy_delay forces the CircularPow2 strategy, which now
    // uses the per-domain fIOTA_d<i> cursor inside the block. With an
    // always-true clock the block fires every tick, so the clocked program
    // must match the plain unclocked one sample for sample.
    let frames = 48;
    let ramp: Vec<f32> = (0..frames).map(|n| n as f32).collect();
    let ones = vec![1.0_f32; frames];

    let clocked = run_interp_with_inputs(
        "od_delay_always",
        r#"process = ((_ != 0), _) : ondemand(_ <: _, @(17) :> +);"#,
        &[ones, ramp.clone()],
    );
    let unclocked = run_interp_with_inputs(
        "delay_reference",
        r#"process = _ <: _, @(17) :> +;"#,
        std::slice::from_ref(&ramp),
    );
    for n in 0..frames {
        assert!(
            (clocked[0][n] - unclocked[0][n]).abs() < 1.0e-6,
            "frame {n}: clocked {} vs unclocked {}",
            clocked[0][n],
            unclocked[0][n]
        );
    }
}

#[test]
fn inner_circular_delay_line_advances_in_fire_time() {
    // Sparse clock (fires on even ticks): the inner delay line must count
    // *fires*, not samples. Reference model: on fire, append the snapshot
    // input; the delayed tap reads the value written 17 fires ago (0 before
    // the line fills); the output holds between fires.
    let frames = 60;
    let ramp: Vec<f32> = (0..frames).map(|n| n as f32).collect();
    let clk: Vec<f32> = (0..frames)
        .map(|n| f32::from(u8::from(n % 2 == 0)))
        .collect();

    let outputs = run_interp_with_inputs(
        "od_delay_sparse",
        r#"process = ((_ != 0), _) : ondemand(_ <: _, @(17) :> +);"#,
        &[clk.clone(), ramp.clone()],
    );

    let mut fired: Vec<f32> = Vec::new();
    let mut held = 0.0_f32;
    for n in 0..frames {
        if clk[n] != 0.0 {
            fired.push(ramp[n]);
            let delayed = if fired.len() > 17 {
                fired[fired.len() - 1 - 17]
            } else {
                0.0
            };
            held = ramp[n] + delayed;
        }
        assert!(
            (outputs[0][n] - held).abs() < 1.0e-6,
            "frame {n}: expected {held}, got {}",
            outputs[0][n]
        );
    }
}

// ── P4 — FAD Phase A: fad strictly inside a clock domain ────────────────────

/// Runs one single-param program at `param` (baked as the hslider init) and
/// returns its outputs.
fn run_od_fad_source(stem: &str, source: String, inputs: &[Vec<f32>]) -> Vec<Vec<f32>> {
    run_interp_with_inputs(stem, &source, inputs)
}

#[test]
fn fad_inside_ondemand_tangent_matches_central_difference() {
    // Phase A (cohabitation §5): differentiation strictly inside one domain
    // needs zero new AD code. The differentiated expression reads only the
    // control seed, so the FAD sweep never touches boundary glue; the
    // primal/tangent pair is held by the block like any other output.
    let frames = 8;
    let ones = vec![1.0_f32; frames];
    let base = 0.7_f32;
    let eps = 1.0e-3_f32;

    let fad_src = |p: f32| {
        format!(
            r#"g = hslider("g", {p}, -10, 10, 0.001);
               process = ((_ != 0), (_ : !)) : ondemand(fad(sin(g) * g, g));"#
        )
    };
    let primal_src = |p: f32| {
        format!(
            r#"g = hslider("g", {p}, -10, 10, 0.001);
               process = ((_ != 0), (_ : !)) : ondemand(sin(g) * g);"#
        )
    };

    let fad_out = run_od_fad_source("od_fad", fad_src(base), &[ones.clone(), ones.clone()]);
    assert_eq!(fad_out.len(), 2, "fad bundle = [primal, tangent]");
    let plus = run_od_fad_source(
        "od_fad_p",
        primal_src(base + eps),
        &[ones.clone(), ones.clone()],
    );
    let minus = run_od_fad_source("od_fad_m", primal_src(base - eps), &[ones.clone(), ones]);

    let expected_primal = base.sin() * base;
    let expected_tangent = (plus[0][frames - 1] - minus[0][frames - 1]) / (2.0 * eps);
    for (n, (&primal, &tangent)) in fad_out[0].iter().zip(fad_out[1].iter()).enumerate() {
        assert!(
            (primal - expected_primal).abs() < 1.0e-5,
            "frame {n}: primal {primal} vs {expected_primal}"
        );
        assert!(
            (tangent - expected_tangent).abs() < 2.0e-3,
            "frame {n}: tangent {tangent} vs central difference {expected_tangent}"
        );
    }
}

#[test]
fn fad_inside_ondemand_tangent_holds_between_fires() {
    // Sparse clock: both held outputs (primal and tangent) are 0 before the
    // first fire and hold their value between fires — the tangent is
    // co-clocked with the primal by construction.
    let frames = 6;
    let clk = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let ones = vec![1.0_f32; frames];
    let g = 0.5_f32;

    let out = run_od_fad_source(
        "od_fad_hold",
        format!(
            r#"g = hslider("g", {g}, -10, 10, 0.001);
               process = ((_ != 0), (_ : !)) : ondemand(fad(g * g, g));"#
        ),
        &[clk.clone(), ones],
    );
    let expected_primal = g * g;
    let expected_tangent = 2.0 * g;
    for (n, (&primal, &tangent)) in out[0].iter().zip(out[1].iter()).enumerate() {
        let (want_p, want_t) = if n < 2 {
            (0.0, 0.0)
        } else {
            (expected_primal, expected_tangent)
        };
        assert!(
            (primal - want_p).abs() < 1.0e-6,
            "frame {n}: primal {primal} vs {want_p}"
        );
        assert!(
            (tangent - want_t).abs() < 1.0e-6,
            "frame {n}: tangent {tangent} vs {want_t}"
        );
    }
}

#[test]
fn fad_inside_ondemand_nonlinear_frame_reduction_matches_central_difference() {
    // FAD Phase B wrapper rules (roadmap P5) enable the "fad inside the block"
    // form of a *differentiable spectral loss* (milestone S4): a nonlinear
    // frame-rate reduction of clocked window inputs, differentiated w.r.t. a
    // parameter `g` that scales the inputs *inside* the block. The seed path
    // crosses the four `TempVar` window taps (zero-tangent inputs) and threads
    // through square / sum / sqrt / square — the same chain a magnitude
    // spectral loss uses. Gradient must match central differences.
    //
    // Frame op reduces 4 clocked taps: loss(g) = (g·‖x‖ − 5)², nonlinear in g.
    let clk = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let x0 = vec![1.0_f32; 6];
    let x1 = vec![2.0_f32; 6];
    let x2 = vec![3.0_f32; 6];
    let x3 = vec![4.0_f32; 6];
    let ins = [clk, x0, x1, x2, x3];
    let g = 0.7_f32;
    let eps = 1.0e-3_f32;

    let body = |op: &str| {
        format!(
            r#"g = hslider("g", {{P}}, -10, 10, 0.0001);
               sq = _ <: _*_;
               norm = par(i, 4, *(g)) : par(i, 4, sq) :> _ : sqrt;
               loss = norm : -(5.0) : sq;
               process = ((_ != 0), _, _, _, _) : ondemand({op});"#
        )
    };
    let render = |p: f32, op: &str| body(op).replace("{P}", &format!("{p}"));

    let fad_out = run_od_fad_source(
        "od_fad_spectral",
        render(g, "fad(loss, g)"),
        &ins.clone().map(|v| v.to_vec()),
    );
    assert_eq!(fad_out.len(), 2, "fad bundle = [primal, tangent]");

    let plus = run_od_fad_source(
        "od_fad_sp_p",
        render(g + eps, "loss"),
        &ins.clone().map(|v| v.to_vec()),
    );
    let minus = run_od_fad_source(
        "od_fad_sp_m",
        render(g - eps, "loss"),
        &ins.map(|v| v.to_vec()),
    );

    // Steady frame after the fire at n=2.
    let n = 5;
    let central = (plus[0][n] - minus[0][n]) / (2.0 * eps);
    assert!(
        (fad_out[1][n] - central).abs() < 2.0e-2,
        "tangent {} vs central difference {central} (must not be silently zero)",
        fad_out[1][n],
    );
    // Sanity: the gradient is genuinely non-zero here.
    assert!(fad_out[1][n].abs() > 1.0, "expected a non-trivial gradient");
}

// ── P0.4 — loud AD diagnostics at domain boundaries ──────────────────────────

fn expect_compile_error(name: &str, source: &str) -> compiler::CompilerError {
    Compiler::new()
        .compile_source_to_signals(name, source)
        .expect_err("program must be rejected during propagation")
}

#[test]
fn fad_around_ondemand_differentiates_through_the_block() {
    // Cohabitation §4 row 2 / §6: `fad(… : ondemand(*(g)), g)` — the *fad-outside*
    // form, where the differentiated output reads the block from outside. FAD
    // Phase B (roadmap P5) block augmentation `OD → OD_aug` (payload carries the
    // interleaved primal + tangent held-outputs, `Seq` re-routed once) makes it
    // exact: the block computes `[g·x, x]`, so tangent = held input, primal =
    // g·tangent. Before P5 this was rejected with FRS-PROP-0004.
    let clk = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let data = vec![1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let g = 0.5_f32;

    let out = run_od_fad_source(
        "fad_around_od",
        format!(
            r#"g = hslider("g", {g}, -10, 10, 0.001);
               process = fad(((_ != 0), _) : ondemand(*(g)), g);"#
        ),
        &[clk, data.clone()],
    );
    assert_eq!(out.len(), 2, "fad bundle = [primal, tangent]");

    // Fire at n=2 snapshots data[2]=3; 0 before the first fire, held after.
    let held = data[2];
    for (n, (&primal, &tangent)) in out[0].iter().zip(out[1].iter()).enumerate() {
        let (want_p, want_t) = if n < 2 { (0.0, 0.0) } else { (g * held, held) };
        assert!(
            (tangent - want_t).abs() < 1.0e-6,
            "frame {n}: tangent {tangent} vs held input {want_t} (must not be silently zero)"
        );
        assert!(
            (primal - want_p).abs() < 1.0e-6,
            "frame {n}: primal {primal} vs {want_p}"
        );
    }
}

/// Nonlinear fad-*outside*: `fad((clk, x) : ondemand((g·x)²), g)` — the block is
/// nonlinear in the seed, so the augmented tangent held-output is `2·g·x²`.
/// Exercises OD_aug through a nonlinear body; gradient checked exactly.
#[test]
fn fad_around_ondemand_nonlinear_body_gradient_is_exact() {
    let clk = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let data = vec![1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let g = 0.5_f32;
    let out = run_od_fad_source(
        "fad_around_od_nl",
        format!(
            r#"g = hslider("g", {g}, -10, 10, 0.001);
               sq = _ <: _*_;
               process = fad(((_ != 0), _) : ondemand(sq(*(g))), g);"#
        ),
        &[clk, data.clone()],
    );
    let held = data[2];
    let n = 5;
    assert!(
        (out[0][n] - (g * held) * (g * held)).abs() < 1.0e-5,
        "primal {} vs (g·x)²={}",
        out[0][n],
        (g * held) * (g * held)
    );
    assert!(
        (out[1][n] - 2.0 * g * held * held).abs() < 1.0e-5,
        "tangent {} vs 2·g·x²={}",
        out[1][n],
        2.0 * g * held * held
    );
}

#[test]
fn fad_inside_ondemand_crossing_seed_reads_clocked_input() {
    // Cohabitation §4 row 3 / §5–6: `ondemand(fad(*(g), g))` differentiates
    // `g · x` where `x` is the block's clocked data input, so the seed path
    // crosses a `TempVar` boundary. FAD Phase B (roadmap P5) wrapper rule
    // `(snap u)' = snap(u')` makes this exact: the tangent is the held input
    // snapshot, so `primal == g · tangent`. Before P5 this was rejected with
    // FRS-PROP-0004; the boundary wrapper rules now differentiate it.
    let clk = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let data = vec![1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let g = 0.5_f32;

    let out = run_od_fad_source(
        "od_fad_cross",
        format!(
            r#"g = hslider("g", {g}, -10, 10, 0.001);
               process = ((_ != 0), _) : ondemand(fad(*(g), g));"#
        ),
        &[clk, data.clone()],
    );
    assert_eq!(out.len(), 2, "fad bundle = [primal, tangent]");

    // Fire at n=2 snapshots data[2]=3; both outputs are 0 before the first
    // fire and hold afterwards. tangent = held input, primal = g · tangent.
    let held = data[2];
    for (n, (&primal, &tangent)) in out[0].iter().zip(out[1].iter()).enumerate() {
        let (want_p, want_t) = if n < 2 { (0.0, 0.0) } else { (g * held, held) };
        assert!(
            (tangent - want_t).abs() < 1.0e-6,
            "frame {n}: tangent {tangent} vs held input {want_t} (must not be silently zero)"
        );
        assert!(
            (primal - want_p).abs() < 1.0e-6,
            "frame {n}: primal {primal} vs {want_p}"
        );
    }
}

#[test]
fn rad_around_ondemand_names_the_construct() {
    // Cohabitation §4 row 4: the RAD rejection must name the clocked
    // construct instead of the generic kind "other".
    let err = expect_compile_error(
        "rad_around_od",
        r#"g = hslider("g", 1, 0, 10, 0.1);
           process = rad((button("gate"), _) : ondemand(*(g)), g);"#,
    );
    let message = err.to_string();
    assert!(
        message.contains("ondemand")
            || message.contains("clocked")
            || message.contains("clock-domain"),
        "RAD rejection must name the clocked construct, got: {message}"
    );
}

// ── S1–S2 — the `interleave` spectral primitive (frame-rate serialization) ──
//
// `interleave(N, FX) = serialize_in(N) : periodic_od(FX) : serialize_out(N)`
// with a boolean periodic clock of period N and phase N-1 (fires at
// t ≡ N-1 mod N). Built entirely on top of the P3 boolean-ondemand block —
// no new compiler primitive (option A: `up0(H, y) = y * (H != 0)`).
// See `porting/interleave-spectral-primitive-2026-07-07-en.md`.
//
// The prelude below is the hermetic (library-free) copy of `interleave.lib`;
// the two must stay in sync.

/// The generic `interleave` prelude, parameterized by `N` and the frame
/// operator `FX` through Faust's `par`. Self-contained (no `stdfaust.lib`).
const INTERLEAVE_PRELUDE: &str = r#"
frame_clock(N) = ((+(1) : %(N)) ~ _) == 0;
bus(N) = par(i, N, _);
serialize_in(N) = _ <: par(i, N, @(N-1-i));
interleave(N, FX) = serialize_in(N) : od : serialize_out
with {
    od = (frame_clock(N), bus(N)) : ondemand(FX);
    up0(y) = y * (frame_clock(N) != 0);
    serialize_out = par(j, N, up0 : @(j)) :> _;
};
"#;

fn interleave_source(process: &str) -> String {
    format!("{INTERLEAVE_PRELUDE}\nprocess = {process};")
}

/// Runs a ramp `x[n] = n` through `process` and returns the single output.
fn run_ramp(stem: &str, process: &str, frames: usize) -> Vec<f32> {
    let ramp: Vec<f32> = (0..frames).map(|n| n as f32).collect();
    let out = run_interp_with_inputs(
        stem,
        &interleave_source(process),
        std::slice::from_ref(&ramp),
    );
    assert_eq!(out.len(), 1, "{stem}: expected a single output channel");
    out.into_iter().next().expect("one output")
}

#[test]
fn interleave_identity_is_a_constant_delay_n_minus_1() {
    // S1 locking milestone: interleave(N, id) == @(N-1) — constant latency
    // N-1, identity up to that delay. For N=2 this is exactly `mem`.
    for n in [2usize, 4, 8] {
        let frames = 6 * n;
        let got = run_ramp(
            &format!("interleave_id_{n}"),
            &format!("interleave({n}, bus({n}))"),
            frames,
        );
        for (t, &value) in got.iter().enumerate() {
            let expected = t.checked_sub(n - 1).map_or(0.0, |k| k as f32);
            assert!(
                (value - expected).abs() < 1.0e-6,
                "N={n} frame {t}: interleave(id) = {value}, expected @(N-1) = {expected}"
            );
        }
    }
}

#[test]
fn interleave_matches_a_plain_mem_for_n_2() {
    // The N=2 unrolled-table anchor of the design note: interleave(2, id)
    // is sample-for-sample identical to `mem`.
    let frames = 16;
    let ramp: Vec<f32> = (0..frames).map(|n| n as f32).collect();
    let both = run_interp_with_inputs(
        "interleave2_vs_mem",
        &format!("{INTERLEAVE_PRELUDE}\nprocess = _ <: interleave(2, bus(2)), mem;"),
        std::slice::from_ref(&ramp),
    );
    for (t, (&il, &m)) in both[0].iter().zip(both[1].iter()).enumerate() {
        assert!(
            (il - m).abs() < 1.0e-6,
            "frame {t}: interleave(2,id) = {il} vs mem = {m}"
        );
    }
}

#[test]
fn interleave_with_a_framewise_gain_scales_the_delayed_stream() {
    // A stateless frame operator (scale every lane by 2) commutes with the
    // serialization: interleave(N, 2*id) == 2 * @(N-1).
    let n = 4usize;
    let frames = 6 * n;
    let got = run_ramp(
        "interleave_gain",
        &format!("interleave({n}, par(i, {n}, *(2.0)))"),
        frames,
    );
    for (t, &value) in got.iter().enumerate() {
        let expected = t.checked_sub(n - 1).map_or(0.0, |k| 2.0 * k as f32);
        assert!(
            (value - expected).abs() < 1.0e-6,
            "frame {t}: interleave(2*id) = {value}, expected 2*@(N-1) = {expected}"
        );
    }
}

#[test]
fn interleave_with_an_internal_frame_delay_adds_one_frame_of_latency() {
    // S2 fixture: a delay inside the frame operator is measured in *frames*.
    // The boolean clock fires once every N samples, so `@(1)` inside the
    // block advances on the per-domain time (P3 slice 3): delaying every
    // lane by one frame shifts the whole reconstructed stream by N samples.
    // interleave(N, frame_delay_1) == @(N-1) then @(N) == @(2N-1).
    for n in [2usize, 4] {
        let frames = 6 * n;
        let got = run_ramp(
            &format!("interleave_framedelay_{n}"),
            &format!("interleave({n}, par(i, {n}, @(1)))"),
            frames,
        );
        for (t, &value) in got.iter().enumerate() {
            let expected = t.checked_sub(2 * n - 1).map_or(0.0, |k| k as f32);
            assert!(
                (value - expected).abs() < 1.0e-6,
                "N={n} frame {t}: interleave(frame_delay) = {value}, expected @(2N-1) = {expected}"
            );
        }
    }
}

// ── P3 slice 4 — circular recursion carriers & IfWrapping delays in blocks ──

#[test]
fn inner_circular_recursion_carrier_matches_unclocked_when_always_firing() {
    // A recursive feedback with a large delay (`+ ~ @(20)`, total lag 21 →
    // CircularPow2 storage) inside a block now advances on the per-domain
    // cursor `fIOTA_d<i>` (roadmap P3 slice 4). With an always-true clock it
    // must match the plain unclocked recursion sample for sample.
    let frames = 64;
    let ramp: Vec<f32> = (0..frames).map(|n| (n % 7) as f32).collect();
    let ones = vec![1.0_f32; frames];

    let clocked = run_interp_with_inputs(
        "od_rec_always",
        r#"process = ((_ != 0), _) : ondemand(+ ~ @(20));"#,
        &[ones, ramp.clone()],
    );
    let unclocked = run_interp_with_inputs(
        "rec_reference",
        r#"process = + ~ @(20);"#,
        std::slice::from_ref(&ramp),
    );
    for (t, (&c, &u)) in clocked[0].iter().zip(unclocked[0].iter()).enumerate() {
        assert!(
            (c - u).abs() < 1.0e-4,
            "frame {t}: clocked {c} vs unclocked {u}"
        );
    }
}

#[test]
fn inner_circular_recursion_carrier_advances_in_fire_time() {
    // Sparse clock (fires on even ticks): the frame-indexed recurrence
    // y_frame[k] = x[fire k] + y_frame[k-21], held between fires.
    let frames = 90;
    let ramp: Vec<f32> = (0..frames).map(|n| (n % 5) as f32).collect();
    let clk: Vec<f32> = (0..frames)
        .map(|n| f32::from(u8::from(n % 2 == 0)))
        .collect();

    let out = run_interp_with_inputs(
        "od_rec_sparse",
        r#"process = ((_ != 0), _) : ondemand(+ ~ @(20));"#,
        &[clk.clone(), ramp.clone()],
    );

    let mut frame_hist: Vec<f32> = Vec::new();
    let mut held = 0.0_f32;
    for (t, &value) in out[0].iter().enumerate() {
        if clk[t] != 0.0 {
            let prev = frame_hist
                .len()
                .checked_sub(21)
                .map_or(0.0, |i| frame_hist[i]);
            let y = ramp[t] + prev;
            frame_hist.push(y);
            held = y;
        }
        assert!(
            (value - held).abs() < 1.0e-4,
            "frame {t}: expected {held}, got {value}"
        );
    }
}

#[test]
fn inner_ifwrapping_delay_lowers_through_clocked_entry() {
    // With `delay_line_threshold` (`-dlt`) below the delay, the inner delay
    // line uses the IfWrapping strategy; its per-line counter advance now
    // moves inside the guarded block (roadmap P3 slice 4) instead of being
    // rejected. The FIR must build (and pass the backend verifier via the
    // C++ path exercised elsewhere).
    use transform::signal_fir::compile_signals_to_fir_fastlane_clocked;

    let out = compile_inline(
        "od_ifwrap",
        r#"process = ((_ != 0), _) : ondemand(_ <: _, @(20) :> +);"#,
    );
    let options = SignalFirOptions {
        delay_line_threshold: 8,
        ..SignalFirOptions::default()
    };
    compile_signals_to_fir_fastlane_clocked(
        &out.parse.state.arena,
        &out.signals,
        out.process_arity.inputs,
        out.process_arity.outputs,
        &out.ui,
        &out.clock_domains,
        &options,
    )
    .expect("inner IfWrapping delay must lower (P3 slice 4), not be rejected");
}

#[test]
fn ui_slider_inside_block_is_read_on_fire() {
    // SR/UI-in-block policy: a control (block-rate value) read inside a
    // clocked block is sampled on fire and held between fires — no special
    // handling needed, it flows through the block like any other value.
    // hslider default 3 => gain 3 applied to the snapshot input on fire.
    let clk = vec![1.0, 0.0, 1.0, 0.0, 0.0];
    let x = vec![10.0, 20.0, 30.0, 40.0, 50.0];
    let out = run_interp_with_inputs(
        "ui_slider_block",
        r#"process = ((_ != 0), _) : ondemand(_ * hslider("g", 3, 0, 10, 0.01));"#,
        &[clk.clone(), x.clone()],
    );
    let mut held = 0.0_f32;
    for (t, &value) in out[0].iter().enumerate() {
        if clk[t] != 0.0 {
            held = 3.0 * x[t];
        }
        assert!(
            (value - held).abs() < 1.0e-4,
            "frame {t}: expected {held}, got {value}"
        );
    }
}
