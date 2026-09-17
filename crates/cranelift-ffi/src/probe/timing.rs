//! What a render cost: the time spent in `compute`, against real time.
//!
//! A compiler change that doubles the cost of a program shows in no sample,
//! and was until now measured by hand, with a stopwatch around the whole
//! command: compilation, rendering and the printing of fifteen thousand rows
//! added up. What is timed here is the `compute` call and nothing else, block
//! by block, because a host has a deadline per block: the mean says whether
//! the program runs in real time, the worst block whether it would have
//! clicked.
//!
//! The numbers differ from one run to the next, which is why they are behind
//! `--time` and never in the default output: that output being the same twice
//! is one of the tool's validation criteria.
//!
//! FFI-free, so that the arithmetic is unit-tested without a clock.

use std::time::Duration;

/// The block that came closest to, or went furthest beyond, its deadline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorstBlock {
    /// Absolute frame the block starts at.
    pub frame: usize,
    /// Frames of the block: the last block of a render, and one shortened by
    /// a scheduled event, are smaller than `--block`, and so is their budget.
    pub frames: usize,
    /// Time its `compute` call took.
    pub seconds: f64,
}

/// The cost of the `compute` calls of one render.
#[derive(Debug, Clone, PartialEq)]
pub struct Timing {
    /// Sample rate the budgets are counted at.
    pub sample_rate: f64,
    /// Frames computed.
    pub frames: usize,
    /// `compute` calls.
    pub blocks: usize,
    /// Time spent in them, and in nothing else.
    pub compute_seconds: f64,
    /// The block with the largest share of its budget; `None` when nothing
    /// was computed.
    pub worst: Option<WorstBlock>,
}

impl Timing {
    /// The duration of the audio that was computed.
    #[must_use]
    pub fn audio_seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate
    }

    /// How many times faster than real time the program ran: the audio's
    /// duration over the time it took to compute. Below 1 it cannot run live.
    #[must_use]
    pub fn realtime_factor(&self) -> f64 {
        self.audio_seconds() / self.compute_seconds
    }

    /// The time a host leaves a block of `frames` frames.
    #[must_use]
    pub fn budget_seconds(&self, frames: usize) -> f64 {
        frames as f64 / self.sample_rate
    }

    /// The worst block's time as a fraction of its budget.
    #[must_use]
    pub fn worst_budget_fraction(&self) -> Option<f64> {
        self.worst
            .map(|worst| worst.seconds / self.budget_seconds(worst.frames))
    }

    /// Adds another render of the same program (the next point of a sweep).
    pub fn absorb(&mut self, other: &Self) {
        let mine = self.worst_budget_fraction();
        let theirs = other.worst_budget_fraction();
        if theirs > mine {
            self.worst = other.worst;
        }
        self.frames += other.frames;
        self.blocks += other.blocks;
        self.compute_seconds += other.compute_seconds;
    }
}

/// Accumulates the blocks of a render.
#[derive(Debug)]
pub struct BlockTimer {
    timing: Timing,
    worst_fraction: f64,
}

impl BlockTimer {
    /// A timer for a program running at `sample_rate`.
    #[must_use]
    pub const fn new(sample_rate: f64) -> Self {
        Self {
            timing: Timing {
                sample_rate,
                frames: 0,
                blocks: 0,
                compute_seconds: 0.0,
                worst: None,
            },
            worst_fraction: f64::NEG_INFINITY,
        }
    }

    /// Records the `compute` call of the block of `frames` frames starting at
    /// `frame`. The first of two equally bad blocks is kept.
    pub fn record(&mut self, frame: usize, frames: usize, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        self.timing.frames += frames;
        self.timing.blocks += 1;
        self.timing.compute_seconds += seconds;
        if frames == 0 {
            return;
        }
        // against its own budget: a block of one frame that takes as long as
        // a block of sixty-four is sixty-four times closer to its deadline
        let fraction = seconds / self.timing.budget_seconds(frames);
        if fraction > self.worst_fraction {
            self.worst_fraction = fraction;
            self.timing.worst = Some(WorstBlock {
                frame,
                frames,
                seconds,
            });
        }
    }

    /// The cost of what was recorded.
    #[must_use]
    pub fn finish(self) -> Timing {
        self.timing
    }
}

/// A duration for a person: three significant digits and the unit that fits.
#[must_use]
pub fn human_seconds(seconds: f64) -> String {
    let (scaled, unit) = if !seconds.is_finite() {
        return format!("{seconds} s");
    } else if seconds >= 1.0 {
        (seconds, "s")
    } else if seconds >= 1e-3 {
        (seconds * 1e3, "ms")
    } else if seconds >= 1e-6 {
        (seconds * 1e6, "us")
    } else {
        (seconds * 1e9, "ns")
    };
    let decimals = if scaled >= 100.0 {
        0
    } else if scaled >= 10.0 {
        1
    } else {
        2
    };
    format!("{scaled:.decimals$} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn micros(n: u64) -> Duration {
        Duration::from_micros(n)
    }

    #[test]
    fn the_compute_time_is_the_sum_of_the_blocks() {
        let mut timer = BlockTimer::new(1000.0);
        timer.record(0, 100, micros(500));
        timer.record(100, 100, micros(300));
        timer.record(200, 50, micros(200));
        let timing = timer.finish();
        assert_eq!((timing.frames, timing.blocks), (250, 3));
        assert!((timing.compute_seconds - 1e-3).abs() < 1e-12);
        // 0.25 s of audio in 1 ms
        assert!((timing.audio_seconds() - 0.25).abs() < 1e-12);
        assert!((timing.realtime_factor() - 250.0).abs() < 1e-6);
    }

    #[test]
    fn the_worst_block_is_the_worst_against_its_own_budget() {
        let mut timer = BlockTimer::new(1000.0);
        timer.record(0, 100, micros(500)); // 0.5% of 100 ms
        timer.record(100, 10, micros(400)); // 4% of 10 ms: shorter, and worse
        timer.record(110, 100, micros(450));
        let timing = timer.finish();
        let worst = timing.worst.expect("a worst block");
        assert_eq!((worst.frame, worst.frames), (100, 10));
        assert!((timing.worst_budget_fraction().unwrap() - 0.04).abs() < 1e-9);
        assert!((timing.budget_seconds(worst.frames) - 0.01).abs() < 1e-12);
    }

    #[test]
    fn a_tie_keeps_the_first_block_and_nothing_computed_has_no_worst() {
        let mut timer = BlockTimer::new(1000.0);
        timer.record(0, 100, micros(500));
        timer.record(100, 100, micros(500));
        assert_eq!(timer.finish().worst.unwrap().frame, 0);
        assert_eq!(BlockTimer::new(1000.0).finish().worst, None);
    }

    #[test]
    fn a_sweep_adds_its_renders_and_keeps_the_worst_of_them() {
        let mut first = BlockTimer::new(1000.0);
        first.record(0, 100, micros(500));
        let mut total = first.finish();
        let mut second = BlockTimer::new(1000.0);
        second.record(0, 100, micros(900));
        total.absorb(&second.finish());
        assert_eq!((total.frames, total.blocks), (200, 2));
        assert!((total.compute_seconds - 1.4e-3).abs() < 1e-12);
        assert!((total.worst.unwrap().seconds - 9e-4).abs() < 1e-12);
        // and an empty render changes nothing
        total.absorb(&BlockTimer::new(1000.0).finish());
        assert!((total.worst.unwrap().seconds - 9e-4).abs() < 1e-12);
    }

    #[test]
    fn durations_read_in_the_unit_that_fits() {
        assert_eq!(human_seconds(2.5), "2.50 s");
        assert_eq!(human_seconds(0.412), "412 ms");
        assert_eq!(human_seconds(0.0123), "12.3 ms");
        assert_eq!(human_seconds(41.2e-6), "41.2 us");
        assert_eq!(human_seconds(3e-9), "3.00 ns");
    }
}
