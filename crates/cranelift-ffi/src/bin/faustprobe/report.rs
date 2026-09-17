//! How results are said: the lines that go with the numbers, their JSON
//! counterparts, the account of `--time`, and where all of it is written.
//!
//! Every mode says the same things of a render (a channel's statistics, a
//! clamp, a note, the cost): they are written once, here, so that two modes
//! cannot word one fact in two ways.

use cranelift_ffi::probe::engine::Probe;
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::params::{ControlKind, ControlMap};
use cranelift_ffi::probe::render::{InputMode, RenderStats};
use cranelift_ffi::probe::timing::{Timing, WorstBlock, human_seconds};

use crate::cli::Args;

/// Where the lines that go with a result are written.
///
/// One rule for every mode: when nothing else owns stdout the annotations are
/// the output, and can be redirected; when a dump or a sweep's rows own it,
/// they annotate it from stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sink {
    /// The annotations are the output.
    Stdout,
    /// Rows own stdout.
    Stderr,
}

impl Sink {
    /// The sink of annotations, given whether rows own stdout.
    pub(crate) const fn beside(rows_own_stdout: bool) -> Self {
        if rows_own_stdout {
            Self::Stderr
        } else {
            Self::Stdout
        }
    }

    /// Writes one line.
    pub(crate) fn say(self, line: impl AsRef<str>) {
        match self {
            Self::Stdout => println!("{}", line.as_ref()),
            Self::Stderr => eprintln!("{}", line.as_ref()),
        }
    }

    /// Writes several.
    pub(crate) fn say_all<L: AsRef<str>>(self, lines: impl IntoIterator<Item = L>) {
        for line in lines {
            self.say(line);
        }
    }
}

/// The table of `--list-params`. `title` heads the path column: a polyphonic
/// instrument lists the controls of one voice.
pub(crate) fn print_controls(title: &str, controls: &ControlMap, fmt: &NumberFormat) {
    println!(
        "{:<44} {:<9} {:>10} {:>10} {:>10} {:>10}",
        title, "kind", "init", "min", "max", "step"
    );
    for control in controls.iter() {
        // a control's numbers live at the program's width: printed through
        // an `f64`, a single-precision step of 0.001 reads
        // 0.0010000000474974513
        println!(
            "{:<44} {:<9} {:>10} {:>10} {:>10} {:>10}",
            control.path,
            control.kind,
            fmt.sample(control.init),
            fmt.sample(control.min),
            fmt.sample(control.max),
            fmt.sample(control.step)
        );
    }
}

/// The `# outN:` line of each channel.
pub(crate) fn channel_lines(stats: &RenderStats, fmt: &NumberFormat) -> Vec<String> {
    stats
        .channels
        .iter()
        .enumerate()
        .map(|(ch, channel)| {
            // `peak_at` comes last: a reader keyed on the older fields does
            // not see it. And `subnormal` only when there is one: it costs CPU
            // on a target that does not flush them, and is where a tail ends.
            let subnormal = channel.subnormal_at.map_or_else(String::new, |frame| {
                format!(" subnormal={} subnormal_at={frame}", channel.subnormal)
            });
            format!(
                "# out{ch}: peak={} rms={} dc={} finite={} peak_at={}{subnormal}",
                fmt.sample(channel.peak),
                fmt.computed(channel.rms),
                fmt.computed(channel.dc),
                if channel.finite { "yes" } else { "no" },
                channel
                    .peak_at
                    .map_or_else(|| "none".to_owned(), |frame| frame.to_string())
            )
        })
        .collect()
}

/// [`channel_lines`] for the JSON documents.
pub(crate) fn channels_json(stats: &RenderStats) -> serde_json::Value {
    serde_json::Value::Array(
        stats
            .channels
            .iter()
            .map(|c| {
                serde_json::json!({
                    "peak": json_number(c.peak),
                    "rms": json_number(c.rms),
                    "dc": json_number(c.dc),
                    "peak_at": c.peak_at,
                    "subnormal": c.subnormal,
                    "subnormal_at": c.subnormal_at,
                })
            })
            .collect(),
    )
}

/// The window the statistics are over, for the JSON documents.
pub(crate) fn window_json(stats: &RenderStats) -> serde_json::Value {
    serde_json::json!({ "start": stats.window_start, "frames": stats.window_len })
}

/// What every JSON document starts with.
pub(crate) fn document(args: &Args) -> serde_json::Map<String, serde_json::Value> {
    let mut document = serde_json::Map::new();
    document.insert("schema_version".to_owned(), serde_json::json!(1));
    document.insert("dsp".to_owned(), serde_json::json!(args.file));
    document.insert("sr".to_owned(), serde_json::json!(args.sr));
    document
}

/// What a document that holds several renders says of `--eval` and of
/// `--time`: what each output computes, and the compilation's cost (each
/// render carries that of its own `compute` calls).
pub(crate) fn describe_program(
    document: &mut serde_json::Map<String, serde_json::Value>,
    args: &Args,
    eval_labels: Option<&[String]>,
    compile_seconds: f64,
) {
    if let Some(labels) = eval_labels {
        document.insert("eval".to_owned(), serde_json::json!(labels));
    }
    if args.time {
        document.insert(
            "timing".to_owned(),
            serde_json::json!({ "compile_s": json_number(compile_seconds) }),
        );
    }
}

/// Prints a JSON document, which is then the whole of stdout.
pub(crate) fn print_json(
    document: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(document))
            .map_err(|e| e.to_string())?
    );
    Ok(())
}

/// [`time_lines`] for a render, whose worst block is named by its frame.
pub(crate) fn render_time_lines(compile_seconds: f64, timing: &Timing) -> Vec<String> {
    time_lines(compile_seconds, timing, |worst| {
        format!("frame {}", worst.frame)
    })
}

/// The cost of several renders of one program, for the outputs that have no
/// statistics block per render to carry it: a sweep's rows, an `.ir` text, a
/// family of frequency responses.
#[derive(Debug, Default)]
pub(crate) struct TotalTiming(Option<Timing>);

impl TotalTiming {
    /// Adds a render, when it was timed.
    pub(crate) fn add(&mut self, timing: Option<&Timing>) {
        let Some(timing) = timing else { return };
        match self.0.as_mut() {
            Some(total) => total.absorb(timing),
            None => self.0 = Some(timing.clone()),
        }
    }

    /// The closing `# time` lines: how many renders when there are several,
    /// then the account of all of them. None when nothing was timed.
    pub(crate) fn lines(&self, compile_seconds: f64, renders: usize) -> Vec<String> {
        let Some(timing) = &self.0 else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        if renders > 1 {
            lines.push(format!("# time: {renders} renders"));
        }
        lines.extend(render_time_lines(compile_seconds, timing));
        lines
    }
}

/// Facts that explain a render whose every output is exactly zero.
///
/// Facts, not guesses: what the tool knows and the statistics do not show.
pub(crate) fn silence_notes(probe: &Probe, input: &InputMode) -> Vec<String> {
    let mut notes = vec!["every output is exactly zero over the window".to_owned()];
    let released: Vec<&str> = probe
        .control_values()
        .into_iter()
        .filter(|(control, value)| {
            matches!(control.kind, ControlKind::Button | ControlKind::CheckButton) && *value == 0.0
        })
        .map(|(control, _)| control.path.as_str())
        .collect();
    if !released.is_empty() {
        notes.push(format!(
            "buttons and checkboxes at 0: {}",
            released.join(" ")
        ));
    }
    if *input == InputMode::Zero && probe.inputs() > 0 {
        notes.push(format!(
            "input is `zero` and the program has {} input(s)",
            probe.inputs()
        ));
    }
    notes
}

/// Three significant digits of a ratio: `27.6`, `1523`, `0.84`.
pub(crate) fn three_digits(value: f64) -> String {
    let magnitude = value.abs();
    if !value.is_finite() || magnitude >= 100.0 {
        format!("{value:.0}")
    } else if magnitude >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// The `# time` lines of `--time`: the compilation, the `compute` calls
/// against real time, the worst block against its deadline. `worst_name` says
/// which block that is, a render counting frames and a descent blocks.
pub(crate) fn time_lines(
    compile_seconds: f64,
    timing: &Timing,
    worst_name: impl Fn(&WorstBlock) -> String,
) -> Vec<String> {
    let mut lines = vec![format!(
        "# time: compile {}",
        human_seconds(compile_seconds)
    )];
    if timing.blocks == 0 {
        return lines;
    }
    lines.push(format!(
        "# time: compute {} for {} of audio ({} frames in {} block{}): {}x real time",
        human_seconds(timing.compute_seconds),
        human_seconds(timing.audio_seconds()),
        timing.frames,
        timing.blocks,
        if timing.blocks == 1 { "" } else { "s" },
        three_digits(timing.realtime_factor())
    ));
    if let (Some(worst), Some(fraction)) = (timing.worst, timing.worst_budget_fraction()) {
        lines.push(format!(
            "# time: worst block: {}, {} frames in {}, {}% of its {} budget",
            worst_name(&worst),
            worst.frames,
            human_seconds(worst.seconds),
            three_digits(100.0 * fraction),
            human_seconds(timing.budget_seconds(worst.frames))
        ));
    }
    lines
}

/// [`time_lines`] for the JSON documents. Seconds throughout.
pub(crate) fn timing_json(timing: &Timing) -> serde_json::Value {
    serde_json::json!({
        "compute_s": json_number(timing.compute_seconds),
        "audio_s": json_number(timing.audio_seconds()),
        "frames": timing.frames,
        "blocks": timing.blocks,
        "realtime_factor": json_number(timing.realtime_factor()),
        "worst_block": timing.worst.map(|worst| serde_json::json!({
            "frame": worst.frame,
            "frames": worst.frames,
            "seconds": json_number(worst.seconds),
            "budget_s": json_number(timing.budget_seconds(worst.frames)),
            "budget_fraction": json_number(
                timing.worst_budget_fraction().unwrap_or(f64::NAN)
            ),
        })),
    })
}

/// Names a non-finite sample the way it prints.
pub(crate) fn non_finite_name(value: f64) -> &'static str {
    if value.is_nan() {
        "NaN"
    } else if value > 0.0 {
        "+inf"
    } else {
        "-inf"
    }
}

/// JSON number, mapping a non-finite value to `null`.
///
/// `serde_json` cannot represent NaN or infinity, and silently dropping such a
/// point would hide exactly the runs worth looking at.
pub(crate) fn json_number(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number)
}
