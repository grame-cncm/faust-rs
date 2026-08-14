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
    /// Constant 1.0 on every channel.
    Dc,
    /// Uniform noise in `[-1, 1)` from a seeded generator.
    ///
    /// Seeded so a run is reproducible: an unseeded probe cannot be used as a
    /// regression baseline.
    White { seed: u64 },
    /// Full-scale sine at the given frequency on every channel.
    Sine { hz: f64 },
}

impl InputMode {
    /// Sample for `channel` at absolute `frame`.
    #[must_use]
    pub fn sample(&self, channel: usize, frame: usize, sample_rate: f64) -> f64 {
        match *self {
            Self::Zero => 0.0,
            Self::Impulse => f64::from(u8::from(frame == 0)),
            Self::ImpulseChannel(ch) => f64::from(u8::from(frame == 0 && channel == ch)),
            Self::Dc => 1.0,
            Self::White { seed } => white(seed, channel, frame),
            Self::Sine { hz } => (std::f64::consts::TAU * hz * frame as f64 / sample_rate).sin(),
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
}

/// Statistics for a whole render, with the window they describe.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderStats {
    /// First frame included in the statistics.
    pub window_start: usize,
    /// Number of frames included.
    pub window_len: usize,
    /// One entry per output channel.
    pub channels: Vec<ChannelStats>,
}

impl RenderStats {
    /// Whether every channel stayed finite.
    #[must_use]
    pub fn all_finite(&self) -> bool {
        self.channels.iter().all(|c| c.finite)
    }
}

/// Accumulates statistics over the measured window.
#[derive(Debug)]
pub(crate) struct StatsAccumulator {
    peak: Vec<f64>,
    sum_sq: Vec<f64>,
    sum: Vec<f64>,
    finite: Vec<bool>,
    counted: usize,
    start: usize,
}

impl StatsAccumulator {
    pub(crate) fn new(channels: usize, start: usize) -> Self {
        Self {
            peak: vec![0.0; channels],
            sum_sq: vec![0.0; channels],
            sum: vec![0.0; channels],
            finite: vec![true; channels],
            counted: 0,
            start,
        }
    }

    /// Record one frame. `frame` is absolute; frames before the window start
    /// are still checked for finiteness but excluded from the statistics.
    pub(crate) fn push(&mut self, frame: usize, samples: &[f64]) {
        let inside = frame >= self.start;
        for (ch, &value) in samples.iter().enumerate() {
            if !value.is_finite() {
                self.finite[ch] = false;
                continue;
            }
            if !inside {
                continue;
            }
            let magnitude = value.abs();
            if magnitude > self.peak[ch] {
                self.peak[ch] = magnitude;
            }
            self.sum_sq[ch] = value.mul_add(value, self.sum_sq[ch]);
            self.sum[ch] += value;
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
            })
            .collect();
        RenderStats {
            window_start: self.start,
            window_len: self.counted,
            channels,
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
    fn dc_detects_an_offset() {
        let mut acc = StatsAccumulator::new(1, 0);
        for f in 0..100 {
            acc.push(f, &[0.5]);
        }
        assert!((acc.finish().channels[0].dc - 0.5).abs() < 1e-12);
    }
}
