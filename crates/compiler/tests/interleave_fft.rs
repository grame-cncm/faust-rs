//! S3 — the framed FFT milestone (roadmap
//! `porting/ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md` §7).
//!
//! A *frame-rate* FFT built on the P3 boolean-`ondemand` block and the
//! spatial `an.fftb` core from `analyzers.lib`: the O(N log N) butterflies run
//! once per hop of N samples (held between frames) instead of every sample.
//! This is the "analysis-only" mode — the bins held by the `PermVar`s at frame
//! rate are consumed directly, no `serialize_out`.
//!
//! Oracle: a direct DFT of the known input window, computed in Rust. The test
//! needs `analyzers.lib`; it **skips gracefully** when faustlibraries is
//! unavailable (set `FAUST_RS_FAUSTLIBRARIES_ROOT`, else the default path).

use std::io::Cursor;
use std::path::PathBuf;
use std::thread;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

/// FFT butterfly lowering recurses deeply; run compilation on a large stack.
fn run_with_large_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    thread::Builder::new()
        .name("fft-framed".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .expect("spawn worker")
        .join()
        .expect("worker thread should finish")
}

fn faustlibraries_root() -> Option<PathBuf> {
    std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(DEFAULT_FAUSTLIBRARIES_ROOT);
            default.exists().then_some(default)
        })
}

/// Framed FFT, analysis-only, for a compile-time size `N`. The frame operator
/// complexifies the N real window taps (`(_, 0)`), runs the standard
/// `bit-reverse-shuffle : butterflies` DFT, and the `ondemand` block holds the
/// 2N bin reals at frame rate.
fn fft_framed_source(n: usize) -> String {
    format!(
        r#"an = library("analyzers.lib");
si = library("signals.lib");
frame_clock(N) = ((+(1) : %(N)) ~ _) == 0;
serialize_in(N) = _ <: par(i, N, @(N-1-i));
fftFX(N) = par(i, N, (_, 0)) : an.c_bit_reverse_shuffle(N) : an.fftb(N);
fft_framed(N) = serialize_in(N) : (frame_clock(N), si.bus(N)) : ondemand(fftFX(N));
process = fft_framed({n});"#
    )
}

fn run_framed_fft(n: usize, input: &[f32]) -> Option<Vec<Vec<f32>>> {
    let root = faustlibraries_root()?;
    let input = input.to_vec();
    Some(run_with_large_stack(move || {
        run_framed_fft_inner(n, &root, &input)
    }))
}

fn run_framed_fft_inner(n: usize, root: &PathBuf, input: &[f32]) -> Vec<Vec<f32>> {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-fftframed-{n}-{}-{:?}.dsp",
        std::process::id(),
        thread::current().id()
    ));
    std::fs::write(&path, fft_framed_source(n)).expect("write temp dsp");
    let fbc = Compiler::new()
        .compile_file_to_interp_with_lane(
            &path,
            std::slice::from_ref(root),
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("framed FFT N={n} compilation failed: {e}"));
    let _ = std::fs::remove_file(&path);

    let frames = input.len();
    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("fbc parse");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);

    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("outputs");
    assert_eq!(
        num_outputs,
        2 * n,
        "analysis-only framed FFT exposes 2N bin reals"
    );
    let mut outputs = vec![vec![0.0_f32; frames]; num_outputs];
    let mut slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frames as i32, &[input], &mut slices)
        .expect("framed FFT execution");
    outputs
}

/// Direct DFT of one real window: `bin_m = Σ_i w[i] · exp(-2πi·m·i/N)`,
/// returned interleaved as `[re0, im0, re1, im1, …]`.
fn dft(window: &[f32]) -> Vec<f32> {
    let n = window.len();
    let mut out = vec![0.0_f32; 2 * n];
    for m in 0..n {
        let (mut re, mut im) = (0.0_f64, 0.0_f64);
        for (i, &w) in window.iter().enumerate() {
            let ang = -2.0 * std::f64::consts::PI * (m * i) as f64 / n as f64;
            re += f64::from(w) * ang.cos();
            im += f64::from(w) * ang.sin();
        }
        out[2 * m] = re as f32;
        out[2 * m + 1] = im as f32;
    }
    out
}

fn check_framed_fft_matches_dft(n: usize) {
    // A deterministic non-trivial input.
    let frames = 6 * n;
    let input: Vec<f32> = (0..frames).map(|k| ((k * 3 % 7) as f32) - 3.0).collect();

    let Some(outputs) = run_framed_fft(n, &input) else {
        eprintln!("Skipping framed FFT N={n}: faustlibraries unavailable");
        return;
    };

    // frame_clock(N) fires at t ≡ N-1 (mod N); at that tick the held bins are
    // the DFT of the window {x[t-N+1 .. t]} (l_i = x[t-N+1+i]).
    let mut checked = 0;
    for t in 0..frames {
        if (t + 1) % n != 0 || t < n - 1 {
            continue;
        }
        let window: Vec<f32> = (0..n).map(|i| input[t - (n - 1) + i]).collect();
        let expected = dft(&window);
        for (bin, &want) in expected.iter().enumerate() {
            let got = outputs[bin][t];
            let tol = 1.0e-3_f32 * (1.0 + want.abs());
            assert!(
                (got - want).abs() <= tol,
                "N={n} frame tick {t}, bin #{bin}: framed FFT {got} vs DFT {want}"
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected several frame ticks, checked {checked}"
    );
}

#[test]
fn framed_fft_bins_match_direct_dft_at_frame_ticks() {
    // N=4 and N=8: the O(N log N) butterflies scale through the pattern
    // matcher and produce the correct per-frame spectrum.
    check_framed_fft_matches_dft(4);
    check_framed_fft_matches_dft(8);
}

#[test]
fn framed_fft_holds_bins_between_frame_ticks() {
    let n = 4usize;
    let frames = 16;
    let input: Vec<f32> = (0..frames).map(|k| (k as f32).sin()).collect();

    let Some(outputs) = run_framed_fft(n, &input) else {
        eprintln!("Skipping framed FFT hold: faustlibraries unavailable");
        return;
    };

    // Between fires the held bins do not change: outputs[b][t] == outputs[b][t-1]
    // for every t that is not a fire tick (and after the first fire).
    for t in n..frames {
        let is_fire = (t + 1) % n == 0;
        if is_fire {
            continue;
        }
        for (bin, channel) in outputs.iter().enumerate() {
            assert!(
                (channel[t] - channel[t - 1]).abs() < 1.0e-6,
                "bin #{bin} changed between frames at t={t}: {} -> {}",
                channel[t - 1],
                channel[t]
            );
        }
    }
}
