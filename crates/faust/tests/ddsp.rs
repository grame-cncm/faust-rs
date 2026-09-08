//! The twelve DDSP examples of `tests/corpus/ddsp_*.dsp`, run through the
//! facade on both backends, with the assertions of
//! `crates/compiler/tests/ddsp_examples.rs` (which runs them on the
//! interpreter's Rust types): the same programs must converge to the same
//! values on the interpreter and on the Cranelift JIT, in `f32` and, for the
//! JIT, in `f64`. The two host-driven examples run their Adam loop the way a
//! Rust host would with this API: `set` on the sliders, `compute_f32` for the
//! loss and gradient lanes, summed per block.
//!
//! The programs import the Faust standard libraries, found through
//! `FAUST_RS_FAUSTLIBRARIES_ROOT` or the default checkout path; the tests
//! skip when neither exists.

use std::path::{Path, PathBuf};

use faust::{Backend, CompileOptions, Dsp, Factory, Precision};

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";
const SAMPLE_RATE: i32 = 44_100;

/// The configurations every example runs under.
const CONFIGS: [(Backend, Precision); 3] = [
    (Backend::Interp, Precision::F32),
    (Backend::Cranelift, Precision::F32),
    (Backend::Cranelift, Precision::F64),
];

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

fn label(cfg: (Backend, Precision)) -> String {
    format!("{}/{:?}", cfg.0, cfg.1)
}

fn compile(stem: &str, cfg: (Backend, Precision), root: &Path) -> Factory {
    let options = CompileOptions {
        backend: cfg.0,
        precision: cfg.1,
        import_dirs: vec![
            workspace_dir("tests/corpus"),
            workspace_dir("libraries"),
            root.to_path_buf(),
        ],
        ..CompileOptions::default()
    };
    Factory::from_file(
        workspace_dir("tests/corpus").join(format!("{stem}.dsp")),
        &options,
    )
    .unwrap_or_else(|e| panic!("{stem} [{}]: {e}", label(cfg)))
}

/// Runs `body` for every configuration on a 64 MiB worker (the pipeline
/// after eval still recurses on the native stack for the deep `fad`
/// expansions), or skips when the standard libraries are unavailable.
fn for_each_config(
    stem: &'static str,
    body: impl Fn((Backend, Precision), &Path) + Send + Sync + 'static,
) {
    let Some(root) = faustlibraries_root() else {
        eprintln!("Skipping {stem}: faustlibraries unavailable");
        return;
    };
    let body = std::sync::Arc::new(body);
    for cfg in CONFIGS {
        let body = std::sync::Arc::clone(&body);
        let root = root.clone();
        std::thread::Builder::new()
            .name(format!("{stem}-{}", label(cfg)))
            .stack_size(64 * 1024 * 1024)
            .spawn(move || body(cfg, &root))
            .expect("spawn worker")
            .join()
            .unwrap_or_else(|_| panic!("{stem} [{}] panicked", label(cfg)));
    }
}

/// Renders an input-less example: every output over `frames` frames.
fn render(stem: &str, cfg: (Backend, Precision), root: &Path, frames: usize) -> Vec<Vec<f32>> {
    let factory = compile(stem, cfg, root);
    let mut dsp = factory
        .instantiate(SAMPLE_RATE)
        .unwrap_or_else(|e| panic!("{stem} [{}]: {e}", label(cfg)));
    assert_eq!(
        dsp.num_inputs(),
        0,
        "{stem}: the example must not need inputs"
    );
    let mut outputs = vec![vec![0.0_f32; frames]; dsp.num_outputs()];
    let mut slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    dsp.compute_f32(&[], &mut slices)
        .unwrap_or_else(|e| panic!("{stem} [{}]: {e}", label(cfg)));
    for (channel, samples) in outputs.iter().enumerate() {
        if let Some(frame) = samples.iter().position(|s| !s.is_finite()) {
            panic!(
                "{stem} [{}]: non-finite output {channel} at frame {frame}",
                label(cfg)
            );
        }
    }
    outputs
}

fn rms(samples: &[f32]) -> f64 {
    let n = samples.len() as f64;
    (samples
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / n)
        .sqrt()
}

fn mean(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x)).sum::<f64>() / samples.len() as f64
}

// ───────────────────────────── FAD ─────────────────────────────

#[test]
fn fad_adaptive_notch_locks_on_the_hum() {
    for_each_config("ddsp_fad_adaptive_notch", |cfg, root| {
        let outs = render("ddsp_fad_adaptive_notch", cfg, root, 40_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 2);
        let f_end = mean(&outs[1][36_000..]);
        assert!(
            (f_end - 1000.0).abs() < 0.5,
            "[{l}] notch frequency should settle at 1000 Hz, got {f_end}"
        );
        let residual = rms(&outs[0][36_000..]);
        assert!(
            residual < 0.02,
            "[{l}] residual should reach the noise floor, got rms {residual}"
        );
        let start = rms(&outs[0][..500]);
        assert!(
            start > 0.05,
            "[{l}] the hum should be audible before adaptation, got rms {start}"
        );
    });
}

#[test]
fn fad_modal_resonator_lm_identifies_frequency_and_q() {
    for_each_config("ddsp_fad_modal_resonator_lm", |cfg, root| {
        let outs = render("ddsp_fad_modal_resonator_lm", cfg, root, 40_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 3);
        let f = mean(&outs[0][36_000..]);
        let q = mean(&outs[1][36_000..]);
        assert!(
            (f - 800.0).abs() < 0.5,
            "[{l}] mode frequency should be 800 Hz, got {f}"
        );
        assert!((q - 25.0).abs() < 0.1, "[{l}] mode Q should be 25, got {q}");
        let residual = rms(&outs[2][36_000..]);
        assert!(
            residual < 1e-3,
            "[{l}] residual should vanish, got rms {residual}"
        );
    });
}

#[test]
fn fad_amp_model_learns_drive_gain_and_tone() {
    for_each_config("ddsp_fad_amp_model", |cfg, root| {
        let outs = render("ddsp_fad_amp_model", cfg, root, 40_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 4);
        let drive = mean(&outs[0][36_000..]);
        let gain = mean(&outs[1][36_000..]);
        let tone = mean(&outs[2][36_000..]);
        assert!(
            (drive - 4.0).abs() < 0.08,
            "[{l}] drive should be 4, got {drive}"
        );
        assert!(
            (gain - 0.7).abs() < 0.014,
            "[{l}] gain should be 0.7, got {gain}"
        );
        assert!(
            (tone - 0.8).abs() < 0.016,
            "[{l}] tone should be 0.8, got {tone}"
        );
    });
}

#[test]
fn fad_diode_clipper_newton_learns_the_circuit_through_the_solver() {
    for_each_config("ddsp_fad_diode_clipper_newton", |cfg, root| {
        let outs = render("ddsp_fad_diode_clipper_newton", cfg, root, 20_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 6);
        let tau = mean(&outs[0][16_000..]);
        let k = mean(&outs[1][16_000..]);
        let residual = rms(&outs[2][16_000..]);
        let newton = outs[3].iter().fold(0.0_f32, |m, &v| m.max(v.abs()));
        let (mut gap, mut scale) = (0.0_f64, 0.0_f64);
        for (&g, &d) in outs[4].iter().zip(&outs[5]) {
            gap = gap.max(f64::from(g).abs());
            scale = scale.max(f64::from(d).abs());
        }
        assert!(
            (tau - 1.0).abs() < 0.01,
            "[{l}] tau should be 1e-4 s, got {tau}e-4"
        );
        assert!((k - 0.1).abs() < 0.001, "[{l}] k should be 0.1, got {k}");
        assert!(
            residual < 1e-3,
            "[{l}] residual should vanish, got rms {residual}"
        );
        assert!(
            newton < 1e-4,
            "[{l}] Newton should converge, worst residual {newton}"
        );
        assert!(
            gap < 1e-3 * scale.max(1.0),
            "[{l}] unrolled and implicit derivatives should agree: gap {gap} for a scale of {scale}"
        );
    });
}

#[test]
fn fad_fdn_reverb_lm_identifies_t60_and_damping() {
    for_each_config("ddsp_fad_fdn_reverb_lm", |cfg, root| {
        let outs = render("ddsp_fad_fdn_reverb_lm", cfg, root, 80_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 3);
        let t60 = mean(&outs[0][72_000..]);
        let damping = mean(&outs[1][72_000..]);
        assert!(
            (t60 - 0.6).abs() < 0.01,
            "[{l}] T60 should be 0.6 s, got {t60}"
        );
        assert!(
            (damping - 0.3).abs() < 0.01,
            "[{l}] damping should be 0.3, got {damping}"
        );
    });
}

#[test]
fn fad_fdn_gated_calibrates_then_switches_its_learning_off() {
    for_each_config("ddsp_fad_fdn_gated", |cfg, root| {
        let outs = render("ddsp_fad_fdn_gated", cfg, root, 160_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 4);
        let done = &outs[2];
        let stop = done
            .iter()
            .position(|&d| d > 0.5)
            .unwrap_or_else(|| panic!("[{l}] the flag should rise within ten periods"));
        assert_eq!(
            (stop + 1) % 16_384,
            0,
            "[{l}] the flag must rise on a period's last sample, not at {stop}"
        );
        let period = (stop + 1) / 16_384;
        assert!(
            (4..=20).contains(&period),
            "[{l}] the flag rose at period {period}"
        );
        let t60 = f64::from(outs[0][stop]);
        let damping = f64::from(outs[1][stop]);
        assert!(
            (t60 - 0.6).abs() < 0.01,
            "[{l}] T60 should be 0.6 s, got {t60}"
        );
        assert!(
            (damping - 0.3).abs() < 0.01,
            "[{l}] damping should be 0.3, got {damping}"
        );
        for n in stop..outs[0].len() {
            assert_eq!(
                outs[0][n], outs[0][stop],
                "[{l}] T60 moved after the stop, at frame {n}"
            );
            assert_eq!(
                outs[1][n], outs[1][stop],
                "[{l}] damping moved after the stop, at frame {n}"
            );
            assert!(done[n] > 0.5, "[{l}] the flag fell at frame {n}");
        }
        let residual = rms(&outs[3][stop..]);
        assert!(
            residual < 1e-4,
            "[{l}] the rendered reverb should match the target once stopped, residual {residual:.3e}"
        );
    });
}

#[test]
fn fad_waveguide_string_tunes_its_pitch_through_the_fractional_delay() {
    for_each_config("ddsp_fad_waveguide_string_pitch", |cfg, root| {
        let outs = render("ddsp_fad_waveguide_string_pitch", cfg, root, 80_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 2);
        let pitch = mean(&outs[0][76_000..]);
        let residual = rms(&outs[1][76_000..]);
        assert!(
            (pitch - 220.0).abs() < 0.05,
            "[{l}] pitch should lock on 220 Hz, got {pitch}"
        );
        assert!(
            residual < 1e-3,
            "[{l}] residual should vanish, got rms {residual}"
        );
    });
}

// ───────────────────────────── RAD ─────────────────────────────

#[test]
fn rad_echo_canceller_reaches_30_db_erle() {
    for_each_config("ddsp_rad_echo_canceller_64", |cfg, root| {
        let outs = render("ddsp_rad_echo_canceller_64", cfg, root, 24_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 2);
        let residual = rms(&outs[0][20_000..]);
        let mic = rms(&outs[1][20_000..]);
        let erle_db = 20.0 * (mic / residual.max(1e-12)).log10();
        assert!(
            erle_db > 30.0,
            "[{l}] ERLE should exceed 30 dB, got {erle_db:.1} dB"
        );
    });
}

#[test]
fn rad_mlp_waveshaper_fits_the_soft_clipper() {
    for_each_config("ddsp_rad_mlp_waveshaper", |cfg, root| {
        let outs = render("ddsp_rad_mlp_waveshaper", cfg, root, 40_000);
        let l = label(cfg);
        assert_eq!(outs.len(), 2);
        let residual = rms(&outs[0][36_000..]);
        let target = rms(&outs[1][36_000..]);
        let ratio_db = 20.0 * (residual / target).log10();
        assert!(
            ratio_db < -20.0,
            "[{l}] residual should be 20 dB under the target, got {ratio_db:.1} dB"
        );
        let early = rms(&outs[0][..2_000]);
        assert!(
            early > 5.0 * residual,
            "[{l}] the fit should improve over time: early rms {early}, late {residual}"
        );
    });
}

#[test]
fn rad_harmonic_synth_fits_the_target_spectrum_frame_by_frame() {
    // The frame loss is a sum of 256 products of 16-term sums; its
    // normalisation takes minutes in a debug build (see ddsp_examples.rs).
    if cfg!(debug_assertions) {
        eprintln!("Skipping ddsp_rad_harmonic_spectral_frame: run with --release");
        return;
    }
    for_each_config("ddsp_rad_harmonic_spectral_frame", |cfg, root| {
        let outs = render("ddsp_rad_harmonic_spectral_frame", cfg, root, 51_200);
        let l = label(cfg);
        assert_eq!(outs.len(), 17);
        for (h, lane) in outs[..16].iter().enumerate() {
            let target = 1.0 / (h as f64 + 1.0);
            let learned = mean(&lane[47_000..]);
            assert!(
                (learned - target).abs() < 0.02 * target,
                "[{l}] harmonic {} should reach {target}, got {learned}",
                h + 1
            );
        }
        let residual = rms(&outs[16][43_200..]);
        assert!(
            residual < 0.01,
            "[{l}] resynthesis should match the target, got rms {residual}"
        );
    });
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

/// The path of the control whose label is `name`, whatever the root group.
fn path_of(dsp: &Dsp, name: &str) -> String {
    let suffix = format!("/{name}");
    dsp.controls()
        .find(|c| c.path.ends_with(&suffix))
        .unwrap_or_else(|| panic!("control {name} not found"))
        .path
        .clone()
}

/// One block of a host-driven example from a fresh instance with the given
/// slider values: the per-lane sums (loss first, then the gradients).
fn block_sums(factory: &Factory, sliders: &[(&str, f64)], x: &[f32]) -> Vec<f64> {
    let mut dsp = factory.instantiate(SAMPLE_RATE).expect("instantiate");
    for &(name, value) in sliders {
        dsp.set(&path_of(&dsp, name), value).expect("set slider");
    }
    let mut lanes = vec![vec![0.0_f32; x.len()]; dsp.num_outputs()];
    let mut outs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
    dsp.compute_f32(&[x], &mut outs).expect("block");
    lanes
        .iter()
        .map(|lane| lane.iter().map(|&v| f64::from(v)).sum::<f64>())
        .collect()
}

#[test]
fn rad_host_block_gradients_identify_the_resonator() {
    for_each_config("ddsp_rad_host_block_resonator", |cfg, root| {
        let l = label(cfg);
        let factory = compile("ddsp_rad_host_block_resonator", cfg, root);

        // 1. The block gradient is the derivative of the block loss: central
        //    finite differences on the sliders, same excitation, fresh instance.
        let mut x = vec![0.0_f32; BLOCK];
        Lcg(1).block(&mut x);
        let (a1, a2) = (-0.8_f64, 0.5_f64);
        let sums = block_sums(&factory, &[("a1", a1), ("a2", a2)], &x);
        let (g1, g2) = (sums[1], sums[2]);
        let h = 1e-3_f64;
        let loss_at = |a1: f64, a2: f64| block_sums(&factory, &[("a1", a1), ("a2", a2)], &x)[0];
        let fd1 = (loss_at(a1 + h, a2) - loss_at(a1 - h, a2)) / (2.0 * h);
        let fd2 = (loss_at(a1, a2 + h) - loss_at(a1, a2 - h)) / (2.0 * h);
        assert!(
            (g1 - fd1).abs() < 2e-2 * fd1.abs().max(1.0),
            "[{l}] block gradient for a1: rad {g1} vs finite differences {fd1}"
        );
        assert!(
            (g2 - fd2).abs() < 2e-2 * fd2.abs().max(1.0),
            "[{l}] block gradient for a2: rad {g2} vs finite differences {fd2}"
        );

        // 2. Training: one instance kept running, one Adam step per block on
        //    the summed lanes, the poles kept inside the stability triangle.
        let mut dsp = factory.instantiate(SAMPLE_RATE).expect("instantiate");
        let paths = [path_of(&dsp, "a1"), path_of(&dsp, "a2")];
        let mut p = [-0.8_f64, 0.5_f64];
        let (mut m, mut v) = ([0.0_f64; 2], [0.0_f64; 2]);
        let (lr, b1, b2, eps) = (0.01_f64, 0.9_f64, 0.999_f64, 1e-8_f64);
        let mut noise = Lcg(7);
        let mut lanes = vec![vec![0.0_f32; BLOCK]; 3];
        let (mut first_loss, mut last_loss) = (0.0_f64, 0.0_f64);
        for iteration in 1..=600 {
            dsp.set(&paths[0], p[0]).unwrap();
            dsp.set(&paths[1], p[1]).unwrap();
            noise.block(&mut x);
            let mut outs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
            dsp.compute_f32(&[&x], &mut outs).expect("training block");
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
            p[1] = p[1].clamp(-0.98, 0.98);
            let bound = 1.0 + p[1] - 0.01;
            p[0] = p[0].clamp(-bound, bound);
        }
        assert!(
            (p[0] + 1.2).abs() < 0.02 && (p[1] - 0.72).abs() < 0.02,
            "[{l}] host training should recover (-1.2, 0.72), got ({}, {})",
            p[0],
            p[1]
        );
        assert!(
            last_loss < 1e-3 * first_loss,
            "[{l}] block loss should fall by 30 dB: first {first_loss}, last {last_loss}"
        );
    });
}

const GRU_PARAMS: [(&str, f64); 27] = [
    ("wz1", 0.5),
    ("wz2", -0.4),
    ("wr1", 0.3),
    ("wr2", 0.6),
    ("wh1", 0.8),
    ("wh2", -0.7),
    ("uz11", 0.1),
    ("uz12", -0.2),
    ("uz21", 0.3),
    ("uz22", 0.05),
    ("ur11", 0.2),
    ("ur12", 0.1),
    ("ur21", -0.3),
    ("ur22", 0.4),
    ("uh11", 0.4),
    ("uh12", -0.5),
    ("uh21", 0.2),
    ("uh22", 0.3),
    ("bz1", 0.0),
    ("bz2", 0.0),
    ("br1", 0.0),
    ("br2", 0.0),
    ("bh1", 0.0),
    ("bh2", 0.0),
    ("wo1", 0.9),
    ("wo2", -0.6),
    ("bo", 0.0),
];

/// The hidden amplifier of the fixture, in Rust: `si.smooth(0.7)` into
/// `0.8 tanh(3 s)`.
fn hidden_amp(x: &[f32]) -> Vec<f64> {
    let mut s = 0.0_f64;
    x.iter()
        .map(|&v| {
            s = 0.3 * f64::from(v) + 0.7 * s;
            0.8 * (3.0 * s).tanh()
        })
        .collect()
}

#[test]
fn rad_gru_amp_trained_by_block_bptt_from_the_host() {
    for_each_config("ddsp_rad_gru_amp_host", |cfg, root| {
        let l = label(cfg);
        let factory = compile("ddsp_rad_gru_amp_host", cfg, root);
        let init: Vec<f64> = GRU_PARAMS.iter().map(|&(_, v)| v).collect();
        let sliders = |p: &[f64]| -> Vec<(&str, f64)> {
            GRU_PARAMS
                .iter()
                .zip(p)
                .map(|(&(n, _), &v)| (n, v))
                .collect()
        };

        // 1. Block gradients through the recurrent cell against central
        //    finite differences, on three parameters of different kinds.
        let mut x = vec![0.0_f32; 128];
        Lcg(3).block(&mut x);
        let sums = block_sums(&factory, &sliders(&init), &x);
        let h = 1e-3_f64;
        for &(k, kind) in &[
            (0_usize, "input weight"),
            (8, "recurrent weight"),
            (24, "readout"),
        ] {
            let mut plus = init.clone();
            plus[k] += h;
            let mut minus = init.clone();
            minus[k] -= h;
            let fd = (block_sums(&factory, &sliders(&plus), &x)[0]
                - block_sums(&factory, &sliders(&minus), &x)[0])
                / (2.0 * h);
            assert!(
                (sums[k + 1] - fd).abs() < 2e-2 * fd.abs().max(1.0),
                "[{l}] block gradient for {} ({kind}): rad {} vs finite differences {fd}",
                GRU_PARAMS[k].0,
                sums[k + 1]
            );
        }

        // 2. Training: truncated BPTT, one Adam step per block of 256 on the
        //    summed lanes, the state carried across blocks.
        let mut dsp = factory.instantiate(SAMPLE_RATE).expect("instantiate");
        let paths: Vec<String> = GRU_PARAMS
            .iter()
            .map(|(name, _)| path_of(&dsp, name))
            .collect();
        let mut p = init.clone();
        let (mut m, mut v) = (vec![0.0_f64; 27], vec![0.0_f64; 27]);
        let (lr, b1, b2, eps) = (0.005_f64, 0.9_f64, 0.999_f64, 1e-8_f64);
        let mut noise = Lcg(11);
        let mut x = vec![0.0_f32; BLOCK];
        let mut lanes = vec![vec![0.0_f32; BLOCK]; 28];
        let blocks = 2000;
        let (mut first, mut last) = (0.0_f64, 0.0_f64);
        for iteration in 1..=blocks {
            for (path, &value) in paths.iter().zip(&p) {
                dsp.set(path, value).unwrap();
            }
            noise.block(&mut x);
            let mut outs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
            dsp.compute_f32(&[&x], &mut outs)
                .expect("gru training block");
            let sums: Vec<f64> = lanes
                .iter()
                .map(|lane| lane.iter().map(|&s| f64::from(s)).sum::<f64>() / BLOCK as f64)
                .collect();
            if iteration <= 100 {
                first += sums[0] / 100.0;
            }
            if iteration > blocks - 100 {
                last += sums[0] / 100.0;
            }
            for k in 0..27 {
                let g = sums[k + 1];
                m[k] = b1 * m[k] + (1.0 - b1) * g;
                v[k] = b2 * v[k] + (1.0 - b2) * g * g;
                let m_hat = m[k] / (1.0 - b1.powi(iteration));
                let v_hat = v[k] / (1.0 - b2.powi(iteration));
                p[k] = (p[k] - lr * m_hat / (v_hat.sqrt() + eps)).clamp(-4.0, 4.0);
            }
        }

        // 3. Evaluation on fresh noise, from a fresh instance.
        let mut x = vec![0.0_f32; 4096];
        Lcg(23).block(&mut x);
        let loss_sum = block_sums(&factory, &sliders(&p), &x)[0];
        let residual = (loss_sum / 4096.0).sqrt();
        let target = hidden_amp(&x);
        let target_rms = (target.iter().map(|t| t * t).sum::<f64>() / 4096.0).sqrt();
        let ratio_db = 20.0 * (residual / target_rms).log10();
        assert!(
            last < 0.1 * first,
            "[{l}] training should cut the block loss by 10 dB: first {first}, last {last}"
        );
        assert!(
            ratio_db < -20.0,
            "[{l}] the trained GRU should be 20 dB under the target, got {ratio_db:.1} dB"
        );
    });
}
