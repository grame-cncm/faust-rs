//! What the command line means, before anything is rendered: the excitation
//! of `--in`, the `PATH=VALUE` assignments, the schedule of `--at`, `--note`
//! and `--chord`, the program to compile, the labels of its outputs, the text
//! of its numbers.

use std::rc::Rc;
use std::time::Instant;

use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::eval::EvalProgram;
use cranelift_ffi::probe::number::{NumberFormat, Precision};
use cranelift_ffi::probe::render::InputMode;
use cranelift_ffi::probe::schedule::{Schedule, parse_at, parse_chord, parse_note};
use cranelift_ffi::probe::sweep::{Axis, parse_axis};

use crate::cli::Args;

/// The program of a run, compiled at the width asked, with what `--time`
/// says of the compilation.
pub(crate) struct Compiled {
    pub(crate) factory: Rc<Factory>,
    /// The wrapped file, under `--eval`.
    pub(crate) eval: Option<EvalProgram>,
    /// What the compilation took.
    pub(crate) seconds: f64,
}

/// [`compile_program`] at the width of `--double`, timed.
pub(crate) fn compile_timed(args: &Args) -> Result<Compiled, String> {
    let (result, seconds) = timed(|| compile_program(args, args.compile.double));
    let (factory, eval) = result?;
    Ok(Compiled {
        factory: Rc::new(factory),
        eval,
        seconds,
    })
}

/// Runs `work`, and says what it took in seconds.
pub(crate) fn timed<T>(work: impl FnOnce() -> T) -> (T, f64) {
    let started = Instant::now();
    let result = work();
    (result, started.elapsed().as_secs_f64())
}

/// Several `PATH=VALUE` assignments (`--set`, `--set-a`, `--set-b`).
pub(crate) fn assignments<'a>(
    texts: impl IntoIterator<Item = &'a String>,
) -> Result<Vec<(&'a str, f64)>, String> {
    texts.into_iter().map(|a| parse_assignment(a)).collect()
}

/// The axes of `--sweep`, in the order given.
pub(crate) fn sweep_axes(args: &Args) -> Result<Vec<Axis>, String> {
    args.sweeps.iter().map(|a| parse_axis(a)).collect()
}

/// Parse an `--in` value into an excitation mode.
pub(crate) fn parse_input(spec: &str) -> Result<InputMode, String> {
    let (head, tail) = spec
        .split_once(':')
        .map_or((spec, None), |(h, t)| (h, Some(t)));
    match (head, tail) {
        ("zero", None) => Ok(InputMode::Zero),
        ("impulse", None) => Ok(InputMode::Impulse),
        ("impulse", Some(ch)) => ch
            .parse()
            .map(InputMode::ImpulseChannel)
            .map_err(|_| format!("invalid channel in `--in impulse:{ch}`")),
        ("dc", None) => Ok(InputMode::Dc),
        ("white", None) => Ok(InputMode::White { seed: 0 }),
        ("white", Some(seed)) => seed
            .parse()
            .map(|seed| InputMode::White { seed })
            .map_err(|_| format!("invalid seed in `--in white:{seed}`")),
        ("sine", Some(hz)) => hz
            .parse()
            .map(|hz| InputMode::Sine { hz })
            .map_err(|_| format!("invalid frequency in `--in sine:{hz}`")),
        ("sine", None) => Err("`--in sine` needs a frequency, e.g. sine:440".to_owned()),
        ("file", Some(rest)) => {
            // `file:PATH` or `file:PATH:CH`; a trailing `:N` is a channel
            let (path, channel) = match rest.rsplit_once(':') {
                Some((path, ch)) if ch.parse::<usize>().is_ok() => (path, ch.parse().ok()),
                _ => (rest, None),
            };
            InputMode::from_file(std::path::Path::new(path), channel)
        }
        ("file", None) => Err("`--in file` needs a path, e.g. file:target.wav".to_owned()),
        _ => Err(format!("unknown input mode `{spec}`")),
    }
}

/// The excitation of `--in`, with a warning on stderr when it is a file
/// recorded at another rate than `--sr`: the program then runs at `--sr`
/// and reads the samples as if they were at that rate.
pub(crate) fn parse_input_at(spec: &str, sr: i32, inputs: usize) -> Result<InputMode, String> {
    let input = parse_input(spec)?;
    check_impulse_channel(&input, inputs)?;
    if let InputMode::File {
        sample_rate: Some(rate),
        ..
    } = &input
        && i64::from(*rate) != i64::from(sr)
    {
        eprintln!(
            "warning: `--in {spec}` is recorded at {rate} Hz, the program runs at {sr} Hz (--sr)"
        );
    }
    Ok(input)
}

/// An impulse on an input the program does not have excites nothing, and
/// the silence that follows does not say why: it is an error, worded the same
/// in every mode.
pub(crate) fn check_impulse_channel(input: &InputMode, inputs: usize) -> Result<(), String> {
    let InputMode::ImpulseChannel(channel) = input else {
        return Ok(());
    };
    if *channel < inputs {
        return Ok(());
    }
    Err(format!(
        "--in impulse:{channel}: {}",
        match inputs {
            0 => "the program has no input".to_owned(),
            1 => "the program has one input, channel 0".to_owned(),
            n => format!("the program has {n} inputs, channels 0 to {}", n - 1),
        }
    ))
}

/// Split a `PATH=VALUE` assignment.
pub(crate) fn parse_assignment(text: &str) -> Result<(&str, f64), String> {
    let (path, value) = text
        .split_once('=')
        .ok_or_else(|| format!("expected PATH=VALUE, got `{text}`"))?;
    let parsed = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number in `{text}`"))?;
    Ok((path, parsed))
}

/// The program to probe: FILE, or under `--eval` the expressions evaluated in
/// FILE's scope. With the wrapped file when there is one, which is what
/// labels the outputs and explains a compile error.
pub(crate) fn compile_program(
    args: &Args,
    double: bool,
) -> Result<(Factory, Option<EvalProgram>), String> {
    if args.evals.is_empty() {
        let factory = Factory::compile_with_args(
            &args.file,
            &args.import_dirs,
            &compiler_args(args),
            double,
            args.opt_level,
        )?;
        return Ok((factory, None));
    }
    let text = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("cannot read '{}': {e}", args.file))?;
    let eval = EvalProgram::new(&text, &args.evals)?;
    // The source is named by the file's path, not by a bare name: that is
    // what a diagnostic cites, what the compiler resolves the file's relative
    // imports against, and what the control paths' root comes from.
    let factory = Factory::compile_from_string_with_args(
        &args.file,
        &eval.source(),
        &args.import_dirs,
        &compiler_args(args),
        double,
        args.opt_level,
    )
    .map_err(|error| eval.explain(&args.file, &error))?;
    Ok((factory, Some(eval)))
}

/// One label per output: `out0, out1, ...`, or under `--eval` the expressions
/// (`EXPR[j]` for one with several outputs).
///
/// With several expressions the columns are attributed by the number of
/// outputs of each, which a second, tiny program computes (`outputs(EXPR)`):
/// assuming one output each would label a column with the wrong expression
/// the day one of them has two.
pub(crate) fn output_labels(
    args: &Args,
    eval: Option<&EvalProgram>,
    outputs: usize,
) -> Result<Vec<String>, String> {
    let Some(eval) = eval else {
        return Ok((0..outputs).map(|ch| format!("out{ch}")).collect());
    };
    if eval.exprs().len() == 1 {
        return eval.labels(&[outputs], outputs);
    }
    let factory = Factory::compile_from_string_with_args(
        &args.file,
        &eval.arity_source(),
        &args.import_dirs,
        &compiler_args(args),
        true,
        0,
    )
    .map_err(|error| eval.explain(&args.file, &error))?;
    let probe = Probe::instantiate(&std::rc::Rc::new(factory), args.sr)?;
    let spec = RenderSpec {
        frames: 1,
        input: InputMode::Zero,
        ..RenderSpec::default()
    };
    let mut arities = Vec::new();
    probe.render(&spec, |_, samples| {
        arities = samples.iter().map(|count| *count as usize).collect();
    });
    eval.labels(&arities, outputs)
}

/// The number text of this run: `--precision`, at the program's width.
pub(crate) fn number_format(args: &Args) -> Result<NumberFormat, String> {
    let precision = match &args.precision {
        Some(text) => Precision::parse(text)?,
        None => Precision::RoundTrip,
    };
    Ok(NumberFormat::new(precision, args.compile.double))
}

/// The compile options the probe forwards to a [`Factory`] constructor:
/// those that differ from their default, as the C API `argv` spells them,
/// less the precision, which every constructor takes as its own argument.
pub(crate) fn compiler_args(args: &Args) -> Vec<String> {
    let mut options = args.compile.clone();
    options.double = false;
    options.single = false;
    options.to_argv()
}

/// Collect `--at`, `--note` and `--chord` into one ordered schedule.
///
/// # Errors
/// Returns the first parse failure, naming the offending argument.
pub(crate) fn build_schedule(args: &Args) -> Result<Schedule, String> {
    let mut schedule = Schedule::new();
    for pair in args.ats.chunks(2) {
        // clap's `num_args = 2` guarantees pairs; be defensive anyway rather
        // than indexing past the end on a future flag-parsing change.
        let [frame, assignment] = pair else {
            return Err("--at takes FRAME PATH=VALUE".to_owned());
        };
        let (at, event) = parse_at(frame, assignment)?;
        schedule.push(at, event);
    }
    for note in &args.notes {
        for (frame, event) in parse_note(note)? {
            schedule.push(frame, event);
        }
    }
    for chord in &args.chords {
        for (frame, event) in parse_chord(chord)? {
            schedule.push(frame, event);
        }
    }
    Ok(schedule)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_input_mode() {
        assert_eq!(parse_input("zero").unwrap(), InputMode::Zero);
        assert_eq!(parse_input("impulse").unwrap(), InputMode::Impulse);
        assert_eq!(
            parse_input("impulse:1").unwrap(),
            InputMode::ImpulseChannel(1)
        );
        assert_eq!(parse_input("dc").unwrap(), InputMode::Dc);
        assert_eq!(parse_input("white").unwrap(), InputMode::White { seed: 0 });
        assert_eq!(
            parse_input("white:9").unwrap(),
            InputMode::White { seed: 9 }
        );
        assert_eq!(
            parse_input("sine:440").unwrap(),
            InputMode::Sine { hz: 440.0 }
        );
    }

    #[test]
    fn rejects_sine_without_frequency() {
        // Defaulting to some arbitrary pitch would silently measure the wrong
        // operating point, which is the failure mode this tool exists to avoid.
        assert!(parse_input("sine").is_err());
    }

    #[test]
    fn rejects_unknown_input_mode() {
        assert!(parse_input("triangle").is_err());
    }

    #[test]
    fn parses_assignment() {
        let (path, value) = parse_assignment("filter_cutoff_hz=1000").unwrap();
        assert_eq!(path, "filter_cutoff_hz");
        assert!((value - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_malformed_assignment() {
        assert!(parse_assignment("cutoff").is_err());
        assert!(parse_assignment("cutoff=loud").is_err());
    }
}
