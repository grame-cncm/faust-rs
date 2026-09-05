//! Runtime checks for the DDSP examples of `tests/corpus/ddsp_*.dsp`, the
//! programs documented in `libraries/ddsp-examples-en.md`: three built on
//! `fad` (an adaptive notch, a modal resonator calibrated by Gauss-Newton,
//! an amplifier model learned end to end) and three on `rad` (a 64-tap echo
//! canceller, a small neural network trained in the graph, block gradients
//! of a resonator handed to a host, whose training loop is the last test
//! here).
//!
//! Every fixture but the host one imports `optimizers.lib`; all of them
//! import the Faust standard libraries, found through
//! `FAUST_RS_FAUSTLIBRARIES_ROOT` or the default checkout path. The tests
//! **skip gracefully** when neither exists, as `optimizers_lib.rs` does.

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::bytecode::FbcUiInstruction;
use codegen::backends::interp::opcode::FbcOpcode;
use codegen::backends::interp::{FbcDspFactory, FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";
const SAMPLE_RATE: i32 = 44_100;

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

fn compile_fixture(stem: &str, root: PathBuf) -> FbcDspFactory<f32> {
    let path = workspace_dir("tests/corpus").join(format!("{stem}.dsp"));
    let search_paths = [
        workspace_dir("tests/corpus"),
        workspace_dir("libraries"),
        root,
    ];
    let fbc = Compiler::new()
        .compile_file_to_interp_with_lane(
            &path,
            &search_paths,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{} interp compilation failed: {e}", path.display()));
    let mut reader = Cursor::new(fbc);
    read_fbc::<f32>(&mut reader)
        .unwrap_or_else(|e| panic!("{} interp bytecode parse failed: {e}", path.display()))
}

fn render_inner(stem: &str, frame_count: usize, root: PathBuf) -> Vec<Vec<f32>> {
    let mut factory = compile_fixture(stem, root);
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(SAMPLE_RATE);
    assert_eq!(
        instance.get_num_inputs(),
        0,
        "{stem}: the fixture must not need inputs"
    );
    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative outputs");
    let mut outputs = vec![vec![0.0_f32; frame_count]; num_outputs];
    let mut output_slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frame_count as i32, &[], &mut output_slices)
        .unwrap_or_else(|e| panic!("{stem} interp execution failed: {e}"));
    outputs
}

/// Renders a fixture on a 64 MB-stack worker (the `fad` expansions build
/// deep evaluation trees). `None` when the standard libraries are
/// unavailable: the test skips.
fn render(stem: &'static str, frame_count: usize) -> Option<Vec<Vec<f32>>> {
    let Some(root) = faustlibraries_root() else {
        eprintln!("Skipping {stem}: faustlibraries unavailable");
        return None;
    };
    Some(
        std::thread::Builder::new()
            .name(format!("ddsp-{stem}"))
            .stack_size(64 * 1024 * 1024)
            .spawn(move || render_inner(stem, frame_count, root))
            .expect("spawn ddsp worker")
            .join()
            .expect("ddsp worker thread should finish"),
    )
}

fn rms(samples: &[f32]) -> f64 {
    let n = samples.len() as f64;
    let sum: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / n).sqrt()
}

fn mean(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x)).sum::<f64>() / samples.len() as f64
}

fn assert_finite(stem: &str, outs: &[Vec<f32>]) {
    for (channel, samples) in outs.iter().enumerate() {
        for (frame, &sample) in samples.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "{stem}: non-finite output {channel} at frame {frame}: {sample}"
            );
        }
    }
}

// ───────────────────────────── FAD ─────────────────────────────

#[test]
fn fad_adaptive_notch_locks_on_the_hum() {
    // [residual, learned frequency in Hz]. The hum is at 1000 Hz, the notch
    // starts at 1400 Hz; the normalised step settles within 0.5 Hz and the
    // residual reaches the noise floor (0.02 uniform noise: rms 0.0115).
    let Some(outs) = render("ddsp_fad_adaptive_notch", 40_000) else {
        return;
    };
    assert_eq!(outs.len(), 2);
    assert_finite("ddsp_fad_adaptive_notch", &outs);
    let f_end = mean(&outs[1][36_000..]);
    assert!(
        (f_end - 1000.0).abs() < 0.5,
        "notch frequency should settle at 1000 Hz, got {f_end}"
    );
    let residual = rms(&outs[0][36_000..]);
    assert!(
        residual < 0.02,
        "residual should reach the noise floor, got rms {residual}"
    );
    // The notch starts 400 Hz away but is wide (r = 0.95): the hum is only
    // partly attenuated at first, well above the floor it ends at.
    let start = rms(&outs[0][..500]);
    assert!(
        start > 0.05,
        "the hum should be audible before adaptation, got rms {start}"
    );
}

#[test]
fn fad_modal_resonator_lm_identifies_frequency_and_q() {
    // [f, q, residual]: damped Gauss-Newton from (600, 10) to (800, 25).
    let Some(outs) = render("ddsp_fad_modal_resonator_lm", 40_000) else {
        return;
    };
    assert_eq!(outs.len(), 3);
    assert_finite("ddsp_fad_modal_resonator_lm", &outs);
    let f = mean(&outs[0][36_000..]);
    let q = mean(&outs[1][36_000..]);
    assert!(
        (f - 800.0).abs() < 0.5,
        "mode frequency should be 800 Hz, got {f}"
    );
    assert!((q - 25.0).abs() < 0.1, "mode Q should be 25, got {q}");
    let residual = rms(&outs[2][36_000..]);
    assert!(
        residual < 1e-3,
        "residual should vanish, got rms {residual}"
    );
}

#[test]
fn fad_amp_model_learns_drive_gain_and_tone() {
    // [drive, gain, tone, residual]: Adam per parameter, drive in the log
    // domain, from (1, 1, 0.5) to (4, 0.7, 0.8). Adam keeps a small jitter,
    // so the last 4000 samples are averaged.
    let Some(outs) = render("ddsp_fad_amp_model", 40_000) else {
        return;
    };
    assert_eq!(outs.len(), 4);
    assert_finite("ddsp_fad_amp_model", &outs);
    let drive = mean(&outs[0][36_000..]);
    let gain = mean(&outs[1][36_000..]);
    let tone = mean(&outs[2][36_000..]);
    assert!((drive - 4.0).abs() < 0.08, "drive should be 4, got {drive}");
    assert!((gain - 0.7).abs() < 0.014, "gain should be 0.7, got {gain}");
    assert!((tone - 0.8).abs() < 0.016, "tone should be 0.8, got {tone}");
}

// ───────────────────────────── RAD ─────────────────────────────

#[test]
fn rad_echo_canceller_reaches_30_db_erle() {
    // [residual echo, microphone]: ERLE = 10 log10(P(mic) / P(residual))
    // over the last 4000 samples.
    let Some(outs) = render("ddsp_rad_echo_canceller_64", 24_000) else {
        return;
    };
    assert_eq!(outs.len(), 2);
    assert_finite("ddsp_rad_echo_canceller_64", &outs);
    let residual = rms(&outs[0][20_000..]);
    let mic = rms(&outs[1][20_000..]);
    let erle_db = 20.0 * (mic / residual.max(1e-12)).log10();
    assert!(
        erle_db > 30.0,
        "ERLE should exceed 30 dB, got {erle_db:.1} dB"
    );
}

#[test]
fn rad_mlp_waveshaper_fits_the_soft_clipper() {
    // [residual, target]: the residual falls more than 20 dB below the
    // target over the last 4000 samples.
    let Some(outs) = render("ddsp_rad_mlp_waveshaper", 40_000) else {
        return;
    };
    assert_eq!(outs.len(), 2);
    assert_finite("ddsp_rad_mlp_waveshaper", &outs);
    let residual = rms(&outs[0][36_000..]);
    let target = rms(&outs[1][36_000..]);
    let ratio_db = 20.0 * (residual / target).log10();
    assert!(
        ratio_db < -20.0,
        "residual should be 20 dB under the target, got {ratio_db:.1} dB"
    );
    let early = rms(&outs[0][..2_000]);
    assert!(
        early > 5.0 * residual,
        "the fit should improve over time: early rms {early}, late {residual}"
    );
}

// ───────────────────── RAD, host-driven ─────────────────────

const BLOCK: usize = 256;

/// Deterministic white noise in [-0.5, 0.5], the LCG of the corpus.
struct Lcg(i32);

impl Lcg {
    fn block(&mut self, out: &mut [f32]) {
        for sample in out.iter_mut() {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *sample = self.0 as f32 * 4.656_613e-10;
        }
    }
}

fn slider_offset(ui: &[FbcUiInstruction<f32>], label: &str) -> i32 {
    ui.iter()
        .find(|instr| {
            matches!(instr.opcode, FbcOpcode::AddHorizontalSlider) && instr.label == label
        })
        .unwrap_or_else(|| panic!("slider {label} not found"))
        .offset
}

/// Runs one block from a fresh instance at `(a1, a2)` on the given
/// excitation; returns the block loss and the two block gradients.
fn resonator_block(
    factory: &mut FbcDspFactory<f32>,
    a1: f32,
    a2: f32,
    x: &[f32],
) -> (f64, f64, f64) {
    let mut instance = FbcDspInstance::new(factory);
    instance.init(SAMPLE_RATE);
    let ui = instance.ui_instructions().to_vec();
    instance.set_real_zone(slider_offset(&ui, "a1"), a1);
    instance.set_real_zone(slider_offset(&ui, "a2"), a2);
    let mut lanes = vec![vec![0.0_f32; x.len()]; 3];
    let mut outs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(x.len() as i32, &[x], &mut outs)
        .expect("resonator block");
    let sum = |lane: &[f32]| lane.iter().map(|&v| f64::from(v)).sum::<f64>();
    (sum(&lanes[0]), sum(&lanes[1]), sum(&lanes[2]))
}

#[test]
fn rad_host_block_gradients_identify_the_resonator() {
    let Some(root) = faustlibraries_root() else {
        eprintln!("Skipping ddsp_rad_host_block_resonator: faustlibraries unavailable");
        return;
    };
    std::thread::Builder::new()
        .name("ddsp-host-block-resonator".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let mut factory = compile_fixture("ddsp_rad_host_block_resonator", root);

            // 1. The block gradient is the derivative of the block loss: the
            //    reverse loop carries the adjoint of the resonator's state
            //    backwards over the block. Central finite differences on the
            //    sliders, same excitation, fresh instance each time.
            let mut x = vec![0.0_f32; BLOCK];
            Lcg(1).block(&mut x);
            let (a1, a2) = (-0.8_f32, 0.5_f32);
            let (_, g1, g2) = resonator_block(&mut factory, a1, a2, &x);
            let h = 1e-3_f32;
            let (lp, _, _) = resonator_block(&mut factory, a1 + h, a2, &x);
            let (lm, _, _) = resonator_block(&mut factory, a1 - h, a2, &x);
            let fd1 = (lp - lm) / (2.0 * f64::from(h));
            let (lp, _, _) = resonator_block(&mut factory, a1, a2 + h, &x);
            let (lm, _, _) = resonator_block(&mut factory, a1, a2 - h, &x);
            let fd2 = (lp - lm) / (2.0 * f64::from(h));
            assert!(
                (g1 - fd1).abs() < 2e-2 * fd1.abs().max(1.0),
                "block gradient for a1: rad {g1} vs finite differences {fd1}"
            );
            assert!(
                (g2 - fd2).abs() < 2e-2 * fd2.abs().max(1.0),
                "block gradient for a2: rad {g2} vs finite differences {fd2}"
            );

            // 2. Training: one instance kept running, one Adam step per block
            //    on the summed lanes, the poles kept inside the stability
            //    triangle. Target (-1.2, 0.72): poles at radius 0.85, 45 degrees.
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(SAMPLE_RATE);
            let ui = instance.ui_instructions().to_vec();
            let off = [slider_offset(&ui, "a1"), slider_offset(&ui, "a2")];
            let mut p = [-0.8_f64, 0.5_f64];
            let (mut m, mut v) = ([0.0_f64; 2], [0.0_f64; 2]);
            let (lr, b1, b2, eps) = (0.01_f64, 0.9_f64, 0.999_f64, 1e-8_f64);
            let mut noise = Lcg(7);
            let mut lanes = vec![vec![0.0_f32; BLOCK]; 3];
            let mut first_loss = 0.0_f64;
            let mut last_loss = 0.0_f64;
            for iteration in 1..=600 {
                instance.set_real_zone(off[0], p[0] as f32);
                instance.set_real_zone(off[1], p[1] as f32);
                noise.block(&mut x);
                let mut outs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
                instance
                    .try_compute(BLOCK as i32, &[&x], &mut outs)
                    .expect("training block");
                let sums: Vec<f64> = lanes
                    .iter()
                    .map(|lane| lane.iter().map(|&s| f64::from(s)).sum::<f64>() / BLOCK as f64)
                    .collect();
                if iteration == 1 {
                    first_loss = sums[0];
                }
                last_loss = sums[0];
                for k in 0..2 {
                    let g = sums[k + 1];
                    m[k] = b1 * m[k] + (1.0 - b1) * g;
                    v[k] = b2 * v[k] + (1.0 - b2) * g * g;
                    let m_hat = m[k] / (1.0 - b1.powi(iteration));
                    let v_hat = v[k] / (1.0 - b2.powi(iteration));
                    p[k] -= lr * m_hat / (v_hat.sqrt() + eps);
                }
                // Stability triangle: |a2| < 1, |a1| < 1 + a2.
                p[1] = p[1].clamp(-0.98, 0.98);
                let bound = 1.0 + p[1] - 0.01;
                p[0] = p[0].clamp(-bound, bound);
            }
            eprintln!(
                "host loop: a1 {} a2 {} first loss {first_loss:.4e} last loss {last_loss:.4e} \
                 gradient a1 {g1:.3} (fd {fd1:.3}) a2 {g2:.3} (fd {fd2:.3})",
                p[0], p[1]
            );
            assert!(
                (p[0] + 1.2).abs() < 0.02 && (p[1] - 0.72).abs() < 0.02,
                "host training should recover (-1.2, 0.72), got ({}, {})",
                p[0],
                p[1]
            );
            assert!(
                last_loss < 1e-3 * first_loss,
                "block loss should fall by 30 dB: first {first_loss}, last {last_loss}"
            );
        })
        .expect("spawn ddsp host worker")
        .join()
        .expect("ddsp host worker should finish");
}
