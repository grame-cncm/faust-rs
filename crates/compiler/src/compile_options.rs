//! The compile options every command line and `argv` of the workspace shares.
//!
//! [`CompileOptionArgs`] is the one declaration of the options that choose the
//! program (`-pn`) or shape the code compiled from it (`-double`, `-vec`,
//! `-ss`, `-mcd`, ...): their long names, defaults, value checks and help
//! text. The `faust-rs` binary, `faustprobe` and the two impulse runners
//! flatten it into their Clap parsers; [`Compiler::with_argv_options`] reads it
//! back from the `argv` of the C API constructors and of
//! [`Compiler::generate_aux_files`]; [`CompileOptionArgs::to_argv`] writes it
//! for a tool that hands its options to those constructors. Every spelling
//! goes through [`crate::normalize_legacy_args`] first.

use clap::{ArgAction, Args, CommandFactory as _, Parser, ValueEnum};

use crate::{
    Compiler, ComputeMode, ControlRateMode, ProcessingApi, RealType, SchedulingStrategy,
    TableInitMode, normalize_legacy_args,
};

/// CLI spelling of the generated-table initialization strategy
/// (`--table-init`), mapped to `transform::signal_fir::TableInitMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum TableInitArg {
    /// Compile each table generator into a sub-module filled at initialization
    /// time (the C++ reference behavior).
    Runtime,
    /// Fold the generator into a literal initializer list at compile time.
    #[default]
    Const,
}

impl From<TableInitArg> for TableInitMode {
    fn from(value: TableInitArg) -> Self {
        match value {
            TableInitArg::Runtime => Self::Runtime,
            TableInitArg::Const => Self::Const,
        }
    }
}

/// The options that choose the program or shape its code, as `faust-rs`
/// declares them. See the module documentation for who shares them.
#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct CompileOptionArgs {
    /// Specify the top-level DSP entry-point name instead of `process`
    /// (`-pn <name>`, `--process-name <name>`).
    #[arg(long = "process-name", value_name = "NAME", default_value = "process")]
    pub process_name: String,

    /// Use double-precision (64-bit) floating-point for internal DSP computation.
    ///
    /// By default, single-precision (32-bit) `float` is used for internal
    /// calculations while the external DSP interface (`FAUSTFLOAT` audio
    /// buffers and UI zones) always stays at the type declared by the
    /// architecture file.  Passing `--double` switches internal arithmetic
    /// to `double`, matching the `-double` option of the reference Faust
    /// compiler.
    #[arg(long = "double", action = ArgAction::SetTrue, overrides_with = "single")]
    pub double: bool,

    /// Use single-precision (32-bit) floating-point for internal DSP
    /// computation (`-single`), the default: cancels an earlier `--double`.
    #[arg(long = "single", action = ArgAction::SetTrue, overrides_with = "double")]
    pub single: bool,

    /// Use the host custom memory manager for eligible native DSP state
    /// (`-mem`/`-mem0`/`--memory-manager`/`--memory-manager0`).
    ///
    /// The four spellings select the same typed `mem0` mode. Only mode zero is
    /// implemented: it is scalar-only and restricted to C, C++, and Cranelift;
    /// `mem1` through `mem3` are deliberately rejected.
    #[arg(
        long = "memory-manager",
        alias = "memory-manager0",
        action = ArgAction::SetTrue
    )]
    pub memory_manager: bool,

    /// Maximum delay (in samples) below which the shift/copy strategy is used
    /// instead of a circular ring buffer (`-mcd N`).
    ///
    /// Delays ≤ `mcd` use a statically-shifted array (no `fIOTA`).
    #[arg(long = "mcd", value_name = "N", default_value_t = 16)]
    pub mcd: u32,

    /// Samples one `BlockReverseAD` tape holds: the largest `compute` block
    /// over which `rad` gradients through delays and recursions are exact
    /// (`-bra-tape N`). A power of two.
    #[arg(long = "bra-tape", value_name = "N", default_value_t = 8192)]
    pub bra_tape: usize,

    /// Delay-line threshold above which the if-based wrapping strategy is used
    /// instead of the default power-of-two circular buffer (`-dlt N`).
    ///
    /// Delays > `dlt` use an exact-size buffer with a per-line counter variable.
    /// Default: disabled (all delays above `mcd` use circular-pow2).
    #[arg(long = "dlt", value_name = "N", default_value_t = u32::MAX, hide_default_value = true)]
    pub dlt: u32,

    /// Check table index range and generate safe accesses (`-ct <0|1>`,
    /// `--check-table <0|1>`).
    ///
    /// With `1` (the default, matching the reference compiler), table
    /// indexes the interval analysis cannot prove in-bounds are clamped at
    /// the signal level to `max(0, min(index, size-1))`. With `0`, accesses
    /// are generated raw and out-of-range indexes are undefined behavior —
    /// the reference `-ct 0` contract.
    #[arg(long = "check-table", value_name = "0|1", default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=1))]
    pub check_table: u8,

    /// How the initial content of `rdtable`/`rwtable` tables is produced.
    ///
    /// `runtime` compiles each table generator into a sub-module whose `fill`
    /// function computes the content at initialization time, as the C++
    /// reference does; this is the only mode that can express content
    /// depending on the sample rate or on a foreign function, and it keeps the
    /// emitted source small. `const` evaluates the generator at compile time
    /// and emits a literal initializer list; a generator using `ma.SR` also
    /// requires `--table-init-sample-rate HZ` to make the frozen value explicit.
    #[arg(long = "table-init", value_name = "MODE", value_enum, default_value_t = TableInitArg::Runtime)]
    pub table_init: TableInitArg,

    /// Sample rate embedded when `--table-init const` folds a generated table
    /// that reads `ma.SR`. Required for that dependency; ignored otherwise.
    #[arg(long = "table-init-sample-rate", value_name = "HZ")]
    pub table_init_sample_rate: Option<i32>,

    /// Vector mode (`-vec`): restructure `compute()` into an outer chunk loop
    /// so the C compiler can auto-vectorize the inner loops (SIMD).
    ///
    /// Selection is checked: a program shape the vector pipeline cannot
    /// certify falls back to scalar lowering instead of emitting unverified
    /// code, and certified vector output is bit-exact against scalar output
    /// for the same program. Use `-vs`/`-lv` to size and shape the chunk loop.
    #[arg(long = "vec", action = ArgAction::SetTrue)]
    pub vec: bool,

    /// Vector size for `-vec` (`-vs N`).
    #[arg(long = "vs", value_name = "N", default_value_t = ComputeMode::DEFAULT_VEC_SIZE)]
    pub vs: u32,

    /// Vector loop variant for `-vec` (`-lv 0|1`, as Faust C++): 0 = fastest
    /// (default) — a constant-trip main loop over `count - count % vs` plus a
    /// scalar remainder, the autovectorization-friendly form; 1 = simple — a
    /// single loop with a runtime `min(vindex + vs, count)` bound.
    #[arg(long = "lv", value_name = "0|1", default_value_t = 0)]
    pub lv: u8,

    /// Signal/loop dependency scheduling strategy (`-ss N`, as Faust C++):
    /// `0` = depth-first (default), `1` = breadth-first, `2` = special
    /// (interleaved), `n >= 3` = reverse breadth-first. Decoded through
    /// `SchedulingStrategy::decode`.
    ///
    /// Independent of `-vec`/`-vs`/`-lv`: it drives the scalar control/signal
    /// schedule and the checked vector loop schedule.
    ///
    /// `adapted` API mapping vs C++ `atoi`: a missing value, a non-integer
    /// value, or a negative value is a hard parse error here instead of
    /// silently falling back to `0`.
    #[arg(long = "scheduling-strategy", value_name = "N", default_value_t = 0)]
    pub scheduling_strategy: u32,

    /// External control (`-ec` / `--external-control`, as Faust C++; the
    /// legacy `--ext-control` spelling is also accepted): emit control-rate
    /// computations in a separate `control` entry point scheduled by the
    /// host instead of inline at the start of each block. Subject to
    /// per-backend capability validation.
    #[arg(
        long = "ec",
        alias = "external-control",
        alias = "ext-control",
        action = ArgAction::SetTrue
    )]
    pub external_control: bool,

    /// One-sample processing (`-os` / `--one-sample`, as Faust C++): emit a
    /// one-sample `frame(inputs, outputs)` entry point over flat channel
    /// arrays; the canonical block `compute` is kept but emitted empty.
    /// Scalar mode only; subject to per-backend capability validation.
    #[arg(long = "os", alias = "one-sample", action = ArgAction::SetTrue)]
    pub one_sample: bool,
}

impl Default for CompileOptionArgs {
    /// The options of an empty command line: Clap's defaults, not the field
    /// types' (`-mcd` is 16, not 0).
    fn default() -> Self {
        Standalone::parse_from(["compile-options"]).options
    }
}

/// A command made of the options alone, to parse them outside a tool's own
/// command line.
#[derive(Parser)]
#[command(no_binary_name = false, args_override_self = true)]
struct Standalone {
    #[command(flatten)]
    options: CompileOptionArgs,
}

impl CompileOptionArgs {
    /// Reads the options out of a C API `argv`, as the C++ constructors do:
    /// every spelling `faust-rs` accepts, the others ignored (`-I`, `-cn`,
    /// `-mem0` belong to the caller). An option this struct declares is
    /// checked as on the command line: a missing or malformed value is an
    /// error, not a default. Given twice, the last one wins.
    ///
    /// # Errors
    /// The first option whose value is refused, in Clap's words.
    pub fn from_argv(argv: &[String]) -> Result<Self, String> {
        let command = Standalone::command();
        let takes_value = |long: &str| {
            command
                .get_arguments()
                .find(|arg| {
                    arg.get_long() == Some(long)
                        || arg
                            .get_all_aliases()
                            .is_some_and(|aliases| aliases.contains(&long))
                })
                .map(|arg| arg.get_action().takes_values())
        };
        let mut picked = vec!["compile-options".to_owned()];
        let mut tokens = normalize_legacy_args(argv.iter().cloned()).into_iter();
        while let Some(token) = tokens.next() {
            let Some(flag) = token.strip_prefix("--") else {
                continue;
            };
            let long = flag.split_once('=').map_or(flag, |(long, _)| long);
            match takes_value(long) {
                Some(true) if !flag.contains('=') => {
                    picked.push(token.clone());
                    picked.extend(tokens.next());
                }
                Some(_) => picked.push(token),
                None => {}
            }
        }
        Standalone::try_parse_from(picked)
            .map(|parsed| parsed.options)
            .map_err(|error| {
                let rendered = error.to_string();
                let first = rendered.lines().next().unwrap_or_default();
                first.trim_start_matches("error: ").to_owned()
            })
    }

    /// The options that differ from their default, spelled as the C++
    /// constructors take them, for a tool that hands its command line to a C
    /// API constructor. [`Self::from_argv`] reads them back unchanged.
    #[must_use]
    pub fn to_argv(&self) -> Vec<String> {
        let d = Self::default();
        let valued = [
            ("-pn", self.process_name.clone(), d.process_name.clone()),
            ("-mcd", self.mcd.to_string(), d.mcd.to_string()),
            (
                "-bra-tape",
                self.bra_tape.to_string(),
                d.bra_tape.to_string(),
            ),
            ("-dlt", self.dlt.to_string(), d.dlt.to_string()),
            (
                "-ct",
                self.check_table.to_string(),
                d.check_table.to_string(),
            ),
            (
                "-table-init",
                table_init_name(self.table_init).to_owned(),
                table_init_name(d.table_init).to_owned(),
            ),
            (
                "--table-init-sample-rate",
                self.table_init_sample_rate
                    .map_or_else(String::new, |rate| rate.to_string()),
                String::new(),
            ),
            ("-vs", self.vs.to_string(), d.vs.to_string()),
            ("-lv", self.lv.to_string(), d.lv.to_string()),
            (
                "-ss",
                self.scheduling_strategy.to_string(),
                d.scheduling_strategy.to_string(),
            ),
        ];
        let mut out = Vec::new();
        for (flag, value, default) in valued {
            if value != default {
                out.extend([flag.to_owned(), value]);
            }
        }
        for (flag, given) in [
            ("-double", self.double),
            ("-vec", self.vec),
            ("-ec", self.external_control),
            ("-os", self.one_sample),
            ("-mem0", self.memory_manager),
        ] {
            if given {
                out.push(flag.to_owned());
            }
        }
        out
    }

    /// `compiler` with every option of `self` applied, the defaults
    /// included: what `self` says replaces what `compiler` held. The memory
    /// manager is not among them: it is an option of the backend a caller
    /// chooses (`codegen::memory_layout::MemoryManagerMode`).
    #[must_use]
    pub fn apply(&self, compiler: Compiler) -> Compiler {
        let compiler = compiler
            .with_process_name(self.process_name.clone())
            .with_real_type(self.real_type())
            .with_mcd(self.mcd)
            .with_dlt(self.dlt)
            .with_bra_tape(self.bra_tape)
            .with_table_init_mode(self.table_init.into())
            .with_compute_mode(self.compute_mode())
            .with_scheduling_strategy(self.scheduling())
            .with_control_rate_mode(self.control_rate_mode())
            .with_processing_api(self.processing_api())
            .with_check_table(self.check_table != 0);
        match self.table_init_sample_rate {
            Some(sample_rate) => compiler.with_table_init_sample_rate(sample_rate),
            None => compiler,
        }
    }

    /// Refuses `-ec` and `-os`, for a tool that runs the program through its
    /// block `compute`: they move the work into a `control` or a `frame` entry
    /// point, and `compute` would run without it.
    ///
    /// # Errors
    /// The message names the option given.
    pub fn require_block_compute(&self) -> Result<(), String> {
        let given: Vec<&str> = [("-ec", self.external_control), ("-os", self.one_sample)]
            .into_iter()
            .filter_map(|(flag, given)| given.then_some(flag))
            .collect();
        if given.is_empty() {
            return Ok(());
        }
        Err(format!(
            "{} compile a `control` or `frame` entry point, and this tool runs `compute`",
            given.join(" and ")
        ))
    }

    /// `-double` / `-single`.
    #[must_use]
    pub fn real_type(&self) -> RealType {
        if self.double {
            RealType::Float64
        } else {
            RealType::Float32
        }
    }

    /// `-vec` with its `-vs` and `-lv`, or scalar.
    #[must_use]
    pub fn compute_mode(&self) -> ComputeMode {
        if self.vec {
            ComputeMode::Vector {
                vec_size: self.vs,
                loop_variant: self.lv,
            }
        } else {
            ComputeMode::Scalar
        }
    }

    /// `-ss`, decoded by [`SchedulingStrategy::decode`]'s total `0/1/2/n>=3`
    /// split; Clap's `u32` parsing has already refused a missing,
    /// non-integer or negative value.
    #[must_use]
    pub fn scheduling(&self) -> SchedulingStrategy {
        SchedulingStrategy::decode(self.scheduling_strategy)
    }

    /// `-ec`.
    #[must_use]
    pub fn control_rate_mode(&self) -> ControlRateMode {
        if self.external_control {
            ControlRateMode::External
        } else {
            ControlRateMode::InlinePerBlock
        }
    }

    /// `-os`.
    #[must_use]
    pub fn processing_api(&self) -> ProcessingApi {
        if self.one_sample {
            ProcessingApi::OneSample
        } else {
            ProcessingApi::Block
        }
    }
}

/// The `--table-init` value naming `mode`.
#[must_use]
pub fn table_init_name(mode: TableInitArg) -> &'static str {
    match mode {
        TableInitArg::Runtime => "runtime",
        TableInitArg::Const => "const",
    }
}

impl Compiler {
    /// This compiler with the compile options of a C API `argv` applied
    /// (see [`CompileOptionArgs::from_argv`]): the ones the `argv` does not
    /// give take their default.
    ///
    /// # Errors
    /// An option of [`CompileOptionArgs`] with a missing or malformed value.
    pub fn with_argv_options(&self, argv: &[String]) -> Result<Self, String> {
        Ok(CompileOptionArgs::from_argv(argv)?.apply(self.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn the_defaults_are_clap_s_and_the_compiler_s() {
        let d = CompileOptionArgs::default();
        assert_eq!(d.mcd, 16);
        assert_eq!(d.dlt, u32::MAX);
        assert_eq!(d.bra_tape, 8192);
        assert_eq!(d.process_name, "process");
        assert_eq!(d.scheduling_strategy, 0);
        assert_eq!(d.table_init, TableInitArg::Runtime);
        assert_eq!(
            (d.vec, d.vs, d.lv),
            (false, ComputeMode::DEFAULT_VEC_SIZE, 0)
        );
        assert_eq!(d.check_table, 1);
        assert!(d.to_argv().is_empty());
        assert_eq!(CompileOptionArgs::from_argv(&[]), Ok(d));
    }

    #[test]
    fn an_argv_is_read_in_every_spelling_and_the_rest_is_ignored() {
        let parsed = CompileOptionArgs::from_argv(&argv(&[
            "-I",
            "lib",
            "-pn",
            "voice",
            "-cn",
            "Dsp",
            "-vec",
            "-vs",
            "8",
            "-lv",
            "1",
            "-ss",
            "2",
            "-mcd",
            "0",
            "-dlt",
            "1024",
            "-ct",
            "0",
            "-table-init",
            "const",
            "--table-init-sample-rate",
            "48000",
            "-bra-tape",
            "64",
            "-double",
            "-mem0",
            "-ec",
        ]))
        .expect("parse");
        assert_eq!(parsed.process_name, "voice");
        assert!(parsed.vec && parsed.double && parsed.external_control && !parsed.one_sample);
        assert_eq!(
            (parsed.vs, parsed.lv, parsed.scheduling_strategy),
            (8, 1, 2)
        );
        assert_eq!((parsed.mcd, parsed.dlt, parsed.check_table), (0, 1024, 0));
        assert_eq!(parsed.table_init, TableInitArg::Const);
        assert_eq!(parsed.table_init_sample_rate, Some(48000));
        assert_eq!(parsed.bra_tape, 64);
        let long = CompileOptionArgs::from_argv(&argv(&[
            "--process-name=effect",
            "--scheduling-strategy",
            "3",
        ]))
        .expect("parse");
        assert_eq!(
            (long.process_name.as_str(), long.scheduling_strategy),
            ("effect", 3)
        );
    }

    #[test]
    fn to_argv_is_read_back_unchanged() {
        let parsed = CompileOptionArgs::from_argv(&argv(&[
            "-pn",
            "voice",
            "-vec",
            "-vs",
            "8",
            "-ss",
            "1",
            "-mcd",
            "4",
            "-ct",
            "0",
            "-double",
            "-os",
            "-table-init",
            "const",
            "--table-init-sample-rate",
            "44100",
        ]))
        .expect("parse");
        assert_eq!(CompileOptionArgs::from_argv(&parsed.to_argv()), Ok(parsed));
    }

    #[test]
    fn a_bad_value_is_refused_and_the_last_precision_wins() {
        for bad in [
            &["-pn"][..],
            &["-mcd"],
            &["-mcd", "-1"],
            &["-dlt", "x"],
            &["-ct", "2"],
            &["-ss", "abc"],
            &["-table-init", "later"],
        ] {
            assert!(
                CompileOptionArgs::from_argv(&argv(bad)).is_err(),
                "{bad:?} should be refused"
            );
        }
        let single = CompileOptionArgs::from_argv(&argv(&["-double", "-single"])).expect("parse");
        assert!(!single.double);
        let double =
            CompileOptionArgs::from_argv(&argv(&["-single", "-double", "-double"])).expect("parse");
        assert!(double.double);
    }
}
