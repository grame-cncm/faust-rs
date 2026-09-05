//! Runtime checks for the project-local `libraries/optimizers.lib`.
//!
//! The library packages in-graph optimization on top of `fad`: update
//! engines, losses, reparameterizations and ready-made loops. It imports no
//! Faust standard library, so its fixtures compile with `tests/corpus` and
//! `libraries` on the import path and nothing else — the same hermetic
//! contract as the rest of the corpus.
//!
//! Each fixture in `tests/corpus/opt_*.dsp` learns a hidden parameter set
//! sample by sample and outputs the residual (or the parameter error) as a
//! stereo pair; the checks below require that residual to fall, which is
//! what a working optimizer looks like from the outside. One more fixture,
//! `opt_all_functions.dsp`, instantiates the `#### Test` entry of every
//! documented function, so the documentation examples are compiled too.
//!
//! The tests use the interpreter fast lane through the public compiler
//! facade, so they exercise propagation (including the `fad` expansion),
//! transform, FIR lowering and the interp backend together.

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

fn workspace_dir(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(rel)
}

fn run_interp_fixture_inner(stem: &str, frame_count: usize) -> Vec<Vec<f32>> {
    let path = workspace_dir("tests/corpus").join(format!("{stem}.dsp"));
    // `libraries` is where `optimizers.lib` lives; the corpus directory is
    // added for symmetry with the other corpus runners. No standard library
    // path: the fixtures must not need one.
    let search_paths = [workspace_dir("tests/corpus"), workspace_dir("libraries")];
    let compiler = Compiler::new();
    let fbc = compiler
        .compile_file_to_interp_with_lane(
            &path,
            &search_paths,
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

/// Same 64 MB-stack worker pattern as `rad_runtime.rs`: the `fad` expansion
/// of a five-parameter recursive model produces deep evaluation trees that
/// overflow the default 2 MB test-thread stack.
fn run_interp_fixture(stem: &'static str, frame_count: usize) -> Vec<Vec<f32>> {
    std::thread::Builder::new()
        .name(format!("optimizers-lib-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_interp_fixture_inner(stem, frame_count))
        .expect("spawn optimizers-lib worker")
        .join()
        .expect("optimizers-lib worker thread should finish")
}

fn rms(samples: &[f32]) -> f32 {
    let n = samples.len() as f64;
    let sum: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    ((sum / n) as f32).sqrt()
}

/// Runs a fixture and checks that:
///   - exactly 2 output channels are produced (stereo residual);
///   - both channels carry the same signal (`process = residual <: _, _`);
///   - every sample is finite;
///   - the RMS over the last `window` frames is below `factor` times the RMS
///     over the first `window` frames.
fn assert_converges(stem: &'static str, frames: usize, window: usize, factor: f32) {
    let outs = run_interp_fixture(stem, frames);
    assert_eq!(
        outs.len(),
        2,
        "{stem}: expected 2 stereo residual channels, got {}",
        outs.len()
    );
    for (frame, (&left, &right)) in outs[0].iter().zip(&outs[1]).enumerate() {
        assert!(
            left.is_finite() && right.is_finite(),
            "{stem}: non-finite sample at frame {frame}: {left} / {right}"
        );
        assert_eq!(left, right, "{stem}: L/R mismatch at frame {frame}");
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
fn descend_1d_with_adam_learns_a_gain() {
    // Loss-first loop + bias-corrected Adam, lr = 0.002: the gain travels 0.7
    // and the residual is at the f32 floor after ~2000 frames.
    assert_converges("opt_descend_adam_gain", 3000, 100, 0.01);
}

#[test]
fn lsq_3d_with_nlms_learns_fir_taps_at_ten_times_the_level() {
    // Normalized LMS on an excitation of level 10: a plain LMS step tuned
    // for level 1 diverges here; NLMS converges in a few hundred frames.
    assert_converges("opt_lsq_nlms_fir3", 2000, 100, 0.01);
}

#[test]
fn lm_2d_identifies_a_pole_and_a_gain() {
    // Damped recursive Gauss-Newton with one gain for two parameters of
    // different units; converged well within 2000 frames.
    assert_converges("opt_lm_2d_pole_gain", 8000, 200, 0.01);
}

#[test]
fn descend_5d_with_lion_learns_a_biquad_through_reflection_coefficients() {
    // Five coefficients, one Lion learning rate on an exponential schedule,
    // poles kept in the stability triangle by construction. The residual
    // drops from ~0.1 RMS to ~1e-3 over 60000 frames.
    assert_converges("opt_descend_lion_biquad_reflection", 60_000, 2000, 0.1);
}

#[test]
fn logcosh_loss_keeps_the_gain_close_under_outliers() {
    // The fixture outputs `g - g_star` with +/-20 spikes every 97 samples in
    // the target: logcosh keeps |g - g_star| around 1e-2, where mse would keep
    // a jitter of ~0.2.
    assert_converges("opt_descend_logcosh_outliers", 20_000, 200, 0.1);
}

#[test]
fn descend_1d_clocked_learns_a_gain_once_per_frame() {
    // Gradient at audio rate, frame-mean over 64 samples, one SGD step per
    // firing of the ondemand block: exact within a few frames.
    assert_converges("opt_descend_clocked_gain", 4000, 200, 0.01);
}

#[test]
fn newton_solves_the_cubic_on_every_frame() {
    // Six unrolled Newton steps on y^3 + y = x, x in [-1, 1]: the residual is
    // at numerical precision from the first frame on.
    let outs = run_interp_fixture("opt_newton_cubic", 256);
    assert_eq!(outs.len(), 2);
    for (frame, (&left, &right)) in outs[0].iter().zip(&outs[1]).enumerate() {
        assert!(
            left.abs() < 1.0e-5 && right.abs() < 1.0e-5,
            "opt_newton_cubic: residual {left} / {right} at frame {frame}"
        );
    }
}

#[test]
fn every_documented_function_compiles_and_runs() {
    // `opt_all_functions.dsp` instantiates the `#### Test` entry of every
    // documented function: 68 entries, 113 outputs. It only has to compile,
    // run, and stay finite.
    let outs = run_interp_fixture("opt_all_functions", 256);
    assert_eq!(outs.len(), 113, "expected the outputs of every Test entry");
    for (channel, samples) in outs.iter().enumerate() {
        for (frame, &sample) in samples.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "opt_all_functions: non-finite output {channel} at frame {frame}: {sample}"
            );
        }
    }
}
