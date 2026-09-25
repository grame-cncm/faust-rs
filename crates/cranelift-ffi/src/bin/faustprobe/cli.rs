//! The command line: `Args`, the enums of its flags, and what a mode refuses.

use clap::{ArgAction, Parser, ValueEnum};

use cranelift_ffi::probe::poly;
use cranelift_ffi::probe::protocol;

/// How rendered frames are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
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
pub(crate) enum Protocol {
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
pub(crate) struct Args {
    /// Internal subprocess protocol for an independent determinism render.
    #[arg(long, hide = true)]
    pub(crate) determinism_worker: bool,

    /// Faust DSP source file.
    pub(crate) file: String,

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
    pub(crate) evals: Vec<String>,

    /// Print the version and the copyright notice (`-v`, as with faust-rs).
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    pub(crate) version: (),

    /// Add a Faust library import directory (repeatable).
    #[arg(short = 'I', long = "import-dir", value_name = "DIR")]
    pub(crate) import_dirs: Vec<String>,

    /// Compile and execute with double-precision samples.
    #[arg(long)]
    pub(crate) double: bool,

    /// Cranelift optimisation level.
    #[arg(long, default_value_t = 0)]
    pub(crate) opt_level: i32,

    /// Samples one `rad` block reverse tape holds (`-bra-tape N` of the
    /// compiler): the largest `--block` over which the gradients of a
    /// `rad` through delays and recursions are exact. A power of two.
    #[arg(long, default_value_t = 8192)]
    pub(crate) bra_tape: usize,

    /// Sample rate in Hz.
    #[arg(long, default_value_t = 44_100)]
    pub(crate) sr: i32,

    /// Frames per compute call.
    #[arg(long, default_value_t = 64)]
    pub(crate) block: usize,

    /// Frames to render.
    #[arg(short = 'n', long, default_value_t = 15_000)]
    pub(crate) render: usize,

    /// Set a control before rendering, as `PATH=VALUE` (repeatable).
    ///
    /// PATH may be a full address or a trailing fragment of one; an ambiguous
    /// fragment is reported with its candidates rather than resolved
    /// arbitrarily. With `--train` / `--fd-check`, a trained control's value
    /// is the descent's starting point instead of its initial value, and any
    /// other control's is a fixed value, rewritten on every instance and
    /// after every `--reset-per-block`.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub(crate) sets: Vec<String>,

    /// Accept a `--set`, `--sweep` or `--at` value outside its control's
    /// range by clamping it, and say so.
    ///
    /// Without this flag such a value is an error: a Faust host never writes
    /// outside a widget's range, so the request is nearly always a typo, and
    /// a render clamped in silence is labelled with a value it never used.
    /// With it, each clamp is reported (`# clamped PATH: 7 -> 1`, a `clamped`
    /// array per JSON run) and a sweep's rows carry the applied value.
    #[arg(long)]
    pub(crate) clamp: bool,

    /// Input excitation: zero, impulse, impulse:CH, dc, `white[:SEED]`, sine:HZ,
    /// `file:PATH[:CH]` (a .wav, .f64 or .f32 file; input i reads channel i, a
    /// mono file feeds every input, `:CH` picks one channel for all).
    ///
    /// `impulse:CH` must name an input the program has. Under `--nvoices`
    /// every playing voice receives the excitation, as in `poly-dsp.h`.
    #[arg(long = "in", value_name = "MODE", default_value = "impulse")]
    pub(crate) input: String,

    /// Exclude the first N frames from both the dump and the statistics.
    #[arg(long, default_value_t = 0)]
    pub(crate) skip: usize,

    /// Print one frame out of N.
    #[arg(long, default_value_t = 1)]
    pub(crate) every: usize,

    /// List the discovered controls and bargraphs, with their kind, and exit.
    #[arg(long)]
    pub(crate) list_params: bool,

    /// Add the bargraphs to the rendered rows: one column per bargraph in the
    /// per-frame CSV dump and in a sweep's rows.
    ///
    /// A bargraph is written by the program, once per sample, and read here
    /// after each compute block: a row carries the value at the end of the
    /// block its frame belongs to, so the time resolution is `--block`.
    /// Without this flag the bargraphs' values at the end of the render are
    /// still reported, with the statistics and in the JSON document.
    #[arg(long)]
    pub(crate) bargraphs: bool,

    /// Print statistics only, no per-frame dump.
    #[arg(long)]
    pub(crate) quiet: bool,

    /// Text of the numbers: `full`, the shortest text that parses back to the
    /// same float at the width the program was compiled in (the default), or
    /// a number of fixed decimals.
    ///
    /// `--precision 9` is the text this tool printed before it had the flag.
    /// Fixed decimals lose small values: `3.3e-8` prints `0.000000033`.
    #[arg(long, value_name = "N|full")]
    pub(crate) precision: Option<String>,

    /// Write the rendered window to FILE and print the statistics only:
    /// `.npy` (shape frames x outputs), `.wav` (IEEE float), or `.f64` /
    /// `.f32` (raw, one output), at the width the program was compiled in.
    ///
    /// For a long render read by a script: binary, exact, and what `--in
    /// file:` reads back. Every frame of the window is written; `--every`
    /// thins the text dump, which this replaces.
    #[arg(long = "out", value_name = "FILE")]
    pub(crate) out: Option<String>,

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
    pub(crate) compare: Option<String>,

    /// Compare the render with the samples of FILE (`.npy`, `.wav`, `.f64`,
    /// `.f32`: what `--out` writes), which must hold the same window.
    #[arg(long = "ref", value_name = "FILE")]
    pub(crate) reference: Option<String>,

    /// `--set` for FILE only, under `--compare` (repeatable).
    #[arg(long = "set-a", value_name = "PATH=VALUE")]
    pub(crate) set_a: Vec<String>,

    /// `--set` for OTHER only, under `--compare` (repeatable).
    #[arg(long = "set-b", value_name = "PATH=VALUE")]
    pub(crate) set_b: Vec<String>,

    /// Largest accepted `|a - b|` in a comparison. Default 0, and with no
    /// `--rel-tolerance` either, agreement is bit equality.
    #[arg(long = "tolerance", value_name = "ABS")]
    pub(crate) tolerance: Option<f64>,

    /// Tolerance relative to the reference's peak on each output, added to
    /// `--tolerance`.
    #[arg(long = "rel-tolerance", value_name = "REL")]
    pub(crate) rel_tolerance: Option<f64>,

    /// Outputs a comparison or a check looks at (default: all), e.g. `0` for
    /// the loss lane of a `rad` program, whose gradient lanes are defined per
    /// block and do move with the block size.
    #[arg(long = "compare-outputs", value_name = "N,...", value_delimiter = ',')]
    pub(crate) compare_outputs: Vec<usize>,

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
    pub(crate) checks: Vec<String>,

    /// Fail when a sample of the window exceeds LEVEL in magnitude, and say
    /// at which frame and output first.
    ///
    /// A feedback loop that leaves its stable region runs away for thousands
    /// of frames before it turns non-finite; this catches it at the start, and
    /// makes "stays bounded under these control changes" an exit status.
    #[arg(long = "fail-above", value_name = "LEVEL")]
    pub(crate) fail_above: Option<f64>,

    /// Output format for rendered frames.
    #[arg(long, value_enum, default_value_t = Format::Csv)]
    pub(crate) format: Format,

    /// Sweep a control over several values, as `PATH=V1,V2,...` (repeatable).
    ///
    /// Repeating the flag takes the cartesian product, with the last axis
    /// varying fastest. Every point renders from a cleared instance, so one
    /// configuration cannot contaminate the next.
    #[arg(long = "sweep", value_name = "PATH=V1,V2,...")]
    pub(crate) sweeps: Vec<String>,

    /// Reduce each render to one number per channel: rms, peak, energy, dc, f0,
    /// `sfdr` or `thd`.
    ///
    /// `sfdr` and `thd` need a fundamental. It is estimated from the strongest
    /// bin unless `--f0` says otherwise, and both want a stationary window:
    /// measuring while the spectrum decays smears every partial and reads as
    /// off-grid energy..
    #[arg(long = "reduce", value_name = "R")]
    pub(crate) reduce: Option<String>,

    /// Fundamental in Hz for `--reduce sfdr` / `--reduce thd`.
    ///
    /// Pins what the estimator would otherwise guess. Worth setting whenever
    /// the fundamental is known: a signal whose loudest partial is not the
    /// fundamental — a bright pluck, a filtered saw — is misread without it.
    #[arg(long = "f0", value_name = "HZ")]
    pub(crate) f0: Option<f64>,

    /// Set a control at an exact frame: `--at FRAME PATH=VALUE` (repeatable).
    ///
    /// The render splits its block so the change lands on the requested frame
    /// rather than the next block boundary.
    #[arg(long = "at", value_names = ["FRAME", "PATH=VALUE"], num_args = 2)]
    pub(crate) ats: Vec<String>,

    /// Play a note: `PITCH[:VEL]@ON[..OFF]` (repeatable). Requires `--nvoices` > 0.
    ///
    /// Velocity defaults to 100. Omitting `..OFF` holds the note to the end of
    /// the render, which is how an attack is measured without a release in the
    /// way.
    #[arg(long = "note", value_name = "PITCH[:VEL]@ON[..OFF]")]
    pub(crate) notes: Vec<String>,

    /// Play several pitches at once: `P1,P2,...[:VEL]@ON[..OFF]` (repeatable).
    #[arg(long = "chord", value_name = "P1,P2,...[:VEL]@ON[..OFF]")]
    pub(crate) chords: Vec<String>,

    /// Rendering protocol.
    ///
    /// `impulse-test` reproduces the reference protocol exactly — sample rate
    /// 44100, block 64, impulse on every input, buttons held for the first
    /// block, `.ir` output — and rejects any flag that would perturb it, so a
    /// regression run cannot be silently mis-configured.
    #[arg(long, value_enum, default_value_t = Protocol::Free)]
    pub(crate) protocol: Protocol,

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
    /// `--note` and `--chord` play it; without one every voice stays free and
    /// the render is silence. `--set` and `--at` broadcast: the control they
    /// name is written on every voice, and on the effect when it resolves
    /// there, each within its own range (an error outside it, or `--clamp`).
    /// What a note writes (its pitch's frequency, its gain, its gate) is
    /// written as computed, whatever the sliders declare, as in `poly-dsp.h`.
    #[arg(long = "nvoices", default_value_t = 0)]
    pub(crate) nvoices: usize,

    /// Separate effect DSP, run once on the voices' mixed output.
    ///
    /// Without this, a single-file instrument that declares both `process`
    /// and `effect` has its effect extracted automatically the way
    /// `FaustPolyDspGenerator` does — wrap the source in `environment{}` and
    /// take `dsp_code.effect` — and this flag is unnecessary; pass it to
    /// override that guess or to pair a process DSP with an effect declared
    /// in a different file. Requires `--nvoices` > 0.
    #[arg(long = "effect", value_name = "FILE")]
    pub(crate) effect: Option<String>,

    /// RMS level below which a releasing voice is reclaimed as free.
    ///
    /// Default `0.00003162` (-90 dB) is `poly-dsp.h`'s `VOICE_STOP_LEVEL` —
    /// the one number in the polyphonic wrapper with an audible consequence
    /// (design §3.2): too high truncates long releases, too low never
    /// reclaims a voice under sustained play. Requires `--nvoices` > 0.
    #[arg(long = "voice-stop-level", default_value_t = poly::DEFAULT_VOICE_STOP_LEVEL)]
    pub(crate) voice_stop_level: f64,

    /// Train these controls by gradient descent, the host loop of a `rad`
    /// program whose loss and gradients leave the graph: per block of
    /// `--block` frames, the loss lane and the gradient lanes are averaged,
    /// the optimizer steps the controls (kept in their range), and the next
    /// block runs on the same instance. Comma-separated exact paths or
    /// unique suffixes, in the order of their gradient lanes. Prints one
    /// CSV row per block (`--every` thins them): block, loss, the controls.
    #[arg(long = "train", value_name = "CONTROLS", value_delimiter = ',')]
    pub(crate) train: Vec<String>,

    /// Output lane of the per-sample loss.
    #[arg(long = "loss-lane", default_value_t = 0)]
    pub(crate) loss_lane: usize,

    /// Output lane of the first control's gradient; the others follow it.
    #[arg(long = "grad-lane", default_value_t = 1)]
    pub(crate) grad_lane: usize,

    /// Update rule of `--train`.
    #[arg(long, value_enum, default_value_t = OptimizerKind::Adam)]
    pub(crate) optimizer: OptimizerKind,

    /// Learning rate of `--train`.
    #[arg(long, default_value_t = 0.01)]
    pub(crate) lr: f64,

    /// Number of blocks, one step each, of `--train`.
    #[arg(long, default_value_t = 100)]
    pub(crate) blocks: usize,

    /// Start every `--train` block from a cleared state and from frame 0 of
    /// the excitation: one pass over the same response per block, an
    /// offline calibration with one epoch per block (with `--in file:` the
    /// response is the file). Without it the blocks are the successive
    /// stretches of one stream and the state carries over.
    #[arg(long = "reset-per-block")]
    pub(crate) reset_per_block: bool,

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
    pub(crate) fd_check: Option<FdWhere>,

    /// Step of the finite differences.
    #[arg(long = "fd-step", default_value_t = 1e-3)]
    pub(crate) fd_step: f64,

    /// Largest accepted `|rad - fd| / max(|fd|, 1)`.
    #[arg(long = "fd-tolerance", default_value_t = 0.02)]
    pub(crate) fd_tolerance: f64,

    /// Add the block's mean gradient, one `grad_CONTROL` column per trained
    /// control, to each row of `--train`.
    ///
    /// A control that stops moving has a vanishing gradient (a flat loss, or
    /// a minimum) or a vanishing step (a learning rate too small for the
    /// gradient's scale); the controls alone do not say which.
    #[arg(long = "train-verbose")]
    pub(crate) train_verbose: bool,

    /// Report what the run cost: the compilation, the `compute` calls against
    /// real time, and the worst block against its own deadline.
    ///
    /// Behind a flag because these are the only numbers here that differ from
    /// one run to the next. What is timed is `compute` alone: not the
    /// excitation, not the statistics, not the printing of the rows.
    #[arg(long = "time")]
    pub(crate) time: bool,

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
    /// whether the response was still ringing when it was cut. With `--sweep`,
    /// one response per point, the swept controls heading the rows, each point
    /// rendered and checked on its own.
    #[arg(long = "freqresp", value_name = "N[:FMIN:FMAX]")]
    pub(crate) freqresp: Option<String>,

    /// Frames of silence rendered before the impulse of `--freqresp`, which
    /// then lands on frame N: the time a smoothed control needs to reach its
    /// value.
    ///
    /// A program that smooths its sliders (`si.smoo`) is time-varying until
    /// they have settled, and the time-invariance check refuses it, rightly:
    /// a response taken during the ramp is that of no filter. The response
    /// and its `-n` frames are counted from the impulse.
    #[arg(long = "settle", value_name = "N", default_value_t = 0)]
    pub(crate) settle: usize,

    /// Largest accepted departure from linearity under `--freqresp`, relative
    /// to the expected response's peak (default 1e-9 in double precision,
    /// 1e-4 in single).
    #[arg(long = "linearity-tolerance", value_name = "REL")]
    pub(crate) linearity_tolerance: Option<f64>,

    /// How a compile failure is reported: `human`, the compiler's rendered
    /// diagnostics on stderr (the default), or `json`, the compiler's
    /// diagnostics-v2 report on stdout (code, ranges, facts,
    /// machine-applicable fixes) and the one-line summary on stderr.
    ///
    /// Any other failure (a value out of range, a non-finite render, a failed
    /// comparison) is reported as text either way.
    #[arg(long = "error-format", value_enum, default_value_t = ErrorFormat::Human)]
    pub(crate) error_format: ErrorFormat,

    /// The options of `faust-rs` that choose the program or shape its code.
    /// Last: their help heading applies to every flag declared after them.
    #[command(flatten)]
    pub(crate) compiler: CompilerOptions,
}

/// The `faust-rs` options a probe forwards to the compiler: those that choose
/// the program or change the code the JIT runs, under their `faust-rs` names.
/// The single-dash spellings (`-pn`, `-vec`, `-vs`, `-lv`, `-ss`, `-mcd`,
/// `-dlt`, `-ct`, `-table-init`, and `-double`, `-bra-tape`) are accepted
/// too: the command line goes through `faust-rs`'s own
/// [`compiler::normalize_legacy_args`] first, so the long names here must
/// stay those of `faust-rs`.
///
/// Left out on purpose: the options of an output (`-o`, `-lang`, `-a`, the
/// dumps), `-mem` (the host would have to supply a memory manager), `-ec`
/// (the host would have to call `control`) and `-os` (a `frame` entry point,
/// where a probe drives `compute`). `-I`, `--double` and `--bra-tape` are
/// probe flags of their own.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "Compiler options (as faust-rs)")]
pub(crate) struct CompilerOptions {
    /// Compile the definition NAME instead of `process` (`-pn NAME`), in FILE
    /// and in the OTHER of `--compare`.
    #[arg(long = "process-name", value_name = "NAME")]
    pub(crate) process_name: Option<String>,

    /// Vector mode (`-vec`): `compute` as an outer loop over chunks of
    /// `--vs` frames.
    #[arg(long = "vec")]
    pub(crate) vec: bool,

    /// Vector size of `-vec` (`-vs N`, default 32).
    #[arg(long = "vs", value_name = "N", requires = "vec")]
    pub(crate) vs: Option<u32>,

    /// Vector loop variant of `-vec` (`-lv 0|1`, default 0).
    #[arg(long = "lv", value_name = "0|1", requires = "vec",
          value_parser = clap::value_parser!(u8).range(0..=1))]
    pub(crate) lv: Option<u8>,

    /// Scheduling strategy (`-ss N`): 0 depth-first (default), 1
    /// breadth-first, 2 interleaved, 3 and above reverse breadth-first.
    #[arg(long = "scheduling-strategy", value_name = "N")]
    pub(crate) scheduling_strategy: Option<u32>,

    /// Largest delay handled by a shifted copy instead of a ring buffer
    /// (`-mcd N`, default 16).
    #[arg(long = "mcd", value_name = "N")]
    pub(crate) mcd: Option<u32>,

    /// Delay above which a line has an exact-size buffer and its own counter
    /// (`-dlt N`, default: never).
    #[arg(long = "dlt", value_name = "N")]
    pub(crate) dlt: Option<u32>,

    /// Table index range check (`-ct 0|1`, default 1): with 0 an index out of
    /// range is not clamped.
    #[arg(long = "check-table", value_name = "0|1",
          value_parser = clap::value_parser!(u8).range(0..=1))]
    pub(crate) check_table: Option<u8>,

    /// How the content of `rdtable`/`rwtable` is produced (`-table-init
    /// runtime|const`, default runtime).
    #[arg(long = "table-init", value_name = "MODE",
          value_parser = ["runtime", "const"])]
    pub(crate) table_init: Option<String>,

    /// The value of `ma.SR` in a table folded by `--table-init const`
    /// (required when a table generator reads it).
    #[arg(long = "table-init-sample-rate", value_name = "HZ")]
    pub(crate) table_init_sample_rate: Option<i32>,
}

impl CompilerOptions {
    /// The options given, as the compiler's `argv` spells them.
    pub(crate) fn argv(&self) -> Vec<String> {
        let valued = [
            ("-pn", self.process_name.clone()),
            ("-ss", self.scheduling_strategy.map(|n| n.to_string())),
            ("-mcd", self.mcd.map(|n| n.to_string())),
            ("-dlt", self.dlt.map(|n| n.to_string())),
            ("-ct", self.check_table.map(|n| n.to_string())),
            ("--table-init", self.table_init.clone()),
            (
                "--table-init-sample-rate",
                self.table_init_sample_rate.map(|n| n.to_string()),
            ),
            ("-vs", self.vs.map(|n| n.to_string())),
            ("-lv", self.lv.map(|n| n.to_string())),
        ];
        let mut out: Vec<String> = self.vec.then(|| "-vec".to_owned()).into_iter().collect();
        for (flag, value) in valued {
            if let Some(value) = value {
                out.extend([flag.to_owned(), value]);
            }
        }
        out
    }

    /// Whether any was given.
    pub(crate) fn any(&self) -> bool {
        !self.argv().is_empty()
    }
}

/// Where `--fd-check` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FdWhere {
    /// At the descent's starting point, before it.
    Start,
    /// At the trained values, after the descent.
    End,
    /// At both.
    Both,
}

/// How a compile failure is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ErrorFormat {
    /// The compiler's rendered diagnostics, on stderr.
    Human,
    /// The compiler's diagnostics-v2 JSON report, on stdout.
    Json,
}

/// The update rule of `--train`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OptimizerKind {
    /// Adam with the paper's betas (0.9, 0.999) and epsilon 1e-8.
    Adam,
    /// Plain gradient descent.
    Sgd,
}

/// Refuses the first flag of `conflicts` that was given, with the message
/// `message` makes of it: what a mode does not take is an error, never a flag
/// that is ignored.
pub(crate) fn refuse<T: Copy>(
    conflicts: &[(T, bool)],
    message: impl Fn(T) -> String,
) -> Result<(), String> {
    conflicts
        .iter()
        .find(|(_, given)| *given)
        .map_or(Ok(()), |(flag, _)| Err(message(*flag)))
}

/// Flags a caller must not combine with `--protocol impulse-test`.
///
/// Rejecting rather than overriding: a protocol run whose sample rate was
/// quietly ignored would produce a `.ir` that looks valid and compares wrong.
pub(crate) fn reject_protocol_conflicts(args: &Args) -> Result<(), String> {
    let offenders: Vec<&str> = [
        ("--sr", args.sr != protocol::SAMPLE_RATE),
        ("--block", args.block != protocol::BLOCK_SIZE),
        ("--in", args.input != "impulse"),
        ("--skip", args.skip != 0),
        ("--bargraphs", args.bargraphs),
        ("--every", args.every != 1),
        ("--set", !args.sets.is_empty()),
        ("--format", args.format != Format::Ir),
        ("--sweep", !args.sweeps.is_empty()),
        ("--reduce", args.reduce.is_some()),
        ("--at", !args.ats.is_empty()),
        (
            "--note/--chord",
            !args.notes.is_empty() || !args.chords.is_empty(),
        ),
        ("--nvoices", args.nvoices != 0),
        // `.ir` has its own number text and is the whole output: a flag that
        // would be ignored there is refused rather than ignored.
        ("--precision", args.precision.is_some()),
        ("--out", args.out.is_some()),
        ("--fail-above", args.fail_above.is_some()),
        ("--eval", !args.evals.is_empty()),
        ("--compare/--ref/--check", verification_requested(args)),
        ("--freqresp", args.freqresp.is_some()),
    ]
    .into_iter()
    .filter_map(|(flag, given)| given.then_some(flag))
    .collect();
    if offenders.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "--protocol impulse-test fixes the rendering conditions; remove {}",
            offenders.join(", ")
        ))
    }
}

/// Whether `--compare`, `--ref` or `--check` was given.
pub(crate) fn verification_requested(args: &Args) -> bool {
    args.compare.is_some() || args.reference.is_some() || !args.checks.is_empty()
}
