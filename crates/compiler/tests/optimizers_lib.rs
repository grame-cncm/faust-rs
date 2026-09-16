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
fn small_steps_on_a_large_parameter_are_kept_in_single_precision() {
    // `init` = 1000, target 1000.001, SGD steps below half an ulp of the value
    // (6e-5 in f32): applied to the deviation from `init` they accumulate and
    // the residual falls; applied to the value they would all be lost.
    assert_converges("opt_descend_small_steps_large_init", 3000, 100, 0.1);
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
fn gated_block_matches_the_plain_one_then_holds() {
    // `gated(learn)`: the gain of the gated block is bit-identical to the
    // plain block's until `stop_after(clock, 5)` raises the flag on the
    // fifth firing (frame 319), then nothing of the block runs and the gain
    // holds; `on_change` recomputes exp(g) on the samples where g changes
    // and holds it in between.
    let Some(outs) = run_interp_fixture("opt_gated_stop_after", 1200) else {
        return;
    };
    assert_eq!(outs.len(), 4, "expected [g_plain, g_gated, done, coef]");
    let (plain, gated, done, coef) = (&outs[0], &outs[1], &outs[2], &outs[3]);
    let stop = done.iter().position(|&d| d > 0.5).expect("the flag rises");
    assert_eq!(stop, 319, "the flag rises on the fifth firing");
    for n in 0..=stop {
        assert_eq!(
            plain[n], gated[n],
            "gated and plain differ before the stop, at frame {n}"
        );
    }
    for n in stop..plain.len() {
        assert_eq!(
            gated[n], gated[stop],
            "the gated gain moved after the stop, at frame {n}"
        );
        assert!(done[n] > 0.5, "the flag fell at frame {n}");
    }
    assert!(
        plain[plain.len() - 1] > gated[stop] + 0.05,
        "the plain block should keep learning"
    );
    for n in 0..coef.len() {
        let expected = gated[n].exp();
        assert!(
            (coef[n] - expected).abs() < 1e-5,
            "on_change is not exp(g) at frame {n}: {} vs {expected}",
            coef[n]
        );
    }
}

#[test]
fn stop_relative_gates_a_converging_loop() {
    // `stop_relative(clock, 2, 4, 40, 0.05)`: the period loss of the gain
    // learner flattens within a few periods; the flag rises on a period
    // boundary after at least four periods and before the cap, and the
    // gated gain holds from there.
    let Some(outs) = run_interp_fixture("opt_gated_stop_relative", 3000) else {
        return;
    };
    assert_eq!(outs.len(), 3, "expected [g_plain, g_gated, done]");
    let (plain, gated, done) = (&outs[0], &outs[1], &outs[2]);
    let stop = done.iter().position(|&d| d > 0.5).expect("the flag rises");
    assert_eq!(
        (stop + 1) % 64,
        0,
        "the flag must rise on a period's last sample, not at {stop}"
    );
    assert!(
        (4 * 64 - 1..40 * 64 - 1).contains(&stop),
        "the flag rose at frame {stop}"
    );
    assert_eq!(stop, 767, "the flag rises at the twelfth period");
    for n in 0..=stop {
        assert_eq!(
            plain[n], gated[n],
            "gated and plain differ before the stop, at frame {n}"
        );
    }
    for n in stop..gated.len() {
        assert_eq!(
            gated[n], gated[stop],
            "the gated gain moved after the stop, at frame {n}"
        );
    }
    assert!(
        (gated[stop] - 0.7).abs() < 0.1,
        "the gain should be near 0.7 when learning stops, got {}",
        gated[stop]
    );
}

#[test]
fn ramp_exp_is_lr_exp_and_ramp_lin_reaches_its_end() {
    // [lr_exp, ramp_exp, ramp_lin], all (0.01 -> 0.0001, T = 4800): the
    // learning-rate schedule is the ramp under another name, bit-identical;
    // the linear ramp is at its midpoint at T/2 and at its end from T on.
    let Some(outs) = run_interp_fixture("opt_ramp_alias", 20_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    for (frame, (&a, &b)) in outs[0].iter().zip(&outs[1]).enumerate() {
        assert!(
            a.is_finite(),
            "opt_ramp_alias: non-finite lr_exp at frame {frame}"
        );
        assert_eq!(
            a, b,
            "opt_ramp_alias: lr_exp and ramp_exp differ at frame {frame}"
        );
    }
    let lin = &outs[2];
    assert_eq!(lin[0], 0.01);
    assert!((lin[2400] - 0.00505).abs() < 1e-6, "midpoint {}", lin[2400]);
    assert!((lin[4800] - 0.0001).abs() < 1e-7, "end {}", lin[4800]);
    assert!((lin[19_999] - 0.0001).abs() < 1e-7, "held {}", lin[19_999]);
}

#[test]
fn stalled_flags_a_plateau_and_neither_a_descent_nor_a_convergence() {
    // [stalled, g, l] over three 20 000-sample segments: descending
    // (0.5, 1.0) -> 0, stuck (0.0, 1.0) -> 1, converged (0.0, 0.001) -> 0.
    // The averages (a = 0.999) settle within 5 000 samples of a segment
    // start, so each segment is read on its last 10 000 samples.
    let Some(outs) = run_interp_fixture("opt_stalled_lanes", 60_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    let flag = &outs[0];
    let mean = |range: std::ops::Range<usize>| {
        flag[range.clone()].iter().sum::<f32>() / range.len() as f32
    };
    assert_eq!(
        mean(10_000..20_000),
        0.0,
        "descending segment read as stalled"
    );
    assert_eq!(
        mean(30_000..40_000),
        1.0,
        "stuck segment not read as stalled"
    );
    assert_eq!(
        mean(50_000..60_000),
        0.0,
        "converged segment read as stalled"
    );
}

#[test]
fn init_latch_starts_the_string_from_its_own_pitch_estimate() {
    // [pitch in Hz, residual, init in Hz]: the loop is held at `init` while
    // an autocorrelation estimate of the target's pitch is observed, `init`
    // is frozen at sample 8 192 (2 % above the target, inside the capture
    // zone of the +-1 Hz well) and the pitch then locks on 220 Hz.
    let Some(outs) = run_interp_fixture("opt_init_latch_string", 80_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    for (channel, samples) in outs.iter().enumerate() {
        for (frame, &sample) in samples.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "opt_init_latch_string: non-finite output {channel} at frame {frame}"
            );
        }
    }
    // While the estimate is observed (up to and including sample 8 192)
    // `init_reset` holds the deviation at zero, so the pitch lane follows
    // the moving init lane to within one engine step (the loops apply the
    // step to a zeroed deviation and report that): a mean gap of 0.04 Hz
    // measured, against 2.8 Hz when the loop is not held.
    let gap = (0..=8_192)
        .map(|frame| (outs[0][frame] - outs[2][frame]).abs())
        .sum::<f32>()
        / 8_193.0;
    assert!(
        gap < 0.5,
        "the loop should be held at init during the observation, mean gap {gap} Hz"
    );
    let init = &outs[2];
    let frozen = init[8_193];
    assert!(
        (220.0..230.0).contains(&frozen),
        "the frozen init should sit just above the target, got {frozen} Hz"
    );
    assert!(
        init[8_193..].iter().all(|&v| v == frozen),
        "init should not move once frozen"
    );
    let pitch = outs[0][76_000..].iter().sum::<f32>() / 4_000.0;
    let residual = rms(&outs[1][76_000..]);
    eprintln!("init_latch string: init {frozen} pitch {pitch} residual {residual:.3e}");
    assert!(
        (pitch - 220.0).abs() < 0.05,
        "pitch should lock on 220 Hz, got {pitch}"
    );
    assert!(
        residual < 1e-3,
        "residual should vanish, got rms {residual}"
    );
}

#[test]
fn langevin_leaves_the_shallow_well_where_sgd_stays() {
    // [sgd_g, langevin_g with temp annealed 0.5 -> 0, langevin_g at temp 0]
    // on (p^2 - 1)^2 + 0.3 p from p = 1: SGD settles in the shallow well at
    // 0.960, Langevin crosses the barrier while hot and cools into the deep
    // well at -1.036; at temperature zero the Langevin step is the SGD step
    // bit for bit.
    let Some(outs) = run_interp_fixture("opt_langevin_two_wells", 200_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    for (frame, (&a, &b)) in outs[0].iter().zip(&outs[2]).enumerate() {
        assert!(
            a.is_finite(),
            "opt_langevin_two_wells: non-finite sgd at frame {frame}"
        );
        assert_eq!(a, b, "langevin at temp 0 differs from sgd at frame {frame}");
    }
    let mean = |lane: &[f32]| lane[180_000..].iter().sum::<f32>() / 20_000.0;
    let sgd = mean(&outs[0]);
    let langevin = mean(&outs[1]);
    eprintln!("two wells: sgd {sgd} langevin {langevin}");
    assert!(
        (sgd - 0.960).abs() < 0.005,
        "sgd should settle in the shallow well, got {sgd}"
    );
    assert!(
        (langevin + 1.036).abs() < 0.03,
        "langevin should cool into the deep well, got {langevin}"
    );
}

#[test]
fn spsa_follows_the_fad_loop_on_a_quadratic_loss() {
    // [gain by spsa_1D_clocked, gain by descend_1D_clocked, difference]:
    // the symmetric difference of a quadratic loss is its exact derivative,
    // so both loops, same engine and clock, reach 0.7 on the same path.
    let Some(outs) = run_interp_fixture("opt_spsa_vs_fad_gain", 6_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    let spsa = outs[0][5_999];
    let fad = outs[1][5_999];
    let max_gap = outs[2].iter().fold(0.0_f32, |m, &d| m.max(d.abs()));
    eprintln!("spsa vs fad: {spsa} {fad} max gap {max_gap:.3e}");
    assert!(
        (spsa - 0.7).abs() < 1e-4,
        "spsa should learn the gain, got {spsa}"
    );
    assert!(
        (fad - 0.7).abs() < 1e-4,
        "descend_1D_clocked should learn the gain, got {fad}"
    );
    assert!(
        max_gap < 1e-4,
        "the two trajectories should coincide, max gap {max_gap}"
    );
}

#[test]
fn spsa_learns_an_integer_delay_that_fad_cannot_see() {
    // [d, int(d), fad tangent, residual]: the comb's delay is an integer, its
    // tangent identically zero; SPSA with c = 2 and Adam 0.5 per 256-sample
    // frame brings d from 160 to the hidden 200 and holds it.
    let Some(outs) = run_interp_fixture("opt_spsa_int_delay", 60_000) else {
        return;
    };
    assert_eq!(outs.len(), 4);
    assert!(
        outs[2].iter().all(|&t| t == 0.0),
        "fad through int(d) should be zero"
    );
    let held = outs[1][50_000..].iter().all(|&i| i == 200.0);
    let residual = rms(&outs[3][50_000..]);
    eprintln!("int delay: d {} residual {residual:.3e}", outs[0][59_999]);
    assert!(held, "int(d) should hold 200 over the last 10 000 samples");
    assert!(
        residual < 1e-6,
        "the comb should match once the delay is right, got {residual}"
    );
}

#[test]
fn search_finds_the_select2_branch_that_descend_never_reaches() {
    // [p by search_1D_clocked, p by descend_1D, frame loss]: the loss is a
    // step in p (a comparison), so the fad loop stays at 0 while the (1+1)
    // strategy accepts a candidate above 0.5 and the loss falls to zero.
    let Some(outs) = run_interp_fixture("opt_search_select2", 6_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    assert!(
        outs[1].iter().all(|&p| p == 0.0),
        "descend_1D should never move p"
    );
    let p = outs[0][5_999];
    let loss = outs[2][5_999];
    eprintln!("search: p {p} loss {loss:.3e}");
    assert!(p > 0.5, "the search should settle above 0.5, got {p}");
    assert_eq!(
        loss, 0.0,
        "the frame loss should be zero on the right branch"
    );
}

#[test]
fn no_progress_flags_a_sloped_plateau_and_neither_a_descent_nor_a_finish() {
    // [no_progress, l] over three 20 000-sample segments: a decaying loss
    // -> 0, a constant high loss -> 1, a constant loss under eps_l -> 0
    // (read on the last 10 000 samples of each, past the warm-up of 2 W).
    let Some(outs) = run_interp_fixture("opt_no_progress_lanes", 60_000) else {
        return;
    };
    assert_eq!(outs.len(), 2);
    let flag = &outs[0];
    let mean = |range: std::ops::Range<usize>| {
        flag[range.clone()].iter().sum::<f32>() / range.len() as f32
    };
    assert_eq!(
        mean(10_000..20_000),
        0.0,
        "a decreasing loss read as no progress"
    );
    assert_eq!(
        mean(30_000..40_000),
        1.0,
        "a stuck high loss not read as no progress"
    );
    assert_eq!(
        mean(50_000..60_000),
        0.0,
        "a finished loss read as no progress"
    );
}

#[test]
fn lsq_restart_takes_the_second_start_out_of_a_local_minimum() {
    // [p, start index, residual]: NLMS on x * L(p) against -0.2 x settles in
    // L's shallow well at 0.96 (residual power above eps_l), the loop
    // restarts after 2 W = 8 000 samples from p = -1 and reaches a root of
    // L(p) = -0.2 in the deep well, where the residual vanishes.
    let Some(outs) = run_interp_fixture("opt_lsq_restart_two_wells", 30_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    let k = &outs[1];
    assert!(
        k[..7_000].iter().all(|&v| v == 0.0),
        "the first start should last 2 W"
    );
    assert!(
        k[12_000..].iter().all(|&v| v == 1.0),
        "the loop should stay on its second start"
    );
    let p = outs[0][29_999];
    let residual = rms(&outs[2][25_000..]);
    eprintln!("lsq restart two wells: p {p} residual {residual:.3e}");
    assert!(
        p < -0.5,
        "the second start should stay in the deep well, got {p}"
    );
    assert!(
        residual < 1e-3,
        "the residual should vanish, got rms {residual}"
    );
}

#[test]
fn restart_leaves_the_shallow_well_for_the_second_start() {
    // [p, start index] on the two-well loss with descend_1D_restart: SGD
    // settles in the shallow well (loss 0.29, above eps_l), the loop restarts
    // after 2 W = 8 000 samples from p = -1 and stays in the deep well.
    let Some(outs) = run_interp_fixture("opt_restart_two_wells", 30_000) else {
        return;
    };
    assert_eq!(outs.len(), 2);
    let k = &outs[1];
    assert!(
        k[..7_000].iter().all(|&v| v == 0.0),
        "the first start should last 2 W"
    );
    assert!(
        k[12_000..].iter().all(|&v| v == 1.0),
        "the loop should stay on its second start"
    );
    let p = outs[0][29_999];
    eprintln!("restart two wells: p {p}");
    assert!(
        (p + 1.036).abs() < 0.01,
        "the second start should settle in the deep well, got {p}"
    );
}

#[test]
fn every_documented_function_compiles_and_runs() {
    // `opt_all_functions.dsp` instantiates the `#### Test` entry of every
    // documented function: 92 entries, 155 outputs. It only has to compile,
    // run, and stay finite.
    let Some(outs) = run_interp_fixture("opt_all_functions", 256) else {
        return;
    };
    assert_eq!(outs.len(), 155, "expected the outputs of every Test entry");
    for (channel, samples) in outs.iter().enumerate() {
        for (frame, &sample) in samples.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "opt_all_functions: non-finite output {channel} at frame {frame}: {sample}"
            );
        }
    }
}
