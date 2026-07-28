//! Runtime regressions for reverse-mode AD (`rad`) on the feed-forward
//! subset.
//!
//! Two complementary checks per case:
//! - **RAD vs FAD parity.** For the same expression and seed list, the
//!   gradient lanes of `rad(expr, seeds)` must agree with the matching
//!   tangent lanes of `fad(expr, seeds)` lane by lane. This is the
//!   strongest invariant because both use the same underlying
//!   differentiable signal subset.
//! - **RAD vs central finite differences.** Each gradient lane is also
//!   checked against the central difference of the primal under
//!   per-seed perturbation. This catches any drift that would slip past
//!   FAD itself if both sides shared a bug.
//!
//! The tests use the interpreter fast lane through the public compiler
//! facade, so they exercise propagation, transform, FIR lowering and the
//! interp backend together.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn run_interp_corpus_inner(stem: &str, frame_count: usize) -> Vec<Vec<f32>> {
    let path = corpus_path(&format!("{stem}.dsp"));
    let compiler = Compiler::new();
    let fbc = compiler
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{} interp compilation failed: {e}", path.display()));
    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader)
        .unwrap_or_else(|e| panic!("{} interp bytecode parse failed: {e}", path.display()));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative outputs");
    let mut outputs = vec![vec![0.0_f32; frame_count]; num_outputs];
    let mut output_slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frame_count as i32, &[], &mut output_slices)
        .unwrap_or_else(|e| panic!("{} interp execution failed: {e}", path.display()));
    outputs
}

/// Same 64 MB-stack worker pattern as `run_interp_temp_source`, but reads
/// fixtures from `tests/corpus/` so they can be inspected directly and
/// reused across the broader corpus tooling.
fn run_interp_corpus(stem: &'static str, frame_count: usize) -> Vec<Vec<f32>> {
    std::thread::Builder::new()
        .name(format!("rad-runtime-corpus-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_interp_corpus_inner(stem, frame_count))
        .expect("spawn rad-runtime-corpus worker")
        .join()
        .expect("rad-runtime-corpus worker thread should finish")
}

static NEXT_TEMP_DSP_ID: AtomicUsize = AtomicUsize::new(0);

fn run_interp_temp_source(stem: &str, source: &str, frame_count: usize) -> Vec<Vec<f32>> {
    let stem = stem.to_owned();
    let source = source.to_owned();
    // Spawn on a 64 MB stack: pipelines that drag `stdfaust.lib` produce
    // deep evaluation trees (same pattern used by `signal_pipeline.rs` and
    // `zita_pipeline.rs`) and overflow the default 2 MB test-thread stack.
    std::thread::Builder::new()
        .name(format!("rad-runtime-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_interp_temp_source_inner(&stem, &source, frame_count))
        .expect("spawn rad-runtime worker")
        .join()
        .expect("rad-runtime worker thread should finish")
}

fn run_interp_temp_source_inner(stem: &str, source: &str, frame_count: usize) -> Vec<Vec<f32>> {
    run_interp_temp_source_with_inputs_inner(stem, source, &[], frame_count)
}

fn run_interp_temp_source_with_inputs(
    stem: &str,
    source: &str,
    inputs: &[Vec<f32>],
    frame_count: usize,
) -> Vec<Vec<f32>> {
    let stem = stem.to_owned();
    let source = source.to_owned();
    let inputs = inputs.to_vec();
    std::thread::Builder::new()
        .name(format!("rad-runtime-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            run_interp_temp_source_with_inputs_inner(&stem, &source, &inputs, frame_count)
        })
        .expect("spawn rad-runtime worker")
        .join()
        .expect("rad-runtime worker thread should finish")
}

fn run_interp_temp_source_with_inputs_inner(
    stem: &str,
    source: &str,
    inputs: &[Vec<f32>],
    frame_count: usize,
) -> Vec<Vec<f32>> {
    let unique_id = NEXT_TEMP_DSP_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "faust-rs-rad-{stem}-{}-{unique_id}.dsp",
        std::process::id()
    ));
    fs::write(&path, source)
        .unwrap_or_else(|e| panic!("failed to write temporary DSP {}: {e}", path.display()));
    let compiler = Compiler::new();
    let fbc = compiler
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{} interp compilation failed: {e}", path.display()));
    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader)
        .unwrap_or_else(|e| panic!("{} interp bytecode parse failed: {e}", path.display()));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative outputs");
    let num_inputs = usize::try_from(instance.get_num_inputs()).expect("non-negative inputs");
    assert_eq!(
        inputs.len(),
        num_inputs,
        "{stem}: input fixture arity must match compiled DSP input count"
    );
    for (index, input) in inputs.iter().enumerate() {
        assert_eq!(
            input.len(),
            frame_count,
            "{stem}: input lane {index} length must match frame count"
        );
    }
    let mut outputs = vec![vec![0.0_f32; frame_count]; num_outputs];
    let input_slices: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
    let mut output_slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frame_count as i32, &input_slices, &mut output_slices)
        .unwrap_or_else(|e| panic!("{} interp execution failed: {e}", path.display()));
    let _ = fs::remove_file(&path);
    outputs
}

/// Compiles `source`, runs two consecutive `compute()` calls each with
/// `block_size` frames, and returns `(block1_outputs, block2_outputs)`.
///
/// Used to verify that primal DSP state persists across host `compute()` calls:
/// the second block should observe the state left by the first block rather than
/// re-starting from zero.
fn run_interp_two_blocks(stem: &str, source: &str, block_size: usize) -> (Vec<f32>, Vec<f32>) {
    let stem = stem.to_owned();
    let source = source.to_owned();
    std::thread::Builder::new()
        .name(format!("rad-runtime-two-blocks-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let unique_id = NEXT_TEMP_DSP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "faust-rs-rad-{stem}-{}-{unique_id}.dsp",
                std::process::id()
            ));
            fs::write(&path, source.as_str())
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            let compiler = Compiler::new();
            let fbc = compiler
                .compile_file_default_to_interp_with_lane(
                    &path,
                    &InterpOptions::default(),
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|e| panic!("{} compile failed: {e}", path.display()));
            let _ = fs::remove_file(&path);
            let mut reader = Cursor::new(fbc);
            let mut factory =
                read_fbc::<f32>(&mut reader).unwrap_or_else(|e| panic!("parse bytecode: {e}"));
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(48_000);
            // First block.
            let num_outputs = instance.get_num_outputs() as usize;
            let mut out1 = vec![vec![0.0_f32; block_size]; num_outputs];
            {
                let mut slices: Vec<&mut [f32]> = out1.iter_mut().map(Vec::as_mut_slice).collect();
                instance
                    .try_compute(block_size as i32, &[], &mut slices)
                    .unwrap_or_else(|e| panic!("block1 compute: {e}"));
            }
            // Second block — same instance, state should persist.
            let mut out2 = vec![vec![0.0_f32; block_size]; num_outputs];
            {
                let mut slices: Vec<&mut [f32]> = out2.iter_mut().map(Vec::as_mut_slice).collect();
                instance
                    .try_compute(block_size as i32, &[], &mut slices)
                    .unwrap_or_else(|e| panic!("block2 compute: {e}"));
            }
            // Return channel 0 from each block.
            (out1.remove(0), out2.remove(0))
        })
        .expect("spawn two-blocks worker")
        .join()
        .expect("two-blocks worker thread should finish")
}

fn assert_close(actual: f32, expected: f32, abs_tol: f32, label: &str) {
    let diff = (actual - expected).abs();
    let rel_tol = 1.0e-5_f32 * actual.abs().max(expected.abs());
    let allowed = abs_tol.max(rel_tol);
    assert!(
        diff <= allowed,
        "{label}: expected {expected}, got {actual}, abs diff {diff}, allowed {allowed}"
    );
}

/// Asserts the **block-level total** TBPTT gradient of a `BlockReverseAD`
/// carrier matches the central finite difference of the block-sum primal.
///
/// This is the correct comparison for recursive (time-dependent) circuits:
/// the per-sample TBPTT adjoint carries information across samples, so only
/// the *total* (sum over the block) can be meaningfully compared with a
/// per-parameter FD perturbation of the block objective `L = Σ_n y[n]`.
///
/// # Arguments
///
/// * `primal_outputs` — number of primal output channels.
/// * `base_seeds` — base parameter values (one per seed).
/// * `epsilons` — FD perturbation magnitudes.
/// * `build_rad_source` — builds the `rad(expr, seeds)` Faust source.
/// * `build_primal_source` — builds the same `expr` without `rad(...)`.
#[allow(clippy::too_many_arguments)]
fn assert_bra_block_total_grad_matches_fd<BuildRad, BuildPrimal>(
    stem: &str,
    primal_outputs: usize,
    frame_count: usize,
    base_seeds: &[f32],
    epsilons: &[f32],
    abs_tol: f32,
    build_rad_source: BuildRad,
    build_primal_source: BuildPrimal,
) where
    BuildRad: Fn(&[f32]) -> String,
    BuildPrimal: Fn(&[f32]) -> String,
{
    assert_eq!(base_seeds.len(), epsilons.len(), "seed/epsilon arity");
    let n = base_seeds.len();

    let rad_outputs = run_interp_temp_source(
        &format!("{stem}-rad"),
        &build_rad_source(base_seeds),
        frame_count,
    );
    assert_eq!(
        rad_outputs.len(),
        primal_outputs + n,
        "{stem}: layout must be [primals…, gradients…]"
    );

    for j in 0..n {
        let mut up = base_seeds.to_vec();
        up[j] += epsilons[j];
        let mut dn = base_seeds.to_vec();
        dn[j] -= epsilons[j];
        let primal_up = run_interp_temp_source(
            &format!("{stem}-plus-{j}"),
            &build_primal_source(&up),
            frame_count,
        );
        let primal_dn = run_interp_temp_source(
            &format!("{stem}-minus-{j}"),
            &build_primal_source(&dn),
            frame_count,
        );

        // Block-level finite difference gradient (sum over all frames and primal channels).
        let mut fd_total = 0.0_f32;
        for pi in 0..primal_outputs {
            let sum_up: f32 = primal_up[pi].iter().sum();
            let sum_dn: f32 = primal_dn[pi].iter().sum();
            fd_total += (sum_up - sum_dn) / (2.0 * epsilons[j]);
        }

        // TBPTT gradient: sum of per-sample adjoint outputs over the block.
        let total_rad_grad: f32 = rad_outputs[primal_outputs + j].iter().sum();

        assert_close(
            total_rad_grad,
            fd_total,
            abs_tol,
            &format!("{stem} total grad[seed {j}]: BRA={total_rad_grad} FD={fd_total}"),
        );
    }
}

/// Asserts RAD output bundle layout `[primals…, grad(seeds)…]` matches:
/// - the primal for each output via direct evaluation,
/// - each gradient lane via central finite difference on the primal source.
///
/// `build_rad_source(seeds)` must yield a `rad(expr, seeds)` program.
/// `build_primal_source(seeds)` builds the same `expr` without `rad(...)`.
#[allow(clippy::too_many_arguments)]
fn assert_rad_matches_central_difference<BuildRad, BuildPrimal>(
    stem: &str,
    primal_outputs: usize,
    frame_count: usize,
    base_seeds: &[f32],
    epsilons: &[f32],
    abs_tol: f32,
    build_rad_source: BuildRad,
    build_primal_source: BuildPrimal,
) where
    BuildRad: Fn(&[f32]) -> String,
    BuildPrimal: Fn(&[f32]) -> String,
{
    assert_eq!(
        base_seeds.len(),
        epsilons.len(),
        "seed/epsilon arity must match"
    );
    let n = base_seeds.len();

    let rad_outputs = run_interp_temp_source(
        &format!("{stem}-rad"),
        &build_rad_source(base_seeds),
        frame_count,
    );
    let primal_base = run_interp_temp_source(
        &format!("{stem}-primal"),
        &build_primal_source(base_seeds),
        frame_count,
    );
    assert_eq!(
        rad_outputs.len(),
        primal_outputs + n,
        "{stem}: rad output bundle layout = [primals…, gradients…]"
    );
    assert_eq!(primal_base.len(), primal_outputs);

    let mut primal_plus = Vec::with_capacity(n);
    let mut primal_minus = Vec::with_capacity(n);
    for j in 0..n {
        let mut up = base_seeds.to_vec();
        up[j] += epsilons[j];
        let mut dn = base_seeds.to_vec();
        dn[j] -= epsilons[j];
        primal_plus.push(run_interp_temp_source(
            &format!("{stem}-plus-{j}"),
            &build_primal_source(&up),
            frame_count,
        ));
        primal_minus.push(run_interp_temp_source(
            &format!("{stem}-minus-{j}"),
            &build_primal_source(&dn),
            frame_count,
        ));
    }

    for primal_index in 0..primal_outputs {
        for frame in 0..frame_count {
            assert_close(
                rad_outputs[primal_index][frame],
                primal_base[primal_index][frame],
                abs_tol,
                &format!("{stem} primal[{primal_index}] frame {frame}"),
            );
        }
    }

    // Gradient lanes are at indices [primal_outputs, primal_outputs + n).
    // Each gradient is the partial of `sum(primals)` w.r.t. seed j.
    for j in 0..n {
        for frame in 0..frame_count {
            let mut expected = 0.0_f32;
            for primal_index in 0..primal_outputs {
                expected += (primal_plus[j][primal_index][frame]
                    - primal_minus[j][primal_index][frame])
                    / (2.0 * epsilons[j]);
            }
            let actual = rad_outputs[primal_outputs + j][frame];
            assert_close(
                actual,
                expected,
                abs_tol,
                &format!("{stem} gradient[seed {j}] frame {frame}"),
            );
        }
    }
}

/// Asserts RAD gradients agree with the matching FAD tangent lanes for a
/// scalar primal. For multi-output primals the FAD lanes are summed, since
/// RAD's implicit cotangent is all-ones.
fn assert_rad_matches_fad<BuildRad, BuildFad>(
    stem: &str,
    primal_outputs: usize,
    seed_count: usize,
    frame_count: usize,
    abs_tol: f32,
    build_rad_source: BuildRad,
    build_fad_source: BuildFad,
) where
    BuildRad: Fn() -> String,
    BuildFad: Fn() -> String,
{
    let rad_outputs = run_interp_temp_source(
        &format!("{stem}-rad-vs-fad-rad"),
        &build_rad_source(),
        frame_count,
    );
    let fad_outputs = run_interp_temp_source(
        &format!("{stem}-rad-vs-fad-fad"),
        &build_fad_source(),
        frame_count,
    );
    // RAD layout: [p_0, …, p_{M-1}, grad_s0, …, grad_s{N-1}]
    // FAD layout: [p_0, t_0_s0, …, t_0_s{N-1}, p_1, t_1_s0, …]
    assert_eq!(rad_outputs.len(), primal_outputs + seed_count);
    assert_eq!(fad_outputs.len(), primal_outputs * (1 + seed_count));

    for primal_index in 0..primal_outputs {
        for frame in 0..frame_count {
            assert_close(
                rad_outputs[primal_index][frame],
                fad_outputs[primal_index * (1 + seed_count)][frame],
                abs_tol,
                &format!("{stem} primal[{primal_index}] RAD vs FAD frame {frame}"),
            );
        }
    }

    for j in 0..seed_count {
        for frame in 0..frame_count {
            let mut fad_sum = 0.0_f32;
            for primal_index in 0..primal_outputs {
                fad_sum += fad_outputs[primal_index * (1 + seed_count) + 1 + j][frame];
            }
            assert_close(
                rad_outputs[primal_outputs + j][frame],
                fad_sum,
                abs_tol,
                &format!("{stem} gradient[seed {j}] RAD vs FAD frame {frame}"),
            );
        }
    }
}

#[test]
fn fastlane_interp_lti_recursive_rad_feedback_coeff_matches_closed_form_contributions() {
    let frame_count = 6;
    let outputs = run_interp_temp_source(
        "rad-lti-recursive-feedback-coeff",
        r#"
p = 0.5;
process = rad((2 : + ~ *(p)), p);
"#,
        frame_count,
    );
    assert_eq!(
        outputs.len(),
        2,
        "RAD recursive fixture layout must be [primal, gradient]"
    );

    let p = 0.5_f32;
    let mut primals = Vec::with_capacity(frame_count);
    let mut previous_primal = 0.0_f32;
    for _ in 0..frame_count {
        let primal = 2.0 + p * previous_primal;
        primals.push(primal);
        previous_primal = primal;
    }

    let mut cotangents = vec![0.0_f32; frame_count];
    let mut next_cotangent = 0.0_f32;
    for frame in (0..frame_count).rev() {
        let cotangent = 1.0 + p * next_cotangent;
        cotangents[frame] = cotangent;
        next_cotangent = cotangent;
    }

    for frame in 0..frame_count {
        assert_close(
            outputs[0][frame],
            primals[frame],
            1.0e-6,
            &format!("rad_lti_recursive_feedback_coeff primal frame {frame}"),
        );

        let previous_state = if frame == 0 { 0.0 } else { primals[frame - 1] };
        let gradient_contribution = cotangents[frame] * previous_state;
        assert_close(
            outputs[1][frame],
            gradient_contribution,
            1.0e-6,
            &format!("rad_lti_recursive_feedback_coeff gradient frame {frame}"),
        );
    }
}

#[test]
fn fastlane_interp_audio_one_pole_lti_recursive_rad_matches_closed_form_contributions() {
    let input = vec![0.25, -0.5, 1.0, 0.75, -0.25, 0.5];
    let frame_count = input.len();
    let source = fs::read_to_string(corpus_path("rad_lti_recursive_one_pole.dsp"))
        .expect("read rad_lti_recursive_one_pole fixture");
    let outputs = run_interp_temp_source_with_inputs(
        "rad-audio-one-pole-lti-recursive-feedback-coeff",
        &source,
        std::slice::from_ref(&input),
        frame_count,
    );
    assert_eq!(
        outputs.len(),
        2,
        "RAD audio one-pole fixture layout must be [y, dp]"
    );

    let p = 0.5_f32;
    let mut primals = Vec::with_capacity(frame_count);
    let mut previous_primal = 0.0_f32;
    for x in &input {
        let primal = x + p * previous_primal;
        primals.push(primal);
        previous_primal = primal;
    }

    let mut cotangents = vec![0.0_f32; frame_count];
    let mut next_cotangent = 0.0_f32;
    for frame in (0..frame_count).rev() {
        let cotangent = 1.0 + p * next_cotangent;
        cotangents[frame] = cotangent;
        next_cotangent = cotangent;
    }

    for frame in 0..frame_count {
        assert_close(
            outputs[0][frame],
            primals[frame],
            1.0e-6,
            &format!("rad_audio_one_pole_lti_recursive primal frame {frame}"),
        );

        let previous_state = if frame == 0 { 0.0 } else { primals[frame - 1] };
        assert_close(
            outputs[1][frame],
            cotangents[frame] * previous_state,
            1.0e-6,
            &format!("rad_audio_one_pole_lti_recursive dp frame {frame}"),
        );
    }
}

#[test]
fn fastlane_interp_multi_output_lti_recursive_rad_matches_closed_form_contributions() {
    let frame_count = 6;
    let outputs = run_interp_corpus("rad_lti_recursive_multi_output", frame_count);
    assert_eq!(
        outputs.len(),
        4,
        "RAD recursive fixture layout must be [y0, y1, dp, dq]"
    );

    let cases = [
        (0usize, 2usize, 0.5_f32, 2.0_f32, "p"),
        (1usize, 3usize, 0.25_f32, 3.0_f32, "q"),
    ];
    for (primal_lane, gradient_lane, coeff, drive, label) in cases {
        let mut primals = Vec::with_capacity(frame_count);
        let mut previous_primal = 0.0_f32;
        for _ in 0..frame_count {
            let primal = drive + coeff * previous_primal;
            primals.push(primal);
            previous_primal = primal;
        }

        let mut cotangents = vec![0.0_f32; frame_count];
        let mut next_cotangent = 0.0_f32;
        for frame in (0..frame_count).rev() {
            let cotangent = 1.0 + coeff * next_cotangent;
            cotangents[frame] = cotangent;
            next_cotangent = cotangent;
        }

        for frame in 0..frame_count {
            assert_close(
                outputs[primal_lane][frame],
                primals[frame],
                1.0e-6,
                &format!("rad_multi_output_lti_recursive {label} primal frame {frame}"),
            );

            let previous_state = if frame == 0 { 0.0 } else { primals[frame - 1] };
            let gradient_contribution = cotangents[frame] * previous_state;
            assert_close(
                outputs[gradient_lane][frame],
                gradient_contribution,
                1.0e-6,
                &format!("rad_multi_output_lti_recursive d{label} frame {frame}"),
            );
        }
    }
}

#[test]
fn fastlane_interp_audio_state_space_lti_recursive_rad_matches_closed_form_contributions() {
    let drive = vec![0.25, -0.5, 1.0, 0.75, -0.25, 0.5];
    let zero = vec![0.0; drive.len()];
    let frame_count = drive.len();
    let source = fs::read_to_string(corpus_path("rad_lti_recursive_state_space.dsp"))
        .expect("read rad_lti_recursive_state_space fixture");
    let outputs = run_interp_temp_source_with_inputs(
        "rad-audio-state-space-lti-recursive-feedback-coeff",
        &source,
        &[drive.clone(), zero],
        frame_count,
    );
    assert_eq!(
        outputs.len(),
        4,
        "RAD audio state-space fixture layout must be [y0, y1, dp, dq]"
    );

    let p = 0.5_f32;
    let q = 0.25_f32;
    let mut y0 = vec![0.0_f32; frame_count];
    let mut y1 = vec![0.0_f32; frame_count];
    let mut prev0 = 0.0_f32;
    let mut prev1 = 0.0_f32;
    for frame in 0..frame_count {
        y0[frame] = drive[frame] + q * prev1;
        y1[frame] = p * prev0;
        prev0 = y0[frame];
        prev1 = y1[frame];
    }

    let mut lambda0 = vec![0.0_f32; frame_count];
    let mut lambda1 = vec![0.0_f32; frame_count];
    let mut next0 = 0.0_f32;
    let mut next1 = 0.0_f32;
    for frame in (0..frame_count).rev() {
        lambda0[frame] = 1.0 + p * next1;
        lambda1[frame] = 1.0 + q * next0;
        next0 = lambda0[frame];
        next1 = lambda1[frame];
    }

    for frame in 0..frame_count {
        assert_close(
            outputs[0][frame],
            y0[frame],
            1.0e-6,
            &format!("rad_audio_state_space_lti_recursive y0 frame {frame}"),
        );
        assert_close(
            outputs[1][frame],
            y1[frame],
            1.0e-6,
            &format!("rad_audio_state_space_lti_recursive y1 frame {frame}"),
        );

        let prev0 = if frame == 0 { 0.0 } else { y0[frame - 1] };
        let prev1 = if frame == 0 { 0.0 } else { y1[frame - 1] };
        assert_close(
            outputs[2][frame],
            lambda1[frame] * prev0,
            1.0e-6,
            &format!("rad_audio_state_space_lti_recursive dp frame {frame}"),
        );
        assert_close(
            outputs[3][frame],
            lambda0[frame] * prev1,
            1.0e-6,
            &format!("rad_audio_state_space_lti_recursive dq frame {frame}"),
        );
    }
}

#[test]
fn fastlane_interp_coupled_lti_recursive_rad_matches_closed_form_contributions() {
    let frame_count = 6;
    let outputs = run_interp_temp_source(
        "rad-coupled-lti-recursive-feedback-coeff",
        r#"
p = 0.5;
q = 0.25;
interleave22 = _,_,_,_ <: _,!,!,!, !,!,_,!, !,_,!,!, !,!,!,_;
cross2 = _,_ <: !,_,_,!;
core = (interleave22 : (+, +)) ~ ((*(p), *(q)) : cross2);
process = rad((2, 3) : core, (p, q));
"#,
        frame_count,
    );
    assert_eq!(
        outputs.len(),
        4,
        "RAD coupled recursive fixture layout must be [y0, y1, dp, dq]"
    );

    let p = 0.5_f32;
    let q = 0.25_f32;
    let mut y0 = vec![0.0_f32; frame_count];
    let mut y1 = vec![0.0_f32; frame_count];
    let mut prev0 = 0.0_f32;
    let mut prev1 = 0.0_f32;
    for frame in 0..frame_count {
        y0[frame] = 2.0 + q * prev1;
        y1[frame] = 3.0 + p * prev0;
        prev0 = y0[frame];
        prev1 = y1[frame];
    }

    let mut lambda0 = vec![0.0_f32; frame_count];
    let mut lambda1 = vec![0.0_f32; frame_count];
    let mut next0 = 0.0_f32;
    let mut next1 = 0.0_f32;
    for frame in (0..frame_count).rev() {
        lambda0[frame] = 1.0 + p * next1;
        lambda1[frame] = 1.0 + q * next0;
        next0 = lambda0[frame];
        next1 = lambda1[frame];
    }

    for frame in 0..frame_count {
        assert_close(
            outputs[0][frame],
            y0[frame],
            1.0e-6,
            &format!("rad_coupled_lti_recursive y0 frame {frame}"),
        );
        assert_close(
            outputs[1][frame],
            y1[frame],
            1.0e-6,
            &format!("rad_coupled_lti_recursive y1 frame {frame}"),
        );

        let prev0 = if frame == 0 { 0.0 } else { y0[frame - 1] };
        let prev1 = if frame == 0 { 0.0 } else { y1[frame - 1] };
        assert_close(
            outputs[2][frame],
            lambda1[frame] * prev0,
            1.0e-6,
            &format!("rad_coupled_lti_recursive dp frame {frame}"),
        );
        assert_close(
            outputs[3][frame],
            lambda0[frame] * prev1,
            1.0e-6,
            &format!("rad_coupled_lti_recursive dq frame {frame}"),
        );
    }
}

#[test]
fn rad_polynomial_two_seeds_matches_central_difference() {
    fn rad_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = rad(a*a*b + b, (a, b));
"#
        )
    }
    fn primal_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = a*a*b + b;
"#
        )
    }
    assert_rad_matches_central_difference(
        "polynomial-two-seeds",
        1,
        4,
        &[1.5, -0.7],
        &[1.0e-3, 1.0e-3],
        5.0e-3,
        rad_source,
        primal_source,
    );
}

#[test]
fn rad_trig_composition_matches_central_difference() {
    fn rad_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = rad(sin(a*b), (a, b));
"#
        )
    }
    fn primal_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = sin(a*b);
"#
        )
    }
    assert_rad_matches_central_difference(
        "trig-composition",
        1,
        4,
        &[0.6, 0.4],
        &[1.0e-3, 1.0e-3],
        5.0e-3,
        rad_source,
        primal_source,
    );
}

#[test]
fn rad_repeated_seed_duplicates_gradient_lane() {
    // rad(a*b, (a, a)) must produce the same gradient on lane 0 and lane 1
    // since both seed lanes refer to the same `SigId`.
    fn rad_source(seeds: &[f32]) -> String {
        let a = seeds[0];
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = rad(a*b, (a, a));
"#
        )
    }
    fn primal_source(seeds: &[f32]) -> String {
        let a = seeds[0];
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = a*b;
"#
        )
    }
    let frame_count = 2;
    let base = [0.5_f32, 0.5_f32];
    let eps = [1.0e-3_f32, 1.0e-3_f32];
    let outs = run_interp_temp_source("rad-repeated-seed-rad", &rad_source(&base), frame_count);
    let primal_plus = run_interp_temp_source(
        "rad-repeated-seed-plus",
        &primal_source(&[base[0] + eps[0]]),
        frame_count,
    );
    let primal_minus = run_interp_temp_source(
        "rad-repeated-seed-minus",
        &primal_source(&[base[0] - eps[0]]),
        frame_count,
    );
    assert_eq!(outs.len(), 3, "1 primal + 2 (repeated) gradient lanes");
    for frame in 0..frame_count {
        let expected = (primal_plus[0][frame] - primal_minus[0][frame]) / (2.0 * eps[0]);
        assert_close(
            outs[1][frame],
            expected,
            5.0e-3,
            &format!("rad-repeated-seed lane 0 frame {frame}"),
        );
        assert_close(
            outs[2][frame],
            outs[1][frame],
            1.0e-6,
            &format!("rad-repeated-seed lane 1 must equal lane 0 frame {frame}"),
        );
    }
}

#[test]
fn rad_absent_seed_yields_zero_gradient() {
    // rad(sin(x), y): y does not appear in sin(x), so its gradient must be
    // exactly zero even though x is a UI control.
    let source = r#"
x = hslider("x", 0.3, -1.0, 1.0, 0.01);
y = hslider("y", 0.7, -1.0, 1.0, 0.01);
process = rad(sin(x), y);
"#;
    let outs = run_interp_temp_source("rad-absent-seed", source, 4);
    assert_eq!(outs.len(), 2, "1 primal + 1 absent-seed gradient");
    for (frame, sample) in outs[1].iter().copied().enumerate().take(4) {
        assert_close(sample, 0.0, 1.0e-6, &format!("absent-seed frame {frame}"));
    }
}

#[test]
fn rad_multi_output_uses_implicit_all_ones_cotangent() {
    // rad((a*b, sin(a)), (a, b)) must produce gradients of (a*b + sin(a))
    // w.r.t. each seed: d/da = b + cos(a); d/db = a.
    fn rad_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = rad((a*b, sin(a)), (a, b));
"#
        )
    }
    fn primal_source(seeds: &[f32]) -> String {
        let (a, b) = (seeds[0], seeds[1]);
        format!(
            r#"
a = hslider("a", {a}, -2.0, 2.0, 0.001);
b = hslider("b", {b}, -2.0, 2.0, 0.001);
process = (a*b, sin(a));
"#
        )
    }
    assert_rad_matches_central_difference(
        "multi-output-implicit-ones",
        2,
        3,
        &[0.4, 0.3],
        &[1.0e-3, 1.0e-3],
        5.0e-3,
        rad_source,
        primal_source,
    );
}

#[test]
fn rad_vs_fad_parity_on_polynomial() {
    let rad = || {
        r#"
a = hslider("a", 0.5, -2.0, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = rad(a*a*b + b, (a, b));
"#
        .to_string()
    };
    let fad = || {
        r#"
a = hslider("a", 0.5, -2.0, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = fad(a*a*b + b, (a, b));
"#
        .to_string()
    };
    assert_rad_matches_fad("rad-vs-fad-polynomial", 1, 2, 4, 1.0e-5, rad, fad);
}

#[test]
fn rad_vs_fad_parity_on_trig_with_min_max() {
    let rad = || {
        r#"
a = hslider("a", 0.5, -2.0, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = rad(min(sin(a*b), max(a, b)), (a, b));
"#
        .to_string()
    };
    let fad = || {
        r#"
a = hslider("a", 0.5, -2.0, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = fad(min(sin(a*b), max(a, b)), (a, b));
"#
        .to_string()
    };
    assert_rad_matches_fad("rad-vs-fad-min-max", 1, 2, 4, 1.0e-5, rad, fad);
}

#[test]
fn rad_vs_fad_parity_on_tanh_ffun() {
    // tanh is a unary foreign function in Faust; phase C wires the same
    // chain rule used by FAD.
    let rad = || {
        r#"
tanh = ffunction(float tanhf|tanh|tanhl (float), <math.h>, "");
a = hslider("a", 0.5, -2.0, 2.0, 0.001);
process = rad(tanh(a*a), a);
"#
        .to_string()
    };
    let fad = || {
        r#"
tanh = ffunction(float tanhf|tanh|tanhl (float), <math.h>, "");
a = hslider("a", 0.5, -2.0, 2.0, 0.001);
process = fad(tanh(a*a), a);
"#
        .to_string()
    };
    assert_rad_matches_fad("rad-vs-fad-tanh-ffun", 1, 1, 4, 5.0e-5, rad, fad);
}

#[test]
fn rad_vs_fad_parity_on_supported_hyperbolic_ffuns() {
    let cases = [
        (
            "rad-vs-fad-sinh-ffun",
            r#"
sinh = ffunction(float sinhf|sinh|sinhl (float), <math.h>, "");
a = hslider("a", 0.35, -2.0, 2.0, 0.001);
process = rad(sinh(a*a), a);
"#,
            r#"
sinh = ffunction(float sinhf|sinh|sinhl (float), <math.h>, "");
a = hslider("a", 0.35, -2.0, 2.0, 0.001);
process = fad(sinh(a*a), a);
"#,
            5.0e-5,
        ),
        (
            "rad-vs-fad-cosh-ffun",
            r#"
cosh = ffunction(float coshf|cosh|coshl (float), <math.h>, "");
a = hslider("a", 0.35, -2.0, 2.0, 0.001);
process = rad(cosh(a*a), a);
"#,
            r#"
cosh = ffunction(float coshf|cosh|coshl (float), <math.h>, "");
a = hslider("a", 0.35, -2.0, 2.0, 0.001);
process = fad(cosh(a*a), a);
"#,
            5.0e-5,
        ),
        (
            "rad-vs-fad-atanh-ffun",
            r#"
atanh = ffunction(float atanhf|atanh|atanhl (float), <math.h>, "");
a = hslider("a", 0.2, -0.8, 0.8, 0.001);
process = rad(atanh(a*a), a);
"#,
            r#"
atanh = ffunction(float atanhf|atanh|atanhl (float), <math.h>, "");
a = hslider("a", 0.2, -0.8, 0.8, 0.001);
process = fad(atanh(a*a), a);
"#,
            5.0e-5,
        ),
        (
            "rad-vs-fad-asinh-ffun",
            r#"
asinh = ffunction(float asinhf|asinh|asinhl (float), <math.h>, "");
a = hslider("a", 0.45, -2.0, 2.0, 0.001);
process = rad(asinh(a*a), a);
"#,
            r#"
asinh = ffunction(float asinhf|asinh|asinhl (float), <math.h>, "");
a = hslider("a", 0.45, -2.0, 2.0, 0.001);
process = fad(asinh(a*a), a);
"#,
            5.0e-5,
        ),
        (
            "rad-vs-fad-acosh-ffun",
            r#"
acosh = ffunction(float acoshf|acosh|acoshl (float), <math.h>, "");
a = hslider("a", 1.4, 1.1, 3.0, 0.001);
process = rad(acosh(a*a), a);
"#,
            r#"
acosh = ffunction(float acoshf|acosh|acoshl (float), <math.h>, "");
a = hslider("a", 1.4, 1.1, 3.0, 0.001);
process = fad(acosh(a*a), a);
"#,
            1.0e-4,
        ),
    ];

    for (stem, rad_source, fad_source, abs_tol) in cases {
        assert_rad_matches_fad(
            stem,
            1,
            1,
            4,
            abs_tol,
            || rad_source.to_string(),
            || fad_source.to_string(),
        );
    }
}

#[test]
fn rad_vs_fad_parity_on_readonly_table_index() {
    // Read-only `rdtable(waveform{...}, idx)` is differentiable through
    // the read index via the symmetric finite-difference slope.
    let rad = || {
        r#"
k = hslider("k", 3.0, 1, 6, 1);
process = rad(rdtable(waveform{0, 1, 4, 9, 16, 25, 36, 49}, k), k);
"#
        .to_string()
    };
    let fad = || {
        r#"
k = hslider("k", 3.0, 1, 6, 1);
process = fad(rdtable(waveform{0, 1, 4, 9, 16, 25, 36, 49}, k), k);
"#
        .to_string()
    };
    assert_rad_matches_fad("rad-vs-fad-rdtbl", 1, 1, 4, 1.0e-5, rad, fad);
}

#[test]
fn rad_vs_fad_parity_on_pow_select2() {
    let rad = || {
        r#"
a = hslider("a", 0.5, 0.1, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = rad(select2(a > 0.0, pow(a, b), b), (a, b));
"#
        .to_string()
    };
    let fad = || {
        r#"
a = hslider("a", 0.5, 0.1, 2.0, 0.001);
b = hslider("b", 0.7, -2.0, 2.0, 0.001);
process = fad(select2(a > 0.0, pow(a, b), b), (a, b));
"#
        .to_string()
    };
    assert_rad_matches_fad("rad-vs-fad-pow-select2", 1, 2, 4, 1.0e-5, rad, fad);
}

// -----------------------------------------------------------------------
// Corpus-driven smoke tests
// -----------------------------------------------------------------------
//
// Each fixture lives in `tests/corpus/`. Validating them through the
// compiler+interp pipeline guarantees parser/eval/propagate/transform/FIR
// all stay aligned with the documented RAD contract. Where the gradient
// has a closed form we also assert the numeric value on frame 0.

#[test]
fn corpus_rad_basic_compiles_and_emits_two_lanes() {
    let outs = run_interp_corpus("rad_basic", 2);
    assert_eq!(outs.len(), 2, "rad_basic = [primal, gradient]");
}

#[test]
fn corpus_rad_product_multi_seed_emits_expected_gradients() {
    // process = rad(a*b, (a, b)) at a=1, b=2 → [a*b, b, a] = [2, 2, 1]
    let outs = run_interp_corpus("rad_product_multi_seed", 1);
    assert_eq!(outs.len(), 3);
    assert_close(outs[0][0], 2.0, 1.0e-5, "primal a*b");
    assert_close(outs[1][0], 2.0, 1.0e-5, "d/da (a*b) = b");
    assert_close(outs[2][0], 1.0, 1.0e-5, "d/db (a*b) = a");
}

#[test]
fn corpus_rad_trig_composition_compiles_with_three_outputs() {
    // process = rad(sin(a*b), (a, b)) → 3 outputs.
    let outs = run_interp_corpus("rad_trig_composition", 1);
    assert_eq!(outs.len(), 3);
}

#[test]
fn corpus_rad_absent_seed_produces_zero_gradient_for_unreachable_seed() {
    // process = rad(sin(x), y) → [sin(x), 0.0]
    let outs = run_interp_corpus("rad_absent_seed", 2);
    assert_eq!(outs.len(), 2);
    for (frame, sample) in outs[1].iter().copied().enumerate().take(2) {
        assert_close(
            sample,
            0.0,
            1.0e-6,
            &format!("absent seed gradient frame {frame}"),
        );
    }
}

#[test]
fn corpus_rad_repeated_seed_duplicates_gradient_lane_verbatim() {
    // process = rad(a*b, (a, a)) → both gradient lanes equal d/da (a*b) = b.
    let outs = run_interp_corpus("rad_repeated_seed", 1);
    assert_eq!(outs.len(), 3);
    assert_close(
        outs[1][0],
        outs[2][0],
        1.0e-6,
        "repeated-seed lanes must alias",
    );
    assert_close(outs[1][0], 0.7, 1.0e-5, "d/da (a*b) at b=0.7");
}

#[test]
fn corpus_rad_multi_output_sum_cotangent_emits_expected_gradients() {
    // process = rad((a*b, sin(a)), (a, b)) at a=0.4, b=0.3
    // primals  = [a*b, sin(a)]                  ≈ [0.12, 0.3894]
    // grad/da  = b + cos(a)                     ≈ 0.3 + cos(0.4) ≈ 1.2211
    // grad/db  = a                              = 0.4
    let outs = run_interp_corpus("rad_multi_output_sum_cotangent", 1);
    assert_eq!(outs.len(), 4);
    assert_close(outs[0][0], 0.4 * 0.3, 5.0e-5, "primal a*b");
    let sin_a = (0.4_f32).sin();
    assert_close(outs[1][0], sin_a, 5.0e-5, "primal sin(a)");
    let cos_a = (0.4_f32).cos();
    assert_close(outs[2][0], 0.3 + cos_a, 5.0e-5, "d/da sum = b + cos(a)");
    assert_close(outs[3][0], 0.4, 5.0e-5, "d/db sum = a");
}

#[test]
fn corpus_rad_rdtbl_index_basic_emits_two_outputs() {
    // process = rad(rdtable(waveform{0,1,4,9,16,25,36,49}, k), k)
    // The slope is the symmetric finite difference. We just confirm the
    // arity here; numeric parity vs FAD is exercised in
    // `rad_vs_fad_parity_on_readonly_table_index`.
    let outs = run_interp_corpus("rad_rdtbl_index_basic", 2);
    assert_eq!(outs.len(), 2);
}

#[test]
fn corpus_err_rad_zero_body_surfaces_rad_body_arity_diagnostic() {
    let path = corpus_path("err_rad_zero_body.dsp");
    let source = std::fs::read_to_string(&path).expect("read err_rad_zero_body fixture");
    let compiler = Compiler::new();
    let err = compiler
        .compile_source_to_signals("err_rad_zero_body.dsp", &source)
        .expect_err("zero-output body must fail at propagate stage");
    let diagnostics = err.diagnostic_bundle();
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|d| d.message.contains("rad body")),
        "diagnostic must name rad body arity: {diagnostics:?}"
    );
}

#[test]
fn corpus_err_rad_zero_seed_surfaces_rad_seed_arity_diagnostic() {
    let path = corpus_path("err_rad_zero_seed.dsp");
    let source = std::fs::read_to_string(&path).expect("read err_rad_zero_seed fixture");
    let compiler = Compiler::new();
    let err = compiler
        .compile_source_to_signals("err_rad_zero_seed.dsp", &source)
        .expect_err("zero-output seeds must fail at propagate stage");
    let diagnostics = err.diagnostic_bundle();
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|d| d.message.contains("rad seeds")),
        "diagnostic must name rad seeds arity: {diagnostics:?}"
    );
}

// -----------------------------------------------------------------------
// Nested AD combinations
// -----------------------------------------------------------------------
//
// FAD and RAD have the same two-child surface, so they can be nested in
// either order. The tests here pin two contracts:
//
// 1. The output bundle layouts compose multiplicatively (FAD) or
//    additively (RAD) as documented in `docs/fad-...md` and
//    `docs/rad-note-en.md`.
// 2. Second-order derivatives computed two different ways agree
//    numerically. Specifically:
//    - `fad(rad(f, s), s)` — outer FAD over RAD: the third lane is the
//      second derivative `f''(s)`.
//    - `rad(fad(f, s), s)` — outer RAD over FAD: the only gradient lane
//      is `d/ds (f + f')(s) = f'(s) + f''(s)` (sum cotangent over the
//      two-output FAD bundle).
//
// Both rely on the feed-forward subset only; nested temporal cases are
// covered by the temporal BlockReverseAD fallback tests further up.

#[test]
fn nested_fad_rad_on_quadratic_matches_second_derivative() {
    // f(x) = x*x  ⇒  f'(x) = 2x, f''(x) = 2
    // inner rad(x*x, x)         → [x*x,  2x]
    // outer fad([x*x, 2x], x)   → [x*x, 2x, 2x, 2]                 (4 lanes)
    let source = r#"
x = hslider("x", 1.5, -2.0, 2.0, 0.001);
process = fad(rad(x*x, x), x);
"#;
    let outs = run_interp_temp_source("nested-fad-rad-quadratic", source, 2);
    assert_eq!(
        outs.len(),
        4,
        "fad(rad(f, s), s) layout = primal+tangent for each of rad's 2 outputs"
    );
    let x = 1.5_f32;
    assert_close(outs[0][0], x * x, 1.0e-5, "primal x*x");
    assert_close(outs[1][0], 2.0 * x, 1.0e-5, "tangent of x*x w.r.t. x");
    assert_close(outs[2][0], 2.0 * x, 1.0e-5, "rad first-order primal");
    assert_close(outs[3][0], 2.0, 1.0e-5, "second derivative f''(x) = 2");
}

#[test]
fn nested_rad_fad_on_quadratic_matches_first_plus_second_derivative() {
    // inner fad(x*x, x)         → [x*x, 2x]
    // outer rad([x*x, 2x], x)   → [x*x, 2x, d/dx(x*x + 2x) = 2x + 2]
    let source = r#"
x = hslider("x", 1.5, -2.0, 2.0, 0.001);
process = rad(fad(x*x, x), x);
"#;
    let outs = run_interp_temp_source("nested-rad-fad-quadratic", source, 2);
    assert_eq!(
        outs.len(),
        3,
        "rad(fad(f, s), s) layout = [primals…, gradient(s)]"
    );
    let x = 1.5_f32;
    assert_close(outs[0][0], x * x, 1.0e-5, "fad primal x*x");
    assert_close(outs[1][0], 2.0 * x, 1.0e-5, "fad tangent 2x");
    assert_close(
        outs[2][0],
        2.0 * x + 2.0,
        1.0e-5,
        "rad sum-cotangent gradient = f'(x) + f''(x)",
    );
}

#[test]
fn nested_fad_rad_on_trig_matches_second_derivative_via_finite_difference() {
    // f(x) = sin(x*x). Inner rad gives [sin(x*x), 2x*cos(x*x)]; outer
    // fad against x gives a 4-output bundle whose last lane is f''(x).
    // We compare that lane against a central finite difference on f' to
    // catch any second-order index-arithmetic regression.
    fn outer_source(x: f32) -> String {
        format!(
            r#"
x = hslider("x", {x}, -2.0, 2.0, 0.001);
process = fad(rad(sin(x*x), x), x);
"#
        )
    }
    fn inner_grad_source(x: f32) -> String {
        // Just rad(sin(x*x), x): first-order gradient as a primal.
        format!(
            r#"
x = hslider("x", {x}, -2.0, 2.0, 0.001);
process = rad(sin(x*x), x);
"#
        )
    }
    let base = 0.7_f32;
    let eps = 1.0e-3_f32;
    let outer = run_interp_temp_source("nested-fad-rad-trig-outer", &outer_source(base), 2);
    let grad_plus = run_interp_temp_source(
        "nested-fad-rad-trig-grad-plus",
        &inner_grad_source(base + eps),
        2,
    );
    let grad_minus = run_interp_temp_source(
        "nested-fad-rad-trig-grad-minus",
        &inner_grad_source(base - eps),
        2,
    );
    // outer layout: [f, df/dx, g = f', dg/dx = f''] across 4 lanes.
    assert_eq!(outer.len(), 4);
    // Frame-0 second-derivative lane vs. central difference on the
    // first-order gradient (lane 1 of inner rad output = the 2nd output).
    let expected_second = (grad_plus[1][0] - grad_minus[1][0]) / (2.0 * eps);
    assert_close(
        outer[3][0],
        expected_second,
        2.0e-3,
        "f''(x) from fad(rad(...)) vs. central diff of rad gradient",
    );
}

#[test]
fn nested_rad_fad_multi_seed_routes_implicit_cotangent_through_inner_lanes() {
    // f(x, y) = x*y ; inner fad against (x, y) → [x*y, y, x] (3 outputs)
    // Outer rad against (x, y) sums the inner lanes:
    //   primals  = [x*y, y, x]
    //   d/dx sum = d/dx (x*y + y + x) = y + 1
    //   d/dy sum = d/dy (x*y + y + x) = x + 1
    // Final bundle: [x*y, y, x, y+1, x+1] (5 outputs).
    let source = r#"
x = hslider("x", 0.6, -2.0, 2.0, 0.001);
y = hslider("y", 0.4, -2.0, 2.0, 0.001);
process = rad(fad(x*y, (x, y)), (x, y));
"#;
    let outs = run_interp_temp_source("nested-rad-fad-multi-seed", source, 1);
    assert_eq!(
        outs.len(),
        5,
        "outer rad bundle = [primals (3), grad/dx, grad/dy]"
    );
    let x = 0.6_f32;
    let y = 0.4_f32;
    assert_close(outs[0][0], x * y, 1.0e-5, "fad primal x*y");
    assert_close(outs[1][0], y, 1.0e-5, "fad tangent w.r.t. x = y");
    assert_close(outs[2][0], x, 1.0e-5, "fad tangent w.r.t. y = x");
    assert_close(outs[3][0], y + 1.0, 1.0e-5, "rad d/dx sum = y + 1");
    assert_close(outs[4][0], x + 1.0, 1.0e-5, "rad d/dy sum = x + 1");
}

// Corpus-driven mixed AD tests. The inline tests above already cover
// the same shapes; these reach the fixtures in tests/corpus/ so the
// source-level surface (parser + eval + propagate + transform + interp)
// is exercised end-to-end against committed DSP files.

#[test]
fn corpus_fad_rad_quadratic_emits_second_derivative_lane() {
    // tests/corpus/fad_rad_quadratic.dsp
    //   process = fad(rad(x*x, x), x);  x = 1.5
    // Expected lanes: [x*x, 2x, 2x, 2].
    let outs = run_interp_corpus("fad_rad_quadratic", 1);
    assert_eq!(outs.len(), 4);
    let x = 1.5_f32;
    assert_close(outs[0][0], x * x, 1.0e-5, "primal x*x");
    assert_close(outs[1][0], 2.0 * x, 1.0e-5, "fad tangent 2x");
    assert_close(outs[2][0], 2.0 * x, 1.0e-5, "rad first-order primal");
    assert_close(outs[3][0], 2.0, 1.0e-5, "f''(x) = 2");
}

#[test]
fn corpus_rad_fad_quadratic_sums_first_and_second_derivative() {
    // tests/corpus/rad_fad_quadratic.dsp
    //   process = rad(fad(x*x, x), x);  x = 1.5
    // Expected lanes: [x*x, 2x, 2x + 2].
    let outs = run_interp_corpus("rad_fad_quadratic", 1);
    assert_eq!(outs.len(), 3);
    let x = 1.5_f32;
    assert_close(outs[0][0], x * x, 1.0e-5, "fad primal x*x");
    assert_close(outs[1][0], 2.0 * x, 1.0e-5, "fad tangent 2x");
    assert_close(
        outs[2][0],
        2.0 * x + 2.0,
        1.0e-5,
        "rad sum-cotangent gradient = f'(x) + f''(x)",
    );
}

#[test]
fn corpus_fad_rad_trig_second_derivative_compiles_and_has_four_lanes() {
    // tests/corpus/fad_rad_trig_second_derivative.dsp
    //   process = fad(rad(sin(x*x), x), x);
    // We just check arity here; numeric parity for the second-derivative
    // lane against a central finite difference is exercised by the
    // inline `nested_fad_rad_on_trig_...` test, which can perturb the
    // seed without needing a separate fixture.
    let outs = run_interp_corpus("fad_rad_trig_second_derivative", 1);
    assert_eq!(outs.len(), 4);
}

#[test]
fn corpus_rad_fad_multi_seed_routes_implicit_cotangent_through_inner_lanes() {
    // tests/corpus/rad_fad_multi_seed.dsp
    //   process = rad(fad(x*y, (x, y)), (x, y));  x = 0.6, y = 0.4
    // Expected lanes: [x*y, y, x, y+1, x+1].
    let outs = run_interp_corpus("rad_fad_multi_seed", 1);
    assert_eq!(outs.len(), 5);
    let x = 0.6_f32;
    let y = 0.4_f32;
    assert_close(outs[0][0], x * y, 1.0e-5, "fad primal x*y");
    assert_close(outs[1][0], y, 1.0e-5, "fad tangent w.r.t. x = y");
    assert_close(outs[2][0], x, 1.0e-5, "fad tangent w.r.t. y = x");
    assert_close(outs[3][0], y + 1.0, 1.0e-5, "rad d/dx sum = y + 1");
    assert_close(outs[4][0], x + 1.0, 1.0e-5, "rad d/dy sum = x + 1");
}

#[test]
fn corpus_err_fad_rad_temporal_inner_falls_back_to_block_mode() {
    // tests/corpus/err_fad_rad_temporal.dsp
    //   process = fad(rad(x', x), x);
    // Phase B1: the inner `rad` falls back to BlockReverseAD, so the outer
    // `fad` receives two BlockReverseAD projections and succeeds.
    // Outputs = 4: two primal projections + two tangent projections.
    let path = corpus_path("err_fad_rad_temporal.dsp");
    let source = std::fs::read_to_string(&path).expect("read err_fad_rad_temporal fixture");
    let compiler = Compiler::new();
    let out = compiler
        .compile_source_to_signals("err_fad_rad_temporal.dsp", &source)
        .expect("fad(rad(x', x), x) must succeed via BlockReverseAD fallback (Phase B1)");
    // fad expands 2 primal outputs × 1 seed → 4 outputs
    assert_eq!(out.signals.len(), 4, "fad(rad(delay1,x),x) → 4 outputs");
}

#[test]
fn nested_rad_in_fad_temporal_inner_falls_back_to_block_mode() {
    // Phase B1: `fad(rad(x', x), x)` no longer errors. The inner rad falls
    // back to BlockReverseAD, and FAD differentiates the resulting projections.
    use compiler::Compiler;
    let source = r#"
x = hslider("x", 0.0, -1.0, 1.0, 0.01);
process = fad(rad(x', x), x);
"#;
    let compiler = Compiler::new();
    let out = compiler
        .compile_source_to_signals("nested-rad-in-fad-temporal.dsp", source)
        .expect("fad(rad(x', x), x) must succeed via BlockReverseAD fallback (Phase B1)");
    assert_eq!(out.signals.len(), 4, "fad(rad(delay1,x),x) → 4 outputs");
}

// ── Phase B6: recursive BlockReverseAD tests ──────────────────────────────────

/// LTV one-pole: `y[n] = c + c * y[n-1]`, seed = c (slider).
///
/// The feedback coefficient IS the seed, making this **linear time-varying**
/// (LTV).  Phase B1 falls back to `BlockReverseAD` (BRA).  Phase B6 lowers
/// the backward sweep through `Proj(0, Rec([Add(c, Mul(c, Delay1(Proj(0,…))))]))`.
///
/// **Verification**: the block-level TBPTT gradient (sum of per-sample
/// adjoints) equals the central finite difference of the block-sum primal
/// `L = Σ_n y[n]` w.r.t. c.  For c = 0.5, BS = 8, L is a geometric series
/// and the total gradient is ~24.09 (analytically).
#[test]
fn rad_ltv_one_pole_bra_total_grad_matches_fd() {
    assert_bra_block_total_grad_matches_fd(
        "rad-ltv-one-pole",
        1,
        8,
        &[0.5],
        &[1e-3],
        5e-2,
        |s| {
            format!(
                r#"c = hslider("c", {}, 0.01, 0.99, 0.001); process = rad(c : +~*(c), c);"#,
                s[0]
            )
        },
        |s| {
            format!(
                r#"c = hslider("c", {}, 0.01, 0.99, 0.001); process = c : +~*(c);"#,
                s[0]
            )
        },
    );
}

/// Nonlinear one-pole: `y[n] = c + sin(y[n-1])`, seed = c (slider).
///
/// `+~sin` is nonlinear in the recursive state → Phase B1 falls back to
/// `BlockReverseAD`.  Phase B6 lowers the backward sweep through
/// `Proj(0, Rec([Add(c, Sin(Delay1(Proj(0,…))))]))`.
///
/// The backward rule for `Sin(x)` uses `load_bra_fwd_value` to read
/// `Delay1(Proj(0, group))` (the previous recursive state) from the forward
/// tape, then computes `adj[Delay1(Proj)] += y_bar * cos(x_taped)`.
///
/// **Verification**: block-level TBPTT total gradient matches central FD.
/// For c = 0.1, BS = 8 the circuit is numerically stable
/// (converges to fixed point ~1.11 for the infinite-time case).
#[test]
fn rad_nonlinear_one_pole_bra_total_grad_matches_fd() {
    assert_bra_block_total_grad_matches_fd(
        "rad-nl-one-pole",
        1,
        8,
        &[0.1],
        &[5e-4],
        5e-2,
        |s| {
            format!(
                r#"c = hslider("c", {}, -0.9, 0.9, 0.001); process = rad(c : +~sin, c);"#,
                s[0]
            )
        },
        |s| {
            format!(
                r#"c = hslider("c", {}, -0.9, 0.9, 0.001); process = c : +~sin;"#,
                s[0]
            )
        },
    );
}

/// BlockReverseAD cross-validation against a seed-independent LTI carrier:
/// the block total must agree numerically on `d(Σ_n y[n])/d(a)` where
/// `y[n] = a * y_lti[n]` and `y_lti[n] = 2 + p * y_lti[n-1]`.
///
/// This used to cross-check the dormant `ReverseTimeRec` fast path. Public RAD
/// now routes the recursive body through BlockReverseAD, so the test is a
/// finite-difference guard for the BRA path when the requested seed is outside
/// the recursive carrier itself.
///
/// `rad((a * (2 : +~*(p))), a)` with `p = 0.5` (constant) — the seed `a`
/// multiplies the primal output but the recursion itself is LTI-in-state.
/// For this layout:
/// - primal[n] = a * y_lti[n]  where y_lti[n] = 2 + p * y_lti[n-1]
/// - gradient[n] = y_lti[n]  (d(a*y_lti[n])/d(a) = y_lti[n])
///
/// Total gradient = Σ_n y_lti[n] — verified by FD with ε = 1e-3.
#[test]
fn rad_lti_vs_bra_seed_independent_total_grad_agrees() {
    // This uses the LTI fast path (a only scales the output, not the recursion).
    assert_bra_block_total_grad_matches_fd(
        "rad-lti-vs-bra-cross",
        1,
        6,
        &[1.0], // a = 1.0 baseline
        &[1e-3],
        5e-2,
        |s| {
            format!(
                r#"
p = 0.5;
a = hslider("a", {}, 0.1, 3.0, 0.001);
process = rad(a * (2 : +~*(p)), a);
"#,
                s[0]
            )
        },
        |s| {
            format!(
                r#"
p = 0.5;
a = hslider("a", {}, 0.1, 3.0, 0.001);
process = a * (2 : +~*(p));
"#,
                s[0]
            )
        },
    );
}

/// BRA primal DSP state persists across consecutive `compute()` calls.
///
/// Circuit: `process = rad((2 : + ~ *(p)), p)` with p = 0.5.
/// Expected recurrence: `y[n] = 2 + 0.5 * y[n-1]`, starting from `y[-1] = 0`.
///
/// The bug (fixed): `fRec<N>` (the SYMREC primal state) was incorrectly
/// reset to 0.0 in the `compute()` preamble by `emit_reverse_time_rec_compute_resets`,
/// which was iterating over ALL carriers in `rec_array_by_group_index` instead of
/// filtering to only `ReverseTimeRec` adjoint carriers.  As a result each
/// `compute()` call restarted the filter from silence, producing the same
/// primal values in every block rather than a diverging ramp.
///
/// **Verification**: the first value of block 2's primal channel must equal
/// `y[block_size]` computed analytically from the recurrence, not `y[0] = 2`.
#[test]
fn rad_bra_primal_state_persists_across_compute_blocks() {
    let src = r#"
p = hslider("p", 0.5, 0.0, 1.0, 0.01);
process = rad((2 : + ~ *(p)), p);
"#;
    let block_size = 4_usize;
    let (block1, block2) = run_interp_two_blocks("rad-bra-persist", src, block_size);

    // Analytically compute the expected primal recurrence y[n] = 2 + 0.5*y[n-1].
    let p = 0.5_f32;
    let mut y = 0.0_f32;
    let mut expected_b1 = Vec::with_capacity(block_size);
    for _ in 0..block_size {
        y = 2.0 + p * y;
        expected_b1.push(y);
    }
    let mut expected_b2 = Vec::with_capacity(block_size);
    for _ in 0..block_size {
        y = 2.0 + p * y;
        expected_b2.push(y);
    }

    // Verify block 1 primal values match the recurrence.
    for (i, (&actual, &expected)) in block1.iter().zip(expected_b1.iter()).enumerate() {
        assert_close(
            actual,
            expected,
            1e-5,
            &format!("block1[{i}]: primal must follow recurrence"),
        );
    }

    // Verify block 2 primal values are a CONTINUATION, not a restart from y[-1]=0.
    // With the old bug, block2[0] would be y[0]=2.0; with the fix it must be
    // y[block_size] which diverges from 2.0 for block_size ≥ 1.
    for (i, (&actual, &expected)) in block2.iter().zip(expected_b2.iter()).enumerate() {
        assert_close(
            actual,
            expected,
            1e-5,
            &format!("block2[{i}]: primal must continue recurrence, not restart"),
        );
    }
    assert!(
        (block2[0] - 2.0).abs() > 0.1,
        "block2[0]={} must differ from 2.0 (a restart from zero) by >0.1",
        block2[0]
    );
}

/// Two independent recursive poles differentiated simultaneously (Phase B6+).
///
/// Circuit:  `y_p[n] = 2 + p * y_p[n-1]`,  `y_q[n] = 3 + q * y_q[n-1]`.
/// Seeds: `(p, q)` — two separate SYMREC groups, each with its own
/// SYMREF variable.
///
/// Before the multi-SYMREC fix the lowering crashed with
/// "unbound symbolic recursion variable" because:
/// (a) the pre-scan mapped both `SYMREF_p` and `SYMREF_q` to
///     `body_sigs[0]` using the slot index, and
/// (b) `ensure_bra_tape_stores` lowered tape signals from SYMREC_q while
///     only SYMREC_p's recursion context was active.
///
/// **Verification**: per-block TBPTT gradient for each seed matches central FD
/// (gradients are independent: perturbing p doesn't change y_q and vice-versa).
#[test]
fn rad_two_independent_poles_bra_total_grad_matches_fd() {
    assert_bra_block_total_grad_matches_fd(
        "rad-two-poles",
        2, // 2 primal outputs [y_p, y_q]
        6,
        &[0.5, 0.25],  // base seeds: p=0.5, q=0.25 (both stable)
        &[5e-4, 5e-4], // epsilons
        5e-2,          // abs_tol
        |s| {
            format!(
                r#"
p = hslider("p", {}, 0.0, 1.0, 0.01);
q = hslider("q", {}, 0.0, 1.0, 0.01);
process = rad(((2 : + ~ *(p)), (3 : + ~ *(q))), (p, q));
"#,
                s[0], s[1]
            )
        },
        |s| {
            format!(
                r#"
p = hslider("p", {}, 0.0, 1.0, 0.01);
q = hslider("q", {}, 0.0, 1.0, 0.01);
process = (2 : + ~ *(p)), (3 : + ~ *(q));
"#,
                s[0], s[1]
            )
        },
    );
}

// -----------------------------------------------------------------------
// TBPTT corpus convergence tests
// -----------------------------------------------------------------------
//
// Each of the `rad_tbptt_*.dsp` corpus files implements a BS=1 online
// gradient-descent loop `loop ~ _` (or `loop ~ (_, _)` etc.) that learns
// one or more parameters from a deterministic LCG noise excitation.
//
// The `process` output is the stereo residual `(y_target - y_ia) <: _, _`
// which starts large and converges toward silence as the learned parameters
// approach their hidden targets.
//
// We validate two things per fixture:
//   1. The pipeline compiles and produces exactly 2 output channels.
//   2. The RMS residual over the last N frames is strictly smaller than
//      over the first N frames (demonstrable convergence).
//
// Failing assertion (1) = pipeline or routing regression.
// Failing assertion (2) = gradient is zero, wrong sign, or BRA not running.

fn rms(samples: &[f32]) -> f32 {
    let n = samples.len() as f64;
    let sum: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    ((sum / n) as f32).sqrt()
}

/// Runs a TBPTT corpus file and checks that:
///   - exactly 2 output channels are produced (stereo residual);
///   - both channels carry the same signal (redundant stereo dup);
///   - the RMS residual in the last `window` frames is smaller than
///     `convergence_factor` × the RMS in the first `window` frames.
fn assert_tbptt_converges(stem: &'static str, frames: usize, window: usize, factor: f32) {
    let outs = run_interp_corpus(stem, frames);
    assert_eq!(
        outs.len(),
        2,
        "{stem}: expected 2 stereo residual channels, got {}",
        outs.len()
    );
    // Both channels must carry the same signal (process = residual <: _, _).
    for (frame, (&left, &right)) in outs[0].iter().zip(&outs[1]).take(frames).enumerate() {
        assert_close(
            left,
            right,
            1.0e-6,
            &format!("{stem}: L/R mismatch at frame {frame}"),
        );
    }
    let rms_start = rms(&outs[0][..window]);
    let rms_end = rms(&outs[0][frames - window..]);
    assert!(
        rms_end < factor * rms_start,
        "{stem}: residual did not converge — rms_start={rms_start:.6}, rms_end={rms_end:.6}, \
         required rms_end < {factor} * rms_start"
    );
}

#[test]
fn corpus_tbptt_gain_converges_to_silence() {
    // Feedforward scalar gain: y_target = g_star * x, g_star = 0.5.
    // lr = 0.05 — convergence in ~50 frames.
    assert_tbptt_converges("rad_tbptt_gain", 400, 20, 0.1);
}

#[test]
fn corpus_tbptt_two_gains_converges_to_silence() {
    // 2-tap FIR, 2-wire SYMREC.  lr = 0.03 — convergence in ~100 frames.
    assert_tbptt_converges("rad_tbptt_two_gains", 600, 30, 0.1);
}

#[test]
fn corpus_tbptt_lms_fir3_converges_to_silence() {
    // 3-tap LMS FIR, 3-wire SYMREC.  lr = 0.02 — convergence in ~200 frames.
    assert_tbptt_converges("rad_tbptt_lms_fir3", 1000, 50, 0.1);
}

#[test]
#[ignore = "abs backward rule produces NaN at u=0 (sign(0)=0/0); fix pending"]
fn corpus_tbptt_softclip_drive_converges_to_silence() {
    // Nonlinear soft-clipper: d_star=3.0, starting from d=0.  lr=0.02.
    // The drive travels 3 units before the residual vanishes — needs ~1000
    // frames for the RMS to drop noticeably.
    assert_tbptt_converges("rad_tbptt_softclip_drive", 2000, 100, 0.5);
}

#[test]
fn bra_reverse_sweep_minmax_conditions_compile() {
    let source = r#"
p_star = 0.75;
lr = 0.005;
noise = +(12345) ~ *(1103515245) : *(4.656612873077393e-10);
soft_clip(v) = max(-1.0, min(1.0, v - (v*v*v)/3.0));
nl_filter(p, input) = input : + ~ (*(p) : soft_clip);
y_target = nl_filter(p_star, noise);
p_learned = loop ~ _
with {
    loop(p) = p_next
    with {
        y_pred = nl_filter(p, noise);
        loss = (y_target - y_pred) * (y_target - y_pred);
        grad = rad(loss, p) : !, _;
        p_next = max(-0.99, min(0.99, p - lr * grad));
    };
};
process = (y_target - nl_filter(p_learned, noise)) <: _, _;
"#;
    let outs = run_interp_temp_source("bra-reverse-sweep-minmax-conditions", source, 8);
    assert_eq!(outs.len(), 2);
}

#[test]
fn bra_reverse_sweep_skips_integer_noise_tape_candidates_compile_to_cpp() {
    std::thread::Builder::new()
        .name("rad-runtime-integer-noise-tape-candidates-cpp".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let source = r#"
clip(v) = max(-1.0, min(1.0, v - (v*v*v)/3.0));
ladder(g1, g2, g3, g4, res, x) = loop ~ (_,_,_,_) : !,!,!,_
with {
    loop(s1, s2, s3, s4) = ns1, ns2, ns3, ns4
    with {
        fb  = clip(x - res * s4);
        ns1 = clip(fb  * g1 + s1 * (1.0 - g1));
        ns2 = clip(ns1 * g2 + s2 * (1.0 - g2));
        ns3 = clip(ns2 * g3 + s3 * (1.0 - g3));
        ns4 = clip(ns3 * g4 + s4 * (1.0 - g4));
    };
};
g1 = hslider("g1", 0.1, 0, 1, 0.01);
g2 = hslider("g2", 0.2, 0, 1, 0.01);
g3 = hslider("g3", 0.3, 0, 1, 0.01);
g4 = hslider("g4", 0.4, 0, 1, 0.01);
res = hslider("res", 0.5, 0, 4, 0.01);
noise = +(12345) ~ *(1103515245) : *(4.656612873077393e-10);
target = noise * 0.5;
pred = ladder(g1, g2, g3, g4, res, noise);
loss = (target - pred) * (target - pred);
process = rad(loss, (g1, g2, g3, g4, res));
"#;
            let unique_id = NEXT_TEMP_DSP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "faust-rs-rad-stress2-integer-noise-{}-{unique_id}.dsp",
                std::process::id()
            ));
            fs::write(&path, source).unwrap_or_else(|e| {
                panic!("failed to write temporary DSP {}: {e}", path.display())
            });
            let cpp = Compiler::new()
                .compile_file_default_to_cpp_with_lane(
                    &path,
                    &codegen::backends::cpp::CppOptions::default(),
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|e| {
                    panic!(
                        "{}: BRA reverse sweep must skip non-real structural tape candidates: {e}",
                        path.display()
                    )
                });
            let _ = fs::remove_file(&path);
            assert!(cpp.contains("class mydsp"));
        })
        .expect("spawn rad-runtime-integer-noise-tape-candidates-cpp worker")
        .join()
        .expect("rad-runtime-integer-noise-tape-candidates-cpp worker should finish")
}

#[test]
fn bra_reverse_sweep_sr_constants_compile_to_cpp() {
    std::thread::Builder::new()
        .name("rad-runtime-sr-constants-cpp".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let source = r#"
SR = 48000.0;
N = 2;
nl_band(g, freq, x) = x * (g * freq / max(1.0, SR)) : max(-1.0) : min(1.0);
massive_bank(g) = _ <: par(i, N, nl_band(g, 100 + i*15)) :> _;
g = hslider("global_gain", 0.5, 0.1, 10, 0.01);
process = rad(massive_bank(g), g);
"#;
            let unique_id = NEXT_TEMP_DSP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "faust-rs-rad-stress3-sr-constants-{}-{unique_id}.dsp",
                std::process::id()
            ));
            fs::write(&path, source).unwrap_or_else(|e| {
                panic!("failed to write temporary DSP {}: {e}", path.display())
            });
            let cpp = Compiler::new()
                .compile_file_default_to_cpp_with_lane(
                    &path,
                    &codegen::backends::cpp::CppOptions::default(),
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|e| {
                    panic!(
                        "{}: BRA reverse sweep with SR-derived constants must compile to C++: {e}",
                        path.display()
                    )
                });
            let _ = fs::remove_file(&path);
            assert!(cpp.contains("class mydsp"));
        })
        .expect("spawn rad-runtime-sr-constants-cpp worker")
        .join()
        .expect("rad-runtime-sr-constants-cpp worker should finish")
}

/// Debug: inspect softclip gradient values to diagnose convergence failure.
#[test]
fn debug_softclip_gradient_values() {
    // Test 1: raw gradient at fixed d=0 (no loop, purely feedforward BRA).
    // Analytical: grad = -2*(y_target)*x at d=0, u=0.
    let source_fixed = r#"
d_star = 3.0;
noise = lcg * 4.656612873077393e-10 with { lcg = +(12345) ~ *(1103515245); };
x = noise;
softclip(d, sig) = u / (1.0 + abs(u)) with { u = d * sig; };
y_target = softclip(d_star, x);
d_fixed = 0.0;
y_pred = softclip(d_fixed, x);
loss   = (y_target - y_pred) * (y_target - y_pred);
// [loss, grad_d] — d_fixed is a constant so grad should be via BRA
grad   = rad(loss, d_fixed);
// output: [loss, grad_d]
process = grad;
"#;
    let outs1 = run_interp_temp_source("softclip-grad-fixed", source_fixed, 5);
    println!("Fixed d=0: outputs.len()={}", outs1.len());
    for (i, ch) in outs1.iter().enumerate() {
        println!("  ch[{i}] first 5: {:?}", &ch[..5.min(ch.len())]);
    }

    // Test 2a: simplest loop with rad — quadratic loss, no abs/div.
    // d starts at 0 always → grad = 2*d = 0 → d_next = 0. Should be stable at 0.
    // This checks if rad inside a loop works WITHOUT the softclip nonlinearity.
    let source_quad = r#"
noise = lcg * 4.656612873077393e-10 with { lcg = +(12345) ~ *(1103515245); };
result = loop ~ _
with {
    loop(d) = d_next
    with {
        x = noise;
        loss = (d - x) * (d - x);
        grad = rad(loss, d) : !, _;
        d_next = d - 0.1 * grad;
    };
};
process = result;
"#;
    let outs_quad = run_interp_temp_source("softclip-quad-loop", source_quad, 5);
    println!("Quad loop d first 5: {:?}", &outs_quad[0][..5]);

    // Test 2b: softclip loop unclipped.
    let source_loop = r#"
d_star = 3.0;
lr     = 0.02;
noise = lcg * 4.656612873077393e-10 with { lcg = +(12345) ~ *(1103515245); };
x = noise;
softclip(d, sig) = u / (1.0 + abs(u)) with { u = d * sig; };
y_target = softclip(d_star, x);
result = loop ~ _
with {
    loop(d) = d_raw
    with {
        y_pred = softclip(d, x);
        loss   = (y_target - y_pred) * (y_target - y_pred);
        grad   = rad(loss, d) : !, _;
        d_raw  = d - lr * grad;
    };
};
process = result;
"#;
    let outs2 = run_interp_temp_source("softclip-grad-loop-raw", source_loop, 5);
    println!("Softclip d_raw first 5: {:?}", &outs2[0][..5]);
}

#[test]
fn corpus_tbptt_one_pole_converges_to_silence() {
    // 1-pole IIR (BRA with Delay1 carry), lr = 0.002.
    // IIR gradient magnitudes can be large; allow more frames.
    assert_tbptt_converges("rad_tbptt_one_pole", 3000, 100, 0.2);
}

#[test]
fn corpus_tbptt_two_poles_converges_to_silence() {
    // 2-pole cascade IIR, 2-wire SYMREC, nested BRA Delay1 carry.
    // Two poles require more iterations to untangle cross-gradients.
    assert_tbptt_converges("rad_tbptt_two_poles", 8000, 200, 0.3);
}

#[test]
fn corpus_tbptt_biquad1_converges_to_silence() {
    // 5-parameter DF-II biquad, 5-wire SYMREC.
    // Feedback coefficients (a1, a2) need small lr — allow many frames.
    assert_tbptt_converges("rad_tbptt_biquad1", 8000, 200, 0.3);
}
