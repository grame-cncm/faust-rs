//! The frequency response of a linear program, from one impulse response
//! (`--freqresp`).
//!
//! The first question asked of a filter is its magnitude response, and it was
//! answered by a sine sweep: one render per frequency, a steady-state window
//! to choose for each, for information that a single impulse response holds
//! in full (design §7.1). This module evaluates the transform of that
//! response, `H(w) = sum h[n] exp(-j w n)`, **at the frequencies asked for**,
//! by direct summation: no bin grid decides where the response is known, and
//! a log-spaced axis costs nothing.
//!
//! # The tool should not pretend
//!
//! That sum is a transfer function only for a program that is linear and
//! time-invariant, and nothing in a Faust program says whether it is: a
//! `tanh` in a feedback path, an LFO on a coefficient, a median filter, a DC
//! offset all have an impulse response, and its transform describes none of
//! them. So before any number is printed, three more renders check the three
//! properties the word "linear" stands for, each against the response `h` to
//! the unit impulse ([`Property`]):
//!
//! - **homogeneity**: an impulse of `-0.5` gives `-0.5 h`. A saturator, a
//!   rectifier (the negative sign is there for it), an offset, a generator
//!   mixed in, all fail here;
//! - **time invariance**: an impulse at frame `D` gives `h` delayed by `D`. A
//!   modulated filter, a tremolo, an envelope, a noise source fail here, and
//!   so does a smoothed control that has not settled, which is an envelope:
//!   `--settle N` renders `N` frames of silence before the impulse;
//! - **superposition**: impulses of `1` and `0.5` on two successive frames
//!   give `h[n] + 0.5 h[n-1]`. A median or any rank-order filter is
//!   homogeneous and time-invariant, and fails here: adjacent frames, because
//!   such filters look at a short window.
//!
//! The scale factors are powers of two, so that in a linear program the first
//! two hold **to the bit** (scaling by a power of two and shifting in time
//! commute with every rounding); the third holds to rounding, hence a
//! tolerance. These are necessary conditions, observed at the amplitudes 1
//! and 0.5 of one excitation: a limiter whose threshold the impulse response
//! never reaches passes, and is indeed linear there.
//!
//! # The window
//!
//! A response still ringing at the last frame is truncated, and its
//! transform is that of the truncation. [`tail_energy_fraction`] reports the
//! share of the response's energy held by the last tenth of the window: for a
//! decaying response, what was cut off is of that order.
//!
//! FFI-free, like [`crate::probe::render`] and [`crate::probe::compare`].

use crate::probe::compare::Samples;

/// The frequencies of a response: `points` of them, log-spaced from `fmin` to
/// `fmax` inclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub points: usize,
    pub fmin: f64,
    pub fmax: f64,
}

/// Lowest frequency when none is given.
pub const DEFAULT_FMIN: f64 = 20.0;

impl Grid {
    /// Parses `N` or `N:FMIN:FMAX`. Without a range: 20 Hz to half the
    /// sample rate.
    ///
    /// # Errors
    /// A count that is not a positive integer, a range that is not
    /// `0 < FMIN <= FMAX <= sample_rate / 2`, or a single bound.
    pub fn parse(spec: &str, sample_rate: f64) -> Result<Self, String> {
        let nyquist = sample_rate / 2.0;
        let fields: Vec<&str> = spec.split(':').collect();
        let points = fields[0]
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                format!(
                    "--freqresp `{spec}`: `{}` is not a number of frequencies",
                    fields[0]
                )
            })?;
        let (fmin, fmax) = match fields.as_slice() {
            [_] => (DEFAULT_FMIN.min(nyquist), nyquist),
            [_, fmin, fmax] => {
                let bound = |text: &str| {
                    text.parse::<f64>()
                        .map_err(|_| format!("--freqresp `{spec}`: `{text}` is not a frequency"))
                };
                (bound(fmin)?, bound(fmax)?)
            }
            _ => return Err(format!("--freqresp `{spec}`: expected N or N:FMIN:FMAX")),
        };
        if !(fmin > 0.0 && fmin <= fmax && fmax <= nyquist) {
            return Err(format!(
                "--freqresp `{spec}`: the range must satisfy 0 < FMIN <= FMAX <= {nyquist} Hz \
                 (half the sample rate); a log axis has no 0 Hz"
            ));
        }
        Ok(Self { points, fmin, fmax })
    }

    /// The frequencies, in Hz. The two ends are the bounds themselves, not
    /// what a power rounds them to, and the points between are rounded to
    /// twelve significant digits, so that the third of `4:250:2000` is 1000
    /// and not 999.9999999999999: the response is evaluated at the frequency
    /// that is printed. A single point is `fmin`.
    #[must_use]
    pub fn frequencies(&self) -> Vec<f64> {
        let last = self.points - 1;
        (0..self.points)
            .map(|i| {
                if i == 0 {
                    self.fmin
                } else if i == last {
                    self.fmax
                } else {
                    let f = self.fmin * (self.fmax / self.fmin).powf(i as f64 / last as f64);
                    let scale = 10.0_f64.powi(11 - f.log10().floor() as i32);
                    (f * scale).round() / scale
                }
            })
            .collect()
    }
}

/// The transform of `response` at the angular frequency `omega`, in radians
/// per sample: `sum response[n] exp(-j omega n)`, as `(re, im)`.
///
/// Horner's rule on the polynomial in `z^-1`, from the last sample: one
/// complex product per sample by a constant of modulus one, and the small
/// samples of a decaying tail are summed first.
#[must_use]
pub fn transform(response: &[f64], omega: f64) -> (f64, f64) {
    let (sin, cos) = omega.sin_cos();
    response.iter().rev().fold((0.0, 0.0), |(re, im), sample| {
        (re * cos + im * sin + sample, im * cos - re * sin)
    })
}

/// One point of a response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// `20 log10 |H|`: negative infinity where the response is exactly zero.
    pub magnitude_db: f64,
    /// The argument of `H` in radians, in `(-pi, pi]`; 0 where `H` is zero.
    pub phase: f64,
}

/// The response of one output at each frequency of `hz`.
#[must_use]
pub fn response(samples: &[f64], sample_rate: f64, hz: &[f64]) -> Vec<Point> {
    hz.iter()
        .map(|f| {
            let (re, im) = transform(samples, std::f64::consts::TAU * f / sample_rate);
            Point {
                magnitude_db: 20.0 * re.hypot(im).log10(),
                phase: im.atan2(re),
            }
        })
        .collect()
}

/// The share of the response's energy held by the last tenth of the window
/// (at least one frame); `None` for a response that is zero throughout.
#[must_use]
pub fn tail_energy_fraction(samples: &[f64]) -> Option<f64> {
    let energy = |part: &[f64]| part.iter().map(|v| v * v).sum::<f64>();
    let total = energy(samples);
    if total == 0.0 {
        return None;
    }
    let tail = (samples.len() / 10).max(1).min(samples.len());
    Some(energy(&samples[samples.len() - tail..]) / total)
}

/// Above this share of the energy in the last tenth of the window, the
/// response is said to be still ringing: what was cut off is of that order,
/// and the magnitude near a resonance is off by about its square root (0.1%).
pub const RINGING: f64 = 1e-6;

/// Largest accepted departure from linearity, relative to the expected
/// response's peak, when none is given: rounding in a linear recursive filter
/// stays orders of magnitude below, a nonlinearity worth the name far above.
#[must_use]
pub const fn default_tolerance(double: bool) -> f64 {
    if double { 1e-9 } else { 1e-4 }
}

/// The delay of the time-invariance check for a window of `frames`: 37, which
/// no block size divides, or a quarter of a short window.
#[must_use]
pub fn shift_for(frames: usize) -> usize {
    37.min(frames / 4).max(1)
}

/// One of the three properties checked before a response is transformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    Homogeneity,
    TimeInvariance,
    Superposition,
}

impl Property {
    /// In the order they are checked: the first failure names the defect.
    pub const ALL: [Self; 3] = [Self::Homogeneity, Self::TimeInvariance, Self::Superposition];

    /// The key of the property in the JSON document and in the report.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Homogeneity => "homogeneity",
            Self::TimeInvariance => "time_invariance",
            Self::Superposition => "superposition",
        }
    }

    /// The excitation of the check, as `(frame, amplitude)`.
    #[must_use]
    pub fn taps(self, shift: usize) -> Vec<(usize, f64)> {
        match self {
            Self::Homogeneity => vec![(0, -0.5)],
            Self::TimeInvariance => vec![(shift, 1.0)],
            Self::Superposition => vec![(0, 1.0), (1, 0.5)],
        }
    }

    /// What a linear, time-invariant program answers to [`Self::taps`], given
    /// its response `h` to the unit impulse.
    #[must_use]
    pub fn expected(self, h: &Samples, shift: usize) -> Samples {
        let delayed = |channel: &[f64], by: usize, k: usize| {
            if k >= by { channel[k - by] } else { 0.0 }
        };
        let channels = h
            .channels
            .iter()
            .map(|channel| {
                (0..channel.len())
                    .map(|k| match self {
                        Self::Homogeneity => -0.5 * channel[k],
                        Self::TimeInvariance => delayed(channel, shift, k),
                        Self::Superposition => channel[k] + 0.5 * delayed(channel, 1, k),
                    })
                    .collect()
            })
            .collect();
        Samples {
            start: h.start,
            channels,
        }
    }

    /// What the check found false, for the error: `at` is the frame of the
    /// unit impulse (`--settle`), `shift` the delay of the second one.
    #[must_use]
    pub fn violation(self, at: usize, shift: usize) -> String {
        match self {
            Self::Homogeneity => "homogeneity: the response to an impulse of -0.5 is not -0.5 \
                                  times the response to an impulse of 1"
                .to_owned(),
            Self::TimeInvariance => format!(
                "time invariance: the response to an impulse at frame {} is not the response \
                 to an impulse at frame {at}, {shift} frames later",
                at + shift
            ),
            Self::Superposition => format!(
                "superposition: the response to impulses of 1 at frame {at} and 0.5 at frame {} \
                 is not the sum of their responses",
                at + 1
            ),
        }
    }

    /// What usually explains the violation.
    #[must_use]
    pub const fn usual_cause(self) -> &'static str {
        match self {
            Self::Homogeneity => {
                "a saturation, a rectifier, a threshold, or an output that does not come from the input"
            }
            Self::TimeInvariance => {
                "a modulation (an LFO on a coefficient, a tremolo), an envelope, a noise source"
            }
            Self::Superposition => {
                "a rank-order operation (median, min, max) over neighbouring samples"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{PI, TAU};

    #[test]
    fn a_grid_is_log_spaced_and_ends_on_its_bounds() {
        let grid = Grid::parse("4:100:800", 44100.0).unwrap();
        let hz = grid.frequencies();
        assert_eq!(hz.len(), 4);
        assert_eq!(hz, [100.0, 200.0, 400.0, 800.0]);
        // a point that is not a round number keeps twelve digits
        let third = Grid::parse("3:100:1000", 44100.0).unwrap().frequencies()[1];
        assert!((third - 1e5_f64.sqrt()).abs() < 1e-9 && third != 1e5_f64.sqrt());
        assert_eq!(format!("{third:?}"), "316.227766017");
        // one point is the lower bound
        assert_eq!(
            Grid::parse("1:1000:1000", 44100.0).unwrap().frequencies(),
            [1000.0]
        );
    }

    #[test]
    fn the_default_range_is_20_hz_to_half_the_sample_rate() {
        let grid = Grid::parse("16", 48000.0).unwrap();
        assert_eq!((grid.points, grid.fmin, grid.fmax), (16, 20.0, 24000.0));
    }

    #[test]
    fn a_grid_that_means_nothing_is_refused() {
        for spec in [
            "0",
            "x",
            "8:100",
            "8:0:1000",
            "8:2000:1000",
            "8:20:30000",
            "8:a:b",
        ] {
            assert!(Grid::parse(spec, 44100.0).is_err(), "{spec}");
        }
    }

    /// `h[n] = a^n`: `H = 1 / (1 - a exp(-jw))`.
    #[test]
    fn the_transform_of_a_geometric_response_is_its_closed_form() {
        let a: f64 = 0.5;
        let h: Vec<f64> = (0..1200).map(|n| a.powi(n)).collect();
        for hz in [20.0, 1000.0, 11025.0, 22050.0] {
            let w = TAU * hz / 44100.0;
            let (re, im) = transform(&h, w);
            // 1 / (1 - a cos w + j a sin w)
            let (dr, di) = (1.0 - a * w.cos(), a * w.sin());
            let d2 = dr * dr + di * di;
            assert!((re - dr / d2).abs() < 1e-14, "{hz}: {re}");
            assert!((im + di / d2).abs() < 1e-14, "{hz}: {im}");
        }
    }

    #[test]
    fn a_delay_is_a_phase_and_a_gain_is_a_level() {
        // 0.5 delayed by one frame, at a quarter of the sample rate:
        // 0.5 exp(-j pi/2)
        let point = response(&[0.0, 0.5, 0.0, 0.0], 4.0, &[1.0])[0];
        assert!((point.magnitude_db - 20.0 * 0.5_f64.log10()).abs() < 1e-12);
        assert!((point.phase + PI / 2.0).abs() < 1e-12);
        // silence: no level, and a phase of 0 rather than a NaN
        let silent = response(&[0.0; 8], 4.0, &[1.0])[0];
        assert!(silent.magnitude_db == f64::NEG_INFINITY && silent.phase == 0.0);
    }

    #[test]
    fn the_tail_is_the_last_tenth_of_the_window() {
        // 20 frames of energy 1 each: the last two hold a tenth
        assert_eq!(tail_energy_fraction(&[1.0; 20]), Some(0.1));
        // a response that has died holds nothing there
        let mut dead = vec![0.0; 100];
        dead[0] = 1.0;
        assert_eq!(tail_energy_fraction(&dead), Some(0.0));
        // a short window still has a tail of one frame
        assert_eq!(tail_energy_fraction(&[3.0, 4.0]), Some(16.0 / 25.0));
        assert_eq!(tail_energy_fraction(&[0.0; 5]), None);
    }

    #[test]
    fn the_expected_responses_are_those_of_a_linear_time_invariant_program() {
        let h = Samples {
            start: 0,
            channels: vec![vec![1.0, 0.5, 0.25, 0.125]],
        };
        assert_eq!(
            Property::Homogeneity.expected(&h, 2).channels[0],
            [-0.5, -0.25, -0.125, -0.0625]
        );
        assert_eq!(
            Property::TimeInvariance.expected(&h, 2).channels[0],
            [0.0, 0.0, 1.0, 0.5]
        );
        assert_eq!(
            Property::Superposition.expected(&h, 2).channels[0],
            [1.0, 1.0, 0.5, 0.25]
        );
        assert_eq!(Property::TimeInvariance.taps(2), [(2, 1.0)]);
    }

    #[test]
    fn the_shift_fits_the_window() {
        assert_eq!(shift_for(15000), 37);
        assert_eq!(shift_for(100), 25);
        assert_eq!(shift_for(3), 1);
    }
}
