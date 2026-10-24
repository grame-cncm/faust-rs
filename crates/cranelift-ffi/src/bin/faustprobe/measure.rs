//! What a render is measured by, beyond its statistics: `--reduce` (one
//! number per output) and `--describe` (what it sounds like). Shared by the
//! scalar and the polyphonic renders, so that the two say the same of the
//! same samples.

use cranelift_ffi::probe::describe::{
    DescribeSettings, Descriptor, check, describe, parse_descriptors,
};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::render::RenderStats;
use cranelift_ffi::probe::spectrum::{dominant_frequency, sfdr_db, thd_db};
use cranelift_ffi::probe::sweep::{Reduction, parse_reduction};

use crate::cli::Args;
use crate::report::json_number;

/// `--reduce` and `--describe`, validated before any render.
pub(crate) struct Measures {
    pub(crate) reduction: Option<Reduction>,
    descriptors: Vec<Descriptor>,
    settings: DescribeSettings,
    f0: Option<f64>,
    sr: f64,
}

/// One render, measured.
pub(crate) struct Measured {
    /// The reduction of each output, under `--reduce`.
    reduced: Option<Vec<f64>>,
    /// The descriptors' values, in their order.
    described: Vec<f64>,
}

impl Measures {
    /// Parses `--reduce` and `--describe`, and checks the descriptors against
    /// a program of `outputs` outputs rendering a window of `window` frames.
    pub(crate) fn prepare(
        args: &Args,
        outputs: usize,
        window: usize,
        sample_rate: i32,
    ) -> Result<Self, String> {
        let reduction = args.reduce.as_deref().map(parse_reduction).transpose()?;
        let settings = DescribeSettings {
            hop_seconds: args.describe_hop,
            fft: args.describe_fft,
        };
        let sr = f64::from(sample_rate);
        let descriptors = match args.describe.as_deref() {
            Some(list) => {
                let descriptors = parse_descriptors(list)?;
                check(&descriptors, settings, outputs, window, sr)?;
                descriptors
            }
            None => Vec::new(),
        };
        Ok(Self {
            reduction,
            descriptors,
            settings,
            f0: args.f0,
            sr,
        })
    }

    /// Whether a render must keep its window's samples.
    pub(crate) fn needs_samples(&self) -> bool {
        !self.descriptors.is_empty()
            || matches!(
                self.reduction,
                Some(Reduction::F0 | Reduction::Sfdr | Reduction::Thd)
            )
    }

    /// The columns of a sweep's row after the swept controls: per output the
    /// reduction, or without one (and without descriptors) the three
    /// statistics; then one column per descriptor.
    pub(crate) fn header(&self, outputs: usize) -> Vec<String> {
        let mut header = Vec::new();
        for ch in 0..outputs {
            match self.reduction {
                Some(r) => header.push(format!("{r}_out{ch}")),
                None if self.descriptors.is_empty() => {
                    header.push(format!("peak_out{ch}"));
                    header.push(format!("rms_out{ch}"));
                    header.push(format!("dc_out{ch}"));
                }
                None => {}
            }
        }
        header.extend(self.descriptors.iter().map(ToString::to_string));
        header
    }

    /// Measures one render: `collected` holds its window's samples when
    /// [`Measures::needs_samples`].
    pub(crate) fn measure(&self, stats: &RenderStats, collected: &[Vec<f64>]) -> Measured {
        let reduced = self.reduction.map(|r| {
            (0..stats.channels.len())
                .map(|ch| reduce_channel(r, stats, ch, collected, self.sr, self.f0))
                .collect()
        });
        let described = if self.descriptors.is_empty() {
            Vec::new()
        } else {
            describe(collected, self.sr, &self.descriptors, self.settings)
        };
        Measured { reduced, described }
    }

    /// The columns of a sweep's row after the swept controls, as
    /// [`Measures::header`] names them.
    pub(crate) fn row(
        &self,
        measured: &Measured,
        stats: &RenderStats,
        fmt: &NumberFormat,
    ) -> Vec<String> {
        let mut row = Vec::new();
        match (&measured.reduced, self.reduction) {
            (Some(values), Some(r)) => {
                // a peak is a sample's magnitude; the others are computed
                row.extend(values.iter().map(|value| {
                    if r == Reduction::Peak {
                        fmt.sample(*value)
                    } else {
                        fmt.computed(*value)
                    }
                }));
            }
            _ if self.descriptors.is_empty() => {
                for channel in &stats.channels {
                    row.push(fmt.sample(channel.peak));
                    row.push(fmt.computed(channel.rms));
                    row.push(fmt.computed(channel.dc));
                }
            }
            _ => {}
        }
        row.extend(measured.described.iter().map(|v| fmt.computed(*v)));
        row
    }

    /// What a render's JSON entry says of the reduction (`"rms": [...]`), or
    /// `None` without one; the caller then gives the full statistics.
    pub(crate) fn reduced_json(&self, measured: &Measured) -> Option<(String, serde_json::Value)> {
        let (r, values) = self.reduction.zip(measured.reduced.as_ref())?;
        let values = values.iter().map(|v| json_number(*v)).collect();
        Some((r.to_string(), serde_json::Value::Array(values)))
    }

    /// The `describe` object of a render's JSON entry, a non-finite value
    /// being `null`; `None` without `--describe`.
    pub(crate) fn described_json(&self, measured: &Measured) -> Option<serde_json::Value> {
        if self.descriptors.is_empty() {
            return None;
        }
        let mut object = serde_json::Map::new();
        for (descriptor, value) in self.descriptors.iter().zip(&measured.described) {
            object.insert(descriptor.to_string(), json_number(*value));
        }
        Some(serde_json::Value::Object(object))
    }

    /// The `# describe NAME=VALUE ...` line of a single render, and the notes
    /// that say why a value is `NaN`.
    pub(crate) fn lines(&self, measured: &Measured, fmt: &NumberFormat) -> Vec<String> {
        if self.descriptors.is_empty() {
            return Vec::new();
        }
        let pairs: Vec<String> = self
            .descriptors
            .iter()
            .zip(&measured.described)
            .map(|(descriptor, value)| format!("{descriptor}={}", fmt.computed(*value)))
            .collect();
        let mut lines = vec![format!("# describe {}", pairs.join(" "))];
        for (descriptor, value) in self.descriptors.iter().zip(&measured.described) {
            if value.is_nan() {
                lines.push(format!(
                    "# note: {descriptor} is NaN: {}",
                    why_nan(*descriptor)
                ));
            }
        }
        lines
    }

    /// After a sweep's rows: for each descriptor that was `NaN` somewhere, at
    /// how many of the points and why.
    pub(crate) fn sweep_notes(&self, all: &[Measured]) -> Vec<String> {
        let mut notes = Vec::new();
        for (index, descriptor) in self.descriptors.iter().enumerate() {
            let count = all.iter().filter(|m| m.described[index].is_nan()).count();
            if count > 0 {
                notes.push(format!(
                    "# note: {descriptor} is NaN at {count} of {} point(s): {}",
                    all.len(),
                    why_nan(*descriptor)
                ));
            }
        }
        notes
    }
}

/// Why a descriptor has no value.
fn why_nan(descriptor: Descriptor) -> &'static str {
    match descriptor {
        Descriptor::Decay(_) => "not reached before the window ends (or the window is silent)",
        Descriptor::Centroid(_) => "its stretch holds no energy above the second bin",
        Descriptor::Width(_) => "both outputs are silent over its stretch",
        Descriptor::Loudness | Descriptor::Attack | Descriptor::Sustain(_) => {
            "the window is silent"
        }
    }
}

/// One channel of a rendered window, reduced to a single number.
///
/// Shared by the JSON and CSV paths so the two cannot report different
/// numbers for the same render.
fn reduce_channel(
    r: Reduction,
    stats: &RenderStats,
    ch: usize,
    collected: &[Vec<f64>],
    sr: f64,
    f0: Option<f64>,
) -> f64 {
    match r {
        Reduction::Rms => stats.channels[ch].rms,
        Reduction::Peak => stats.channels[ch].peak,
        Reduction::Energy => stats.channels[ch].rms.powi(2) * stats.window_len as f64,
        Reduction::Dc => stats.channels[ch].dc,
        Reduction::F0 => dominant_frequency(&collected[ch], sr),
        Reduction::Sfdr => sfdr_db(&collected[ch], sr, f0.unwrap_or(0.0)),
        Reduction::Thd => thd_db(&collected[ch], sr, f0.unwrap_or(0.0)),
    }
}
