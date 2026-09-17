//! `faustprobe` command-line entry point.
//!
//! See the crate documentation for why this exists alongside the two impulse
//! runners, and `porting/faustprobe-generic-test-tool-design-2026-08-14-en.md`
//! for the full design.

use std::process::ExitCode;
use std::thread;
use std::time::Instant;

use clap::{ArgAction, Parser, ValueEnum};

use cranelift_ffi::probe::audio_file::read_channels;
use cranelift_ffi::probe::audio_out::SampleWriter;
use cranelift_ffi::probe::compare::{Comparison, Samples, Tolerance, compare};
use cranelift_ffi::probe::engine::{Factory, PolyProbe, Probe, RenderSpec, last_compile_failure};
use cranelift_ffi::probe::eval::{EvalProgram, csv_field};
use cranelift_ffi::probe::freqresp::{self, Grid, Property};
use cranelift_ffi::probe::number::{NumberFormat, Precision};
use cranelift_ffi::probe::params::{ControlKind, ControlMap};
use cranelift_ffi::probe::poly;
use cranelift_ffi::probe::protocol;
use cranelift_ffi::probe::render::{InputMode, RenderStats};
use cranelift_ffi::probe::schedule::{Event, Schedule, parse_at, parse_chord, parse_note};
use cranelift_ffi::probe::spectrum::{dominant_frequency, sfdr_db, thd_db};
use cranelift_ffi::probe::sweep::{Reduction, cartesian, parse_axis, parse_reduction};
use cranelift_ffi::probe::timing::{BlockTimer, Timing, WorstBlock, human_seconds};
use cranelift_ffi::probe::train::{self, BoundStats, FdCheck, Optimizer, TrainSpec};

/// How rendered frames are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// `frame,out0,out1`, each number the shortest text that parses back to
    /// the same float (see `--precision`) — the default, pipeable.
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
#[command(
    name = "faustprobe",
    version = concat!(
        env!("CARGO_PKG_VERSION"),
        "\nCopyright (C) 2002-2026, GRAME - Centre National de Creation Musicale. All rights reserved."
    ),
    about,
    long_about = None,
    disable_version_flag = true
)]
struct Args {
    /// Faust DSP source file.
    file: String,

    /// Evaluate EXPR in the scope of FILE and probe that instead of FILE's
    /// `process` (repeatable: the expressions' outputs side by side).
    ///
    /// FILE may be a `.lib`, which has no `process`: the expression sees its
    /// definitions unprefixed, as inside the library, and its imports. A
    /// question about a sub-expression costs a command, not a file:
    /// `--eval 'absorb_pole(1709, 2.0, 0.5)' -n 1 --in zero jot.lib`. The
    /// expressions head the CSV columns (`EXPR[j]` for one with several
    /// outputs) and everything else applies unchanged: `--set`, `--sweep`,
    /// `--list-params`, `--out`, `--train`. A compile error keeps the file's
    /// line numbers, and one in an expression is located in `<eval k>`.
    #[arg(long = "eval", value_name = "EXPR")]
    evals: Vec<String>,

    /// Print the version and the copyright notice (`-v`, as with faust-rs).
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    version: (),

    /// Add a Faust library import directory (repeatable).
    #[arg(short = 'I', long = "import-dir", value_name = "DIR")]
    import_dirs: Vec<String>,

    /// Compile and execute with double-precision samples.
    #[arg(long)]
    double: bool,

    /// Cranelift optimisation level.
    #[arg(long, default_value_t = 0)]
    opt_level: i32,

    /// Samples one `rad` block reverse tape holds (`-bra-tape N` of the
    /// compiler): the largest `--block` over which the gradients of a
    /// `rad` through delays and recursions are exact. A power of two.
    #[arg(long, default_value_t = 8192)]
    bra_tape: usize,

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
    /// arbitrarily. With `--train` / `--fd-check`, a trained control's value
    /// is the descent's starting point instead of its initial value, and any
    /// other control's is a fixed value, rewritten on every instance and
    /// after every `--reset-per-block`.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    sets: Vec<String>,

    /// Accept a `--set`, `--sweep` or `--at` value outside its control's
    /// range by clamping it, and say so.
    ///
    /// Without this flag such a value is an error: a Faust host never writes
    /// outside a widget's range, so the request is nearly always a typo, and
    /// a render clamped in silence is labelled with a value it never used.
    /// With it, each clamp is reported (`# clamped PATH: 7 -> 1`, a `clamped`
    /// array per JSON run) and a sweep's rows carry the applied value.
    #[arg(long)]
    clamp: bool,

    /// Input excitation: zero, impulse, impulse:CH, dc, `white[:SEED]`, sine:HZ,
    /// `file:PATH[:CH]` (a .wav, .f64 or .f32 file; input i reads channel i, a
    /// mono file feeds every input, `:CH` picks one channel for all).
    #[arg(long = "in", value_name = "MODE", default_value = "impulse")]
    input: String,

    /// Exclude the first N frames from both the dump and the statistics.
    #[arg(long, default_value_t = 0)]
    skip: usize,

    /// Print one frame out of N.
    #[arg(long, default_value_t = 1)]
    every: usize,

    /// List the discovered controls and bargraphs, with their kind, and exit.
    #[arg(long)]
    list_params: bool,

    /// Add the bargraphs to the rendered rows: one column per bargraph in the
    /// per-frame CSV dump and in a sweep's rows.
    ///
    /// A bargraph is written by the program, once per sample, and read here
    /// after each compute block: a row carries the value at the end of the
    /// block its frame belongs to, so the time resolution is `--block`.
    /// Without this flag the bargraphs' values at the end of the render are
    /// still reported, with the statistics and in the JSON document.
    #[arg(long)]
    bargraphs: bool,

    /// Print statistics only, no per-frame dump.
    #[arg(long)]
    quiet: bool,

    /// Text of the numbers: `full`, the shortest text that parses back to the
    /// same float at the width the program was compiled in (the default), or
    /// a number of fixed decimals.
    ///
    /// `--precision 9` is the text this tool printed before it had the flag.
    /// Fixed decimals lose small values: `3.3e-8` prints `0.000000033`.
    #[arg(long, value_name = "N|full")]
    precision: Option<String>,

    /// Write the rendered window to FILE and print the statistics only:
    /// `.npy` (shape frames x outputs), `.wav` (IEEE float), or `.f64` /
    /// `.f32` (raw, one output), at the width the program was compiled in.
    ///
    /// For a long render read by a script: binary, exact, and what `--in
    /// file:` reads back. Every frame of the window is written; `--every`
    /// thins the text dump, which this replaces.
    #[arg(long = "out", value_name = "FILE")]
    out: Option<String>,

    /// Compare the render with that of OTHER, a second program compiled in the
    /// same process and rendered under the same excitation, schedule and
    /// window: per output, the largest difference and where, and **the first
    /// frame beyond the tolerance**. Exit status 1 beyond it.
    ///
    /// For "this change must not alter a sample", two routes to one result, a
    /// preset against the adjustable program set to its values. `--set` applies
    /// to both programs (a trailing fragment resolves in each) and must resolve
    /// in both; `--set-a` and `--set-b` address FILE or OTHER alone.
    #[arg(long = "compare", value_name = "OTHER")]
    compare: Option<String>,

    /// Compare the render with the samples of FILE (`.npy`, `.wav`, `.f64`,
    /// `.f32`: what `--out` writes), which must hold the same window.
    #[arg(long = "ref", value_name = "FILE")]
    reference: Option<String>,

    /// `--set` for FILE only, under `--compare` (repeatable).
    #[arg(long = "set-a", value_name = "PATH=VALUE")]
    set_a: Vec<String>,

    /// `--set` for OTHER only, under `--compare` (repeatable).
    #[arg(long = "set-b", value_name = "PATH=VALUE")]
    set_b: Vec<String>,

    /// Largest accepted `|a - b|` in a comparison. Default 0, and with no
    /// `--rel-tolerance` either, agreement is bit equality.
    #[arg(long = "tolerance", value_name = "ABS")]
    tolerance: Option<f64>,

    /// Tolerance relative to the reference's peak on each output, added to
    /// `--tolerance`.
    #[arg(long = "rel-tolerance", value_name = "REL")]
    rel_tolerance: Option<f64>,

    /// Outputs a comparison or a check looks at (default: all), e.g. `0` for
    /// the loss lane of a `rad` program, whose gradient lanes are defined per
    /// block and do move with the block size.
    #[arg(long = "compare-outputs", value_name = "N,...", value_delimiter = ',')]
    compare_outputs: Vec<usize>,

    /// Check an invariant of the render (repeatable): `block[=N1,N2,...]`, the
    /// same samples at other block sizes (default 1, 7 and 512); `reset`, the
    /// same samples again after a reset of the instance, which sweeps and
    /// `--reset-per-block` rely on; `determinism`, the same samples from a
    /// second compilation; `width`, the distance between the single and the
    /// double precision render; `all`.
    ///
    /// The first three must hold to the tolerance (bit equality by default)
    /// and fail the command otherwise, naming the first differing frame.
    /// `width` is a report, and a gate only when a tolerance is given.
    #[arg(long = "check", value_name = "CHECK")]
    checks: Vec<String>,

    /// Fail when a sample of the window exceeds LEVEL in magnitude, and say
    /// at which frame and output first.
    ///
    /// A feedback loop that leaves its stable region runs away for thousands
    /// of frames before it turns non-finite; this catches it at the start, and
    /// makes "stays bounded under these control changes" an exit status.
    #[arg(long = "fail-above", value_name = "LEVEL")]
    fail_above: Option<f64>,

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

    /// Train these controls by gradient descent, the host loop of a `rad`
    /// program whose loss and gradients leave the graph: per block of
    /// `--block` frames, the loss lane and the gradient lanes are averaged,
    /// the optimizer steps the controls (kept in their range), and the next
    /// block runs on the same instance. Comma-separated exact paths or
    /// unique suffixes, in the order of their gradient lanes. Prints one
    /// CSV row per block (`--every` thins them): block, loss, the controls.
    #[arg(long = "train", value_name = "CONTROLS", value_delimiter = ',')]
    train: Vec<String>,

    /// Output lane of the per-sample loss.
    #[arg(long = "loss-lane", default_value_t = 0)]
    loss_lane: usize,

    /// Output lane of the first control's gradient; the others follow it.
    #[arg(long = "grad-lane", default_value_t = 1)]
    grad_lane: usize,

    /// Update rule of `--train`.
    #[arg(long, value_enum, default_value_t = OptimizerKind::Adam)]
    optimizer: OptimizerKind,

    /// Learning rate of `--train`.
    #[arg(long, default_value_t = 0.01)]
    lr: f64,

    /// Number of blocks, one step each, of `--train`.
    #[arg(long, default_value_t = 100)]
    blocks: usize,

    /// Start every `--train` block from a cleared state and from frame 0 of
    /// the excitation: one pass over the same response per block, an
    /// offline calibration with one epoch per block (with `--in file:` the
    /// response is the file). Without it the blocks are the successive
    /// stretches of one stream and the state carries over.
    #[arg(long = "reset-per-block")]
    reset_per_block: bool,

    /// Check the gradient lanes of the `--train` controls against central
    /// finite differences of the loss lane, on one block from a fresh
    /// instance per evaluation. Fails when a relative error exceeds
    /// `--fd-tolerance`.
    ///
    /// `--fd-check` alone, or `--fd-check=start`, checks at the descent's
    /// starting point, before it (or instead of it, with `--blocks 0`);
    /// `--fd-check=end` at the trained values, after it, where a descent is
    /// finished only if the gradient is small *and* right; `--fd-check=both`
    /// does both.
    #[arg(
        long = "fd-check",
        value_enum,
        value_name = "WHERE",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "start"
    )]
    fd_check: Option<FdWhere>,

    /// Step of the finite differences.
    #[arg(long = "fd-step", default_value_t = 1e-3)]
    fd_step: f64,

    /// Largest accepted `|rad - fd| / max(|fd|, 1)`.
    #[arg(long = "fd-tolerance", default_value_t = 0.02)]
    fd_tolerance: f64,

    /// Add the block's mean gradient, one `grad_CONTROL` column per trained
    /// control, to each row of `--train`.
    ///
    /// A control that stops moving has a vanishing gradient (a flat loss, or
    /// a minimum) or a vanishing step (a learning rate too small for the
    /// gradient's scale); the controls alone do not say which.
    #[arg(long = "train-verbose")]
    train_verbose: bool,

    /// Report what the run cost: the compilation, the `compute` calls against
    /// real time, and the worst block against its own deadline.
    ///
    /// Behind a flag because these are the only numbers here that differ from
    /// one run to the next. What is timed is `compute` alone: not the
    /// excitation, not the statistics, not the printing of the rows.
    #[arg(long = "time")]
    time: bool,

    /// The frequency response of a linear program, from its impulse
    /// response: `N` log-spaced frequencies from 20 Hz to half the sample
    /// rate, or `N:FMIN:FMAX`. Rows `hz,mag_db_out0,phase_out0,...`, the
    /// phase in radians.
    ///
    /// The transform of the `-n` frames of the response is evaluated at each
    /// frequency by direct summation, so no bin grid decides where the
    /// response is known. Three more renders first check that the program is
    /// linear and time-invariant (an impulse of -0.5, one delayed, a sum of
    /// two): one that is not is refused, with the first frame at which the
    /// property breaks, since its impulse response has no transfer function
    /// to give. The share of the energy in the last tenth of the window says
    /// whether the response was still ringing when it was cut.
    #[arg(long = "freqresp", value_name = "N[:FMIN:FMAX]")]
    freqresp: Option<String>,

    /// Frames of silence rendered before the impulse of `--freqresp`, which
    /// then lands on frame N: the time a smoothed control needs to reach its
    /// value.
    ///
    /// A program that smooths its sliders (`si.smoo`) is time-varying until
    /// they have settled, and the time-invariance check refuses it, rightly:
    /// a response taken during the ramp is that of no filter. The response
    /// and its `-n` frames are counted from the impulse.
    #[arg(long = "settle", value_name = "N", default_value_t = 0)]
    settle: usize,

    /// Largest accepted departure from linearity under `--freqresp`, relative
    /// to the expected response's peak (default 1e-9 in double precision,
    /// 1e-4 in single).
    #[arg(long = "linearity-tolerance", value_name = "REL")]
    linearity_tolerance: Option<f64>,

    /// How a compile failure is reported: `human`, the compiler's rendered
    /// diagnostics on stderr (the default), or `json`, the compiler's
    /// diagnostics-v2 report on stdout (code, ranges, facts,
    /// machine-applicable fixes) and the one-line summary on stderr.
    ///
    /// Any other failure (a value out of range, a non-finite render, a failed
    /// comparison) is reported as text either way.
    #[arg(long = "error-format", value_enum, default_value_t = ErrorFormat::Human)]
    error_format: ErrorFormat,
}

/// Where `--fd-check` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FdWhere {
    /// At the descent's starting point, before it.
    Start,
    /// At the trained values, after the descent.
    End,
    /// At both.
    Both,
}

/// How a compile failure is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ErrorFormat {
    /// The compiler's rendered diagnostics, on stderr.
    Human,
    /// The compiler's diagnostics-v2 JSON report, on stdout.
    Json,
}

/// The update rule of `--train`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OptimizerKind {
    /// Adam with the paper's betas (0.9, 0.999) and epsilon 1e-8.
    Adam,
    /// Plain gradient descent.
    Sgd,
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
    if args.bargraphs {
        offenders.push("--bargraphs");
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
    // `.ir` has its own number text and is the whole output: a flag that
    // would be ignored there is refused rather than ignored.
    if args.precision.is_some() {
        offenders.push("--precision");
    }
    if args.out.is_some() {
        offenders.push("--out");
    }
    if args.fail_above.is_some() {
        offenders.push("--fail-above");
    }
    if !args.evals.is_empty() {
        offenders.push("--eval");
    }
    if verification_requested(args) {
        offenders.push("--compare/--ref/--check");
    }
    if args.freqresp.is_some() {
        offenders.push("--freqresp");
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
fn parse_input_at(spec: &str, sr: i32) -> Result<InputMode, String> {
    let input = parse_input(spec)?;
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

/// The program to probe: FILE, or under `--eval` the expressions evaluated in
/// FILE's scope. With the wrapped file when there is one, which is what
/// labels the outputs and explains a compile error.
fn compile_program(args: &Args, double: bool) -> Result<(Factory, Option<EvalProgram>), String> {
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
fn output_labels(
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

/// Whether `--compare`, `--ref` or `--check` was given.
fn verification_requested(args: &Args) -> bool {
    args.compare.is_some() || args.reference.is_some() || !args.checks.is_empty()
}

/// An invariant `--check` verifies on the render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// The same samples at this block size.
    Block(usize),
    /// The same samples again after a reset of the instance.
    Reset,
    /// The same samples from a second compilation.
    Determinism,
    /// The distance between the two sample widths.
    Width,
}

impl Check {
    fn name(self) -> String {
        match self {
            Self::Block(size) => format!("block={size}"),
            Self::Reset => "reset".to_owned(),
            Self::Determinism => "determinism".to_owned(),
            Self::Width => "width".to_owned(),
        }
    }
}

/// Block sizes `--check block` tries when none is given: a sample at a time,
/// a size that divides nothing, and a large one.
const DEFAULT_CHECK_BLOCKS: [usize; 3] = [1, 7, 512];

/// Parses the `--check` occurrences. `block` sizes equal to the render's own
/// are dropped: that render is what the others are compared with.
fn parse_checks(specs: &[String], own_block: usize) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();
    let mut push = |check: Check| {
        if check != Check::Block(own_block) && !checks.contains(&check) {
            checks.push(check);
        }
    };
    for spec in specs {
        match spec.as_str() {
            "reset" => push(Check::Reset),
            "determinism" => push(Check::Determinism),
            "width" => push(Check::Width),
            "block" => DEFAULT_CHECK_BLOCKS
                .into_iter()
                .for_each(|n| push(Check::Block(n))),
            "all" => {
                DEFAULT_CHECK_BLOCKS
                    .into_iter()
                    .for_each(|n| push(Check::Block(n)));
                push(Check::Reset);
                push(Check::Determinism);
                push(Check::Width);
            }
            other => {
                let sizes = other.strip_prefix("block=").ok_or_else(|| {
                    format!(
                        "unknown check `{other}` (expected block[=N1,N2,...], reset, determinism, width or all)"
                    )
                })?;
                for size in sizes.split(',') {
                    match size.trim().parse::<usize>() {
                        Ok(n) if n > 0 => push(Check::Block(n)),
                        _ => {
                            return Err(format!("`--check {other}`: `{size}` is not a block size"));
                        }
                    }
                }
            }
        }
    }
    Ok(checks)
}

/// Renders `probe` from a cleared instance with `sets` written, keeping the
/// window's samples. A reference that is not finite cannot be compared with.
fn render_for_comparison(
    probe: &Probe,
    spec: &RenderSpec,
    sets: &[(&str, f64)],
    what: &str,
) -> Result<Samples, String> {
    probe.reset();
    for (path, value) in sets {
        probe.set(path, *value)?;
    }
    let (stats, samples) = probe.collect(spec);
    if let Some((channel, located)) = stats.first_non_finite() {
        return Err(format!(
            "{what} produced non-finite samples: first at frame {}, out{channel} ({})",
            located.frame,
            non_finite_name(located.value)
        ));
    }
    Ok(samples)
}

/// One comparison, as the lines printed with the statistics. `gate` says
/// whether a disagreement fails the command or is only reported (`--check
/// width` without a tolerance).
fn comparison_lines(
    tag: &str,
    comparison: &Comparison,
    gate: bool,
    fmt: &NumberFormat,
) -> Vec<String> {
    comparison
        .channels
        .iter()
        .map(|(ch, diff)| {
            if diff.identical {
                return format!("# {tag} out{ch}: identical");
            }
            let at = diff.max_abs_at.map_or_else(
                || "a non-finite sample".to_owned(),
                |frame| format!("frame {frame}"),
            );
            let verdict = match diff.first_beyond {
                Some(d) if gate => format!(
                    "first beyond tolerance: frame {} ({} vs {})",
                    d.frame,
                    fmt.sample(d.value),
                    fmt.sample(d.reference)
                ),
                Some(d) => format!("first difference: frame {}", d.frame),
                None => "within tolerance".to_owned(),
            };
            format!(
                "# {tag} out{ch}: max_abs={} at {at}, max_rel={}, {verdict}",
                fmt.computed(diff.max_abs),
                fmt.computed(diff.max_rel)
            )
        })
        .collect()
}

/// One comparison, for the JSON document.
fn comparison_json(comparison: &Comparison) -> serde_json::Value {
    let channels: Vec<serde_json::Value> = comparison
        .channels
        .iter()
        .map(|(ch, diff)| {
            serde_json::json!({
                "output": ch,
                "identical": diff.identical,
                "max_abs": json_number(diff.max_abs),
                "max_abs_at": diff.max_abs_at,
                "max_rel": json_number(diff.max_rel),
                "first_beyond": diff.first_beyond.map(|d| serde_json::json!({
                    "frame": d.frame,
                    "value": json_number(d.value),
                    "reference": json_number(d.reference),
                })),
            })
        })
        .collect();
    serde_json::json!({ "agrees": comparison.agrees(), "channels": channels })
}

/// The number text of this run: `--precision`, at the program's width.
fn number_format(args: &Args) -> Result<NumberFormat, String> {
    let precision = match &args.precision {
        Some(text) => Precision::parse(text)?,
        None => Precision::RoundTrip,
    };
    Ok(NumberFormat::new(precision, args.double))
}

/// A value that was clamped under `--clamp`, for the notices.
#[derive(Debug, Clone, PartialEq)]
struct Clamped {
    path: String,
    requested: f64,
    applied: f64,
}

impl Clamped {
    fn line(&self) -> String {
        format!(
            "# clamped {}: {} -> {}",
            self.path, self.requested, self.applied
        )
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path,
            "requested": json_number(self.requested),
            "applied": json_number(self.applied),
        })
    }
}

/// Validates one write before any render and returns the value the control
/// takes. Outside the range: an error, or under `--clamp` a recorded clamp.
fn check_value(
    controls: &ControlMap,
    query: &str,
    value: f64,
    clamp: bool,
    clamped: &mut Vec<Clamped>,
) -> Result<f64, String> {
    let write = controls.check_write(query, value)?;
    if !write.in_range() {
        if !clamp {
            return Err(format!(
                "{} (--clamp accepts it, clamped to the range)",
                write.range_error(query)
            ));
        }
        clamped.push(Clamped {
            path: write.control.path.clone(),
            requested: value,
            applied: write.applied,
        });
    }
    Ok(write.applied)
}

/// Facts that explain a render whose every output is exactly zero.
///
/// Facts, not guesses: what the tool knows and the statistics do not show.
fn silence_notes(probe: &Probe, input: &InputMode) -> Vec<String> {
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

/// What a failed render is explained with: where it failed, the controls
/// written by then, and the last scheduled write before it.
fn failure_context(
    frame: usize,
    written: &[(String, f64)],
    schedule: &Schedule,
    controls: &ControlMap,
) -> String {
    // the value of each written control at `frame`: its `--set` or sweep
    // value, then every scheduled write up to that frame
    let mut then: Vec<(String, f64)> = written.to_vec();
    let mut last_event = None;
    for (at, query, value) in schedule.param_writes() {
        if at > frame {
            break;
        }
        let Ok(write) = controls.check_write(query, value) else {
            continue;
        };
        let path = write.control.path.clone();
        match then.iter_mut().find(|(p, _)| *p == path) {
            Some(entry) => entry.1 = write.applied,
            None => then.push((path.clone(), write.applied)),
        }
        last_event = Some((at, path, write.applied));
    }
    let mut text = String::new();
    if then.is_empty() {
        text.push_str("\n  controls then: all at their initial values");
    } else {
        let listed: Vec<String> = then.iter().map(|(p, v)| format!("{p}={v}")).collect();
        text.push_str(&format!(
            "\n  controls written by then: {}",
            listed.join(" ")
        ));
    }
    if let Some((at, path, value)) = last_event {
        text.push_str(&format!(
            "\n  last scheduled write before it: frame {at}, {path}={value}"
        ));
    }
    text
}

/// Three significant digits of a ratio: `27.6`, `1523`, `0.84`.
fn three_digits(value: f64) -> String {
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
fn time_lines(
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
fn timing_json(timing: &Timing) -> serde_json::Value {
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
fn non_finite_name(value: f64) -> &'static str {
    if value.is_nan() {
        "NaN"
    } else if value > 0.0 {
        "+inf"
    } else {
        "-inf"
    }
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
/// Compiler arguments the probe forwards verbatim.
fn compiler_args(args: &Args) -> Vec<String> {
    let mut out = Vec::new();
    if args.bra_tape != 8192 {
        out.push("-bra-tape".to_owned());
        out.push(args.bra_tape.to_string());
    }
    out
}

fn run_poly(args: &Args) -> Result<(), String> {
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
    for (flag, set) in [
        ("--out", args.out.is_some()),
        ("--fail-above", args.fail_above.is_some()),
        ("--clamp", args.clamp),
        ("--eval", !args.evals.is_empty()),
        ("--compare/--ref/--check", verification_requested(args)),
        ("--freqresp", args.freqresp.is_some()),
    ] {
        if set {
            return Err(format!(
                "{flag} operates on the scalar Probe only; use --nvoices 0"
            ));
        }
    }
    let fmt = number_format(args)?;

    let compile_started = Instant::now();
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
    // the voices and the effect: everything a poly render compiles
    let compile_seconds = compile_started.elapsed().as_secs_f64();
    let mut timer = args.time.then(|| BlockTimer::new(f64::from(args.sr)));

    if args.list_params {
        println!(
            "{} voice(s), {} input(s)/voice, {} output(s), effect: {}",
            poly.voice_count(),
            poly.inputs(),
            poly.outputs(),
            if poly.has_effect() { "yes" } else { "no" }
        );
        println!(
            "{:<44} {:<9} {:>10} {:>10} {:>10} {:>10}",
            "path (per voice)", "kind", "init", "min", "max", "step"
        );
        for control in poly.voice_controls().iter() {
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
        // the voices, their mix and the effect: what a host's callback runs
        let started = timer.is_some().then(Instant::now);
        let block_out = poly.compute(n);
        if let (Some(timer), Some(started)) = (timer.as_mut(), started) {
            timer.record(written, n, started.elapsed());
        }
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
                    line.push_str(&fmt.sample(channel[j]));
                }
                println!("{line}");
            }
        }
        written += n;
    }

    let denom = counted.max(1) as f64;
    let timing = timer.map(BlockTimer::finish);
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
        let mut document = document;
        if let Some(timing) = &timing {
            let mut json = timing_json(timing);
            json["compile_s"] = json_number(compile_seconds);
            document["timing"] = json;
        }
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
                "# out{ch}: peak={} rms={}",
                fmt.sample(peak[ch]),
                fmt.computed((sum_sq[ch] / denom).sqrt())
            ));
        }
        // A poly render is silent until a note plays: say which it is.
        if counted > 0 && peak.iter().all(|p| *p == 0.0) {
            emit("# note: every output is exactly zero over the window".to_owned());
            if !schedule.needs_poly() {
                emit(
                    "# note: no --note or --chord is scheduled: every voice stays free".to_owned(),
                );
            }
        }
        if let Some(timing) = &timing {
            for line in time_lines(compile_seconds, timing, |w| format!("frame {}", w.frame)) {
                emit(line);
            }
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
    if args.train_verbose && args.train.is_empty() {
        return Err("--train-verbose adds the gradients to the rows of --train".to_owned());
    }
    // The report's ranges are byte offsets in the source that was compiled,
    // which under `--eval` is the file wrapped and followed by the
    // expressions: a fix applied to FILE at those offsets would land
    // elsewhere. The human text is rewritten for that case; the report is not.
    if args.error_format == ErrorFormat::Json && !args.evals.is_empty() {
        return Err(
            "--error-format json cannot be combined with --eval: the report's offsets would be \
             those of the wrapped source, not of FILE"
                .to_owned(),
        );
    }
    if args.nvoices > 0 {
        return run_poly(&args);
    }
    if !args.train.is_empty() || args.fd_check.is_some() {
        return run_train(&args);
    }
    if args.linearity_tolerance.is_some() && args.freqresp.is_none() {
        return Err("--linearity-tolerance is the tolerance of --freqresp's checks".to_owned());
    }
    if args.settle != 0 && args.freqresp.is_none() {
        return Err(
            "--settle delays the impulse of --freqresp; a plain render has --skip".to_owned(),
        );
    }
    if args.freqresp.is_some() {
        return run_freqresp(&args);
    }

    let compile_started = Instant::now();
    let (factory, eval) = compile_program(&args, args.double)?;
    let compile_seconds = compile_started.elapsed().as_secs_f64();
    let factory = std::rc::Rc::new(factory);
    let probe = Probe::instantiate(&factory, args.sr)?;

    let fmt = number_format(&args)?;
    if args.list_params {
        println!(
            "{:<44} {:<9} {:>10} {:>10} {:>10} {:>10}",
            "path", "kind", "init", "min", "max", "step"
        );
        for control in probe.controls().iter() {
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
    // `--set` and, under `--compare`, the values of FILE alone
    let fixed = args
        .sets
        .iter()
        .chain(&args.set_a)
        .map(|a| parse_assignment(a))
        .collect::<Result<Vec<_>, _>>()?;
    // Everything a render will write is checked before any render: a
    // bargraph resolves like a control but is an output the program
    // overwrites, and the scheduled writes of `--at` ignore their errors
    // inside the render loop, so an unknown or unwritable path would
    // otherwise pass in silence.
    //
    // A value outside its control's range is checked here too: the render
    // clamps it, and a run at the clamped value labelled with the requested
    // one looks like a measurement of the requested one. It is an error, or
    // under `--clamp` a clamp that is reported.
    // The clamps every render runs under (`--set`, `--at`), and those of the
    // sweep's values, each of which concerns the points that use it.
    let mut clamped: Vec<Clamped> = Vec::new();
    let mut clamped_axes: Vec<(usize, Clamped)> = Vec::new();
    for (path, value) in &fixed {
        check_value(probe.controls(), path, *value, args.clamp, &mut clamped)?;
    }
    for (_, path, value) in schedule.param_writes() {
        check_value(probe.controls(), path, value, args.clamp, &mut clamped)?;
    }
    for (index, axis) in axes.iter().enumerate() {
        let mut of_axis = Vec::new();
        for value in &axis.values {
            check_value(
                probe.controls(),
                &axis.path,
                *value,
                args.clamp,
                &mut of_axis,
            )?;
        }
        clamped_axes.extend(of_axis.into_iter().map(|c| (index, c)));
    }
    if args.bargraphs && args.format == Format::Ir {
        return Err("--bargraphs cannot be combined with --format ir".to_owned());
    }
    // The bargraphs' paths, for the headers; their values are read after each
    // block (`Probe::bargraphs`, in the same order).
    let bargraph_paths: Vec<String> = probe.bargraphs().into_iter().map(|(p, _)| p).collect();
    // What each output is: `outN`, or under `--eval` the expression it computes.
    let labels = output_labels(&args, eval.as_ref(), probe.outputs())?;
    let legend: Vec<String> = if eval.is_some() {
        labels
            .iter()
            .enumerate()
            .map(|(ch, label)| format!("# eval out{ch} = {label}"))
            .collect()
    } else {
        Vec::new()
    };

    // `--compare`, `--ref`, `--check`: validated like everything else before
    // any render. The second program is compiled and its own writes checked
    // here; the reference file is read here.
    let verifying = verification_requested(&args);
    if verifying {
        for (flag, set) in [
            ("--sweep", !axes.is_empty()),
            ("--format ir", args.format == Format::Ir),
        ] {
            if set {
                return Err(format!(
                    "{flag} cannot be combined with --compare, --ref or --check, which look at one render"
                ));
            }
        }
    }
    if args.compare.is_some() && args.reference.is_some() {
        return Err("--compare and --ref both name the reference: give one".to_owned());
    }
    if args.compare.is_none() && (!args.set_a.is_empty() || !args.set_b.is_empty()) {
        return Err("--set-a and --set-b address the two programs of --compare".to_owned());
    }
    for (flag, value) in [
        ("--tolerance", args.tolerance),
        ("--rel-tolerance", args.rel_tolerance),
    ] {
        if value.is_some_and(|v| !(v >= 0.0 && v.is_finite())) {
            return Err(format!("{flag} must be a non-negative number"));
        }
    }
    let tolerance = Tolerance {
        abs: args.tolerance.unwrap_or(0.0),
        rel: args.rel_tolerance.unwrap_or(0.0),
    };
    let explicit_tolerance = args.tolerance.is_some() || args.rel_tolerance.is_some();
    let compared_outputs =
        (!args.compare_outputs.is_empty()).then_some(args.compare_outputs.as_slice());
    if let Some(&beyond) = args
        .compare_outputs
        .iter()
        .find(|&&ch| ch >= probe.outputs())
    {
        return Err(format!(
            "--compare-outputs {beyond}: the program has {} output(s)",
            probe.outputs()
        ));
    }
    let checks = parse_checks(&args.checks, args.block)?;
    // the second program, with the values it alone is given
    let other_sets = args
        .sets
        .iter()
        .chain(&args.set_b)
        .map(|a| parse_assignment(a))
        .collect::<Result<Vec<_>, _>>()?;
    let other = match &args.compare {
        Some(path) => {
            let other_factory = Factory::compile_with_args(
                path,
                &args.import_dirs,
                &compiler_args(&args),
                args.double,
                args.opt_level,
            )
            .map_err(|error| format!("--compare: {error}"))?;
            let other = Probe::instantiate(&std::rc::Rc::new(other_factory), args.sr)?;
            if other.outputs() != probe.outputs() {
                return Err(format!(
                    "--compare: `{}` has {} output(s) and `{path}` {}",
                    args.file,
                    probe.outputs(),
                    other.outputs()
                ));
            }
            // what is written to it is checked as for the first program
            for (query, value) in &other_sets {
                check_value(other.controls(), query, *value, args.clamp, &mut clamped)
                    .map_err(|error| format!("--compare `{path}`: {error}"))?;
            }
            for (_, query, value) in schedule.param_writes() {
                check_value(other.controls(), query, value, args.clamp, &mut clamped)
                    .map_err(|error| format!("--compare `{path}`: {error}"))?;
            }
            Some(other)
        }
        None => None,
    };
    let reference_file = match &args.reference {
        Some(path) => {
            let (channels, _) = read_channels(std::path::Path::new(path))?;
            Some(Samples {
                start: args.skip,
                channels,
            })
        }
        None => None,
    };

    let spec = RenderSpec {
        frames: args.render,
        block: args.block,
        input: parse_input_at(&args.input, args.sr)?,
        skip: args.skip,
        schedule: schedule.clone(),
        drive_buttons: impulse_test,
        limit: args.fail_above,
        time: args.time,
    };

    let points = cartesian(&axes);
    let sweeping = !axes.is_empty();
    if args.out.is_some() {
        for (flag, set) in [
            ("--sweep", sweeping),
            ("--format ir", args.format == Format::Ir),
            ("--every", args.every != 1),
        ] {
            if set {
                return Err(format!(
                    "{flag} cannot be combined with --out, which writes every frame of one render"
                ));
            }
        }
    }
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
        if args.bargraphs {
            header.extend(bargraph_paths.iter().cloned());
        }
        println!("{}", header.join(","));
    }
    // Where the annotations of a render go: under `--quiet` they are the
    // output (stdout, redirectable); otherwise they annotate a dump or a
    // sweep's rows, which own stdout, and belong on stderr.
    let annotate = |line: String| {
        if args.quiet && !sweep_csv && args.format != Format::Ir {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    };
    // A sweep's rows and an `.ir` text have no statistics block to carry the
    // clamps: say them once, before the rows.
    if sweep_csv || args.format == Format::Ir {
        for clamp in clamped.iter().chain(clamped_axes.iter().map(|(_, c)| c)) {
            annotate(clamp.line());
        }
    }
    // A sweep's columns keep their `REDUCTION_outN` names, which scripts key
    // on; what each `outN` is goes before the rows.
    if sweep_csv {
        for line in &legend {
            annotate(line.clone());
        }
    }

    let every = args.every.max(1);
    let mut runs: Vec<serde_json::Value> = Vec::new();
    let mut silent_points = 0usize;
    // `--time` over the renders that have no statistics block of their own
    // to carry it: a sweep's rows, an `.ir` text
    let mut total_timing: Option<Timing> = None;
    // a comparison or a check that failed: reported after the output, which
    // carries its details
    let mut verification_failure: Option<String> = None;

    for point in &points {
        // Every point starts from the same known state (see probe::sweep).
        probe.reset();
        for (path, value) in &fixed {
            probe.set(path, *value)?;
        }
        for (path, value) in &point.assignments {
            probe.set(path, *value)?;
        }

        let mut writer = match &args.out {
            Some(path) => Some(SampleWriter::create(
                std::path::Path::new(path),
                probe.outputs(),
                args.render.saturating_sub(args.skip),
                args.double,
                args.sr,
            )?),
            None => None,
        };
        let dumping = !args.quiet && writer.is_none();
        let header_needed = dumping && args.format != Format::Json && !sweep_csv;
        if header_needed {
            match args.format {
                Format::Csv => {
                    print!("frame");
                    for label in &labels {
                        print!(",{}", csv_field(label));
                    }
                    if args.bargraphs {
                        for path in &bargraph_paths {
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

        // `f0` needs the samples, so collect them only when it is asked for.
        let want_samples = verifying
            || matches!(
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
            if let Some(writer) = writer.as_mut() {
                writer.push(samples);
            }
            if !dumping || args.format == Format::Json || sweep_csv {
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
                        line.push_str(&fmt.sample(*value));
                    }
                    if args.bargraphs {
                        // read after the block this frame belongs to was
                        // computed: the value at that block's last sample
                        for (_, value) in probe.bargraphs() {
                            line.push(',');
                            line.push_str(&fmt.sample(value));
                        }
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
        if let Some(writer) = writer {
            writer.finish()?;
        }
        // The controls this render wrote before its first frame, for the
        // context of a failure; the applied values, as the render used them.
        let written_controls = || -> Vec<(String, f64)> {
            fixed
                .iter()
                .map(|(path, value)| (*path, *value))
                .chain(point.assignments.iter().map(|(p, v)| (p.as_str(), *v)))
                .filter_map(|(query, value)| probe.controls().check_write(query, value).ok())
                .map(|write| (write.control.path.clone(), write.applied))
                .collect()
        };
        // A runaway is reported where it starts: a loop that leaves its
        // stable region passes any level long before it overflows, so the
        // sample above `--fail-above` comes first, and the non-finite frame
        // that follows is mentioned with it.
        let non_finite = (args.format != Format::Ir)
            .then(|| stats.first_non_finite())
            .flatten();
        if let Some((channel, located)) = stats.first_above()
            && non_finite.is_none_or(|(_, nf)| located.frame <= nf.frame)
        {
            let later = non_finite.map_or_else(String::new, |(ch, nf)| {
                format!(
                    "\n  the render turns non-finite at frame {}, out{ch} ({})",
                    nf.frame,
                    non_finite_name(nf.value)
                )
            });
            return Err(format!(
                "a sample exceeds --fail-above {}\n  first: frame {}, out{channel} = {}{later}{}",
                args.fail_above.unwrap_or_default(),
                located.frame,
                fmt.sample(located.value),
                failure_context(
                    located.frame,
                    &written_controls(),
                    &schedule,
                    probe.controls()
                )
            ));
        }
        if let Some((channel, located)) = non_finite {
            return Err(format!(
                "render produced non-finite samples\n  first: frame {}, out{channel} ({}); {} of {} frames affected{}",
                located.frame,
                non_finite_name(located.value),
                stats.non_finite_frames,
                args.render,
                failure_context(
                    located.frame,
                    &written_controls(),
                    &schedule,
                    probe.controls()
                )
            ));
        }
        // Exact silence is nearly always a gate never pressed or an input
        // never fed: the facts the tool has go with the numbers.
        let notes = if stats.is_silent() {
            silent_points += 1;
            silence_notes(&probe, &spec.input)
        } else {
            Vec::new()
        };
        // the clamps this render ran under: the fixed and scheduled ones, and
        // those of its own sweep values (the axes and a point's assignments
        // are in the same order)
        let run_clamped: Vec<&Clamped> = clamped
            .iter()
            .chain(clamped_axes.iter().filter_map(|(axis, c)| {
                (point.assignments.get(*axis).map(|(_, v)| v.to_bits())
                    == Some(c.requested.to_bits()))
                .then_some(c)
            }))
            .collect();
        // what the program's bargraphs show at the end of this render
        let bargraphs = probe.bargraphs();

        // ── --compare / --ref / --check ──────────────────────────────────
        // After the bargraphs were read: `--check reset` renders again on
        // this instance.
        let mut verify_lines: Vec<String> = Vec::new();
        let mut verify_json = serde_json::Map::new();
        if verifying {
            let render = Samples {
                start: spec.skip,
                channels: collected.clone(),
            };
            let mut failed = |what: String, comparison: &Comparison| {
                if verification_failure.is_none()
                    && let Some((channel, d)) = comparison.first_beyond()
                {
                    verification_failure = Some(format!(
                        "{what}\n  first: frame {}, out{channel}: {} vs {}{}",
                        d.frame,
                        fmt.sample(d.value),
                        fmt.sample(d.reference),
                        failure_context(d.frame, &written_controls(), &schedule, probe.controls())
                    ));
                }
            };
            let reference = match (&other, &reference_file, &args.compare, &args.reference) {
                (Some(other), _, Some(path), _) => {
                    let sets: Vec<(&str, f64)> = other_sets.clone();
                    Some((
                        path.clone(),
                        render_for_comparison(other, &spec, &sets, &format!("`{path}`"))?,
                    ))
                }
                (_, Some(samples), _, Some(path)) => Some((path.clone(), samples.clone())),
                _ => None,
            };
            if let Some((name, reference)) = reference {
                let comparison = compare(&render, &reference, tolerance, compared_outputs)
                    .map_err(|error| format!("cannot compare with `{name}`: {error}"))?;
                verify_lines.push(format!(
                    "# compare: against {name}, tolerance abs={} rel={}",
                    tolerance.abs, tolerance.rel
                ));
                verify_lines.extend(comparison_lines("compare", &comparison, true, &fmt));
                let mut json = comparison_json(&comparison);
                json["reference"] = serde_json::json!(name);
                verify_json.insert("compare".to_owned(), json);
                if !comparison.agrees() {
                    failed(
                        format!("the render differs from `{name}` beyond the tolerance"),
                        &comparison,
                    );
                }
            }
            let mut checks_json = Vec::new();
            for check in &checks {
                let tag = format!("check {}", check.name());
                let (samples, gate, limit) = match check {
                    Check::Block(size) => {
                        let fresh = Probe::instantiate(&factory, args.sr)?;
                        let at_size = RenderSpec {
                            block: *size,
                            ..spec.clone()
                        };
                        let samples = render_for_comparison(&fresh, &at_size, &fixed, &tag)?;
                        (samples, true, tolerance)
                    }
                    Check::Reset => (
                        render_for_comparison(&probe, &spec, &fixed, &tag)?,
                        true,
                        tolerance,
                    ),
                    Check::Determinism => {
                        let (again, _) = compile_program(&args, args.double)?;
                        let same_key = again.sha_key() == factory.sha_key();
                        verify_lines.push(format!(
                            "# {tag}: a second compilation gives {} program key",
                            if same_key { "the same" } else { "ANOTHER" }
                        ));
                        let fresh = Probe::instantiate(&std::rc::Rc::new(again), args.sr)?;
                        // two compilations of one source owe each other the very bits
                        (
                            render_for_comparison(&fresh, &spec, &fixed, &tag)?,
                            true,
                            Tolerance::default(),
                        )
                    }
                    Check::Width => {
                        let (other_width, _) = compile_program(&args, !args.double)?;
                        let fresh = Probe::instantiate(&std::rc::Rc::new(other_width), args.sr)?;
                        verify_lines.push(format!(
                            "# {tag}: this render in {} precision against the {} one{}",
                            if args.double { "double" } else { "single" },
                            if args.double { "single" } else { "double" },
                            if explicit_tolerance {
                                ""
                            } else {
                                " (a report: no tolerance given)"
                            }
                        ));
                        (
                            render_for_comparison(&fresh, &spec, &fixed, &tag)?,
                            explicit_tolerance,
                            tolerance,
                        )
                    }
                };
                let comparison = compare(&render, &samples, limit, compared_outputs)
                    .map_err(|error| format!("{tag}: {error}"))?;
                verify_lines.extend(comparison_lines(&tag, &comparison, gate, &fmt));
                let mut json = comparison_json(&comparison);
                json["check"] = serde_json::json!(check.name());
                json["gate"] = serde_json::json!(gate);
                checks_json.push(json);
                if gate && !comparison.agrees() {
                    failed(
                        format!("--{tag} failed: the render is not the same"),
                        &comparison,
                    );
                }
            }
            if !checks_json.is_empty() {
                verify_json.insert("checks".to_owned(), serde_json::Value::Array(checks_json));
            }
        }

        if args.format == Format::Json {
            let mut entry = serde_json::Map::new();
            let mut set = serde_json::Map::new();
            for (path, value) in &point.assignments {
                // the value the render used
                let applied = probe
                    .controls()
                    .check_write(path, *value)
                    .map_or(*value, |w| w.applied);
                set.insert(path.clone(), json_number(applied));
            }
            entry.insert("set".to_owned(), serde_json::Value::Object(set));
            if !run_clamped.is_empty() {
                entry.insert(
                    "clamped".to_owned(),
                    serde_json::Value::Array(run_clamped.iter().map(|c| c.json()).collect()),
                );
            }
            if !notes.is_empty() {
                entry.insert("notes".to_owned(), serde_json::json!(notes));
            }
            for (key, value) in verify_json {
                entry.insert(key, value);
            }
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
                            "peak_at": c.peak_at,
                            "subnormal": c.subnormal,
                            "subnormal_at": c.subnormal_at,
                        })
                    })
                    .collect();
                entry.insert("channels".to_owned(), serde_json::Value::Array(channels));
            }
            if !bargraphs.is_empty() {
                let mut shown = serde_json::Map::new();
                for (path, value) in &bargraphs {
                    shown.insert(path.clone(), json_number(*value));
                }
                entry.insert("bargraphs".to_owned(), serde_json::Value::Object(shown));
            }
            if let Some(timing) = &stats.timing {
                entry.insert("timing".to_owned(), timing_json(timing));
            }
            runs.push(serde_json::Value::Object(entry));
        } else if args.format == Format::Ir {
            // The .ir text is compared byte for byte; emit nothing else.
        } else if sweep_csv {
            // the value the render used: the requested one, unless `--clamp`
            // clamped it, and then the row must not claim the requested one
            let mut row: Vec<String> = point
                .assignments
                .iter()
                .map(|(query, v)| {
                    let applied = probe
                        .controls()
                        .check_write(query, *v)
                        .map_or(*v, |w| w.applied);
                    format!("{applied}")
                })
                .collect();
            for ch in 0..probe.outputs() {
                match reduction {
                    Some(r) => {
                        let value =
                            reduce_channel(r, &stats, ch, &collected, probe.sample_rate(), args.f0);
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
            if args.bargraphs {
                row.extend(bargraphs.iter().map(|(_, v)| fmt.sample(*v)));
            }
            println!("{}", row.join(","));
        } else {
            // With `--quiet` or `--out` the statistics are the whole output,
            // so they go to stdout and can be redirected; otherwise they
            // annotate a dump that already owns stdout, and belong on stderr.
            let emit = |line: String| {
                if dumping {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
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
            for line in &legend {
                emit(line.clone());
            }
            for (ch, channel) in stats.channels.iter().enumerate() {
                // `peak_at` comes last: a reader keyed on the older fields
                // does not see it
                // and `subnormal` only when there is one: it costs CPU on a
                // target that does not flush them, and is where a tail ends
                let subnormal = channel.subnormal_at.map_or_else(String::new, |frame| {
                    format!(" subnormal={} subnormal_at={frame}", channel.subnormal)
                });
                emit(format!(
                    "# out{ch}: peak={} rms={} dc={} finite={} peak_at={}{subnormal}",
                    fmt.sample(channel.peak),
                    fmt.computed(channel.rms),
                    fmt.computed(channel.dc),
                    if channel.finite { "yes" } else { "no" },
                    channel
                        .peak_at
                        .map_or_else(|| "none".to_owned(), |frame| frame.to_string())
                ));
            }
            for (path, value) in &bargraphs {
                emit(format!("# bargraph {path}={}", fmt.sample(*value)));
            }
            for clamp in &run_clamped {
                emit(clamp.line());
            }
            for note in &notes {
                emit(format!("# note: {note}"));
            }
            for line in &verify_lines {
                emit(line.clone());
            }
            if let Some(timing) = &stats.timing {
                for line in time_lines(compile_seconds, timing, |w| format!("frame {}", w.frame)) {
                    emit(line);
                }
            }
        }
        if let Some(timing) = &stats.timing {
            match total_timing.as_mut() {
                Some(total) => total.absorb(timing),
                None => total_timing = Some(timing.clone()),
            }
        }
    }

    // A sweep's rows and an `.ir` text: one account for all the renders.
    if (sweep_csv || args.format == Format::Ir)
        && let Some(timing) = &total_timing
    {
        if points.len() > 1 {
            annotate(format!("# time: {} renders", points.len()));
        }
        for line in time_lines(compile_seconds, timing, |w| format!("frame {}", w.frame)) {
            annotate(line);
        }
    }

    // A sweep's rows show their zeros; what they do not show is why. One
    // render's notes stand for all when every point was silent.
    if sweep_csv && silent_points == points.len() {
        for note in silence_notes(&probe, &spec.input) {
            annotate(format!("# note: {note} (at every sweep point)"));
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
        let mut document = document;
        if eval.is_some()
            && let Some(object) = document.as_object_mut()
        {
            // what each output, in order, computes
            object.insert("eval".to_owned(), serde_json::json!(labels));
        }
        if args.time
            && let Some(object) = document.as_object_mut()
        {
            // each run carries the cost of its own `compute` calls
            object.insert(
                "timing".to_owned(),
                serde_json::json!({ "compile_s": json_number(compile_seconds) }),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?
        );
    }

    // The output above carries the details of a comparison or a check that
    // failed; the verdict is the exit status.
    verification_failure.map_or(Ok(()), Err)
}

/// One channel of a rendered window, reduced to a single number.
///
/// Shared by the JSON and CSV sweep paths so the two cannot report different
/// numbers for the same render.
/// `--freqresp`: the frequency response of a linear program from one impulse
/// response, after the checks that it is one (see `probe::freqresp`).
fn run_freqresp(args: &Args) -> Result<(), String> {
    let spec_text = args.freqresp.as_deref().unwrap_or_default();
    for (flag, set, why) in [
        (
            "--sweep",
            !args.sweeps.is_empty(),
            "one response per command",
        ),
        (
            "--reduce",
            args.reduce.is_some(),
            "the response is the reduction",
        ),
        (
            "--at",
            !args.ats.is_empty(),
            "a control that changes during the response makes the program time-varying",
        ),
        (
            "--note/--chord",
            !args.notes.is_empty() || !args.chords.is_empty(),
            "they drive the polyphonic wrapper",
        ),
        (
            "--skip",
            args.skip != 0,
            "the transform is that of the whole response, from frame 0",
        ),
        (
            "--every",
            args.every != 1,
            "the rows are frequencies, not frames",
        ),
        (
            "--bargraphs",
            args.bargraphs,
            "the rows are frequencies, not frames",
        ),
        (
            "--out",
            args.out.is_some(),
            "a plain render writes the impulse response",
        ),
        (
            "--fail-above",
            args.fail_above.is_some(),
            "it gates a plain render",
        ),
        (
            "--compare/--ref/--check",
            verification_requested(args),
            "they look at the samples of a plain render",
        ),
        (
            "--format ir",
            args.format == Format::Ir,
            "`.ir` holds frames",
        ),
    ] {
        if set {
            return Err(format!("{flag} cannot be combined with --freqresp: {why}"));
        }
    }
    let grid = Grid::parse(spec_text, f64::from(args.sr))?;
    let tolerance = match args.linearity_tolerance {
        Some(value) if value >= 0.0 && value.is_finite() => value,
        Some(_) => return Err("--linearity-tolerance must be a non-negative number".to_owned()),
        None => freqresp::default_tolerance(args.double),
    };
    if args.render < 4 {
        return Err(
            "--freqresp needs a window of at least 4 frames (-n) for its linearity checks"
                .to_owned(),
        );
    }
    let fmt = number_format(args)?;

    let compile_started = Instant::now();
    let (factory, eval) = compile_program(args, args.double)?;
    let compile_seconds = compile_started.elapsed().as_secs_f64();
    let probe = Probe::instantiate(&std::rc::Rc::new(factory), args.sr)?;
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
    let mut clamped = Vec::new();
    let fixed = args
        .sets
        .iter()
        .map(|a| parse_assignment(a))
        .collect::<Result<Vec<_>, _>>()?;
    for (path, value) in &fixed {
        check_value(probe.controls(), path, *value, args.clamp, &mut clamped)?;
    }
    let labels = output_labels(args, eval.as_ref(), probe.outputs())?;

    // One render per excitation, each from a cleared instance with the
    // `--set` controls written: `--settle` frames of silence, then the
    // impulses, the window being the `-n` frames that follow.
    let render_taps =
        |taps: Vec<(usize, f64)>, time: bool| -> Result<(RenderStats, Samples), String> {
            probe.reset();
            for (path, value) in &fixed {
                probe.set(path, *value)?;
            }
            let spec = RenderSpec {
                frames: args.settle + args.render,
                block: args.block,
                input: InputMode::Impulses {
                    channel,
                    taps: taps
                        .into_iter()
                        .map(|(frame, amplitude)| (args.settle + frame, amplitude))
                        .collect(),
                },
                skip: args.settle,
                time,
                ..RenderSpec::default()
            };
            Ok(probe.collect(&spec))
        };
    let (stats, h) = render_taps(vec![(0, 1.0)], args.time)?;
    if let Some((output, located)) = stats.first_non_finite() {
        return Err(format!(
            "the impulse response is not finite\n  first: frame {}, out{output} ({}); {} of {} frames affected",
            located.frame,
            non_finite_name(located.value),
            stats.non_finite_frames,
            args.render
        ));
    }

    // ── is there a transfer function to measure? ─────────────────────────
    let shift = freqresp::shift_for(args.render);
    let mut deviations = Vec::new();
    for property in Property::ALL {
        let (_, answer) = render_taps(property.taps(shift), false)?;
        let expected = property.expected(&h, shift);
        let comparison = compare(
            &answer,
            &expected,
            Tolerance {
                abs: 0.0,
                rel: tolerance,
            },
            None,
        )?;
        if let Some((output, d)) = comparison.first_beyond() {
            let mut error = format!(
                "--freqresp: the program is not linear and time-invariant: its impulse response has no transfer function to give\n  \
                 {}\n  first: frame {}, out{output}: {} where {} was expected (tolerance {tolerance:e} of the peak)\n  \
                 usual cause: {}",
                property.violation(args.settle, shift),
                d.frame,
                fmt.sample(d.value),
                fmt.sample(d.reference),
                property.usual_cause()
            );
            // an output that does not come from the input is a fact the tool
            // can establish: what the program says to silence. Said when it
            // is of a size to explain the refusal: the 1e-20 a reverberator
            // injects against subnormals is not.
            let (silence, _) = render_taps(Vec::new(), false)?;
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
            return Err(error);
        }
        let worst = comparison
            .channels
            .iter()
            .map(|(_, diff)| diff.max_rel)
            .fold(0.0_f64, f64::max);
        deviations.push((property, worst));
    }

    // ── the response ─────────────────────────────────────────────────────
    let hz = grid.frequencies();
    let sample_rate = f64::from(args.sr);
    let responses: Vec<Vec<freqresp::Point>> = h
        .channels
        .iter()
        .map(|channel| freqresp::response(channel, sample_rate, &hz))
        .collect();
    let tails: Vec<Option<f64>> = h
        .channels
        .iter()
        .map(|channel| freqresp::tail_energy_fraction(channel))
        .collect();
    let notes: Vec<String> = tails
        .iter()
        .enumerate()
        .filter_map(|(output, tail)| {
            let tail = (*tail)?;
            (tail > freqresp::RINGING).then(|| {
                format!(
                    "out{output} is still ringing {} frames after the impulse: what -n cut off is of the order of the last tenth's share, and near a resonance the magnitude is off by about its square root ({}%); raise -n",
                    args.render,
                    three_digits(100.0 * tail.sqrt())
                )
            })
        })
        .collect();
    let excited = channel.map_or_else(
        || {
            if probe.inputs() == 1 {
                "the input".to_owned()
            } else {
                format!("all {} inputs at once", probe.inputs())
            }
        },
        |ch| format!("input {ch}"),
    );

    if args.format == Format::Json {
        let outputs: Vec<serde_json::Value> = responses
            .iter()
            .zip(&tails)
            .enumerate()
            .map(|(output, (points, tail))| {
                serde_json::json!({
                    "output": output,
                    "mag_db": points.iter().map(|p| json_number(p.magnitude_db)).collect::<Vec<_>>(),
                    "phase": points.iter().map(|p| json_number(p.phase)).collect::<Vec<_>>(),
                    "tail_energy_fraction": tail.map(json_number),
                    "peak": json_number(stats.channels[output].peak),
                })
            })
            .collect();
        let mut linearity = serde_json::Map::new();
        linearity.insert("tolerance".to_owned(), json_number(tolerance));
        linearity.insert("shift".to_owned(), serde_json::json!(shift));
        for (property, worst) in &deviations {
            linearity.insert(property.name().to_owned(), json_number(*worst));
        }
        let mut document = serde_json::json!({
            "schema_version": 1,
            "dsp": args.file,
            "sr": args.sr,
            "frames": args.render,
            "freqresp": {
                "input": channel,
                "settle": args.settle,
                "hz": hz.iter().map(|f| json_number(*f)).collect::<Vec<_>>(),
                "linearity": linearity,
                "outputs": outputs,
            },
        });
        if let Some(object) = document.as_object_mut() {
            if eval.is_some() {
                object.insert("eval".to_owned(), serde_json::json!(labels));
            }
            if !clamped.is_empty() {
                object.insert(
                    "clamped".to_owned(),
                    serde_json::Value::Array(clamped.iter().map(Clamped::json).collect()),
                );
            }
            if !notes.is_empty() {
                object.insert("notes".to_owned(), serde_json::json!(notes));
            }
            if let Some(timing) = &stats.timing {
                let mut timing = timing_json(timing);
                timing["compile_s"] = json_number(compile_seconds);
                object.insert("timing".to_owned(), timing);
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if !args.quiet {
        let mut header = vec!["hz".to_owned()];
        for output in 0..probe.outputs() {
            header.push(format!("mag_db_out{output}"));
            header.push(format!("phase_out{output}"));
        }
        println!("{}", header.join(","));
        for (k, frequency) in hz.iter().enumerate() {
            let mut row = vec![fmt.computed(*frequency)];
            for points in &responses {
                row.push(fmt.computed(points[k].magnitude_db));
                row.push(fmt.computed(points[k].phase));
            }
            println!("{}", row.join(","));
        }
    }
    // As for a render: under `--quiet` the annotations are the output.
    let emit = |line: String| {
        if args.quiet {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    };
    emit(format!(
        "# freqresp: {} frequenc{} from {} to {} Hz, from the response of {} frames to an impulse on {excited}{}",
        hz.len(),
        if hz.len() == 1 { "y" } else { "ies" },
        fmt.computed(grid.fmin),
        fmt.computed(grid.fmax),
        args.render,
        if args.settle == 0 {
            String::new()
        } else {
            format!(" at frame {}", args.settle)
        }
    ));
    if eval.is_some() {
        for (output, label) in labels.iter().enumerate() {
            emit(format!("# eval out{output} = {label}"));
        }
    }
    let shown: Vec<String> = deviations
        .iter()
        .map(|(property, worst)| format!("{} {}", property.name(), fmt.computed(*worst)))
        .collect();
    emit(format!(
        "# freqresp: linear and time-invariant within {tolerance:e} of the peak ({})",
        shown.join(", ")
    ));
    for (output, tail) in tails.iter().enumerate() {
        emit(match tail {
            Some(tail) => format!(
                "# freqresp out{output}: peak={} peak_at={}, the last tenth of the window holds {} of the energy",
                fmt.sample(stats.channels[output].peak),
                stats.channels[output]
                    .peak_at
                    .map_or_else(|| "none".to_owned(), |frame| frame.to_string()),
                fmt.computed(*tail)
            ),
            None => format!(
                "# freqresp out{output}: the response is exactly zero: nothing reaches this output from {excited}"
            ),
        });
    }
    for clamp in &clamped {
        emit(clamp.line());
    }
    for note in &notes {
        emit(format!("# note: {note}"));
    }
    if let Some(timing) = &stats.timing {
        for line in time_lines(compile_seconds, timing, |w| format!("frame {}", w.frame)) {
            emit(line);
        }
    }
    Ok(())
}

/// What the projection onto its range did to a trained control, for the
/// `# trained` line: nothing is said of a control that never met a bound.
fn bound_text(stats: &BoundStats, blocks: usize) -> Option<String> {
    let parts: Vec<String> = [
        (train::Bound::Lower, stats.on_lower),
        (train::Bound::Upper, stats.on_upper),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(bound, count)| {
        format!(
            "on its {} bound for {count} of {blocks} blocks, {}",
            bound.name(),
            if stats.ends_on == Some(bound) {
                "the last one included"
            } else {
                "not the last one"
            }
        )
    })
    .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// One `--fd-check`: its lines (none under `--format json`), its JSON, and
/// the error when a lane departs from the finite differences. `place` is
/// `start` or `end`; the lines of `start` are those the tool always printed.
fn fd_check_report(
    args: &Args,
    place: &str,
    checks: &[FdCheck],
) -> (Vec<String>, serde_json::Value, Option<String>) {
    let tag = if place == "start" {
        String::new()
    } else {
        format!(" {place}")
    };
    let mut worst = 0.0_f64;
    let mut lines = Vec::new();
    for check in checks {
        // at the end of a descent the gradient is small, which is the point:
        // six decimals would print it as zero
        lines.push(if place == "start" {
            format!(
                "# fd-check {}: rad {:.6} fd {:.6} relative error {:.2e}",
                check.path, check.rad, check.fd, check.relative_error
            )
        } else {
            format!(
                "# fd-check{tag} {}: rad {:.6e} fd {:.6e} relative error {:.2e}",
                check.path, check.rad, check.fd, check.relative_error
            )
        });
        // a NaN error is the worst there is, and `max` would drop it
        worst = if check.relative_error.is_nan() || worst.is_nan() {
            f64::NAN
        } else {
            worst.max(check.relative_error)
        };
    }
    lines.push(format!(
        "# fd-check{tag}: block {} frames, step {}, worst relative error {worst:.2e} (tolerance {})",
        args.block, args.fd_step, args.fd_tolerance
    ));
    let passes = worst <= args.fd_tolerance;
    let json = serde_json::json!({
        "block": args.block,
        "step": json_number(args.fd_step),
        "tolerance": json_number(args.fd_tolerance),
        "worst_relative_error": json_number(worst),
        "passes": passes,
        "checks": checks.iter().map(|check| serde_json::json!({
            "path": check.path,
            "rad": json_number(check.rad),
            "fd": json_number(check.fd),
            "fd_plain": json_number(check.fd_plain),
            "relative_error": json_number(check.relative_error),
        })).collect::<Vec<_>>(),
    });
    let failure = (!passes).then(|| {
        format!(
            "a gradient lane departs from finite differences by {worst:.2e} at the {place} of the descent, above --fd-tolerance {}",
            args.fd_tolerance
        )
    });
    (lines, json, failure)
}

/// `--train` and `--fd-check`: the host loop of a program whose loss and
/// gradient lanes leave the graph (see `probe::train`).
fn run_train(args: &Args) -> Result<(), String> {
    if args.train.is_empty() {
        return Err("--fd-check needs the controls to check: --train CONTROLS".to_owned());
    }
    for (flag, set) in [
        ("--reduce", args.reduce.is_some()),
        ("--at", !args.ats.is_empty()),
        ("--bargraphs", args.bargraphs),
        (
            "--protocol impulse-test",
            args.protocol == Protocol::ImpulseTest,
        ),
        ("--format ir", args.format == Format::Ir),
        ("--out", args.out.is_some()),
        ("--fail-above", args.fail_above.is_some()),
        ("--compare/--ref/--check", verification_requested(args)),
        ("--freqresp", args.freqresp.is_some()),
    ] {
        if set {
            return Err(format!(
                "{flag} cannot be combined with --train / --fd-check"
            ));
        }
    }
    let fd_at_start = matches!(args.fd_check, Some(FdWhere::Start | FdWhere::Both));
    let fd_at_end = matches!(args.fd_check, Some(FdWhere::End | FdWhere::Both));
    if fd_at_end && args.blocks == 0 {
        return Err(
            "--fd-check=end checks the trained values: it needs a descent, --blocks > 0".to_owned(),
        );
    }
    let as_json = args.format == Format::Json;
    let fmt = number_format(args)?;
    let compile_started = Instant::now();
    let factory = std::rc::Rc::new(compile_program(args, args.double)?.0);
    let compile_seconds = compile_started.elapsed().as_secs_f64();

    // The lines are the output as it comes, a descent being watched; the
    // JSON document is printed once, at the end or with the failure.
    let say = |line: String| {
        if !as_json {
            println!("{line}");
        }
    };
    let mut report = serde_json::Map::new();
    let finish = |report: serde_json::Map<String, serde_json::Value>| -> Result<(), String> {
        if !as_json {
            return Ok(());
        }
        let document = serde_json::json!({
            "schema_version": 1,
            "dsp": args.file,
            "sr": args.sr,
            "train": report,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?
        );
        Ok(())
    };

    // A `--set` outside its range: on a trained control it would move the
    // starting point, on another it would fix a value that is not the one
    // asked for, and the descent's rows would show neither. The values of a
    // `--sweep` are starting points and are checked alike.
    let mut clamped = Vec::new();
    let mut axes: Vec<(String, Vec<f64>)> = Vec::new();
    {
        let probe = Probe::instantiate(&factory, args.sr)?;
        for assignment in &args.sets {
            let (path, value) = parse_assignment(assignment)?;
            check_value(probe.controls(), path, value, args.clamp, &mut clamped)?;
        }
        for sweep in &args.sweeps {
            let axis = parse_axis(sweep)?;
            let applied = axis
                .values
                .iter()
                .map(|value| {
                    check_value(
                        probe.controls(),
                        &axis.path,
                        *value,
                        args.clamp,
                        &mut clamped,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            axes.push((axis.path, applied));
        }
    }
    for clamp in &clamped {
        say(clamp.line());
    }
    if !clamped.is_empty() {
        report.insert(
            "clamped".to_owned(),
            serde_json::Value::Array(clamped.iter().map(Clamped::json).collect()),
        );
    }
    let mut spec = TrainSpec {
        params: args.train.clone(),
        loss_lane: args.loss_lane,
        first_grad_lane: args.grad_lane,
        optimizer: match args.optimizer {
            OptimizerKind::Adam => Optimizer::ADAM,
            OptimizerKind::Sgd => Optimizer::Sgd,
        },
        lr: args.lr,
        block: args.block,
        blocks: args.blocks,
        input: parse_input_at(&args.input, args.sr)?,
        reset_per_block: args.reset_per_block,
        sets: args
            .sets
            .iter()
            .map(|a| parse_assignment(a).map(|(path, value)| (path.to_owned(), value)))
            .collect::<Result<Vec<_>, _>>()?,
    };
    report.insert(
        "options".to_owned(),
        serde_json::json!({
            "optimizer": match args.optimizer {
                OptimizerKind::Adam => "adam",
                OptimizerKind::Sgd => "sgd",
            },
            "lr": json_number(args.lr),
            "block": args.block,
            "blocks": args.blocks,
            "reset_per_block": args.reset_per_block,
        }),
    );

    // ── grid, then descent ───────────────────────────────────────────────
    // `--sweep` over trained controls: the loss of one block at every point,
    // and the descent leaves from the best. What a non-convex loss needs, and
    // what took a sweep, a parse and a second command.
    if !axes.is_empty() {
        let grid = train::grid(&factory, args.sr, &spec, &axes)?;
        let mut points = Vec::new();
        for (index, (values, loss)) in grid.points.iter().enumerate() {
            let at: Vec<String> = grid
                .paths
                .iter()
                .zip(values)
                .map(|(path, value)| format!("{path}={value}"))
                .collect();
            say(format!(
                "# grid {} loss={}{}",
                at.join(" "),
                fmt.loss(*loss),
                if index == grid.best { " (best)" } else { "" }
            ));
            let mut set = serde_json::Map::new();
            for (path, value) in grid.paths.iter().zip(values) {
                set.insert(path.clone(), json_number(*value));
            }
            points.push(serde_json::json!({ "set": set, "loss": json_number(*loss) }));
        }
        say(format!(
            "# grid: {} points, one block of {} frames each; the descent starts from the best",
            grid.points.len(),
            args.block
        ));
        report.insert(
            "grid".to_owned(),
            serde_json::json!({ "points": points, "best": grid.best }),
        );
        // after the `--set` values: on a control given both, the grid decides
        spec.sets.extend(grid.best_assignments());
    }

    let mut fd_reports = serde_json::Map::new();
    if fd_at_start {
        let checks = train::fd_check(&factory, args.sr, &spec, args.fd_step, None)?;
        let (lines, json, failure) = fd_check_report(args, "start", &checks);
        lines.into_iter().for_each(&say);
        fd_reports.insert("start".to_owned(), json);
        if let Some(failure) = failure {
            report.insert("fd_check".to_owned(), serde_json::Value::Object(fd_reports));
            finish(report)?;
            return Err(failure);
        }
    }
    if args.blocks == 0 {
        if !fd_reports.is_empty() {
            report.insert("fd_check".to_owned(), serde_json::Value::Object(fd_reports));
        }
        if args.time {
            let none = BlockTimer::new(f64::from(args.sr)).finish();
            time_lines(compile_seconds, &none, |_| String::new())
                .into_iter()
                .for_each(&say);
            report.insert(
                "timing".to_owned(),
                serde_json::json!({ "compile_s": json_number(compile_seconds) }),
            );
        }
        return finish(report);
    }

    let mut header_done = false;
    let mut rows = Vec::new();
    let every = args.every.max(1);
    let outcome = train::train(&factory, args.sr, &spec, |step| {
        if step.block % every != 0 && step.block != args.blocks {
            return;
        }
        if as_json {
            rows.push(serde_json::json!({
                "block": step.block,
                "loss": json_number(step.loss),
                "values": step.params.iter().map(|v| json_number(*v)).collect::<Vec<_>>(),
                "grads": step.grads.iter().map(|g| json_number(*g)).collect::<Vec<_>>(),
            }));
            return;
        }
        if !header_done {
            let mut header = format!("block,loss,{}", spec.params.join(","));
            if args.train_verbose {
                for param in &spec.params {
                    header.push_str(&format!(",grad_{param}"));
                }
            }
            println!("{header}");
            header_done = true;
        }
        let mut fields: Vec<String> = step.params.iter().map(|v| fmt.computed(*v)).collect();
        if args.train_verbose {
            fields.extend(step.grads.iter().map(|g| fmt.loss(*g)));
        }
        println!(
            "{},{},{}",
            step.block,
            fmt.loss(step.loss),
            fields.join(",")
        );
    });
    report.insert("rows".to_owned(), serde_json::Value::Array(rows));
    let trained = match outcome {
        Ok(trained) => trained,
        Err(error) => {
            // the rows up to the block that failed are the evidence
            finish(report)?;
            return Err(error);
        }
    };

    let mut notes = Vec::new();
    let mut trained_json = Vec::new();
    let mut stopped = Vec::new();
    for (k, path) in trained.paths.iter().enumerate() {
        let bounds = &trained.bounds[k];
        let bound =
            bound_text(bounds, args.blocks).map_or_else(String::new, |text| format!(" ({text})"));
        say(format!(
            "# trained {path}={}{bound}",
            fmt.computed(trained.values[k])
        ));
        if bounds.ends_on.is_some() {
            stopped.push(path.as_str());
        }
        trained_json.push(serde_json::json!({
            "path": path,
            "value": json_number(trained.values[k]),
            "min": json_number(trained.ranges[k].0),
            "max": json_number(trained.ranges[k].1),
            "blocks_on_lower": bounds.on_lower,
            "blocks_on_upper": bounds.on_upper,
            "ends_on": bounds.ends_on.map(train::Bound::name),
        }));
    }
    say(format!(
        "# loss: block 1 {:.6e}, block {} {:.6e}",
        trained.first_loss, args.blocks, trained.last_loss
    ));
    say(format!(
        "# loss: minimum {:.6e} at block {}",
        trained.min_loss, trained.min_loss_block
    ));
    if !stopped.is_empty() {
        notes.push(format!(
            "a control that ends on a bound is stopped, not converged: {}",
            stopped.join(" ")
        ));
    }
    // a ratio means something for a positive loss only
    if trained.min_loss > 0.0 && trained.last_loss > 10.0 * trained.min_loss {
        let then: Vec<String> = trained
            .paths
            .iter()
            .zip(&trained.values_at_min)
            .map(|(path, value)| format!("{path}={}", fmt.computed(*value)))
            .collect();
        notes.push(format!(
            "the last loss is {} times the minimum, which block {} reached with {}",
            three_digits(trained.last_loss / trained.min_loss),
            trained.min_loss_block,
            then.join(" ")
        ));
    }
    for note in &notes {
        say(format!("# note: {note}"));
    }
    report.insert("trained".to_owned(), serde_json::Value::Array(trained_json));
    report.insert(
        "loss".to_owned(),
        serde_json::json!({
            "first": json_number(trained.first_loss),
            "last": json_number(trained.last_loss),
            "min": json_number(trained.min_loss),
            "min_block": trained.min_loss_block,
            "values_at_min": trained.values_at_min.iter().map(|v| json_number(*v)).collect::<Vec<_>>(),
        }),
    );
    if !notes.is_empty() {
        report.insert("notes".to_owned(), serde_json::json!(notes));
    }

    // A descent is finished where the gradient is small *and* right: the
    // same check, at the trained values.
    let mut failure = None;
    if fd_at_end {
        let checks = train::fd_check(
            &factory,
            args.sr,
            &spec,
            args.fd_step,
            Some(&trained.values),
        )?;
        let (lines, json, failed) = fd_check_report(args, "end", &checks);
        lines.into_iter().for_each(&say);
        fd_reports.insert("end".to_owned(), json);
        failure = failed;
    }
    if !fd_reports.is_empty() {
        report.insert("fd_check".to_owned(), serde_json::Value::Object(fd_reports));
    }
    if args.time {
        time_lines(compile_seconds, &trained.timing, |worst| {
            format!("block {}", worst.frame / args.block.max(1) + 1)
        })
        .into_iter()
        .for_each(&say);
        let mut timing = timing_json(&trained.timing);
        timing["compile_s"] = json_number(compile_seconds);
        report.insert("timing".to_owned(), timing);
    }
    finish(report)?;
    failure.map_or(Ok(()), Err)
}

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
    let error_format = args.error_format;
    let result = thread::Builder::new()
        .name("faustprobe".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run(args).map_err(|error| compile_failure_report(error, error_format)))
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

/// Under `--error-format json`, a run that ends on a compile failure prints
/// the compiler's diagnostics-v2 report on stdout and keeps the first line of
/// the error, its summary, for stderr. Any other error is returned unchanged.
///
/// Called on the thread that compiled: the report is per thread. Whether the
/// error *is* the last compile failure is decided by its text, since a
/// failure can be recovered from (the polyphonic wrapper looks for an
/// `effect` and carries on without one) and the run end on something else.
fn compile_failure_report(error: String, format: ErrorFormat) -> String {
    if format != ErrorFormat::Json {
        return error;
    }
    let Some(failure) = last_compile_failure() else {
        return error;
    };
    match failure.diagnostics_json {
        Some(report) if error.contains(failure.text.as_str()) => {
            println!("{report}");
            error.lines().next().unwrap_or_default().to_owned()
        }
        _ => error,
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
