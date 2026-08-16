//! `faustprobe` command-line entry point.
//!
//! See the crate documentation for why this exists alongside the two impulse
//! runners, and `porting/faustprobe-generic-test-tool-design-2026-08-14-en.md`
//! for the full design.

use std::process::ExitCode;
use std::thread;

use clap::{Parser, ValueEnum};

use cranelift_ffi::probe::engine::{PolyProbe, Probe, RenderSpec};
use cranelift_ffi::probe::poly;
use cranelift_ffi::probe::protocol;
use cranelift_ffi::probe::render::{InputMode, RenderStats};
use cranelift_ffi::probe::schedule::{Event, Schedule, parse_at, parse_chord, parse_note};
use cranelift_ffi::probe::spectrum::{dominant_frequency, sfdr_db, thd_db};
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
#[command(name = "faustprobe", version, about, long_about = None)]
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

    /// Reduce each render to one number per channel: rms, peak, energy, dc, f0,
    /// `sfdr` or `thd`.
    ///
    /// `sfdr` and `thd` need a fundamental. It is estimated from the strongest
    /// bin unless `--f0` says otherwise, and both want a stationary window:
    /// measuring while the spectrum decays smears every partial and reads as
    /// off-grid energy..
    #[arg(long = "reduce", value_name = "R")]
    reduce: Option<String>,

    /// Fundamental in Hz for `--reduce sfdr` / `--reduce thd`.
    ///
    /// Pins what the estimator would otherwise guess. Worth setting whenever
    /// the fundamental is known: a signal whose loudest partial is not the
    /// fundamental — a bright pluck, a filtered saw — is misread without it.
    #[arg(long = "f0", value_name = "HZ")]
    f0: Option<f64>,

    /// Set a control at an exact frame: `--at FRAME PATH=VALUE` (repeatable).
    ///
    /// The render splits its block so the change lands on the requested frame
    /// rather than the next block boundary.
    #[arg(long = "at", value_names = ["FRAME", "PATH=VALUE"], num_args = 2)]
    ats: Vec<String>,

    /// Play a note: `PITCH[:VEL]@ON[..OFF]` (repeatable). Requires `--nvoices` > 0.
    ///
    /// Velocity defaults to 100. Omitting `..OFF` holds the note to the end of
    /// the render, which is how an attack is measured without a release in the
    /// way.
    #[arg(long = "note", value_name = "PITCH[:VEL]@ON[..OFF]")]
    notes: Vec<String>,

    /// Play several pitches at once: `P1,P2,...[:VEL]@ON[..OFF]` (repeatable).
    #[arg(long = "chord", value_name = "P1,P2,...[:VEL]@ON[..OFF]")]
    chords: Vec<String>,

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
    /// `impulse-cranelift`, which this phase must not disturb.
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
    if !args.ats.is_empty() {
        offenders.push("--at");
    }
    if !args.notes.is_empty() || !args.chords.is_empty() {
        offenders.push("--note/--chord");
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
    let schedule = build_schedule(args)?;

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
        // Apply what is due exactly here, then shorten the block so the next
        // event also lands on a boundary — the note timing is what a release
        // measurement reads, so rounding it to the block grid would put a
        // systematic error straight into the result.
        for event in schedule.at(written) {
            match event {
                Event::NoteOn { pitch, velocity } => {
                    poly.key_on(*pitch, *velocity);
                }
                Event::NoteOff { pitch } => {
                    poly.key_off(*pitch, false);
                }
                Event::SetParam { path, value } => poly.set_all(path, *value)?,
            }
        }
        let mut n = args.block.min(args.render - written);
        if let Some(next) = schedule.next_after(written)
            && next > written
        {
            n = n.min(next - written);
        }
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
    } else {
        // Same rule as the scalar path: under `--quiet` the statistics are the
        // output and go to stdout; otherwise they annotate a dump that already
        // owns stdout, and belong on stderr.
        let emit = |line: String| {
            if args.quiet {
                println!("{line}");
            } else {
                eprintln!("{line}");
            }
        };
        emit(format!(
            "# frames={} sr={} nvoices={} active_voices={}",
            args.render,
            args.sr,
            args.nvoices,
            poly.active_voice_count()
        ));
        for ch in 0..poly.outputs() {
            emit(format!(
                "# out{ch}: peak={:.9} rms={:.9}",
                peak[ch],
                (sum_sq[ch] / denom).sqrt()
            ));
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
    let schedule = build_schedule(&args)?;
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
        schedule: schedule.clone(),
        drive_buttons: impulse_test,
    };

    let points = cartesian(&axes);
    let sweeping = !axes.is_empty();
    // A sweep produces one row per point. In CSV that row *is* the output —
    // the swept values and what each render reduced to — so the per-frame dump
    // is suppressed. `.ir` describes exactly one render and cannot hold a
    // sweep at all.
    if sweeping && args.format == Format::Ir {
        return Err("--sweep cannot be combined with --format ir".to_owned());
    }
    let sweep_csv = sweeping && args.format == Format::Csv;
    if sweep_csv && !args.quiet {
        let mut header: Vec<String> = axes.iter().map(|a| a.path.clone()).collect();
        for ch in 0..probe.outputs() {
            match reduction {
                Some(r) => header.push(format!("{r}_out{ch}")),
                None => {
                    header.push(format!("peak_out{ch}"));
                    header.push(format!("rms_out{ch}"));
                    header.push(format!("dc_out{ch}"));
                }
            }
        }
        println!("{}", header.join(","));
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

        let header_needed = !args.quiet && args.format != Format::Json && !sweep_csv;
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
        let want_samples = matches!(
            reduction,
            Some(Reduction::F0 | Reduction::Sfdr | Reduction::Thd)
        );
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
            if args.quiet || args.format == Format::Json || sweep_csv {
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
        // `impulse-cranelift` exits 0 there, and the probe must match it to be
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
                        json_number(reduce_channel(
                            r,
                            &stats,
                            ch,
                            &collected,
                            probe.sample_rate(),
                            args.f0,
                        ))
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
        } else if sweep_csv {
            let mut row: Vec<String> = point
                .assignments
                .iter()
                .map(|(_, v)| format!("{v}"))
                .collect();
            for ch in 0..probe.outputs() {
                match reduction {
                    Some(r) => row.push(format!(
                        "{:.9}",
                        reduce_channel(r, &stats, ch, &collected, probe.sample_rate(), args.f0)
                    )),
                    None => {
                        row.push(format!("{:.9}", stats.channels[ch].peak));
                        row.push(format!("{:.9}", stats.channels[ch].rms));
                        row.push(format!("{:.9}", stats.channels[ch].dc));
                    }
                }
            }
            println!("{}", row.join(","));
        } else {
            // With `--quiet` the statistics are the whole output, so they go to
            // stdout and can be redirected; otherwise they annotate a dump that
            // already owns stdout, and belong on stderr.
            let emit = |line: String| {
                if args.quiet {
                    println!("{line}");
                } else {
                    eprintln!("{line}");
                }
            };
            emit(format!(
                "# frames={} sr={} window={}..{} ({} frames)",
                args.render,
                args.sr,
                stats.window_start,
                stats.window_start + stats.window_len,
                stats.window_len
            ));
            for (ch, channel) in stats.channels.iter().enumerate() {
                emit(format!(
                    "# out{ch}: peak={:.9} rms={:.9} dc={:.9} finite={}",
                    channel.peak,
                    channel.rms,
                    channel.dc,
                    if channel.finite { "yes" } else { "no" }
                ));
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

/// Collect `--at`, `--note` and `--chord` into one ordered schedule.
///
/// # Errors
/// Returns the first parse failure, naming the offending argument.
fn build_schedule(args: &Args) -> Result<Schedule, String> {
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
    // stack, as `impulse-cranelift` and the differential tests do.
    let result = thread::Builder::new()
        .name("faustprobe".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run(args))
        .expect("spawn worker thread")
        .join()
        .expect("join worker thread");

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("faustprobe: {error}");
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
