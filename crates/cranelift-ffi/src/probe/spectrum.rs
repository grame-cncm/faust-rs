//! Minimal spectral analysis for the `f0`, `sfdr` and `thd` reductions.
//!
//! A radix-2 FFT written here rather than pulled in as a dependency: the tool
//! needs one transform for one reduction, and a test binary is a poor reason
//! to add a numerics crate to the workspace.
//!
//! # Reading the result
//! Bin resolution is `sample_rate / n`, so a peak reported at 439.45 Hz for a
//! 440 Hz signal is the nearest bin, not an error. Callers wanting exactness
//! should choose a frame count that puts the frequency of interest on a bin
//! centre — which also removes the spectral leakage that otherwise smears
//! every harmonic across the whole spectrum and makes any "energy outside the
//! harmonics" measurement meaningless.

/// In-place iterative radix-2 FFT. `re`/`im` must have the same power-of-two length.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());
    debug_assert_eq!(n, im.len());

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (ar, ai) = (re[start + k], im[start + k]);
                let (br, bi) = (re[start + k + len / 2], im[start + k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                re[start + k] = ar + tr;
                im[start + k] = ai + ti;
                re[start + k + len / 2] = ar - tr;
                im[start + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Frequency of the strongest non-DC bin, in Hz.
///
/// Returns `0.0` for fewer than two samples. The signal is zero-padded to the
/// next power of two; DC is excluded because a DSP with an offset would
/// otherwise always report 0 Hz.
#[must_use]
pub fn dominant_frequency(samples: &[f64], sample_rate: f64) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let n = samples.len().next_power_of_two();
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    re[..samples.len()].copy_from_slice(samples);
    fft(&mut re, &mut im);

    let mut best = 1usize;
    let mut best_mag = f64::NEG_INFINITY;
    for k in 1..n / 2 {
        let mag = re[k].mul_add(re[k], im[k] * im[k]);
        if mag > best_mag {
            best_mag = mag;
            best = k;
        }
    }
    best as f64 * sample_rate / n as f64
}

/// Blackman-Harris 4-term window.
///
/// Chosen over a rectangular window because both measurements below compare a
/// loud fundamental against components 100 dB or more beneath it. Rectangular
/// leakage would bury them: its first sidelobe is 13 dB down, this window's is
/// 92 dB down, which is what makes the floor meaningful rather than an artifact
/// of the transform.
fn blackman_harris(n: usize) -> Vec<f64> {
    const A: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];
    (0..n)
        .map(|i| {
            let x = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            A[0] - A[1] * x.cos() + A[2] * (2.0 * x).cos() - A[3] * (3.0 * x).cos()
        })
        .collect()
}

/// Windowed magnitude spectrum of `samples`, and its bin width in Hz.
fn magnitudes(samples: &[f64], sample_rate: f64) -> (Vec<f64>, f64) {
    let n = samples.len().next_power_of_two();
    let w = blackman_harris(samples.len());
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for (i, (x, wi)) in samples.iter().zip(&w).enumerate() {
        re[i] = x * wi;
    }
    fft(&mut re, &mut im);
    let mags = (0..=n / 2).map(|k| re[k].hypot(im[k])).collect::<Vec<_>>();
    (mags, sample_rate / n as f64)
}

/// Bins within `±HARMONIC_GUARD` of a harmonic are attributed to it.
///
/// The window's main lobe is four bins wide, so a narrower guard would read the
/// skirt of a partial as a spurious component and report aliasing that is not
/// there. A wider one would start hiding real spurs that sit close to a
/// harmonic.
const HARMONIC_GUARD: i64 = 4;

/// Whether bin `k` belongs to the harmonic grid of `f0`.
fn on_grid(k: usize, f0_bin: f64, bins: usize) -> bool {
    if f0_bin <= 0.0 {
        return false;
    }
    let k = k as f64;
    let nearest = (k / f0_bin).round().max(0.0);
    let centre = nearest * f0_bin;
    (k - centre).abs() <= HARMONIC_GUARD as f64 && (nearest as usize) <= bins
}

/// Spurious-free dynamic range: the largest component **off** the harmonic grid
/// of `f0`, in dB below the fundamental. Larger is cleaner.
///
/// This answers "how much aliasing is left", which harmonic distortion measures
/// cannot: for a band-limited oscillator or an antialiased waveshaper the
/// harmonics are wanted and everything else is not.
///
/// # Measurement floor
/// The window sets the floor. Blackman-Harris sidelobes are 92 dB down, so an
/// arbitrary tone reads about 93 dB however clean the DSP is, and a result near
/// that number measures the transform rather than the signal. Choosing a frame
/// count that puts `f0` on a bin centre removes the leakage entirely and takes
/// the floor down to numerical precision.
///
/// # Stationarity
/// The window must be stationary. Measuring while the spectrum decays smears
/// every partial, and the smearing appears as off-grid energy — a decaying
/// pluck can read 20 dB when it is in fact alias-free. Use `--skip` and `-n` to
/// select a steady stretch.
///
/// Returns `f64::INFINITY` when nothing off-grid is found, and `0.0` when the
/// input is too short or silent.
#[must_use]
pub fn sfdr_db(samples: &[f64], sample_rate: f64, f0: f64) -> f64 {
    let Some((mags, bin_hz, f0_bin)) = prepare(samples, sample_rate, f0) else {
        return 0.0;
    };
    let bins = mags.len();
    let fundamental = peak_near(&mags, f0_bin);
    if fundamental <= 0.0 {
        return 0.0;
    }
    let mut worst = 0.0_f64;
    for (k, m) in mags.iter().enumerate().skip(1) {
        if !on_grid(k, f0_bin, bins) {
            worst = worst.max(*m);
        }
    }
    let _ = bin_hz;
    if worst <= 0.0 {
        return f64::INFINITY;
    }
    20.0 * (fundamental / worst).log10()
}

/// Total harmonic distortion: the energy in harmonics 2, 3, … relative to the
/// fundamental, in dB. Larger (less negative) means more distortion.
///
/// The companion to `sfdr_db` and a different question: here the harmonics are
/// what is being measured, not what is being excluded.
///
/// Returns `f64::NEG_INFINITY` for a pure tone, and `0.0` when the input is too
/// short or silent.
#[must_use]
pub fn thd_db(samples: &[f64], sample_rate: f64, f0: f64) -> f64 {
    let Some((mags, _, f0_bin)) = prepare(samples, sample_rate, f0) else {
        return 0.0;
    };
    let fundamental = peak_near(&mags, f0_bin);
    if fundamental <= 0.0 {
        return 0.0;
    }
    let mut energy = 0.0_f64;
    let mut n = 2.0_f64;
    while n * f0_bin < mags.len() as f64 {
        let m = peak_near(&mags, n * f0_bin);
        energy += m * m;
        n += 1.0;
    }
    if energy <= 0.0 {
        return f64::NEG_INFINITY;
    }
    20.0 * (energy.sqrt() / fundamental).log10()
}

/// Shared setup: window, transform, and the fundamental's bin position.
fn prepare(samples: &[f64], sample_rate: f64, f0: f64) -> Option<(Vec<f64>, f64, f64)> {
    if samples.len() < 8 || sample_rate <= 0.0 {
        return None;
    }
    let f0 = if f0 > 0.0 {
        f0
    } else {
        dominant_frequency(samples, sample_rate)
    };
    if f0 <= 0.0 {
        return None;
    }
    let (mags, bin_hz) = magnitudes(samples, sample_rate);
    Some((mags, bin_hz, f0 / bin_hz))
}

/// Largest magnitude within the guard band around a bin position.
fn peak_near(mags: &[f64], bin: f64) -> f64 {
    let lo = (bin.round() as i64 - HARMONIC_GUARD).max(1) as usize;
    let hi = ((bin.round() as i64 + HARMONIC_GUARD) as usize).min(mags.len() - 1);
    if lo > hi {
        return 0.0;
    }
    mags[lo..=hi].iter().copied().fold(0.0_f64, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, hz: f64, sr: f64) -> Vec<f64> {
        (0..n)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / sr).sin())
            .collect()
    }

    #[test]
    fn finds_a_sine_on_a_bin_centre_exactly() {
        // 1024 samples at 48 kHz: bin width 46.875 Hz, so 468.75 Hz is bin 10.
        let sr = 48_000.0;
        let x = sine(1024, 468.75, sr);
        assert!((dominant_frequency(&x, sr) - 468.75).abs() < 1e-9);
    }

    #[test]
    fn finds_a_sine_within_one_bin_off_centre() {
        let sr = 48_000.0;
        let x = sine(4096, 440.0, sr);
        let bin = sr / 4096.0;
        assert!((dominant_frequency(&x, sr) - 440.0).abs() <= bin);
    }

    #[test]
    fn ignores_dc() {
        // A constant plus a small sine must report the sine, not 0 Hz.
        let sr = 8_000.0;
        let x: Vec<f64> = sine(1024, 500.0, sr)
            .iter()
            .map(|v| v * 0.1 + 5.0)
            .collect();
        assert!((dominant_frequency(&x, sr) - 500.0).abs() < sr / 1024.0);
    }

    #[test]
    fn handles_degenerate_input() {
        assert!(dominant_frequency(&[], 48_000.0).abs() < f64::EPSILON);
        assert!(dominant_frequency(&[1.0], 48_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fft_matches_a_direct_dft() {
        let n = 32;
        let x: Vec<f64> = (0..n).map(|i| (i as f64 * 0.37).sin()).collect();
        let mut re = x.clone();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        for k in [0usize, 1, 5, 16] {
            let (mut dr, mut di) = (0.0, 0.0);
            for (i, v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * i) as f64 / n as f64;
                dr += v * a.cos();
                di += v * a.sin();
            }
            assert!((re[k] - dr).abs() < 1e-9, "bin {k} real");
            assert!((im[k] - di).abs() < 1e-9, "bin {k} imag");
        }
    }

    /// A pure tone has nothing off the harmonic grid.
    #[test]
    fn sfdr_is_large_for_a_clean_sine() {
        let sr = 48000.0;
        let n = 8192;
        let x: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / sr).sin())
            .collect();
        // 1000 Hz is not on a bin centre here, so this measures the window's
        // sidelobes, not the sine: ~93 dB is the floor, and that is the point.
        let got = sfdr_db(&x, sr, 1000.0);
        assert!(got > 85.0, "expected the window floor, got {got}");
    }

    /// A tone plus a deliberate non-harmonic spur is measured at its true depth.
    #[test]
    fn sfdr_reports_the_depth_of_a_planted_spur() {
        let sr = 48000.0;
        let n = 8192;
        let depth = 60.0_f64;
        let a = 10f64.powf(-depth / 20.0);
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
                    + a * (2.0 * std::f64::consts::PI * 1531.0 * t).sin()
            })
            .collect();
        let got = sfdr_db(&x, sr, 1000.0);
        assert!((got - depth).abs() < 2.0, "expected ~{depth} dB, got {got}");
    }

    /// Harmonics are not spurious: adding one must not move the SFDR.
    #[test]
    fn sfdr_ignores_harmonics() {
        let sr = 48000.0;
        let n = 8192;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
                    + 0.5 * (2.0 * std::f64::consts::PI * 3000.0 * t).sin()
            })
            .collect();
        assert!(sfdr_db(&x, sr, 1000.0) > 85.0);
    }

    /// Putting the tone on a bin centre removes leakage, and the floor drops far
    /// below what the window alone allows. This is why the report's own
    /// measurements chose frame counts that align.
    #[test]
    fn bin_aligned_tone_beats_the_window_floor() {
        let sr = 48000.0;
        let n = 8192;
        let f0 = 128.0 * sr / n as f64; // exactly on a bin
        let x: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f0 * i as f64 / sr).sin())
            .collect();
        let got = sfdr_db(&x, sr, f0);
        assert!(
            got > 150.0,
            "expected well past the window floor, got {got}"
        );
    }

    /// THD measures exactly what SFDR excludes: a third harmonic at -6 dB.
    #[test]
    fn thd_measures_a_planted_harmonic() {
        let sr = 48000.0;
        let n = 8192;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
                    + 0.5 * (2.0 * std::f64::consts::PI * 3000.0 * t).sin()
            })
            .collect();
        let got = thd_db(&x, sr, 1000.0);
        assert!((got + 6.02).abs() < 1.0, "expected ~-6 dB, got {got}");
    }

    /// An explicit f0 must be honoured rather than re-estimated: a signal whose
    /// loudest component is not the fundamental would otherwise be misread.
    #[test]
    fn explicit_f0_overrides_the_estimate() {
        let sr = 48000.0;
        let n = 8192;
        // The second harmonic dominates, so the estimator would pick 2000 Hz.
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                0.2 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
                    + (2.0 * std::f64::consts::PI * 2000.0 * t).sin()
            })
            .collect();
        assert!(dominant_frequency(&x, sr) > 1500.0);
        // Told the truth, THD sees a very loud second harmonic.
        assert!(thd_db(&x, sr, 1000.0) > 10.0);
    }
}
