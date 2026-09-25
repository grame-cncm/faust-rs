//! The plain render and the sweep: one program, its controls set, rendered
//! once or once per point.
//!
//! Three phases, in [`run_render`]:
//!
//! 1. [`Plan::prepare`]: everything the command line asks is validated before
//!    any render, in one place and one order, so that which of two errors is
//!    reported does not depend on where a check happened to be written;
//! 2. [`Run::render_point`]: one render from a cleared instance, its rows
//!    streamed as they come, then its failure if it has one;
//! 3. what is said of it, which depends on the output only: a JSON run
//!    ([`Run::json_run`]), a sweep's row ([`Run::sweep_row`]), or the
//!    statistics block ([`Run::statistics`]). An `.ir` text says nothing more.

use cranelift_ffi::probe::audio_out::SampleWriter;
use cranelift_ffi::probe::compare::Samples;
use cranelift_ffi::probe::engine::{Probe, RenderSpec};
use cranelift_ffi::probe::eval::csv_field;
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::protocol;
use cranelift_ffi::probe::render::RenderStats;
use cranelift_ffi::probe::schedule::Schedule;
use cranelift_ffi::probe::spectrum::{dominant_frequency, sfdr_db, thd_db};
use cranelift_ffi::probe::sweep::{Axis, Point, Reduction, cartesian, parse_reduction};

use crate::cli::{Args, Format, refuse};
use crate::failure::{failure_context, render_failure};
use crate::report::{
    Sink, TotalTiming, channel_lines, channels_json, describe_program, document, json_number,
    print_controls, print_json, render_time_lines, silence_notes, timing_json, window_json,
};
use crate::setup::{
    Compiled, assignments, build_schedule, compile_timed, number_format, output_labels,
    parse_input_at, sweep_axes,
};
use crate::verify::{Subject, Verdict, Verification};
use crate::writes::{Clamped, Clamps, applied, check_value, prime, written};

/// A plain render, or one per point of a sweep. `impulse_test` is the
/// reference protocol, which drives the buttons.
pub(crate) fn run_render(args: &Args, impulse_test: bool) -> Result<(), String> {
    let compiled = compile_timed(args)?;
    let probe = Probe::instantiate(&compiled.factory, args.sr)?;
    let fmt = number_format(args)?;
    if args.list_params {
        print_controls("path", probe.controls(), &fmt);
        return Ok(());
    }
    let plan = Plan::prepare(args, &compiled, &probe, impulse_test)?;
    Run {
        args,
        compiled: &compiled,
        probe: &probe,
        fmt,
        plan,
    }
    .run()
}

/// What a run will do, validated before any render.
struct Plan<'a> {
    axes: Vec<Axis>,
    reduction: Option<Reduction>,
    schedule: Schedule,
    /// `--set` and, under `--compare`, the values of FILE alone.
    fixed: Vec<(&'a str, f64)>,
    clamps: Clamps,
    /// The bargraphs' paths, for the headers; their values are read after
    /// each block (`Probe::bargraphs`, in the same order).
    bargraph_paths: Vec<String>,
    /// What each output is: `outN`, or under `--eval` the expression it
    /// computes.
    labels: Vec<String>,
    /// The `# eval outN = EXPR` lines, under `--eval`.
    legend: Vec<String>,
    verification: Verification<'a>,
    spec: RenderSpec,
    points: Vec<Point>,
    /// A sweep printed as CSV: one row per point. That row *is* the output,
    /// the swept values and what each render reduced to, so the per-frame
    /// dump is suppressed.
    sweep_csv: bool,
}

impl<'a> Plan<'a> {
    fn prepare(
        args: &'a Args,
        compiled: &Compiled,
        probe: &Probe,
        impulse_test: bool,
    ) -> Result<Self, String> {
        let axes = sweep_axes(args)?;
        let reduction = args.reduce.as_deref().map(parse_reduction).transpose()?;
        let schedule = build_schedule(args)?;
        if schedule.needs_poly() {
            return Err("--note/--chord require --nvoices > 0".to_owned());
        }
        // A schedule replays identically at every sweep point, which is exactly
        // what measuring a triggered instrument needs: strike the note once per
        // point while the swept parameter changes. The one genuine conflict is a
        // schedule that writes a control the sweep is also driving, where the
        // scheduled write would silently override the swept value.
        if !axes.is_empty() {
            for path in schedule.param_paths() {
                if let Some(axis) = axes.iter().find(|a| a.path == path) {
                    return Err(format!(
                        "--at writes `{}`, which --sweep is also driving; \
                         the scheduled write would override the swept value",
                        axis.path
                    ));
                }
            }
        }
        let fixed = assignments(args.sets.iter().chain(&args.set_a))?;
        // Everything a render will write is checked before any render: a
        // bargraph resolves like a control but is an output the program
        // overwrites, and the scheduled writes of `--at` ignore their errors
        // inside the render loop, so an unknown or unwritable path would
        // otherwise pass in silence. A value outside its control's range is
        // checked here too (see `crate::writes`).
        let mut clamps = Clamps::default();
        for (path, value) in &fixed {
            check_value(
                probe.controls(),
                path,
                *value,
                args.clamp,
                &mut clamps.fixed,
            )?;
        }
        for (_, path, value) in schedule.param_writes() {
            check_value(probe.controls(), path, value, args.clamp, &mut clamps.fixed)?;
        }
        clamps.check_axes(probe.controls(), &axes, args.clamp)?;
        if args.bargraphs && args.format == Format::Ir {
            return Err("--bargraphs cannot be combined with --format ir".to_owned());
        }
        let bargraph_paths: Vec<String> = probe.bargraphs().into_iter().map(|(p, _)| p).collect();
        let labels = output_labels(args, compiled.eval.as_ref(), probe.outputs())?;
        let legend: Vec<String> = if compiled.eval.is_some() {
            labels
                .iter()
                .enumerate()
                .map(|(ch, label)| format!("# eval out{ch} = {label}"))
                .collect()
        } else {
            Vec::new()
        };

        // `--compare`, `--ref`, `--check`: validated like everything else
        // before any render.
        let sweeping = !axes.is_empty();
        let verification =
            Verification::prepare(args, probe, &schedule, sweeping, &mut clamps.fixed)?;

        let spec = RenderSpec {
            frames: args.render,
            block: args.block,
            input: parse_input_at(&args.input, args.sr, probe.inputs())?,
            skip: args.skip,
            schedule: schedule.clone(),
            drive_buttons: impulse_test,
            limit: args.fail_above,
            time: args.time,
        };

        let points = cartesian(&axes);
        if args.out.is_some() {
            refuse(
                &[
                    ("--sweep", sweeping),
                    ("--format ir", args.format == Format::Ir),
                    ("--every", args.every != 1),
                ],
                |flag| {
                    format!(
                        "{flag} cannot be combined with --out, which writes every frame of one render"
                    )
                },
            )?;
        }
        // `.ir` describes exactly one render and cannot hold a sweep at all.
        if sweeping && args.format == Format::Ir {
            return Err("--sweep cannot be combined with --format ir".to_owned());
        }
        Ok(Self {
            sweep_csv: sweeping && args.format == Format::Csv,
            axes,
            reduction,
            schedule,
            fixed,
            clamps,
            bargraph_paths,
            labels,
            legend,
            verification,
            spec,
            points,
        })
    }
}

/// One render, once it succeeded: what the outputs are made of.
struct Rendered<'r> {
    stats: RenderStats,
    /// The window's samples, per output, when something needs them.
    collected: Vec<Vec<f64>>,
    /// Facts that explain an exactly silent render.
    notes: Vec<String>,
    /// The clamps this render ran under.
    clamps: Vec<&'r Clamped>,
    /// What the program's bargraphs show at the end of this render.
    bargraphs: Vec<(String, f64)>,
    /// Whether the frames were dumped on stdout.
    dumping: bool,
}

/// A validated run, and what it renders with.
struct Run<'a> {
    args: &'a Args,
    compiled: &'a Compiled,
    probe: &'a Probe,
    fmt: NumberFormat,
    plan: Plan<'a>,
}

impl Run<'_> {
    fn run(&self) -> Result<(), String> {
        let (args, plan) = (self.args, &self.plan);
        let rows_alone = plan.sweep_csv || args.format == Format::Ir;
        if plan.sweep_csv && !args.quiet {
            println!("{}", self.sweep_header().join(","));
        }
        // Where the annotations of a render go: under `--quiet` they are the
        // output (stdout, redirectable); otherwise they annotate a dump or a
        // sweep's rows, which own stdout, and belong on stderr.
        let annotate = Sink::beside(!args.quiet || rows_alone);
        // A sweep's rows and an `.ir` text have no statistics block to carry
        // the clamps: say them once, before the rows.
        if rows_alone {
            annotate.say_all(plan.clamps.all().map(Clamped::line));
        }
        // A sweep's columns keep their `REDUCTION_outN` names, which scripts
        // key on; what each `outN` is goes before the rows.
        if plan.sweep_csv {
            annotate.say_all(&plan.legend);
        }

        let mut runs: Vec<serde_json::Value> = Vec::new();
        let mut silent_points = 0usize;
        // `--time` over the renders that have no statistics block of their
        // own to carry it: a sweep's rows, an `.ir` text
        let mut total_timing = TotalTiming::default();
        // a comparison or a check that failed: reported after the output,
        // which carries its details
        let mut verification_failure: Option<String> = None;

        for point in &plan.points {
            let rendered = self.render_point(point)?;
            if !rendered.notes.is_empty() {
                silent_points += 1;
            }
            // After the bargraphs were read: `--check reset` renders again on
            // this instance.
            let verdict = self.verify(point, &rendered)?;
            if verification_failure.is_none() {
                verification_failure.clone_from(&verdict.failure);
            }
            total_timing.add(rendered.stats.timing.as_ref());
            match args.format {
                Format::Json => runs.push(self.json_run(point, &rendered, verdict)),
                // The .ir text is compared byte for byte; emit nothing else.
                Format::Ir => {}
                Format::Csv if plan.sweep_csv => {
                    println!("{}", self.sweep_row(point, &rendered).join(","));
                }
                Format::Csv => self.statistics(&rendered, &verdict),
            }
        }

        // A sweep's rows and an `.ir` text: one account for all the renders.
        if rows_alone {
            annotate.say_all(total_timing.lines(self.compiled.seconds, plan.points.len()));
        }
        // A sweep's rows show their zeros; what they do not show is why. One
        // render's notes stand for all when every point was silent.
        if plan.sweep_csv && silent_points == plan.points.len() {
            for note in silence_notes(self.probe, &plan.spec.input) {
                annotate.say(format!("# note: {note} (at every sweep point)"));
            }
        }
        if args.format == Format::Json {
            let mut document = document(args);
            document.insert("frames".to_owned(), serde_json::json!(args.render));
            document.insert(
                "reduce".to_owned(),
                serde_json::json!(plan.reduction.map(|r| r.to_string())),
            );
            document.insert("runs".to_owned(), serde_json::Value::Array(runs));
            let labels = self
                .compiled
                .eval
                .is_some()
                .then_some(plan.labels.as_slice());
            describe_program(&mut document, args, labels, self.compiled.seconds);
            print_json(document)?;
        }
        // The output above carries the details of a comparison or a check
        // that failed; the verdict is the exit status.
        verification_failure.map_or(Ok(()), Err)
    }

    /// The header of a sweep's rows: the swept controls, then per output the
    /// reduction or the three statistics, then the bargraphs.
    fn sweep_header(&self) -> Vec<String> {
        let plan = &self.plan;
        let mut header: Vec<String> = plan.axes.iter().map(|a| a.path.clone()).collect();
        for ch in 0..self.probe.outputs() {
            match plan.reduction {
                Some(r) => header.push(format!("{r}_out{ch}")),
                None => {
                    header.push(format!("peak_out{ch}"));
                    header.push(format!("rms_out{ch}"));
                    header.push(format!("dc_out{ch}"));
                }
            }
        }
        if self.args.bargraphs {
            header.extend(plan.bargraph_paths.iter().cloned());
        }
        header
    }

    /// The header of the per-frame dump.
    fn print_frames_header(&self) {
        let (args, probe) = (self.args, self.probe);
        match args.format {
            Format::Csv => {
                print!("frame");
                for label in &self.plan.labels {
                    print!(",{}", csv_field(label));
                }
                if args.bargraphs {
                    for path in &self.plan.bargraph_paths {
                        print!(",{path}");
                    }
                }
                println!();
            }
            Format::Ir => print!(
                "{}",
                protocol::header(probe.inputs(), probe.outputs(), args.render)
            ),
            Format::Json => {}
        }
    }

    /// One frame of the per-frame dump.
    fn print_frame(&self, frame: usize, samples: &[f64]) {
        match self.args.format {
            Format::Csv => {
                let mut line = frame.to_string();
                for value in samples {
                    line.push(',');
                    line.push_str(&self.fmt.sample(*value));
                }
                if self.args.bargraphs {
                    // read after the block this frame belongs to was
                    // computed: the value at that block's last sample
                    for (_, value) in self.probe.bargraphs() {
                        line.push(',');
                        line.push_str(&self.fmt.sample(value));
                    }
                }
                println!("{line}");
            }
            Format::Ir => print!("{}", protocol::frame_line(frame, samples)),
            Format::Json => {}
        }
    }

    /// Renders one point from a cleared instance, streaming its rows, and
    /// fails if the render does.
    fn render_point<'r>(&'r self, point: &Point) -> Result<Rendered<'r>, String> {
        let (args, plan, probe) = (self.args, &self.plan, self.probe);
        prime(probe, &plan.fixed, point)?;

        let mut writer = match &args.out {
            Some(path) => Some(SampleWriter::create(
                std::path::Path::new(path),
                probe.outputs(),
                args.render.saturating_sub(args.skip),
                args.compile.double,
                args.sr,
            )?),
            None => None,
        };
        let dumping = !args.quiet && writer.is_none();
        let streaming = dumping && args.format != Format::Json && !plan.sweep_csv;
        if streaming {
            self.print_frames_header();
        }

        // `f0` needs the samples, so collect them only when it is asked for.
        let want_samples = plan.verification.requested
            || matches!(
                plan.reduction,
                Some(Reduction::F0 | Reduction::Sfdr | Reduction::Thd)
            );
        let mut collected: Vec<Vec<f64>> = if want_samples {
            vec![Vec::new(); probe.outputs()]
        } else {
            Vec::new()
        };
        let every = args.every.max(1);
        let stats = probe.render(&plan.spec, |frame, samples| {
            if want_samples {
                for (ch, value) in samples.iter().enumerate() {
                    collected[ch].push(*value);
                }
            }
            if let Some(writer) = writer.as_mut() {
                writer.push(samples);
            }
            if streaming && (frame - plan.spec.skip).is_multiple_of(every) {
                self.print_frame(frame, samples);
            }
        });
        if let Some(writer) = writer {
            writer.finish()?;
        }

        // A non-finite sample invalidates a measurement, so the free path
        // fails on it. The `.ir` path must not: the reference corpus contains
        // DSPs whose expected output has NaN in it (`sound.dsp`, frames 41 and
        // 845), and the artifact is what `filesCompare` judges: the exit code
        // says whether the render was produced, not whether the DSP diverged.
        // `impulse-cranelift` exits 0 there, and the probe must match it to be
        // a drop-in replacement.
        let non_finite = (args.format != Format::Ir)
            .then(|| stats.first_non_finite())
            .flatten();
        if let Some(failure) = render_failure(
            &stats,
            non_finite,
            args.fail_above,
            args.render,
            &self.fmt,
            |frame| self.context(point, frame),
        ) {
            return Err(failure);
        }
        // Exact silence is nearly always a gate never pressed or an input
        // never fed: the facts the tool has go with the numbers.
        let notes = if stats.is_silent() {
            silence_notes(probe, &plan.spec.input)
        } else {
            Vec::new()
        };
        Ok(Rendered {
            stats,
            collected,
            notes,
            clamps: plan.clamps.of_point(point),
            bargraphs: probe.bargraphs(),
            dumping,
        })
    }

    /// What a failure at `frame` of this point's render is explained with.
    fn context(&self, point: &Point, frame: usize) -> String {
        failure_context(
            frame,
            &written(self.probe.controls(), &self.plan.fixed, point),
            &self.plan.schedule,
            self.probe.controls(),
        )
    }

    /// `--compare`, `--ref`, `--check` on one render.
    fn verify(&self, point: &Point, rendered: &Rendered<'_>) -> Result<Verdict, String> {
        let plan = &self.plan;
        if !plan.verification.requested {
            return Ok(Verdict::default());
        }
        let render = Samples {
            start: plan.spec.skip,
            channels: rendered.collected.clone(),
        };
        let subject = Subject {
            args: self.args,
            factory: &self.compiled.factory,
            probe: self.probe,
            spec: &plan.spec,
            fixed: &plan.fixed,
            fmt: &self.fmt,
        };
        plan.verification
            .judge(&subject, &render, |frame| self.context(point, frame))
    }

    /// One channel of this render, reduced to the number of `--reduce`.
    fn reduced(&self, r: Reduction, rendered: &Rendered<'_>, ch: usize) -> f64 {
        reduce_channel(
            r,
            &rendered.stats,
            ch,
            &rendered.collected,
            self.probe.sample_rate(),
            self.args.f0,
        )
    }

    /// One render, as an entry of the JSON document's `runs`.
    fn json_run(
        &self,
        point: &Point,
        rendered: &Rendered<'_>,
        verdict: Verdict,
    ) -> serde_json::Value {
        let stats = &rendered.stats;
        let mut entry = serde_json::Map::new();
        let mut set = serde_json::Map::new();
        // the values the render used
        for (path, value) in applied(self.probe.controls(), point) {
            set.insert(path, json_number(value));
        }
        entry.insert("set".to_owned(), serde_json::Value::Object(set));
        if !rendered.clamps.is_empty() {
            entry.insert(
                "clamped".to_owned(),
                serde_json::Value::Array(rendered.clamps.iter().map(|c| c.json()).collect()),
            );
        }
        if !rendered.notes.is_empty() {
            entry.insert("notes".to_owned(), serde_json::json!(rendered.notes));
        }
        entry.extend(verdict.json);
        entry.insert("window".to_owned(), window_json(stats));
        if let Some(r) = self.plan.reduction {
            let values: Vec<serde_json::Value> = (0..self.probe.outputs())
                .map(|ch| json_number(self.reduced(r, rendered, ch)))
                .collect();
            entry.insert(r.to_string(), serde_json::Value::Array(values));
        } else {
            // Without an explicit reduction, report the full statistics
            // rather than nothing: a sweep with no numbers is useless.
            entry.insert("channels".to_owned(), channels_json(stats));
        }
        if !rendered.bargraphs.is_empty() {
            let mut shown = serde_json::Map::new();
            for (path, value) in &rendered.bargraphs {
                shown.insert(path.clone(), json_number(*value));
            }
            entry.insert("bargraphs".to_owned(), serde_json::Value::Object(shown));
        }
        if let Some(timing) = &stats.timing {
            entry.insert("timing".to_owned(), timing_json(timing));
        }
        serde_json::Value::Object(entry)
    }

    /// One render, as a row of a sweep.
    fn sweep_row(&self, point: &Point, rendered: &Rendered<'_>) -> Vec<String> {
        let (fmt, stats) = (&self.fmt, &rendered.stats);
        // the value the render used: the requested one, unless `--clamp`
        // clamped it, and then the row must not claim the requested one
        let mut row: Vec<String> = applied(self.probe.controls(), point)
            .into_iter()
            .map(|(_, value)| format!("{value}"))
            .collect();
        for ch in 0..self.probe.outputs() {
            match self.plan.reduction {
                Some(r) => {
                    let value = self.reduced(r, rendered, ch);
                    // a peak is a sample's magnitude; the others are computed
                    row.push(if r == Reduction::Peak {
                        fmt.sample(value)
                    } else {
                        fmt.computed(value)
                    });
                }
                None => {
                    row.push(fmt.sample(stats.channels[ch].peak));
                    row.push(fmt.computed(stats.channels[ch].rms));
                    row.push(fmt.computed(stats.channels[ch].dc));
                }
            }
        }
        if self.args.bargraphs {
            row.extend(rendered.bargraphs.iter().map(|(_, v)| fmt.sample(*v)));
        }
        row
    }

    /// One render, as the statistics block.
    fn statistics(&self, rendered: &Rendered<'_>, verdict: &Verdict) {
        let (args, fmt, stats) = (self.args, &self.fmt, &rendered.stats);
        // With `--quiet` or `--out` the statistics are the whole output, so
        // they go to stdout and can be redirected; otherwise they annotate a
        // dump that already owns stdout, and belong on stderr.
        let sink = Sink::beside(rendered.dumping);
        sink.say(format!(
            "# frames={} sr={} window={}..{} ({} frames)",
            args.render,
            args.sr,
            stats.window_start,
            stats.window_start + stats.window_len,
            stats.window_len
        ));
        sink.say_all(&self.plan.legend);
        sink.say_all(channel_lines(stats, fmt));
        for (path, value) in &rendered.bargraphs {
            sink.say(format!("# bargraph {path}={}", fmt.sample(*value)));
        }
        sink.say_all(rendered.clamps.iter().map(|c| c.line()));
        sink.say_all(rendered.notes.iter().map(|note| format!("# note: {note}")));
        sink.say_all(&verdict.lines);
        if let Some(timing) = &stats.timing {
            sink.say_all(render_time_lines(self.compiled.seconds, timing));
        }
    }
}

/// One channel of a rendered window, reduced to a single number.
///
/// Shared by the JSON and CSV sweep paths so the two cannot report different
/// numbers for the same render.
fn reduce_channel(
    r: Reduction,
    stats: &RenderStats,
    ch: usize,
    collected: &[Vec<f64>],
    sample_rate: i32,
    f0: Option<f64>,
) -> f64 {
    let sr = f64::from(sample_rate);
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
