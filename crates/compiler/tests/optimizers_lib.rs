//! Runtime checks for the project-local `libraries/optimizers.lib`.
//!
//! The library packages in-graph optimization on top of `fad` and `rad`:
//! update engines, losses, reparameterizations and ready-made loops. It
//! imports `signals.lib`, `basics.lib`, `routes.lib` and `maths.lib`, so its
//! fixtures compile with `tests/corpus`, `libraries` and the Faust standard
//! libraries on the import path. The standard libraries are found through
//! `FAUST_RS_FAUSTLIBRARIES_ROOT` or the default checkout path, and every
//! test **skips gracefully** when neither exists, as `interleave_fft.rs`
//! does.
//!
//! Each fixture in `tests/corpus/opt_*.dsp` learns a hidden parameter set
//! sample by sample and outputs the residual (or the parameter error) as a
//! stereo pair; the checks below require that residual to fall, which is
//! what a working optimizer looks like from the outside. One more fixture,
//! `opt_all_functions.dsp`, instantiates the `#### Test` entry of every
//! documented function, so the documentation examples are compiled too.
//!
//! The tests use the interpreter fast lane through the public compiler
//! facade, so they exercise propagation (including the `fad` and `rad`
//! expansions), transform, FIR lowering and the interp backend together.

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

fn workspace_dir(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(rel)
}

fn faustlibraries_root() -> Option<PathBuf> {
    std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(DEFAULT_FAUSTLIBRARIES_ROOT);
            default.exists().then_some(default)
        })
}

fn run_interp_fixture_inner(stem: &str, frame_count: usize, root: PathBuf) -> Vec<Vec<f32>> {
    let path = workspace_dir("tests/corpus").join(format!("{stem}.dsp"));
    // `libraries` is where `optimizers.lib` lives, `root` holds the standard
    // libraries it imports; the corpus directory is added for symmetry with
    // the other corpus runners.
    let search_paths = [
        workspace_dir("tests/corpus"),
        workspace_dir("libraries"),
        root,
    ];
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
/// overflow the default 2 MB test-thread stack. `None` when the standard
/// libraries are unavailable (the test skips).
fn run_interp_fixture(stem: &'static str, frame_count: usize) -> Option<Vec<Vec<f32>>> {
    let Some(root) = faustlibraries_root() else {
        eprintln!("Skipping {stem}: faustlibraries unavailable");
        return None;
    };
    Some(
        std::thread::Builder::new()
            .name(format!("optimizers-lib-{stem}"))
            .stack_size(64 * 1024 * 1024)
            .spawn(move || run_interp_fixture_inner(stem, frame_count, root))
            .expect("spawn optimizers-lib worker")
            .join()
            .expect("optimizers-lib worker thread should finish"),
    )
}

fn rms(samples: &[f32]) -> f32 {
    let n = samples.len() as f64;
    let sum: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    ((sum / n) as f32).sqrt()
}

/// Checks that every sample of `channel` is finite and that its RMS over the
/// last `window` frames is below `factor` times its RMS over the first
/// `window` frames.
fn assert_channel_converges(stem: &str, channel: &[f32], window: usize, factor: f32) {
    for (frame, &sample) in channel.iter().enumerate() {
        assert!(
            sample.is_finite(),
            "{stem}: non-finite sample at frame {frame}: {sample}"
        );
    }
    let frames = channel.len();
    let rms_start = rms(&channel[..window]);
    let rms_end = rms(&channel[frames - window..]);
    assert!(
        rms_end < factor * rms_start,
        "{stem}: residual did not converge — rms_start={rms_start:.6}, rms_end={rms_end:.6}, \
         required rms_end < {factor} * rms_start"
    );
}

/// Runs a fixture and checks that:
///   - exactly 2 output channels are produced (stereo residual);
///   - both channels carry the same signal (`process = residual <: _, _`);
///   - every sample is finite;
///   - the RMS over the last `window` frames is below `factor` times the RMS
///     over the first `window` frames.
fn assert_converges(stem: &'static str, frames: usize, window: usize, factor: f32) {
    let Some(outs) = run_interp_fixture(stem, frames) else {
        return;
    };
    assert_eq!(
        outs.len(),
        2,
        "{stem}: expected 2 stereo residual channels, got {}",
        outs.len()
    );
    for (frame, (&left, &right)) in outs[0].iter().zip(&outs[1]).enumerate() {
        assert_eq!(left, right, "{stem}: L/R mismatch at frame {frame}");
    }
    assert_channel_converges(stem, &outs[0], window, factor);
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
fn descend_1d_inside_an_ondemand_block_learns_a_gain() {
    // The whole optimizer inside an `ondemand` block fired every 64 samples
    // (tutorial, section 11.1): the loop's state must not depend on a
    // first-sample gate captured across the clock boundary.
    assert_converges("opt_descend_in_ondemand_gain", 20_000, 500, 0.01);
}

#[test]
fn lsq_n_rad_with_nlms_learns_eight_fir_taps_from_one_reverse_sweep() {
    // Bus least-squares loop, eight taps, NLMS at level 10: the eight
    // sensitivities come from one reverse sweep per sample.
    assert_converges("opt_lsq_n_rad_nlms_fir8", 4000, 200, 0.01);
}

#[test]
fn descend_n_rad_learns_sixteen_fir_taps() {
    // Bus loss-first loop with `rad`, sixteen taps, LMS step 0.02 on white
    // noise: one reverse sweep per sample instead of sixteen tangents.
    assert_converges("opt_descend_n_rad_fir16", 8000, 400, 0.05);
}

#[test]
fn bus_loops_fad_and_rad_follow_the_same_trajectory_on_an_fir() {
    // The fixture outputs the residual of `descend_N` and of `descend_N_rad`
    // on the same sixteen-tap FIR: both converge, and since the loss has no
    // recursion between the taps and the output, both gradients are the same
    // and the residuals agree to rounding.
    let Some(outs) = run_interp_fixture("opt_bus_fad_vs_rad_fir16", 8000) else {
        return;
    };
    assert_eq!(outs.len(), 2, "expected the two residuals");
    assert_channel_converges("opt_bus_fad_vs_rad_fir16 (fad)", &outs[0], 400, 0.05);
    assert_channel_converges("opt_bus_fad_vs_rad_fir16 (rad)", &outs[1], 400, 0.05);
    let max_gap = outs[0]
        .iter()
        .zip(&outs[1])
        .map(|(&a, &b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_gap < 1e-4,
        "fad and rad bus loops diverged: max residual gap {max_gap}"
    );
}

#[test]
fn newton_solves_the_cubic_on_every_frame() {
    // Six unrolled Newton steps on y^3 + y = x, x in [-1, 1]: the residual is
    // at numerical precision from the first frame on.
    let Some(outs) = run_interp_fixture("opt_newton_cubic", 256) else {
        return;
    };
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
    // documented function: 74 entries, 131 outputs. It only has to compile,
    // run, and stay finite.
    let Some(outs) = run_interp_fixture("opt_all_functions", 256) else {
        return;
    };
    assert_eq!(outs.len(), 131, "expected the outputs of every Test entry");
    for (channel, samples) in outs.iter().enumerate() {
        for (frame, &sample) in samples.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "opt_all_functions: non-finite output {channel} at frame {frame}: {sample}"
            );
        }
    }
}
