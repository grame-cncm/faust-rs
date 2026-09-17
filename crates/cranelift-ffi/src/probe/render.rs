//! Offline rendering: input generation, the block loop, and reductions.
//!
//! This module is deliberately free of FFI: it describes *what* to render and
//! *how to summarize it*, and takes the actual `compute` as a callback. That
//! keeps the render policy unit-testable without a JIT, and leaves room for a
//! second backend behind the same policy (design §8).

/// What to feed the DSP inputs.
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    /// Silence on every channel.
    Zero,
    /// Unit impulse on frame 0 of every input channel.
    ///
    /// This is the reference impulse-test excitation. Note that it excites all
    /// channels at once, which is precisely why it cannot exercise a
    /// cross-channel effect such as a ping-pong delay — see [`InputMode::ImpulseChannel`].
    Impulse,
    /// Unit impulse on frame 0 of one channel only, silence elsewhere.
    ImpulseChannel(usize),
    /// Weighted impulses, `(frame, amplitude)`, on one channel or on all:
    /// the excitations a linearity check compares with the unit impulse
    /// (another level, another time, a sum of two), see
    /// [`crate::probe::freqresp`].
    Impulses {
        channel: Option<usize>,
        taps: Vec<(usize, f64)>,
    },
    /// Constant 1.0 on every channel.
    Dc,
    /// Uniform noise in `[-1, 1)` from a seeded generator.
    ///
    /// Seeded so a run is reproducible: an unseeded probe cannot be used as a
    /// regression baseline.
    White { seed: u64 },
    /// Full-scale sine at the given frequency on every channel.
    Sine { hz: f64 },
    /// The channels of an audio file (`--in file:PATH[:CH]`): input `i` reads
    /// channel `i`, or the last channel when the file has fewer (a mono file
    /// feeds every input); silence past the end of the file.
    File {
        channels: std::sync::Arc<Vec<Vec<f64>>>,
        /// The file's own sample rate when its format records one.
        sample_rate: Option<u32>,
    },
}

impl InputMode {
    /// The excitation read from `path` (`.wav`, `.f64` or `.f32`, see
    /// [`crate::probe::audio_file`]); with `channel`, that channel alone
    /// feeds every input.
    pub fn from_file(path: &std::path::Path, channel: Option<usize>) -> Result<Self, String> {
        let (mut channels, sample_rate) = crate::probe::audio_file::read_channels(path)?;
        if channels.is_empty() || channels[0].is_empty() {
            return Err(format!("{}: no samples", path.display()));
        }
        if let Some(ch) = channel {
            if ch >= channels.len() {
                return Err(format!(
                    "{}: channel {ch} requested, the file has {}",
                    path.display(),
                    channels.len()
                ));
            }
            channels = vec![channels.swap_remove(ch)];
        }
        Ok(Self::File {
            channels: std::sync::Arc::new(channels),
            sample_rate,
        })
    }

    /// Sample for `channel` at absolute `frame`.
    #[must_use]
    pub fn sample(&self, channel: usize, frame: usize, sample_rate: f64) -> f64 {
        match self {
            Self::Zero => 0.0,
            Self::Impulse => f64::from(u8::from(frame == 0)),
            Self::ImpulseChannel(ch) => f64::from(u8::from(frame == 0 && channel == *ch)),
            Self::Impulses {
                channel: only,
                taps,
            } => {
                if only.is_some_and(|ch| ch != channel) {
                    return 0.0;
                }
                taps.iter()
                    .filter(|(at, _)| *at == frame)
                    .map(|(_, amplitude)| amplitude)
                    .sum()
            }
            Self::Dc => 1.0,
            Self::White { seed } => white(*seed, channel, frame),
            Self::Sine { hz } => (std::f64::consts::TAU * hz * frame as f64 / sample_rate).sin(),
            Self::File { channels, .. } => channels[channel.min(channels.len() - 1)]
                .get(frame)
                .copied()
                .unwrap_or(0.0),
        }
    }
}

/// Position-addressed uniform noise in `[-1, 1)`.
///
/// Derived from (seed, channel, frame) rather than carried as running state so
/// the value at a frame does not depend on how the render was blocked. A probe
/// whose noise changes with `--block` could not be compared across runs.
fn white(seed: u64, channel: usize, frame: usize) -> f64 {
    // SplitMix64 finalizer: cheap, and good enough for excitation.
    let mut z = seed
        .wrapping_add((channel as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add((frame as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Map the top 53 bits to [0,1), then to [-1,1).
    let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
    unit.mul_add(2.0, -1.0)
}

/// Per-channel statistics over the measured window.
///
/// The window is what `--skip` leaves: statistics and dump must agree on it,
/// or a strongly attenuated steady state gets swamped by a startup transient
/// and the resulting "discrepancy" is blamed on the DSP. That happened during
/// the port this tool comes from (design §7.2), so the window is reported
/// alongside the values rather than left implicit.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelStats {
    /// Largest absolute value.
    pub peak: f64,
    /// Root mean square.
    pub rms: f64,
    /// Mean — a non-zero value flags a DC offset.
    pub dc: f64,
    /// Whether every sample was finite.
    pub finite: bool,
    /// Absolute frame of the first sample that reached `peak`, `None` when the
    /// window holds no non-zero finite sample. Where the maximum is says
    /// whether it is an onset, a control event, or a level still rising at the
    /// end of the render.
    pub peak_at: Option<usize>,
    /// The first non-finite sample of this channel, in or out of the window.
    pub first_non_finite: Option<Located>,
    /// The first sample of the window whose magnitude exceeds
    /// [`RenderLimit`], when a limit was given.
    pub first_above: Option<Located>,
    /// Samples of the window that are subnormal **at the width the program
    /// was compiled in**: non-zero and below the smallest normal number of
    /// that width (about `1.2e-38` in single precision, `2.2e-308` in double).
    ///
    /// A decaying tail ends in them, and on a target that does not flush them
    /// to zero they cost far more than a normal number. Only the outputs are
    /// seen: a subnormal inside a feedback loop that a later gain lifts or a
    /// later stage absorbs does not show here.
    pub subnormal: usize,
    /// Absolute frame of the first subnormal sample of the window.
    pub subnormal_at: Option<usize>,
}

/// A sample and where it is: what turns "the render failed" into a frame to
/// look at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// Absolute frame index.
    pub frame: usize,
    /// The sample there (`NaN`, an infinity, or the value above the limit).
    pub value: f64,
}

/// Magnitude a render's window must stay under (`--fail-above`).
///
/// A feedback loop that leaves its stable region runs away for thousands of
/// frames before it overflows to infinity and then to `NaN`. The first frame
/// above a level a sane signal never reaches is the frame at which to look,
/// and it is known long before the render turns non-finite.
pub type RenderLimit = Option<f64>;

/// Statistics for a whole render, with the window they describe.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderStats {
    /// First frame included in the statistics.
    pub window_start: usize,
    /// Number of frames included.
    pub window_len: usize,
    /// One entry per output channel.
    pub channels: Vec<ChannelStats>,
    /// Frames, in or out of the window, with a non-finite sample on any
    /// channel: whether a render went wrong once and recovered, or for good.
    pub non_finite_frames: usize,
    /// What the `compute` calls cost, when the render was asked to time them
    /// (`--time`): never otherwise, a clock being the one thing here that
    /// differs between two runs.
    pub timing: Option<crate::probe::timing::Timing>,
}

impl RenderStats {
    /// Whether every channel stayed finite.
    #[must_use]
    pub fn all_finite(&self) -> bool {
        self.channels.iter().all(|c| c.finite)
    }

    /// The earliest non-finite sample of the render and its channel.
    #[must_use]
    pub fn first_non_finite(&self) -> Option<(usize, Located)> {
        earliest(self.channels.iter().map(|c| c.first_non_finite))
    }

    /// The earliest sample above the limit and its channel.
    #[must_use]
    pub fn first_above(&self) -> Option<(usize, Located)> {
        earliest(self.channels.iter().map(|c| c.first_above))
    }

    /// Whether every output is exactly zero over the window: not quiet,
    /// silent. Nearly always a gate never pressed or an input never fed, and
    /// never a rounding matter, hence the exact comparison.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.window_len > 0 && self.channels.iter().all(|c| c.finite && c.peak == 0.0)
    }
}

/// The earliest of the per-channel locations, with its channel; the lowest
/// channel wins a tie, so the answer does not depend on iteration details.
fn earliest(per_channel: impl Iterator<Item = Option<Located>>) -> Option<(usize, Located)> {
    per_channel
        .enumerate()
        .filter_map(|(ch, located)| located.map(|l| (ch, l)))
        .min_by_key(|(ch, located)| (located.frame, *ch))
}

/// Accumulates statistics over the measured window.
#[derive(Debug)]
pub(crate) struct StatsAccumulator {
    peak: Vec<f64>,
    sum_sq: Vec<f64>,
    sum: Vec<f64>,
    finite: Vec<bool>,
    peak_at: Vec<Option<usize>>,
    first_non_finite: Vec<Option<Located>>,
    first_above: Vec<Option<Located>>,
    non_finite_frames: usize,
    subnormal: Vec<usize>,
    subnormal_at: Vec<Option<usize>>,
    /// Whether the samples come from a single-precision program: what
    /// "subnormal" is measured against.
    single: bool,
    limit: RenderLimit,
    counted: usize,
    start: usize,
}

impl StatsAccumulator {
    #[cfg(test)]
    pub(crate) fn new(channels: usize, start: usize) -> Self {
        Self::with_limit(channels, start, None)
    }

    pub(crate) fn with_limit(channels: usize, start: usize, limit: RenderLimit) -> Self {
        Self {
            peak: vec![0.0; channels],
            sum_sq: vec![0.0; channels],
            sum: vec![0.0; channels],
            finite: vec![true; channels],
            peak_at: vec![None; channels],
            first_non_finite: vec![None; channels],
            first_above: vec![None; channels],
            non_finite_frames: 0,
            subnormal: vec![0; channels],
            subnormal_at: vec![None; channels],
            single: false,
            limit,
            counted: 0,
            start,
        }
    }

    /// The width the samples were computed in: a sample handed over as an
    /// `f64` is subnormal when it was in the program, and a single-precision
    /// subnormal is a perfectly normal `f64`.
    pub(crate) const fn at_width(mut self, double: bool) -> Self {
        self.single = !double;
        self
    }

    /// Record one frame. `frame` is absolute; frames before the window start
    /// are still checked for finiteness but excluded from the statistics.
    pub(crate) fn push(&mut self, frame: usize, samples: &[f64]) {
        let inside = frame >= self.start;
        let mut frame_non_finite = false;
        for (ch, &value) in samples.iter().enumerate() {
            if !value.is_finite() {
                self.finite[ch] = false;
                frame_non_finite = true;
                if self.first_non_finite[ch].is_none() {
                    self.first_non_finite[ch] = Some(Located { frame, value });
                }
                continue;
            }
            if !inside {
                continue;
            }
            let magnitude = value.abs();
            if magnitude > self.peak[ch] {
                self.peak[ch] = magnitude;
                self.peak_at[ch] = Some(frame);
            }
            if let Some(limit) = self.limit
                && magnitude > limit
                && self.first_above[ch].is_none()
            {
                self.first_above[ch] = Some(Located { frame, value });
            }
            self.sum_sq[ch] = value.mul_add(value, self.sum_sq[ch]);
            self.sum[ch] += value;
            // exact in single precision: the sample was an `f32`
            let subnormal = if self.single {
                (value as f32).is_subnormal()
            } else {
                value.is_subnormal()
            };
            if subnormal {
                self.subnormal[ch] += 1;
                self.subnormal_at[ch].get_or_insert(frame);
            }
        }
        if frame_non_finite {
            self.non_finite_frames += 1;
        }
        if inside {
            self.counted += 1;
        }
    }

    pub(crate) fn finish(self) -> RenderStats {
        let n = self.counted.max(1) as f64;
        let channels = (0..self.peak.len())
            .map(|ch| ChannelStats {
                peak: self.peak[ch],
                rms: (self.sum_sq[ch] / n).sqrt(),
                dc: self.sum[ch] / n,
                finite: self.finite[ch],
                peak_at: self.peak_at[ch],
                first_non_finite: self.first_non_finite[ch],
                first_above: self.first_above[ch],
                subnormal: self.subnormal[ch],
                subnormal_at: self.subnormal_at[ch],
            })
            .collect();
        RenderStats {
            window_start: self.start,
            window_len: self.counted,
            channels,
            non_finite_frames: self.non_finite_frames,
            timing: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_excites_frame_zero_on_every_channel() {
        let m = InputMode::Impulse;
        assert!((m.sample(0, 0, 44100.0) - 1.0).abs() < f64::EPSILON);
        assert!((m.sample(1, 0, 44100.0) - 1.0).abs() < f64::EPSILON);
        assert!(m.sample(0, 1, 44100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn weighted_impulses_land_on_their_frames_and_their_channel() {
        let m = InputMode::Impulses {
            channel: Some(1),
            taps: vec![(0, 1.0), (3, -0.5)],
        };
        assert!((m.sample(1, 0, 44100.0) - 1.0).abs() < f64::EPSILON);
        assert!((m.sample(1, 3, 44100.0) + 0.5).abs() < f64::EPSILON);
        assert!(m.sample(1, 1, 44100.0).abs() < f64::EPSILON);
        assert!(m.sample(0, 0, 44100.0).abs() < f64::EPSILON);
        // on every channel when none is named, as `Impulse` does
        let all = InputMode::Impulses {
            channel: None,
            taps: vec![(2, 0.25)],
        };
        assert!((all.sample(0, 2, 44100.0) - 0.25).abs() < f64::EPSILON);
        assert!((all.sample(5, 2, 44100.0) - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn channel_impulse_excites_one_channel_only() {
        // The property the reference protocol cannot express, and the reason
        // a ping-pong delay is untestable with it.
        let m = InputMode::ImpulseChannel(0);
        assert!((m.sample(0, 0, 44100.0) - 1.0).abs() < f64::EPSILON);
        assert!(m.sample(1, 0, 44100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn white_noise_is_position_addressed_not_stateful() {
        // Same (seed, channel, frame) must give the same sample regardless of
        // how the render was blocked.
        let m = InputMode::White { seed: 7 };
        assert!((m.sample(1, 500, 44100.0) - m.sample(1, 500, 44100.0)).abs() < f64::EPSILON);
        assert!((m.sample(1, 500, 44100.0) - m.sample(1, 501, 44100.0)).abs() > f64::EPSILON);
    }

    #[test]
    fn white_noise_stays_in_range() {
        let m = InputMode::White { seed: 1 };
        for frame in 0..2000 {
            let v = m.sample(0, frame, 44100.0);
            assert!((-1.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn sine_completes_one_cycle_per_period() {
        let m = InputMode::Sine { hz: 1.0 };
        assert!(m.sample(0, 0, 4.0).abs() < 1e-12);
        assert!((m.sample(0, 1, 4.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn statistics_exclude_frames_before_the_window() {
        let mut acc = StatsAccumulator::new(1, 2);
        acc.push(0, &[10.0]); // transient, excluded
        acc.push(1, &[10.0]); // transient, excluded
        acc.push(2, &[1.0]);
        acc.push(3, &[1.0]);
        let stats = acc.finish();
        assert_eq!(stats.window_len, 2);
        assert!((stats.channels[0].peak - 1.0).abs() < f64::EPSILON);
        assert!((stats.channels[0].rms - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn non_finite_is_reported_even_outside_the_window() {
        // A NaN in the transient still invalidates the render: it means the
        // DSP diverged, whether or not it recovered inside the window.
        let mut acc = StatsAccumulator::new(1, 10);
        acc.push(0, &[f64::NAN]);
        acc.push(10, &[0.0]);
        assert!(!acc.finish().all_finite());
    }

    #[test]
    fn the_first_non_finite_sample_is_located_and_the_frames_counted() {
        let mut acc = StatsAccumulator::new(2, 0);
        acc.push(0, &[0.0, 0.0]);
        acc.push(1, &[0.0, f64::INFINITY]);
        acc.push(2, &[f64::NAN, f64::NAN]);
        acc.push(3, &[0.0, 0.0]);
        let stats = acc.finish();
        // the earliest over the channels, with its channel and its value
        let (channel, located) = stats.first_non_finite().unwrap();
        assert_eq!((channel, located.frame), (1, 1));
        assert!(located.value.is_infinite());
        // and each channel's own first
        assert_eq!(stats.channels[0].first_non_finite.unwrap().frame, 2);
        assert_eq!(stats.non_finite_frames, 2);
    }

    #[test]
    fn the_peak_is_located_at_its_first_occurrence() {
        let mut acc = StatsAccumulator::new(1, 1);
        for (frame, value) in [9.0, 0.5, -2.0, 2.0, 1.0].into_iter().enumerate() {
            acc.push(frame, &[value]);
        }
        let stats = acc.finish();
        // frame 0 is before the window; -2 at frame 2 comes before +2 at frame 3
        assert_eq!(stats.channels[0].peak_at, Some(2));
        assert!((stats.channels[0].peak - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_limit_locates_the_first_sample_above_it_inside_the_window() {
        let mut acc = StatsAccumulator::with_limit(1, 2, Some(1.0));
        for (frame, value) in [5.0, 0.0, 1.0, -1.5, 3.0].into_iter().enumerate() {
            acc.push(frame, &[value]);
        }
        let stats = acc.finish();
        // 5.0 is before the window, 1.0 is not above 1.0, -1.5 is
        let (channel, located) = stats.first_above().unwrap();
        assert_eq!((channel, located.frame), (0, 3));
        assert!((located.value + 1.5).abs() < f64::EPSILON);
        // without a limit nothing is ever above it
        let mut free = StatsAccumulator::new(1, 0);
        free.push(0, &[1e30]);
        assert!(free.finish().first_above().is_none());
    }

    #[test]
    fn silence_is_exact_zero_over_a_non_empty_window() {
        let mut silent = StatsAccumulator::new(2, 0);
        silent.push(0, &[0.0, -0.0]);
        let silent = silent.finish();
        assert!(silent.is_silent());
        assert_eq!(silent.channels[0].peak_at, None);

        let mut quiet = StatsAccumulator::new(2, 0);
        quiet.push(0, &[0.0, 1e-300]);
        assert!(!quiet.finish().is_silent());

        // an empty window says nothing about the program
        assert!(!StatsAccumulator::new(1, 5).finish().is_silent());
    }

    #[test]
    fn subnormals_are_counted_at_the_width_of_the_program() {
        // 1e-39 is below the smallest normal `f32` (1.17e-38) and a perfectly
        // normal `f64`; 1e-310 is subnormal in both.
        let samples = [1.0, 1e-39, 0.0, -1e-39, 1e-310];
        let mut single = StatsAccumulator::new(1, 1).at_width(false);
        let mut double = StatsAccumulator::new(1, 1).at_width(true);
        // frame 0 is before the window: not counted
        single.push(0, &[1e-39]);
        double.push(0, &[1e-310]);
        for (frame, value) in samples.into_iter().enumerate() {
            single.push(frame + 1, &[value]);
            double.push(frame + 1, &[value]);
        }
        let (single, double) = (single.finish(), double.finish());
        // 1e-310 as an `f32` is zero, and zero is not subnormal
        assert_eq!(single.channels[0].subnormal, 2);
        assert_eq!(single.channels[0].subnormal_at, Some(2));
        assert_eq!(double.channels[0].subnormal, 1);
        assert_eq!(double.channels[0].subnormal_at, Some(5));
    }

    #[test]
    fn dc_detects_an_offset() {
        let mut acc = StatsAccumulator::new(1, 0);
        for f in 0..100 {
            acc.push(f, &[0.5]);
        }
        assert!((acc.finish().channels[0].dc - 0.5).abs() < 1e-12);
    }
}
