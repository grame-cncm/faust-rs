//! `faust-probe` command-line entry point.
//!
//! See the crate documentation for why this exists alongside the two impulse
//! runners, and `porting/faust-probe-generic-test-tool-design-2026-08-14-en.md`
//! for the full design.

use std::process::ExitCode;
use std::thread;

use clap::{Parser, ValueEnum};

use cranelift_ffi::probe::engine::{PolyProbe, Probe, RenderSpec};
use cranelift_ffi::probe::poly;
use cranelift_ffi::probe::protocol;
use cranelift_ffi::probe::render::InputMode;
use cranelift_ffi::probe::spectrum::dominant_frequency;
use cranelift_ffi::probe::sweep::{Reduction, cartesian, parse_axis, parse_reduction};

/// How rendered frames are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// `frame,out0,out1` with full precision — the default, pipeable.
    Csv,
    /// The reference impulse-test `.ir` text, with its zero-clamp.
    Ir,
    /// One versioned JSON object; the only format that carries a sweep.
    Json,
}

/// Which rendering protocol to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Protocol {
    /// Whatever the individual flags say.
    Free,
    /// Pin every knob to the reference impulse-test values.
    ImpulseTest,
}

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

    /// Input excitation: zero, impulse, impulse:CH, dc, `white[:SEED]`, sine:HZ.
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

    /// Output format for rendered frames.
    #[arg(long, value_enum, default_value_t = Format::Csv)]
    format: Format,

    /// Sweep a control over several values, as `PATH=V1,V2,...` (repeatable).
    ///
    /// Repeating the flag takes the cartesian product, with the last axis
    /// varying fastest. Every point renders from a cleared instance, so one
    /// configuration cannot contaminate the next.
    #[arg(long = "sweep", value_name = "PATH=V1,V2,...")]
    sweeps: Vec<String>,

    /// Reduce each render to one number per channel: rms, peak, energy, dc, f0.
    #[arg(long = "reduce", value_name = "R")]
    reduce: Option<String>,

    /// Rendering protocol.
    ///
    /// `impulse-test` reproduces the reference protocol exactly — sample rate
    /// 44100, block 64, impulse on every input, buttons held for the first
    /// block, `.ir` output — and rejects any flag that would perturb it, so a
    /// regression run cannot be silently mis-configured.
    #[arg(long, value_enum, default_value_t = Protocol::Free)]
    protocol: Protocol,

    /// Polyphonic voice count; 0 renders the DSP directly (default, and the
    /// only mode `--protocol impulse-test` accepts).
    ///
    /// N > 0 compiles N instances from one JIT and drives them through the
    /// polyphonic wrapper ported from `poly-dsp.h` (allocation, stealing,
    /// mixing, reclamation below `--voice-stop-level`). The design's `-n`
    /// short form is not used here: `-n` already names `--render` (frames),
    /// including in this tool's own regression check against
    /// `impulse_cranelift`, which this phase must not disturb.
    ///
    /// This phase exposes no `--note`/`--chord`/`--at` scheduling (design
    /// phase P5): the polyphonic engine is driven at the library level
    /// (`PolyProbe::key_on`/`key_off`), not from this command line yet, so a
    /// poly render with no `--set` broadcast onto a voice's own gate/freq/gain
    /// is silence — every voice starts and stays free.
    #[arg(long = "nvoices", default_value_t = 0)]
    nvoices: usize,

    /// Separate effect DSP, run once on the voices' mixed output.
    ///
    /// Without this, a single-file instrument that declares both `process`
    /// and `effect` has its effect extracted automatically the way
    /// `FaustPolyDspGenerator` does — wrap the source in `environment{}` and
    /// take `dsp_code.effect` — and this flag is unnecessary; pass it to
    /// override that guess or to pair a process DSP with an effect declared
    /// in a different file. Requires `--nvoices` > 0.
    #[arg(long = "effect", value_name = "FILE")]
    effect: Option<String>,

    /// RMS level below which a releasing voice is reclaimed as free.
    ///
    /// Default `0.00003162` (-90 dB) is `poly-dsp.h`'s `VOICE_STOP_LEVEL` —
    /// the one number in the polyphonic wrapper with an audible consequence
    /// (design §3.2): too high truncates long releases, too low never
    /// reclaims a voice under sustained play. Requires `--nvoices` > 0.
    #[arg(long = "voice-stop-level", default_value_t = poly::DEFAULT_VOICE_STOP_LEVEL)]
    voice_stop_level: f64,
}

/// Flags a caller must not combine with `--protocol impulse-test`.
///
/// Rejecting rather than overriding: a protocol run whose sample rate was
/// quietly ignored would produce a `.ir` that looks valid and compares wrong.
fn reject_protocol_conflicts(args: &Args) -> Result<(), String> {
    let mut offenders = Vec::new();
    if args.sr != protocol::SAMPLE_RATE {
        offenders.push("--sr");
    }
    if args.block != protocol::BLOCK_SIZE {
        offenders.push("--block");
    }
    if args.input != "impulse" {
        offenders.push("--in");
    }
    if args.skip != 0 {
        offenders.push("--skip");
    }
    if args.every != 1 {
        offenders.push("--every");
    }
    if !args.sets.is_empty() {
        offenders.push("--set");
    }
    if args.format != Format::Ir {
        offenders.push("--format");
    }
    if !args.sweeps.is_empty() {
        offenders.push("--sweep");
    }
    if args.reduce.is_some() {
        offenders.push("--reduce");
    }
    if args.nvoices != 0 {
        offenders.push("--nvoices");
    }
    if offenders.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "--protocol impulse-test fixes the rendering conditions; remove {}",
            offenders.join(", ")
        ))
    }
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

/// Render `args.nvoices` > 0 through the polyphonic wrapper.
///
/// Split from [`run`] because the two paths share almost nothing below
/// compilation: a poly render mixes N voices and an optional effect rather
/// than driving one `Probe`, and this phase has no `--note`/`--chord`/`--at`
/// scheduling (design phase P5), so `--set` broadcasting to every voice is
/// the only way this entry point can make a render produce sound — genuine
/// note-driven verification goes through [`PolyProbe::key_on`]/`key_off`
/// directly, exercised by this crate's tests rather than this binary.
fn run_poly(args: &Args) -> Result<(), String> {
    if !args.sweeps.is_empty() || args.reduce.is_some() {
        return Err(
            "--sweep/--reduce operate on the scalar Probe only; use --nvoices 0".to_owned(),
        );
    }
    if args.format == Format::Ir {
        return Err("--format ir is scoped to the scalar impulse-test protocol".to_owned());
    }

    let mut poly = PolyProbe::compile(
        &args.file,
        &args.import_dirs,
        args.sr,
        args.double,
        args.opt_level,
        args.nvoices,
        args.effect.as_deref(),
        args.voice_stop_level,
    )?;

    if args.list_params {
        println!(
            "{} voice(s), {} input(s)/voice, {} output(s), effect: {}",
            poly.voice_count(),
            poly.inputs(),
            poly.outputs(),
            if poly.has_effect() { "yes" } else { "no" }
        );
        println!(
            "{:<44} {:>10} {:>10} {:>10} {:>10}",
            "path (per voice)", "init", "min", "max", "step"
        );
        for control in poly.voice_controls().iter() {
            println!(
                "{:<44} {:>10} {:>10} {:>10} {:>10}",
                control.path, control.init, control.min, control.max, control.step
            );
        }
        return Ok(());
    }

    let fixed = args
        .sets
        .iter()
        .map(|a| parse_assignment(a))
        .collect::<Result<Vec<_>, _>>()?;
    for (path, value) in &fixed {
        poly.set_all(path, *value)?;
    }

    let every = args.every.max(1);
    let mut peak = vec![0.0_f64; poly.outputs()];
    let mut sum_sq = vec![0.0_f64; poly.outputs()];
    let mut counted = 0usize;

    let header_needed = !args.quiet && args.format == Format::Csv;
    if header_needed {
        print!("frame");
        for ch in 0..poly.outputs() {
            print!(",out{ch}");
        }
        println!();
    }

    let mut written = 0usize;
    while written < args.render {
        let n = args.block.min(args.render - written);
        let block_out = poly.compute(n);
        for j in 0..n {
            let frame = written + j;
            if frame < args.skip {
                continue;
            }
            for (ch, channel) in block_out.iter().enumerate() {
                let value = channel[j];
                if value.is_finite() {
                    peak[ch] = peak[ch].max(value.abs());
                    sum_sq[ch] = value.mul_add(value, sum_sq[ch]);
                }
            }
            counted += 1;
            if !args.quiet
                && args.format == Format::Csv
                && (frame - args.skip).is_multiple_of(every)
            {
                let mut line = frame.to_string();
                for channel in &block_out {
                    line.push(',');
                    line.push_str(&format!("{:.9}", channel[j]));
                }
                println!("{line}");
            }
        }
        written += n;
    }

    let denom = counted.max(1) as f64;
    if args.format == Format::Json {
        let channels: Vec<serde_json::Value> = (0..poly.outputs())
            .map(|ch| {
                serde_json::json!({
                    "peak": json_number(peak[ch]),
                    "rms": json_number((sum_sq[ch] / denom).sqrt()),
                })
            })
            .collect();
        let document = serde_json::json!({
            "schema_version": 1,
            "dsp": args.file,
            "sr": args.sr,
            "nvoices": args.nvoices,
            "frames": args.render,
            "active_voices": poly.active_voice_count(),
            "channels": channels,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?
        );
    } else if !args.quiet {
        eprintln!(
            "# frames={} sr={} nvoices={} active_voices={}",
            args.render,
            args.sr,
            args.nvoices,
            poly.active_voice_count()
        );
        for ch in 0..poly.outputs() {
            eprintln!(
                "# out{ch}: peak={:.9} rms={:.9}",
                peak[ch],
                (sum_sq[ch] / denom).sqrt()
            );
        }
    }

    Ok(())
}

fn run(mut args: Args) -> Result<(), String> {
    let impulse_test = args.protocol == Protocol::ImpulseTest;
    if impulse_test {
        // Defaults are the reference values already, so only an explicitly
        // conflicting flag is an error. `--format ir` is implied.
        if args.format == Format::Csv {
            args.format = Format::Ir;
        }
        reject_protocol_conflicts(&args)?;
        if args.render == 15_000 {
            args.render = protocol::DEFAULT_FRAMES;
        }
    }

    if args.effect.is_some() && args.nvoices == 0 {
        return Err("--effect requires --nvoices > 0".to_owned());
    }
    if args.nvoices > 0 {
        return run_poly(&args);
    }

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

    let axes = args
        .sweeps
        .iter()
        .map(|a| parse_axis(a))
        .collect::<Result<Vec<_>, _>>()?;
    let reduction = args.reduce.as_deref().map(parse_reduction).transpose()?;
    let fixed = args
        .sets
        .iter()
        .map(|a| parse_assignment(a))
        .collect::<Result<Vec<_>, _>>()?;

    let spec = RenderSpec {
        frames: args.render,
        block: args.block,
        input: parse_input(&args.input)?,
        skip: args.skip,
        drive_buttons: impulse_test,
    };

    let points = cartesian(&axes);
    let sweeping = !axes.is_empty();
    if sweeping && args.format != Format::Json {
        // A sweep produces one row per point; CSV and .ir describe a single
        // render and would concatenate them into something unreadable.
        return Err("--sweep requires --format json".to_owned());
    }

    let every = args.every.max(1);
    let mut runs: Vec<serde_json::Value> = Vec::new();

    for point in &points {
        // Every point starts from the same known state (see probe::sweep).
        probe.reset();
        for (path, value) in &fixed {
            probe.set(path, *value)?;
        }
        for (path, value) in &point.assignments {
            probe.set(path, *value)?;
        }

        let header_needed = !args.quiet && args.format != Format::Json;
        if header_needed {
            match args.format {
                Format::Csv => {
                    print!("frame");
                    for ch in 0..probe.outputs() {
                        print!(",out{ch}");
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

        // `f0` needs the samples, so collect them only when it is asked for.
        let want_samples = reduction == Some(Reduction::F0);
        let mut collected: Vec<Vec<f64>> = if want_samples {
            vec![Vec::new(); probe.outputs()]
        } else {
            Vec::new()
        };

        let stats = probe.render(&spec, |frame, samples| {
            if want_samples {
                for (ch, value) in samples.iter().enumerate() {
                    collected[ch].push(*value);
                }
            }
            if args.quiet || args.format == Format::Json {
                return;
            }
            if !(frame - spec.skip).is_multiple_of(every) {
                return;
            }
            match args.format {
                Format::Csv => {
                    let mut line = frame.to_string();
                    for value in samples {
                        line.push(',');
                        line.push_str(&format!("{value:.9}"));
                    }
                    println!("{line}");
                }
                Format::Ir => print!("{}", protocol::frame_line(frame, samples)),
                Format::Json => {}
            }
        });

        // A non-finite sample invalidates a measurement, so the free path
        // fails on it. The `.ir` path must not: the reference corpus contains
        // DSPs whose expected output has NaN in it (`sound.dsp`, frames 41 and
        // 845), and the artifact is what `filesCompare` judges — the exit code
        // says whether the render was produced, not whether the DSP diverged.
        // `impulse_cranelift` exits 0 there, and the probe must match it to be
        // a drop-in replacement.
        if args.format != Format::Ir && !stats.all_finite() {
            return Err("render produced non-finite samples".to_owned());
        }

        if args.format == Format::Json {
            let mut entry = serde_json::Map::new();
            let mut set = serde_json::Map::new();
            for (path, value) in &point.assignments {
                set.insert(path.clone(), json_number(*value));
            }
            entry.insert("set".to_owned(), serde_json::Value::Object(set));
            entry.insert(
                "window".to_owned(),
                serde_json::json!({
                    "start": stats.window_start,
                    "frames": stats.window_len,
                }),
            );
            if let Some(r) = reduction {
                let values: Vec<serde_json::Value> = (0..probe.outputs())
                    .map(|ch| {
                        let v = match r {
                            Reduction::Rms => stats.channels[ch].rms,
                            Reduction::Peak => stats.channels[ch].peak,
                            Reduction::Energy => {
                                stats.channels[ch].rms.powi(2) * stats.window_len as f64
                            }
                            Reduction::Dc => stats.channels[ch].dc,
                            Reduction::F0 => {
                                dominant_frequency(&collected[ch], f64::from(probe.sample_rate()))
                            }
                        };
                        json_number(v)
                    })
                    .collect();
                entry.insert(r.to_string(), serde_json::Value::Array(values));
            } else {
                // Without an explicit reduction, report the full statistics
                // rather than nothing: a sweep with no numbers is useless.
                let channels: Vec<serde_json::Value> = stats
                    .channels
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "peak": json_number(c.peak),
                            "rms": json_number(c.rms),
                            "dc": json_number(c.dc),
                        })
                    })
                    .collect();
                entry.insert("channels".to_owned(), serde_json::Value::Array(channels));
            }
            runs.push(serde_json::Value::Object(entry));
        } else if args.format == Format::Ir {
            // The .ir text is compared byte for byte; emit nothing else.
        } else {
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
        }
    }

    if args.format == Format::Json {
        let document = serde_json::json!({
            "schema_version": 1,
            "dsp": args.file,
            "sr": args.sr,
            "frames": args.render,
            "reduce": reduction.map(|r| r.to_string()),
            "runs": runs,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?
        );
    }

    Ok(())
}

/// JSON number, mapping a non-finite value to `null`.
///
/// `serde_json` cannot represent NaN or infinity, and silently dropping such a
/// point would hide exactly the runs worth looking at.
fn json_number(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value).map_or(serde_json::Value::Null, serde_json::Value::Number)
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
