//! `--nvoices`: the polyphonic wrapper, played by `--note` and `--chord`.
//!
//! Apart from [`crate::render`] because the two share almost nothing below
//! compilation: a poly render mixes N voices and an optional effect rather
//! than driving one `Probe`. What they say of a render, how a render fails
//! and what it is measured by, they share: see [`crate::report`],
//! [`crate::failure`] and [`crate::measure`].
//!
//! A sweep (`--sweep`, `--points`) renders every point from a reset
//! instrument ([`PolyProbe::reset`]): the voices and the effect cleared, every
//! voice free, the notes of the schedule played again.

use cranelift_ffi::probe::audio_out::SampleWriter;
use cranelift_ffi::probe::engine::{PolyProbe, PolyRenderSpec};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::render::{InputMode, RenderStats};
use cranelift_ffi::probe::schedule::Schedule;
use cranelift_ffi::probe::sweep::{Axis, Point};

use crate::cli::{Args, Format, refuse, verification_requested};
use crate::failure::{poly_failure_context, render_failure};
use crate::measure::{Measured, Measures};
use crate::report::{
    Sink, TotalTiming, channel_lines, channels_json, document, json_number, print_controls,
    print_json, render_time_lines, timing_json, window_json,
};
use crate::setup::{
    assignments, build_schedule, compiler_args, number_format, parse_input_at, sweep_flag,
    sweep_points, timed,
};
use crate::writes::{Clamped, check_poly_value};

/// What operates on the scalar `Probe` only is refused.
fn refuse_scalar_only(args: &Args) -> Result<(), String> {
    if args.format == Format::Ir {
        return Err("--format ir is scoped to the scalar impulse-test protocol".to_owned());
    }
    if args.bargraphs {
        return Err("--bargraphs reads the scalar Probe only; use --nvoices 0".to_owned());
    }
    let given = compiler_args(args);
    if !given.is_empty() {
        return Err(format!(
            "the compiler options ({}) reach the scalar Probe only; use --nvoices 0",
            given.join(" ")
        ));
    }
    refuse(
        &[
            ("--eval", !args.evals.is_empty()),
            ("--compare/--ref/--check", verification_requested(args)),
            ("--freqresp", args.freqresp.is_some()),
        ],
        |flag| format!("{flag} operates on the scalar Probe only; use --nvoices 0"),
    )
}

/// Render `args.nvoices` > 0 through the polyphonic wrapper.
///
/// `--note` and `--chord` play the voices; `--set` and `--at` are broadcast to
/// every voice and to the effect, and checked against their ranges before the
/// render, as a scalar program's are. `--out` writes the instrument's outputs,
/// the effect's when there is one, as it writes a scalar program's.
pub(crate) fn run_poly(args: &Args) -> Result<(), String> {
    refuse_scalar_only(args)?;
    let (axes, points) = sweep_points(args)?;
    let sweeping = !axes.is_empty();
    if args.out.is_some() {
        // the scalar path's refusal, worded the same: `--format ir` is
        // already refused above
        refuse(
            &[(sweep_flag(args), sweeping), ("--every", args.every != 1)],
            |flag| {
                format!(
                    "{flag} cannot be combined with --out, which writes every frame of one render"
                )
            },
        )?;
    }
    let fmt = number_format(args)?;

    // the voices and the effect: everything a poly render compiles
    let (compiled, compile_seconds) = timed(|| {
        PolyProbe::compile(
            &args.file,
            &args.import_dirs,
            args.sr,
            args.compile.double,
            args.opt_level,
            args.nvoices,
            args.effect.as_deref(),
            args.voice_stop_level,
        )
    });
    let mut poly = compiled?;
    // an inline `effect` left out: the render that follows is not the
    // instrument the program declares
    for warning in poly.warnings() {
        eprintln!("warning: {warning}");
    }

    if args.list_params {
        // the instrument's outputs, after the effect, which says what it
        // makes of the voices' channels
        println!(
            "{} voice(s), {} input(s)/voice, {} output(s), effect: {}",
            poly.voice_count(),
            poly.inputs(),
            poly.outputs(),
            if poly.has_effect() {
                format!("yes ({} -> {})", poly.bus_outputs(), poly.outputs())
            } else {
                "no".to_owned()
            }
        );
        print_controls("path (per voice)", poly.voice_controls(), &fmt);
        return Ok(());
    }

    let fixed = assignments(&args.sets)?;
    let schedule = build_schedule(args)?;
    let clamped = check_writes(args, &poly, &fixed, &schedule, &axes)?;
    let measures = Measures::prepare(
        args,
        poly.outputs(),
        args.render.saturating_sub(args.skip),
        poly.sample_rate(),
    )?;

    let spec = PolyRenderSpec {
        frames: args.render,
        block: args.block,
        // every playing voice receives the excitation, as the reference's
        // `mydsp_poly::compute` hands the host's inputs to each of them;
        // validated, like every write, before anything is printed
        input: parse_input_at(&args.input, args.sr, poly.inputs())?,
        skip: args.skip,
        schedule: schedule.clone(),
        limit: args.fail_above,
        time: args.time,
    };
    if sweeping {
        return Sweep {
            args,
            poly: &mut poly,
            spec: &spec,
            fixed: &fixed,
            schedule: &schedule,
            measures: &measures,
            clamped: &clamped,
            fmt: &fmt,
            compile_seconds,
        }
        .run(&axes, &points);
    }
    for (path, value) in &fixed {
        poly.set_all(path, *value)?;
    }
    // created before anything is printed, as on the scalar path: an
    // extension it cannot write, or a raw format for several outputs, is an
    // error of the command line
    let mut writer = match &args.out {
        Some(path) => Some(SampleWriter::create(
            std::path::Path::new(path),
            poly.outputs(),
            args.render.saturating_sub(args.skip),
            args.compile.double,
            args.sr,
        )?),
        None => None,
    };
    let every = args.every.max(1);
    let dumping = !args.quiet && args.format == Format::Csv && writer.is_none();
    if dumping {
        print!("frame");
        for ch in 0..poly.outputs() {
            print!(",out{ch}");
        }
        println!();
    }
    let mut collected: Vec<Vec<f64>> = if measures.needs_samples() {
        vec![Vec::new(); poly.outputs()]
    } else {
        Vec::new()
    };
    let stats = poly.render(&spec, |frame, samples| {
        for (channel, value) in collected.iter_mut().zip(samples) {
            channel.push(*value);
        }
        if let Some(writer) = writer.as_mut() {
            writer.push(samples);
        }
        if !dumping || !(frame - spec.skip).is_multiple_of(every) {
            return;
        }
        let mut line = frame.to_string();
        for value in samples {
            line.push(',');
            line.push_str(&fmt.sample(*value));
        }
        println!("{line}");
    })?;
    if let Some(writer) = writer {
        writer.finish()?;
    }

    // A render that went wrong fails, as a scalar one does, and says where it
    // starts and what had been written and played by then. The statistics of
    // this path used to skip the samples that were not finite: a voice that
    // ran away to infinity printed `rms=inf` and the command succeeded.
    if let Some(failure) = render_failure(
        &stats,
        stats.first_non_finite(),
        args.fail_above,
        args.render,
        &fmt,
        |frame| poly_failure_context(frame, &fixed, &schedule, &poly),
    ) {
        return Err(failure);
    }

    let notes = silence_notes(&stats, &schedule, &spec.input, &poly);
    let measured = measures.measure(&stats, &collected);
    let said = Said {
        args,
        poly: &poly,
        stats: &stats,
        clamped: &clamped,
        notes: &notes,
        measures: &measures,
        measured: &measured,
        compile_seconds,
    };
    if args.format == Format::Json {
        said.json()
    } else {
        said.lines(&fmt);
        Ok(())
    }
}

/// Every write of the command line, checked before any render as for a
/// scalar program: an unknown path, a bargraph, and a value outside its
/// control's range, which was written as it is on the voices and clamped in
/// silence on the effect. What a note writes (its pitch's frequency, its
/// gain, its gate) is not a widget's doing and is not checked. Returns the
/// clamps `--clamp` accepted.
fn check_writes(
    args: &Args,
    poly: &PolyProbe,
    fixed: &[(&str, f64)],
    schedule: &Schedule,
    axes: &[Axis],
) -> Result<Vec<Clamped>, String> {
    let mut clamped: Vec<Clamped> = Vec::new();
    for (query, value) in fixed {
        check_poly_value(poly, query, *value, args.clamp, &mut clamped)?;
    }
    for (_, query, value) in schedule.param_writes() {
        check_poly_value(poly, query, value, args.clamp, &mut clamped)?;
    }
    // as on the scalar path: a scheduled write to a swept control would
    // override the swept value
    for path in schedule.param_paths() {
        if let Some(axis) = axes.iter().find(|a| a.path == path) {
            return Err(format!(
                "--at writes `{}`, which {} is also driving; \
                 the scheduled write would override the swept value",
                axis.path,
                sweep_flag(args)
            ));
        }
    }
    for axis in axes {
        for value in &axis.values {
            check_poly_value(poly, &axis.path, *value, args.clamp, &mut clamped)?;
        }
    }
    Ok(clamped)
}

/// A poly render is silent until a note plays: say which it is.
fn silence_notes(
    stats: &RenderStats,
    schedule: &Schedule,
    input: &InputMode,
    poly: &PolyProbe,
) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    if stats.is_silent() {
        notes.push("every output is exactly zero over the window".to_owned());
        if !schedule.needs_poly() {
            notes.push("no --note or --chord is scheduled: every voice stays free".to_owned());
        }
        if *input == InputMode::Zero && poly.inputs() > 0 {
            notes.push(format!(
                "input is `zero` and a voice has {} input(s)",
                poly.inputs()
            ));
        }
    }
    notes
}

/// What is said of a polyphonic render, once it succeeded.
struct Said<'a> {
    args: &'a Args,
    poly: &'a PolyProbe,
    stats: &'a RenderStats,
    clamped: &'a [Clamped],
    notes: &'a [String],
    measures: &'a Measures,
    measured: &'a Measured,
    compile_seconds: f64,
}

impl Said<'_> {
    fn json(&self) -> Result<(), String> {
        let (args, stats) = (self.args, self.stats);
        let mut document = document(args);
        document.insert("nvoices".to_owned(), serde_json::json!(args.nvoices));
        document.insert("frames".to_owned(), serde_json::json!(args.render));
        document.insert("window".to_owned(), window_json(stats));
        document.insert(
            "active_voices".to_owned(),
            serde_json::json!(self.poly.active_voice_count()),
        );
        document.insert("channels".to_owned(), channels_json(stats));
        if let Some((name, values)) = self.measures.reduced_json(self.measured) {
            document.insert(name, values);
        }
        if let Some(described) = self.measures.described_json(self.measured) {
            document.insert("describe".to_owned(), described);
        }
        if !self.clamped.is_empty() {
            document.insert(
                "clamped".to_owned(),
                serde_json::Value::Array(self.clamped.iter().map(Clamped::json).collect()),
            );
        }
        if !self.notes.is_empty() {
            document.insert("notes".to_owned(), serde_json::json!(self.notes));
        }
        if let Some(timing) = &stats.timing {
            let mut json = timing_json(timing);
            json["compile_s"] = json_number(self.compile_seconds);
            document.insert("timing".to_owned(), json);
        }
        print_json(document)
    }

    fn lines(&self, fmt: &NumberFormat) {
        let (args, stats) = (self.args, self.stats);
        // Same rule as the scalar path: under `--quiet` or `--out` the
        // statistics are the output and go to stdout; otherwise they annotate
        // a dump that already owns stdout, and belong on stderr.
        let sink = Sink::beside(!args.quiet && args.out.is_none());
        // the fields a reader keyed on come first; the window and what a
        // scalar render says of a channel follow them
        sink.say(format!(
            "# frames={} sr={} nvoices={} active_voices={} window={}..{} ({} frames)",
            args.render,
            args.sr,
            args.nvoices,
            self.poly.active_voice_count(),
            stats.window_start,
            stats.window_start + stats.window_len,
            stats.window_len
        ));
        sink.say_all(channel_lines(stats, fmt));
        sink.say_all(self.measures.lines(self.measured, fmt));
        sink.say_all(self.clamped.iter().map(Clamped::line));
        sink.say_all(self.notes.iter().map(|note| format!("# note: {note}")));
        if let Some(timing) = &stats.timing {
            sink.say_all(render_time_lines(self.compile_seconds, timing));
        }
    }
}

/// A sweep of a polyphonic instrument: one render per point, each from a
/// reset instrument, said as rows (CSV) or as the `runs` of one document.
struct Sweep<'a> {
    args: &'a Args,
    poly: &'a mut PolyProbe,
    spec: &'a PolyRenderSpec,
    fixed: &'a [(&'a str, f64)],
    schedule: &'a Schedule,
    measures: &'a Measures,
    clamped: &'a [Clamped],
    fmt: &'a NumberFormat,
    compile_seconds: f64,
}

impl Sweep<'_> {
    fn run(mut self, axes: &[Axis], points: &[Point]) -> Result<(), String> {
        let (args, fmt) = (self.args, self.fmt);
        let csv = args.format == Format::Csv;
        if csv && !args.quiet {
            let mut header: Vec<String> = axes.iter().map(|a| a.path.clone()).collect();
            header.extend(self.measures.header(self.poly.outputs()));
            println!("{}", header.join(","));
        }
        // the rows own stdout: what annotates them goes beside, once
        let annotate = Sink::beside(true);
        if csv {
            annotate.say_all(self.clamped.iter().map(Clamped::line));
        }
        let mut runs: Vec<serde_json::Value> = Vec::new();
        let mut measured: Vec<Measured> = Vec::new();
        let mut silent_points = 0usize;
        let mut total_timing = TotalTiming::default();
        let mut last_notes = Vec::new();
        for point in points {
            let (stats, values) = self.render_point(point)?;
            let notes = silence_notes(&stats, self.schedule, &self.spec.input, self.poly);
            if !notes.is_empty() {
                silent_points += 1;
            }
            total_timing.add(stats.timing.as_ref());
            let applied = self.applied(point);
            if csv {
                let mut row: Vec<String> = applied.iter().map(|(_, v)| format!("{v}")).collect();
                row.extend(self.measures.row(&values, &stats, fmt));
                println!("{}", row.join(","));
            } else {
                runs.push(self.json_run(&applied, &stats, &values, &notes));
            }
            measured.push(values);
            last_notes = notes;
        }
        if csv {
            annotate.say_all(total_timing.lines(self.compile_seconds, points.len()));
            if silent_points == points.len() {
                for note in &last_notes {
                    annotate.say(format!("# note: {note} (at every sweep point)"));
                }
            }
            annotate.say_all(self.measures.sweep_notes(&measured));
            return Ok(());
        }
        let mut document = document(args);
        document.insert("nvoices".to_owned(), serde_json::json!(args.nvoices));
        document.insert("frames".to_owned(), serde_json::json!(args.render));
        document.insert(
            "reduce".to_owned(),
            serde_json::json!(self.measures.reduction.map(|r| r.to_string())),
        );
        if !self.clamped.is_empty() {
            document.insert(
                "clamped".to_owned(),
                serde_json::Value::Array(self.clamped.iter().map(Clamped::json).collect()),
            );
        }
        document.insert("runs".to_owned(), serde_json::Value::Array(runs));
        if args.time {
            document.insert(
                "timing".to_owned(),
                serde_json::json!({ "compile_s": json_number(self.compile_seconds) }),
            );
        }
        print_json(document)
    }

    /// Renders one point from a reset instrument, and fails if the render
    /// does.
    fn render_point(&mut self, point: &Point) -> Result<(RenderStats, Measured), String> {
        self.poly.reset();
        for (path, value) in self.fixed {
            self.poly.set_all(path, *value)?;
        }
        for (path, value) in &point.assignments {
            self.poly.set_all(path, *value)?;
        }
        let mut collected: Vec<Vec<f64>> = if self.measures.needs_samples() {
            vec![Vec::new(); self.poly.outputs()]
        } else {
            Vec::new()
        };
        let stats = self.poly.render(self.spec, |_, samples| {
            for (channel, value) in collected.iter_mut().zip(samples) {
                channel.push(*value);
            }
        })?;
        let written: Vec<(&str, f64)> = self
            .fixed
            .iter()
            .copied()
            .chain(point.assignments.iter().map(|(p, v)| (p.as_str(), *v)))
            .collect();
        if let Some(failure) = render_failure(
            &stats,
            stats.first_non_finite(),
            self.args.fail_above,
            self.args.render,
            self.fmt,
            |frame| poly_failure_context(frame, &written, self.schedule, self.poly),
        ) {
            return Err(failure);
        }
        let measured = self.measures.measure(&stats, &collected);
        Ok((stats, measured))
    }

    /// The values a point's render used: the requested ones, unless `--clamp`
    /// clamped them (on the voices, or on the effect when only it has the
    /// control).
    fn applied(&self, point: &Point) -> Vec<(String, f64)> {
        point
            .assignments
            .iter()
            .map(|(query, value)| {
                let applied = self
                    .poly
                    .check_write(query, *value)
                    .ok()
                    .and_then(|writes| writes.first().map(|w| w.write.applied))
                    .unwrap_or(*value);
                (query.clone(), applied)
            })
            .collect()
    }

    /// One render, as an entry of the document's `runs`.
    fn json_run(
        &self,
        applied: &[(String, f64)],
        stats: &RenderStats,
        measured: &Measured,
        notes: &[String],
    ) -> serde_json::Value {
        let mut entry = serde_json::Map::new();
        let mut set = serde_json::Map::new();
        for (path, value) in applied {
            set.insert(path.clone(), json_number(*value));
        }
        entry.insert("set".to_owned(), serde_json::Value::Object(set));
        if !notes.is_empty() {
            entry.insert("notes".to_owned(), serde_json::json!(notes));
        }
        entry.insert("window".to_owned(), window_json(stats));
        entry.insert(
            "active_voices".to_owned(),
            serde_json::json!(self.poly.active_voice_count()),
        );
        if let Some((name, values)) = self.measures.reduced_json(measured) {
            entry.insert(name, values);
        } else {
            entry.insert("channels".to_owned(), channels_json(stats));
        }
        if let Some(described) = self.measures.described_json(measured) {
            entry.insert("describe".to_owned(), described);
        }
        if let Some(timing) = &stats.timing {
            entry.insert("timing".to_owned(), timing_json(timing));
        }
        serde_json::Value::Object(entry)
    }
}
