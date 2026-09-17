//! `--freqresp`: the frequency response of a linear program from one impulse
//! response, after the checks that it is one (see `probe::freqresp`). With
//! `--sweep`, one response per point, each checked.
//!
//! [`Family::prepare`] validates the command line, [`Family::measure`] renders
//! one point and refuses it if the program is not linear and time-invariant
//! there, and nothing is printed before every point has been measured: a
//! family of curves with a member that is not a frequency response is refused
//! whole, with the point that broke.

use cranelift_ffi::probe::compare::{Disagreement, Samples, Tolerance, compare};
use cranelift_ffi::probe::engine::{Probe, RenderSpec};
use cranelift_ffi::probe::freqresp::{self, Grid, Property};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::render::{InputMode, RenderStats};
use cranelift_ffi::probe::sweep::{Axis, Point, cartesian};

use crate::cli::{Args, Format, refuse, verification_requested};
use crate::report::{
    Sink, TotalTiming, describe_program, document, json_number, non_finite_name, print_json,
    three_digits, timing_json,
};
use crate::setup::{
    Compiled, assignments, compile_timed, number_format, output_labels, parse_input, sweep_axes,
};
use crate::writes::{Clamped, Clamps, applied, check_value, prime};

/// The response of one configuration of the controls: one point of a sweep,
/// or the only one.
struct PointResponse {
    /// The swept controls at this point, as (query, applied value).
    set: Vec<(String, f64)>,
    /// The statistics of the unit impulse response.
    stats: RenderStats,
    /// The measured departure from each property, relative to the peak.
    deviations: Vec<(Property, f64)>,
    /// Per output, the response at each frequency.
    responses: Vec<Vec<freqresp::Point>>,
    /// Per output, the share of the energy in the last tenth of the window.
    tails: Vec<Option<f64>>,
}

impl PointResponse {
    /// `cutoff=200 q=5`, empty without a sweep.
    fn label(&self) -> String {
        self.set
            .iter()
            .map(|(query, value)| format!("{query}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// ` [cutoff=200 q=5]`, which names the point in a line about it; empty
    /// without a sweep, whose lines are then those of a single response.
    fn tag(&self) -> String {
        if self.set.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.label())
        }
    }

    /// What is said of a response that was cut while it rang.
    fn notes(&self, frames: usize) -> Vec<String> {
        self.tails
            .iter()
            .enumerate()
            .filter_map(|(output, tail)| {
                let tail = (*tail)?;
                (tail > freqresp::RINGING).then(|| {
                    format!(
                        "{}out{output} is still ringing {frames} frames after the impulse: what -n cut off is of the order of the last tenth's share, and near a resonance the magnitude is off by about its square root ({}%); raise -n",
                        if self.set.is_empty() {
                            String::new()
                        } else {
                            format!("[{}] ", self.label())
                        },
                        three_digits(100.0 * tail.sqrt())
                    )
                })
            })
            .collect()
    }
}

/// What a frequency response is not combined with, and why.
fn refuse_flags(args: &Args) -> Result<(), String> {
    refuse(
        &[
            (
                ("--reduce", "the response is the reduction"),
                args.reduce.is_some(),
            ),
            (
                (
                    "--at",
                    "a control that changes during the response makes the program time-varying",
                ),
                !args.ats.is_empty(),
            ),
            (
                ("--note/--chord", "they drive the polyphonic wrapper"),
                !args.notes.is_empty() || !args.chords.is_empty(),
            ),
            (
                (
                    "--skip",
                    "the transform is that of the whole response, from frame 0",
                ),
                args.skip != 0,
            ),
            (
                ("--every", "the rows are frequencies, not frames"),
                args.every != 1,
            ),
            (
                ("--bargraphs", "the rows are frequencies, not frames"),
                args.bargraphs,
            ),
            (
                ("--out", "a plain render writes the impulse response"),
                args.out.is_some(),
            ),
            (
                ("--fail-above", "it gates a plain render"),
                args.fail_above.is_some(),
            ),
            (
                (
                    "--compare/--ref/--check",
                    "they look at the samples of a plain render",
                ),
                verification_requested(args),
            ),
            (
                ("--format ir", "`.ir` holds frames"),
                args.format == Format::Ir,
            ),
        ],
        |(flag, why)| format!("{flag} cannot be combined with --freqresp: {why}"),
    )
}

/// `--freqresp`: measures every point, then prints the family.
pub(crate) fn run_freqresp(args: &Args) -> Result<(), String> {
    let family = Family::prepare(args)?;
    let mut measured: Vec<PointResponse> = Vec::with_capacity(family.points.len());
    let mut total_timing = TotalTiming::default();
    for point in &family.points {
        let response = family.measure(point)?;
        total_timing.add(response.stats.timing.as_ref());
        measured.push(response);
    }
    if args.format == Format::Json {
        return family.json(&measured);
    }
    if !args.quiet {
        family.rows(&measured);
    }
    family.annotations(&measured, &total_timing);
    Ok(())
}

/// A validated `--freqresp`: the program, the grid, and the points to measure.
struct Family<'a> {
    args: &'a Args,
    fmt: NumberFormat,
    compiled: Compiled,
    probe: Probe,
    grid: Grid,
    /// The grid's frequencies.
    hz: Vec<f64>,
    /// What the linearity checks tolerate, relative to the response's peak.
    tolerance: f64,
    /// The delay of the time-invariance check.
    shift: usize,
    /// The excited input, or all of them.
    channel: Option<usize>,
    fixed: Vec<(&'a str, f64)>,
    axes: Vec<Axis>,
    clamps: Clamps,
    /// What each output is, under `--eval`.
    labels: Vec<String>,
    points: Vec<Point>,
}

impl<'a> Family<'a> {
    fn prepare(args: &'a Args) -> Result<Self, String> {
        let spec_text = args.freqresp.as_deref().unwrap_or_default();
        refuse_flags(args)?;
        let grid = Grid::parse(spec_text, f64::from(args.sr))?;
        let tolerance = match args.linearity_tolerance {
            Some(value) if value >= 0.0 && value.is_finite() => value,
            Some(_) => {
                return Err("--linearity-tolerance must be a non-negative number".to_owned());
            }
            None => freqresp::default_tolerance(args.double),
        };
        if args.render < 4 {
            return Err(
                "--freqresp needs a window of at least 4 frames (-n) for its linearity checks"
                    .to_owned(),
            );
        }
        let fmt = number_format(args)?;

        let compiled = compile_timed(args)?;
        let probe = Probe::instantiate(&compiled.factory, args.sr)?;
        if probe.inputs() == 0 {
            return Err(
                "--freqresp measures the response to an input, and the program has none".to_owned(),
            );
        }
        // The excitation is an impulse, on every input or on one: a transfer
        // function is a response to that and to nothing else.
        let channel = match parse_input(&args.input)? {
            InputMode::Impulse => None,
            InputMode::ImpulseChannel(ch) if ch < probe.inputs() => Some(ch),
            InputMode::ImpulseChannel(ch) => {
                return Err(format!(
                    "--in impulse:{ch}: the program has {} input(s)",
                    probe.inputs()
                ));
            }
            _ => {
                return Err(format!(
                    "--freqresp measures an impulse response: `--in {}` is not `impulse` or `impulse:CH`",
                    args.input
                ));
            }
        };
        // Everything that will be written is checked before any render, the
        // values of a sweep like those of `--set`.
        let mut clamps = Clamps::default();
        let fixed = assignments(&args.sets)?;
        for (path, value) in &fixed {
            check_value(
                probe.controls(),
                path,
                *value,
                args.clamp,
                &mut clamps.fixed,
            )?;
        }
        let axes = sweep_axes(args)?;
        clamps.check_axes(probe.controls(), &axes, args.clamp)?;
        let labels = output_labels(args, compiled.eval.as_ref(), probe.outputs())?;
        Ok(Self {
            args,
            fmt,
            hz: grid.frequencies(),
            grid,
            tolerance,
            shift: freqresp::shift_for(args.render),
            channel,
            fixed,
            points: cartesian(&axes),
            axes,
            clamps,
            labels,
            compiled,
            probe,
        })
    }

    /// One render per excitation, each from a cleared instance with the
    /// `--set` controls and the point's written: `--settle` frames of silence,
    /// then the impulses, the window being the `-n` frames that follow.
    fn render_taps(
        &self,
        point: &Point,
        taps: Vec<(usize, f64)>,
        time: bool,
    ) -> Result<(RenderStats, Samples), String> {
        let args = self.args;
        prime(&self.probe, &self.fixed, point)?;
        let spec = RenderSpec {
            frames: args.settle + args.render,
            block: args.block,
            input: InputMode::Impulses {
                channel: self.channel,
                taps: taps
                    .into_iter()
                    .map(|(frame, amplitude)| (args.settle + frame, amplitude))
                    .collect(),
            },
            skip: args.settle,
            time,
            ..RenderSpec::default()
        };
        Ok(self.probe.collect(&spec))
    }

    /// The response at one point, after the checks that there is one.
    fn measure(&self, point: &Point) -> Result<PointResponse, String> {
        let args = self.args;
        let set = applied(self.probe.controls(), point);
        let at = if set.is_empty() {
            String::new()
        } else {
            let label: Vec<String> = set.iter().map(|(q, v)| format!("{q}={v}")).collect();
            format!("at `{}`, ", label.join(" "))
        };
        let (stats, h) = self.render_taps(point, vec![(0, 1.0)], args.time)?;
        if let Some((output, located)) = stats.first_non_finite() {
            return Err(format!(
                "{at}the impulse response is not finite\n  first: frame {}, out{output} ({}); {} of {} frames affected",
                located.frame,
                non_finite_name(located.value),
                stats.non_finite_frames,
                args.render
            ));
        }

        // is there a transfer function to measure?
        let mut deviations = Vec::new();
        for property in Property::ALL {
            let (_, answer) = self.render_taps(point, property.taps(self.shift), false)?;
            let expected = property.expected(&h, self.shift);
            let comparison = compare(
                &answer,
                &expected,
                Tolerance {
                    abs: 0.0,
                    rel: self.tolerance,
                },
                None,
            )?;
            if let Some((output, d)) = comparison.first_beyond() {
                return Err(self.not_a_response(point, &at, property, output, d, &stats)?);
            }
            let worst = comparison
                .channels
                .iter()
                .map(|(_, diff)| diff.max_rel)
                .fold(0.0_f64, f64::max);
            deviations.push((property, worst));
        }
        let sample_rate = f64::from(args.sr);
        Ok(PointResponse {
            set,
            responses: h
                .channels
                .iter()
                .map(|channel| freqresp::response(channel, sample_rate, &self.hz))
                .collect(),
            tails: h
                .channels
                .iter()
                .map(|channel| freqresp::tail_energy_fraction(channel))
                .collect(),
            stats,
            deviations,
        })
    }

    /// The refusal of a program that is not linear and time-invariant at a
    /// point: the property it breaks, where, and what the tool can establish
    /// of the cause.
    fn not_a_response(
        &self,
        point: &Point,
        at: &str,
        property: Property,
        output: usize,
        d: Disagreement,
        stats: &RenderStats,
    ) -> Result<String, String> {
        let (args, fmt, tolerance) = (self.args, &self.fmt, self.tolerance);
        let mut error = format!(
            "--freqresp: {at}the program is not linear and time-invariant: its impulse response has no transfer function to give\n  \
             {}\n  first: frame {}, out{output}: {} where {} was expected (tolerance {tolerance:e} of the peak)\n  \
             usual cause: {}",
            property.violation(args.settle, self.shift),
            d.frame,
            fmt.sample(d.value),
            fmt.sample(d.reference),
            property.usual_cause()
        );
        // an output that does not come from the input is a fact the tool can
        // establish: what the program says to silence. Said when it is of a
        // size to explain the refusal: the 1e-20 a reverberator injects
        // against subnormals is not.
        let (silence, _) = self.render_taps(point, Vec::new(), false)?;
        let (loudest, peak) = silence
            .channels
            .iter()
            .enumerate()
            .map(|(ch, c)| (ch, c.peak))
            .fold(
                (0, 0.0),
                |best, now| if now.1 > best.1 { now } else { best },
            );
        if peak > tolerance * stats.channels[loudest].peak {
            error.push_str(&format!(
                "\n  with no input at all the program outputs a signal (out{loudest} peaks at {}): it is not a function of its input alone",
                fmt.sample(peak)
            ));
        }
        if property == Property::TimeInvariance {
            error.push_str(
                "\n  a smoothed control (si.smoo) is such an envelope until it has settled: --settle N renders N frames of silence before the impulse",
            );
        }
        error.push_str(
            "\n  a program that is not linear is measured at one level and one frequency at a time: --in sine:HZ --skip N --reduce rms",
        );
        Ok(error)
    }

    /// The JSON document: one run per point.
    fn json(&self, measured: &[PointResponse]) -> Result<(), String> {
        let args = self.args;
        let runs: Vec<serde_json::Value> = measured
            .iter()
            .zip(&self.points)
            .map(|(response, point)| self.json_run(response, point))
            .collect();
        let mut document = document(args);
        document.insert("frames".to_owned(), serde_json::json!(args.render));
        document.insert(
            "freqresp".to_owned(),
            serde_json::json!({
                "input": self.channel,
                "settle": args.settle,
                "hz": self.hz.iter().map(|f| json_number(*f)).collect::<Vec<_>>(),
                "runs": runs,
            }),
        );
        let labels = self
            .compiled
            .eval
            .is_some()
            .then_some(self.labels.as_slice());
        describe_program(&mut document, args, labels, self.compiled.seconds);
        print_json(document)
    }

    fn json_run(&self, response: &PointResponse, point: &Point) -> serde_json::Value {
        let outputs: Vec<serde_json::Value> = response
            .responses
            .iter()
            .zip(&response.tails)
            .enumerate()
            .map(|(output, (points, tail))| {
                serde_json::json!({
                    "output": output,
                    "mag_db": points.iter().map(|p| json_number(p.magnitude_db)).collect::<Vec<_>>(),
                    "phase": points.iter().map(|p| json_number(p.phase)).collect::<Vec<_>>(),
                    "tail_energy_fraction": tail.map(json_number),
                    "peak": json_number(response.stats.channels[output].peak),
                })
            })
            .collect();
        let mut linearity = serde_json::Map::new();
        linearity.insert("tolerance".to_owned(), json_number(self.tolerance));
        linearity.insert("shift".to_owned(), serde_json::json!(self.shift));
        for (property, worst) in &response.deviations {
            linearity.insert(property.name().to_owned(), json_number(*worst));
        }
        let mut set = serde_json::Map::new();
        for (query, value) in &response.set {
            set.insert(query.clone(), json_number(*value));
        }
        let mut run = serde_json::json!({
            "set": set,
            "linearity": linearity,
            "outputs": outputs,
        });
        let clamps = self.clamps.of_point(point);
        if !clamps.is_empty() {
            run["clamped"] = serde_json::Value::Array(clamps.iter().map(|c| c.json()).collect());
        }
        let notes = response.notes(self.args.render);
        if !notes.is_empty() {
            run["notes"] = serde_json::json!(notes);
        }
        if let Some(timing) = &response.stats.timing {
            run["timing"] = timing_json(timing);
        }
        run
    }

    /// The CSV rows: the swept controls head them, as in a sweep of renders.
    fn rows(&self, measured: &[PointResponse]) {
        let fmt = &self.fmt;
        let mut header: Vec<String> = self.axes.iter().map(|axis| axis.path.clone()).collect();
        header.push("hz".to_owned());
        for output in 0..self.probe.outputs() {
            header.push(format!("mag_db_out{output}"));
            header.push(format!("phase_out{output}"));
        }
        println!("{}", header.join(","));
        for response in measured {
            for (k, frequency) in self.hz.iter().enumerate() {
                let mut row: Vec<String> = response
                    .set
                    .iter()
                    .map(|(_, value)| format!("{value}"))
                    .collect();
                row.push(fmt.computed(*frequency));
                for points in &response.responses {
                    row.push(fmt.computed(points[k].magnitude_db));
                    row.push(fmt.computed(points[k].phase));
                }
                println!("{}", row.join(","));
            }
        }
    }

    /// What is said of the family, and of each of its points.
    fn annotations(&self, measured: &[PointResponse], total_timing: &TotalTiming) {
        let (args, fmt, tolerance) = (self.args, &self.fmt, self.tolerance);
        let excited = self.channel.map_or_else(
            || {
                if self.probe.inputs() == 1 {
                    "the input".to_owned()
                } else {
                    format!("all {} inputs at once", self.probe.inputs())
                }
            },
            |ch| format!("input {ch}"),
        );
        // As for a render: under `--quiet` the annotations are the output.
        let sink = Sink::beside(!args.quiet);
        sink.say(format!(
            "# freqresp: {} frequenc{} from {} to {} Hz, from the response of {} frames to an impulse on {excited}{}{}",
            self.hz.len(),
            if self.hz.len() == 1 { "y" } else { "ies" },
            fmt.computed(self.grid.fmin),
            fmt.computed(self.grid.fmax),
            args.render,
            if args.settle == 0 {
                String::new()
            } else {
                format!(" at frame {}", args.settle)
            },
            if self.axes.is_empty() {
                String::new()
            } else {
                format!(", at each of {} sweep points", measured.len())
            }
        ));
        if self.compiled.eval.is_some() {
            for (output, label) in self.labels.iter().enumerate() {
                sink.say(format!("# eval out{output} = {label}"));
            }
        }
        // a sweep's clamps are said once, before what concerns each point
        sink.say_all(self.clamps.all().map(Clamped::line));
        for response in measured {
            let tag = response.tag();
            let shown: Vec<String> = response
                .deviations
                .iter()
                .map(|(property, worst)| format!("{} {}", property.name(), fmt.computed(*worst)))
                .collect();
            sink.say(format!(
                "# freqresp{tag}: linear and time-invariant within {tolerance:e} of the peak ({})",
                shown.join(", ")
            ));
            for (output, tail) in response.tails.iter().enumerate() {
                sink.say(match tail {
                    Some(tail) => format!(
                        "# freqresp{tag} out{output}: peak={} peak_at={}, the last tenth of the window holds {} of the energy",
                        fmt.sample(response.stats.channels[output].peak),
                        response.stats.channels[output]
                            .peak_at
                            .map_or_else(|| "none".to_owned(), |frame| frame.to_string()),
                        fmt.computed(*tail)
                    ),
                    None => format!(
                        "# freqresp{tag} out{output}: the response is exactly zero: nothing reaches this output from {excited}"
                    ),
                });
            }
            sink.say_all(
                response
                    .notes(args.render)
                    .into_iter()
                    .map(|note| format!("# note: {note}")),
            );
        }
        sink.say_all(total_timing.lines(self.compiled.seconds, measured.len()));
    }
}
