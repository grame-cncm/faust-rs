//! Minimal spectral analysis for the `f0` reduction.
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
}
