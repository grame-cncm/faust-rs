//! `faust-probe` command-line entry point.
//!
//! See the crate documentation for why this exists alongside the two impulse
//! runners, and `porting/faust-probe-generic-test-tool-design-2026-08-14-en.md`
//! for the full design.

use std::process::ExitCode;
use std::thread;

use clap::Parser;

use cranelift_ffi::probe::engine::{Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;

/// Probe a Faust DSP: set controls, render offline, report samples and statistics.
#[derive(Debug, Parser)]
#[command(name = "faust-probe", version, about, long_about = None)]
struct Args {
    /// Faust DSP source file.
    file: String,

    /// Add a Faust library import directory (repeatable).
    #[arg(short = 'I', long = "import-dir", value_name = "DIR")]
    import_dirs: Vec<String>,

    /// Compile and execute with double-precision samples.
    #[arg(long)]
    double: bool,

    /// Cranelift optimisation level.
    #[arg(long, default_value_t = 0)]
    opt_level: i32,

    /// Sample rate in Hz.
    #[arg(long, default_value_t = 44_100)]
    sr: i32,

    /// Frames per compute call.
    #[arg(long, default_value_t = 64)]
    block: usize,

    /// Frames to render.
    #[arg(short = 'n', long, default_value_t = 15_000)]
    render: usize,

    /// Set a control before rendering, as `PATH=VALUE` (repeatable).
    ///
    /// PATH may be a full address or a trailing fragment of one; an ambiguous
    /// fragment is reported with its candidates rather than resolved
    /// arbitrarily.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    sets: Vec<String>,

    /// Input excitation: zero, impulse, impulse:CH, dc, white[:SEED], sine:HZ.
    #[arg(long = "in", value_name = "MODE", default_value = "impulse")]
    input: String,

    /// Exclude the first N frames from both the dump and the statistics.
    #[arg(long, default_value_t = 0)]
    skip: usize,

    /// Print one frame out of N.
    #[arg(long, default_value_t = 1)]
    every: usize,

    /// List the discovered controls and exit.
    #[arg(long)]
    list_params: bool,

    /// Print statistics only, no per-frame dump.
    #[arg(long)]
    quiet: bool,
}

/// Parse an `--in` value into an excitation mode.
fn parse_input(spec: &str) -> Result<InputMode, String> {
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
        _ => Err(format!("unknown input mode `{spec}`")),
    }
}

/// Split a `PATH=VALUE` assignment.
fn parse_assignment(text: &str) -> Result<(&str, f64), String> {
    let (path, value) = text
        .split_once('=')
        .ok_or_else(|| format!("expected PATH=VALUE, got `{text}`"))?;
    let parsed = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number in `{text}`"))?;
    Ok((path, parsed))
}

fn run(args: Args) -> Result<(), String> {
    let probe = Probe::compile(
        &args.file,
        &args.import_dirs,
        args.sr,
        args.double,
        args.opt_level,
    )?;

    if args.list_params {
        println!(
            "{:<44} {:>10} {:>10} {:>10} {:>10}",
            "path", "init", "min", "max", "step"
        );
        for control in probe.controls().iter() {
            println!(
                "{:<44} {:>10} {:>10} {:>10} {:>10}",
                control.path, control.init, control.min, control.max, control.step
            );
        }
        return Ok(());
    }

    for assignment in &args.sets {
        let (path, value) = parse_assignment(assignment)?;
        probe.set(path, value)?;
    }

    let spec = RenderSpec {
        frames: args.render,
        block: args.block,
        input: parse_input(&args.input)?,
        skip: args.skip,
    };

    let every = args.every.max(1);
    if !args.quiet {
        print!("frame");
        for ch in 0..probe.outputs() {
            print!(",out{ch}");
        }
        println!();
    }

    let stats = probe.render(&spec, |frame, samples| {
        if args.quiet || !(frame - spec.skip).is_multiple_of(every) {
            return;
        }
        let mut line = frame.to_string();
        for value in samples {
            line.push(',');
            line.push_str(&format!("{value:.9}"));
        }
        println!("{line}");
    });

    // To stderr so a CSV dump stays pipeable.
    eprintln!(
        "# frames={} sr={} window={}..{} ({} frames)",
        args.render,
        args.sr,
        stats.window_start,
        stats.window_start + stats.window_len,
        stats.window_len
    );
    for (ch, channel) in stats.channels.iter().enumerate() {
        eprintln!(
            "# out{ch}: peak={:.9} rms={:.9} dc={:.9} finite={}",
            channel.peak,
            channel.rms,
            channel.dc,
            if channel.finite { "yes" } else { "no" }
        );
    }

    if stats.all_finite() {
        Ok(())
    } else {
        Err("render produced non-finite samples".to_owned())
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    // Cranelift JIT plus the faust-rs front end recurse deeply; run on a large
    // stack, as `impulse_cranelift` and the differential tests do.
    let result = thread::Builder::new()
        .name("faust-probe".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run(args))
        .expect("spawn worker thread")
        .join()
        .expect("join worker thread");

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("faust-probe: {error}");
            ExitCode::FAILURE
        }
    }
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
