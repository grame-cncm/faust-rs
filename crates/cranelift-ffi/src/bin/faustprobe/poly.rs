//! `--nvoices`: the polyphonic wrapper, played by `--note` and `--chord`.
//!
//! Apart from [`crate::render`] because the two share almost nothing below
//! compilation: a poly render mixes N voices and an optional effect rather
//! than driving one `Probe`. What they say of a render, and how a render
//! fails, they share: see [`crate::report`] and [`crate::failure`].

use cranelift_ffi::probe::engine::{PolyProbe, PolyRenderSpec};
use cranelift_ffi::probe::number::NumberFormat;
use cranelift_ffi::probe::render::{InputMode, RenderStats};
use cranelift_ffi::probe::schedule::Schedule;

use crate::cli::{Args, Format, refuse, verification_requested};
use crate::failure::{poly_failure_context, render_failure};
use crate::report::{
    Sink, channel_lines, channels_json, document, json_number, print_controls, print_json,
    render_time_lines, timing_json, window_json,
};
use crate::setup::{assignments, build_schedule, number_format, parse_input_at, timed};
use crate::writes::{Clamped, check_poly_value};

/// What operates on the scalar `Probe` only is refused.
fn refuse_scalar_only(args: &Args) -> Result<(), String> {
    if !args.sweeps.is_empty() || args.reduce.is_some() {
        return Err(
            "--sweep/--reduce operate on the scalar Probe only; use --nvoices 0".to_owned(),
        );
    }
    if args.format == Format::Ir {
        return Err("--format ir is scoped to the scalar impulse-test protocol".to_owned());
    }
    if args.bargraphs {
        return Err("--bargraphs reads the scalar Probe only; use --nvoices 0".to_owned());
    }
    if args.compiler.any() || args.bra_tape != 8192 {
        return Err(
            "the compiler options (-pn, -vec, -ss, -mcd, -dlt, -ct, -table-init, --bra-tape) \
             reach the scalar Probe only; use --nvoices 0"
                .to_owned(),
        );
    }
    refuse(
        &[
            ("--out", args.out.is_some()),
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
/// render, as a scalar program's are.
pub(crate) fn run_poly(args: &Args) -> Result<(), String> {
    refuse_scalar_only(args)?;
    let fmt = number_format(args)?;

    // the voices and the effect: everything a poly render compiles
    let (compiled, compile_seconds) = timed(|| {
        PolyProbe::compile(
            &args.file,
            &args.import_dirs,
            args.sr,
            args.double,
            args.opt_level,
            args.nvoices,
            args.effect.as_deref(),
            args.voice_stop_level,
        )
    });
    let mut poly = compiled?;

    if args.list_params {
        println!(
            "{} voice(s), {} input(s)/voice, {} output(s), effect: {}",
            poly.voice_count(),
            poly.inputs(),
            poly.outputs(),
            if poly.has_effect() { "yes" } else { "no" }
        );
        print_controls("path (per voice)", poly.voice_controls(), &fmt);
        return Ok(());
    }

    let fixed = assignments(&args.sets)?;
    let schedule = build_schedule(args)?;
    // Every write of the command line is checked before any render, as for a
    // scalar program: an unknown path, a bargraph, and a value outside its
    // control's range, which was written as it is on the voices and clamped
    // in silence on the effect. What a note writes (its pitch's frequency,
    // its gain, its gate) is not a widget's doing and is not checked.
    let mut clamped: Vec<Clamped> = Vec::new();
    for (query, value) in &fixed {
        check_poly_value(&poly, query, *value, args.clamp, &mut clamped)?;
    }
    for (_, query, value) in schedule.param_writes() {
        check_poly_value(&poly, query, value, args.clamp, &mut clamped)?;
    }
    for (path, value) in &fixed {
        poly.set_all(path, *value)?;
    }

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
    let every = args.every.max(1);
    let dumping = !args.quiet && args.format == Format::Csv;
    if dumping {
        print!("frame");
        for ch in 0..poly.outputs() {
            print!(",out{ch}");
        }
        println!();
    }
    let stats = poly.render(&spec, |frame, samples| {
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
    let said = Said {
        args,
        poly: &poly,
        stats: &stats,
        clamped: &clamped,
        notes: &notes,
        compile_seconds,
    };
    if args.format == Format::Json {
        said.json()
    } else {
        said.lines(&fmt);
        Ok(())
    }
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
        // Same rule as the scalar path: under `--quiet` the statistics are the
        // output and go to stdout; otherwise they annotate a dump that already
        // owns stdout, and belong on stderr.
        let sink = Sink::beside(!args.quiet);
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
        sink.say_all(self.clamped.iter().map(Clamped::line));
        sink.say_all(self.notes.iter().map(|note| format!("# note: {note}")));
        if let Some(timing) = &stats.timing {
            sink.say_all(render_time_lines(self.compile_seconds, timing));
        }
    }
}
