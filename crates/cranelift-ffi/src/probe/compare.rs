//! Comparing two renders (`--compare`, `--ref`, `--check`).
//!
//! The question "did this change a sample?" was answered in Python about ten
//! times in one sibling project: a refactoring that must not change the sound,
//! `fad` against `rad`, a preset against the adjustable program set to its
//! values. Each time two renders, two parses and the maximum of a difference,
//! which is also the least informative answer: **the first frame that
//! differs** says whether two programs part at the onset, at a control event,
//! or slowly, and the maximum does not.
//!
//! Like [`crate::probe::render`], this module is free of FFI: it compares
//! samples, whoever rendered them.

/// The samples of one render: `channels[ch][k]` is output `ch` at the `k`-th
/// frame of the window, whose first frame is `start`.
#[derive(Debug, Clone, PartialEq)]
pub struct Samples {
    /// Absolute index of the window's first frame (`--skip`).
    pub start: usize,
    /// One vector per output, all of the same length.
    pub channels: Vec<Vec<f64>>,
}

impl Samples {
    /// Frames in the window.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
}

/// How far two samples may be apart and still agree.
///
/// A pair agrees when `|a - b| <= abs + rel * peak`, `peak` being the largest
/// magnitude of the **reference** on that output. The default is zero on both
/// counts, and then agreement is bit equality: what a refactoring, a change of
/// block size or a second compilation must give. A non-finite sample agrees
/// only with the very same bits.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tolerance {
    /// Absolute part.
    pub abs: f64,
    /// Part relative to the reference's peak on the output.
    pub rel: f64,
}

/// A pair of samples that does not agree, and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Disagreement {
    /// Absolute frame index.
    pub frame: usize,
    /// The sample of the render under test.
    pub value: f64,
    /// The sample of the reference.
    pub reference: f64,
}

/// One output of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelDiff {
    /// Whether every pair of samples has the same bits.
    pub identical: bool,
    /// Largest `|a - b|` over the finite pairs.
    pub max_abs: f64,
    /// Absolute frame of the first pair at that distance.
    pub max_abs_at: Option<usize>,
    /// `max_abs` over the reference's peak on this output: infinite when the
    /// reference is silent and the render is not.
    pub max_rel: f64,
    /// The first pair beyond the tolerance.
    pub first_beyond: Option<Disagreement>,
}

/// A render compared with a reference, output by output.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// One entry per compared output, with its index.
    pub channels: Vec<(usize, ChannelDiff)>,
}

impl Comparison {
    /// Whether every compared output is within the tolerance.
    #[must_use]
    pub fn agrees(&self) -> bool {
        self.channels.iter().all(|(_, c)| c.first_beyond.is_none())
    }

    /// Whether every compared output has the reference's very bits.
    #[must_use]
    pub fn identical(&self) -> bool {
        self.channels.iter().all(|(_, c)| c.identical)
    }

    /// The earliest disagreement and its output: the lowest output wins a tie.
    #[must_use]
    pub fn first_beyond(&self) -> Option<(usize, Disagreement)> {
        self.channels
            .iter()
            .filter_map(|(ch, c)| c.first_beyond.map(|d| (*ch, d)))
            .min_by_key(|(ch, d)| (d.frame, *ch))
    }
}

/// Compares `render` with `reference` on `outputs` (every output when `None`).
///
/// # Errors
/// Renders that cannot be compared: different numbers of outputs, windows of
/// different lengths or starts, an output index neither has. A comparison that
/// silently covered only what the two have in common would call two different
/// renders equal.
pub fn compare(
    render: &Samples,
    reference: &Samples,
    tolerance: Tolerance,
    outputs: Option<&[usize]>,
) -> Result<Comparison, String> {
    if render.channels.len() != reference.channels.len() {
        return Err(format!(
            "the render has {} output(s) and the reference {}",
            render.channels.len(),
            reference.channels.len()
        ));
    }
    if render.frames() != reference.frames() {
        return Err(format!(
            "the render's window holds {} frames and the reference {}",
            render.frames(),
            reference.frames()
        ));
    }
    if render.start != reference.start {
        return Err(format!(
            "the render's window starts at frame {} and the reference's at {}",
            render.start, reference.start
        ));
    }
    let all: Vec<usize> = (0..render.channels.len()).collect();
    let outputs = outputs.unwrap_or(&all);
    let mut channels = Vec::with_capacity(outputs.len());
    for &ch in outputs {
        let (Some(a), Some(b)) = (render.channels.get(ch), reference.channels.get(ch)) else {
            return Err(format!(
                "output {ch} does not exist: the renders have {} output(s)",
                render.channels.len()
            ));
        };
        channels.push((ch, compare_channel(a, b, render.start, tolerance)));
    }
    Ok(Comparison { channels })
}

fn compare_channel(a: &[f64], b: &[f64], start: usize, tolerance: Tolerance) -> ChannelDiff {
    let peak = b
        .iter()
        .filter(|v| v.is_finite())
        .fold(0.0_f64, |peak, v| peak.max(v.abs()));
    let limit = tolerance.abs + tolerance.rel * peak;
    let mut diff = ChannelDiff {
        identical: true,
        max_abs: 0.0,
        max_abs_at: None,
        max_rel: 0.0,
        first_beyond: None,
    };
    for (k, (&value, &reference)) in a.iter().zip(b).enumerate() {
        if value.to_bits() == reference.to_bits() {
            continue;
        }
        diff.identical = false;
        let distance = (value - reference).abs();
        if distance > diff.max_abs {
            diff.max_abs = distance;
            diff.max_abs_at = Some(start + k);
        }
        // a NaN distance, from a non-finite sample whose bits differ, is
        // beyond any tolerance: `distance > limit` alone would let it through
        let beyond = distance.is_nan() || distance > limit;
        if beyond && diff.first_beyond.is_none() {
            diff.first_beyond = Some(Disagreement {
                frame: start + k,
                value,
                reference,
            });
        }
    }
    diff.max_rel = if diff.max_abs == 0.0 {
        0.0
    } else if peak > 0.0 {
        diff.max_abs / peak
    } else {
        f64::INFINITY
    };
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(start: usize, channels: &[&[f64]]) -> Samples {
        Samples {
            start,
            channels: channels.iter().map(|c| c.to_vec()).collect(),
        }
    }

    #[test]
    fn identical_renders_are_identical_and_agree() {
        let a = samples(0, &[&[0.0, 0.5, -1.0], &[1.0, 1.0, 1.0]]);
        let comparison = compare(&a, &a.clone(), Tolerance::default(), None).unwrap();
        assert!(comparison.identical() && comparison.agrees());
        assert_eq!(comparison.channels[1].1.max_abs_at, None);
    }

    #[test]
    fn the_first_frame_that_differs_is_found_not_only_the_largest_gap() {
        // the window starts at frame 10; the renders part at its third frame,
        // and are furthest apart at its fifth
        let a = samples(10, &[&[1.0, 1.0, 1.001, 1.0, 1.5]]);
        let b = samples(10, &[&[1.0, 1.0, 1.0, 1.0, 1.0]]);
        let comparison = compare(&a, &b, Tolerance::default(), None).unwrap();
        let diff = &comparison.channels[0].1;
        assert!(!diff.identical);
        assert_eq!(diff.first_beyond.unwrap().frame, 12);
        assert!((diff.first_beyond.unwrap().value - 1.001).abs() < 1e-15);
        assert_eq!(diff.max_abs_at, Some(14));
        assert!((diff.max_abs - 0.5).abs() < 1e-15);
        // relative to the reference's peak, which is 1
        assert!((diff.max_rel - 0.5).abs() < 1e-15);
    }

    #[test]
    fn a_tolerance_is_absolute_plus_relative_to_the_reference_peak() {
        let a = samples(0, &[&[2.0, 0.003, 0.02]]);
        let b = samples(0, &[&[2.0, 0.0, 0.0]]);
        // peak 2: abs 0.001 + rel 0.001 * 2 = 0.003; 0.003 is within, 0.02 is not
        let tolerance = Tolerance {
            abs: 0.001,
            rel: 0.001,
        };
        let diff = &compare(&a, &b, tolerance, None).unwrap().channels[0].1;
        assert!(!diff.identical);
        assert_eq!(diff.first_beyond.unwrap().frame, 2);
        let loose = Tolerance {
            abs: 0.05,
            rel: 0.0,
        };
        assert!(compare(&a, &b, loose, None).unwrap().agrees());
    }

    #[test]
    fn a_non_finite_sample_agrees_only_with_the_same_bits() {
        let a = samples(0, &[&[f64::NAN, 1.0, f64::INFINITY]]);
        assert!(
            compare(&a, &a.clone(), Tolerance::default(), None)
                .unwrap()
                .identical()
        );
        let b = samples(0, &[&[0.0, 1.0, f64::INFINITY]]);
        // however loose the tolerance
        let loose = Tolerance {
            abs: 1e300,
            rel: 0.0,
        };
        let comparison = compare(&a, &b, loose, None).unwrap();
        assert_eq!(comparison.first_beyond().unwrap().1.frame, 0);
    }

    #[test]
    fn the_earliest_disagreement_is_reported_with_its_output() {
        let a = samples(0, &[&[0.0, 0.0, 9.0], &[0.0, 7.0, 0.0], &[0.0, 8.0, 0.0]]);
        let b = samples(0, &[&[0.0; 3], &[0.0; 3], &[0.0; 3]]);
        let comparison = compare(&a, &b, Tolerance::default(), None).unwrap();
        // outputs 1 and 2 part at frame 1: the lower output is named
        assert_eq!(
            comparison.first_beyond().map(|(ch, d)| (ch, d.frame)),
            Some((1, 1))
        );
        // a silent reference has no peak to be relative to
        assert!(comparison.channels[0].1.max_rel.is_infinite());
    }

    #[test]
    fn only_the_outputs_asked_for_are_compared() {
        let a = samples(0, &[&[1.0], &[2.0]]);
        let b = samples(0, &[&[1.0], &[3.0]]);
        let first = compare(&a, &b, Tolerance::default(), Some(&[0])).unwrap();
        assert!(first.agrees());
        assert_eq!(first.channels.len(), 1);
        assert!(compare(&a, &b, Tolerance::default(), Some(&[2])).is_err());
    }

    #[test]
    fn renders_that_cannot_be_compared_are_refused() {
        let a = samples(0, &[&[1.0, 2.0]]);
        let error = |b: &Samples| compare(&a, b, Tolerance::default(), None).unwrap_err();
        assert!(error(&samples(0, &[&[1.0, 2.0], &[0.0, 0.0]])).contains("output(s)"));
        assert!(error(&samples(0, &[&[1.0]])).contains("frames"));
        assert!(error(&samples(4, &[&[1.0, 2.0]])).contains("starts at frame"));
    }
}
