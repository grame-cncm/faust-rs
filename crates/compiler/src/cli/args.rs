//! CLI argument model and legacy Faust-style flag normalization.
//!
//! This module owns the `clap` surface for the `faust-rs` binary.  It keeps the
//! typed command-line model in one place while preserving historical Faust
//! spelling such as `-lang`, `-cn`, `-pn`, and related short options through
//! [`normalize_legacy_args`].  Operational code should consume [`CliArgs`]
//! rather than re-reading raw process arguments.

use clap::{ArgAction, Parser, ValueEnum};
use compiler::SignalFirLane;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
/// Code generation language/backend selected from the CLI.
pub enum CliLang {
    #[value(alias = "c99")]
    C,
    #[value(alias = "cxx", alias = "c++")]
    Cpp,
    Asc,
    Fir,
    #[value(alias = "interp-fbc")]
    Interp,
    #[value(alias = "clif")]
    Cranelift,
    #[value(alias = "jl")]
    Julia,
    Wasm,
    #[value(alias = "wat")]
    Wast,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
/// Structured error rendering format for CLI diagnostics.
pub enum ErrorFormat {
    #[default]
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
/// Diagnostic verbosity level for CLI rendering.
pub enum ErrorVerbosity {
    #[default]
    Standard,
    Debug,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
/// Signal->FIR lane selected from the CLI.
pub enum CliSignalFirLane {
    #[default]
    Fast,
}

impl CliSignalFirLane {
    /// Converts the CLI lane selection into the internal [`SignalFirLane`] used
    /// by the compiler library.
    pub fn into_compiler_lane(self) -> SignalFirLane {
        match self {
            Self::Fast => SignalFirLane::TransformFastLane,
        }
    }
}

/// Command-line arguments for the compiler binary.
///
/// Legacy mode flags are intentionally kept (`--parse`, `--dump-box`, etc.)
/// to avoid breaking existing scripts while benefiting from robust `clap`
/// parsing and help generation.
#[derive(Debug, Parser)]
#[command(name = "faust-rs", disable_version_flag = true)]
/// Parsed CLI arguments for the `compiler` binary.
pub struct CliArgs {
    /// Generate the golden snapshot output for one DSP file.
    #[arg(long, action = ArgAction::SetTrue)]
    pub golden: bool,
    /// Parse one DSP file and print parser status.
    #[arg(long, action = ArgAction::SetTrue)]
    pub parse: bool,
    /// Parse and dump box IR.
    #[arg(long = "dump-box", action = ArgAction::SetTrue)]
    pub dump_box: bool,
    /// Compile to signals and dump signal IR.
    #[arg(long = "dump-sig", action = ArgAction::SetTrue)]
    pub dump_sig: bool,
    /// Compile to C++ and print generated code.
    #[arg(long = "dump-cpp", action = ArgAction::SetTrue)]
    pub dump_cpp: bool,
    /// Read interpreter `.fbc` text and emit self-contained native C++.
    #[arg(long = "dump-cpp-from-fbc", action = ArgAction::SetTrue)]
    pub dump_cpp_from_fbc: bool,
    /// Compile to C and print generated code.
    #[arg(long = "dump-c", action = ArgAction::SetTrue)]
    pub dump_c: bool,
    /// Compile to FIR and dump FIR IR.
    #[arg(long = "dump-fir", action = ArgAction::SetTrue)]
    pub dump_fir: bool,
    /// Run FIR verifier and dump the verification report (no codegen).
    #[arg(long = "dump-fir-verify", action = ArgAction::SetTrue)]
    pub dump_fir_verify: bool,
    /// Compile to interpreter bytecode and print `.fbc` text.
    #[arg(long = "dump-interp", action = ArgAction::SetTrue)]
    pub dump_interp: bool,
    /// Compile through the experimental Cranelift backend and print a backend report.
    #[arg(long = "dump-cranelift", action = ArgAction::SetTrue)]
    pub dump_cranelift: bool,
    /// Emit strict C++-style JSON description.
    #[arg(long = "json", action = ArgAction::SetTrue)]
    pub dump_json: bool,
    /// Select backend language (Faust-style): `-lang asc`, `-lang c`, `-lang cpp`, `-lang cranelift`, `-lang fir`, `-lang interp`, `-lang julia`, `-lang wasm`, or `-lang wast`.
    ///
    /// This option is equivalent to `--dump-c` / `--dump-cpp` / `--dump-fir`
    /// / `--dump-interp` / `--dump-cranelift` / `-lang asc` / `-lang julia` / `-lang wasm` / `-lang wast`.
    #[arg(long = "lang", value_enum, allow_hyphen_values = true)]
    pub lang: Option<CliLang>,
    /// Print version information and exit.
    #[arg(short = 'v', long = "version", action = ArgAction::SetTrue)]
    pub version: bool,
    /// Print dedicated help for diagnostic output formats and exit.
    #[arg(long = "help-error-format", action = ArgAction::SetTrue)]
    pub help_error_format: bool,
    /// List built-in FIR fixtures available for backend debugging and exit.
    #[arg(long = "list-fir-fixtures", action = ArgAction::SetTrue)]
    pub list_fir_fixtures: bool,
    /// Use a built-in FIR fixture instead of compiling a DSP input file.
    ///
    /// This is intended for backend debugging / bring-up. Combine with
    /// `-lang fir|c|cpp|interp|cranelift` (or corresponding `--dump-*` flags).
    #[arg(long = "fir-fixture")]
    pub fir_fixture: Option<String>,
    /// Optional DSP input file (required by operational modes).
    pub input: Option<PathBuf>,
    /// Optional output file. When omitted, generated text is written to stdout.
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,
    /// Specify the DSP class name used instead of `mydsp` (`-cn <name>`,
    /// `--class-name <name>`).
    #[arg(long = "class-name")]
    pub class_name: Option<String>,
    /// Specify the DSP superclass name used instead of `dsp`
    /// (`-scn <name>`, `--super-class-name <name>`).
    #[arg(long = "super-class-name")]
    pub super_class_name: Option<String>,
    /// Override generated C++ class name for `--dump-cpp-from-fbc`.
    ///
    /// This applies only to `.fbc` -> native C++ emission, distinct from DSP generation
    /// `-cn/--class-name`.
    #[arg(long = "cpp-class-name")]
    pub cpp_class_name: Option<String>,
    /// Extra import search directories.
    #[arg(short = 'I', long = "import-dir")]
    pub import_dir: Vec<PathBuf>,
    /// Specify the top-level DSP entry-point name instead of `process`
    /// (`-pn <name>`, `--process-name <name>`).
    #[arg(long = "process-name", default_value = "process")]
    pub process_name: String,
    /// Wrapper architecture file (`-a` compatibility).
    #[arg(short = 'a', long = "architecture")]
    pub architecture: Option<PathBuf>,
    /// Additional architecture search directories.
    #[arg(short = 'A', long = "architecture-dir")]
    pub architecture_dir: Vec<PathBuf>,
    /// Inline `#include <faust/...>` architecture files.
    #[arg(short = 'i', long = "inline-architecture-files", action = ArgAction::SetTrue)]
    pub inline_architecture_files: bool,
    /// Diagnostic output format.
    #[arg(long = "error-format", value_enum, default_value_t = ErrorFormat::Human)]
    pub error_format: ErrorFormat,
    /// Diagnostic verbosity level.
    #[arg(
        long = "error-verbosity",
        value_enum,
        default_value_t = ErrorVerbosity::Standard
    )]
    pub error_verbosity: ErrorVerbosity,
    /// Signal->FIR compilation lane.
    #[arg(long = "signal-fir-lane", value_enum)]
    pub signal_fir_lane: Option<CliSignalFirLane>,
    /// Disable FIR verification before codegen / FIR dump.
    #[arg(long = "no-fir-verify", action = ArgAction::SetTrue)]
    pub no_fir_verify: bool,
    /// Treat FIR verifier warnings as fatal.
    #[arg(long = "fir-verify-strict", action = ArgAction::SetTrue)]
    pub fir_verify_strict: bool,
    /// Use double-precision (64-bit) floating-point for internal DSP computation.
    ///
    /// By default, single-precision (32-bit) `float` is used for internal
    /// calculations while the external DSP interface (`FAUSTFLOAT` audio
    /// buffers and UI zones) always stays at the type declared by the
    /// architecture file.  Passing `--double` switches internal arithmetic
    /// to `double`, matching the `-double` option of the reference Faust
    /// compiler.
    #[arg(long = "double", action = ArgAction::SetTrue)]
    pub double: bool,
    /// Maximum delay (in samples) below which the shift/copy strategy is used
    /// instead of a circular ring buffer (`-mcd N`).
    ///
    /// Delays ≤ `mcd` use a statically-shifted array (no `fIOTA`). Default: 16.
    #[arg(long = "mcd", default_value_t = 16)]
    pub mcd: u32,
    /// Delay-line threshold above which the if-based wrapping strategy is used
    /// instead of the default power-of-two circular buffer (`-dlt N`).
    ///
    /// Delays > `dlt` use an exact-size buffer with a per-line counter variable.
    /// Default: disabled (all delays above `mcd` use circular-pow2).
    #[arg(long = "dlt", default_value_t = u32::MAX)]
    pub dlt: u32,
    /// Display compilation phases timing information (`-time`).
    #[arg(long = "compilation-time", action = ArgAction::SetTrue)]
    pub compilation_time: bool,
    /// Maximum compilation time in seconds (default: 120).
    #[arg(long = "timeout", default_value_t = 120)]
    pub timeout: u64,
    /// Generate SVG block-diagram files in `<name>-svg/` (`-svg`).
    #[arg(long = "svg", action = ArgAction::SetTrue)]
    pub svg: bool,
    /// SVG: add Gaussian drop-shadow to boxes (`-blur`).
    #[arg(long = "shadow-blur", action = ArgAction::SetTrue)]
    pub shadow_blur: bool,
    /// SVG: emit a viewBox-only (responsive) header instead of fixed mm size (`-sc`).
    #[arg(long = "scaled-svg", action = ArgAction::SetTrue)]
    pub scaled_svg: bool,
    /// SVG: draw a visible frame around route boxes (`-drf`).
    #[arg(long = "draw-route-frame", action = ArgAction::SetTrue)]
    pub draw_route_frame: bool,
    /// SVG: maximum label length before truncation (default 40) (`-mns N`).
    #[arg(long = "max-name-size", default_value_t = 40)]
    pub max_name_size: usize,
    /// SVG: fold diagrams with complexity above N into separate files (0 = off, default 25) (`-f N`).
    #[arg(long = "fold", default_value_t = 25)]
    pub fold: usize,
    /// SVG: minimum per-expression complexity to trigger folding (default 2) (`-fc N`).
    #[arg(long = "fold-complexity", default_value_t = 2)]
    pub fold_complexity: usize,
}

/// Normalizes legacy Faust-style flags to the current `clap` surface.
pub fn normalize_legacy_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "-lang" {
            normalized.push("--lang".to_owned());
            if let Some(value) = it.next() {
                let mapped = match value.as_str() {
                    "-c" => "c".to_owned(),
                    "-cpp" => "cpp".to_owned(),
                    "-fir" => "fir".to_owned(),
                    "-interp" => "interp".to_owned(),
                    _ => value,
                };
                normalized.push(mapped);
            }
            continue;
        }
        if arg == "-pn" {
            normalized.push("--process-name".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-cn" {
            normalized.push("--class-name".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-scn" {
            normalized.push("--super-class-name".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-double" {
            normalized.push("--double".to_owned());
            continue;
        }
        if arg == "-json" {
            normalized.push("--json".to_owned());
            continue;
        }
        if arg == "-version" {
            normalized.push("--version".to_owned());
            continue;
        }
        if arg == "-mcd" {
            normalized.push("--mcd".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-dlt" {
            normalized.push("--dlt".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-time" {
            normalized.push("--compilation-time".to_owned());
            continue;
        }
        if arg == "-svg" {
            normalized.push("--svg".to_owned());
            continue;
        }
        if arg == "-blur" {
            normalized.push("--shadow-blur".to_owned());
            continue;
        }
        if arg == "-sc" {
            normalized.push("--scaled-svg".to_owned());
            continue;
        }
        if arg == "-drf" {
            normalized.push("--draw-route-frame".to_owned());
            continue;
        }
        if arg == "-mns" {
            normalized.push("--max-name-size".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-f" {
            normalized.push("--fold".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-fc" {
            normalized.push("--fold-complexity".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        if arg == "-timeout" {
            normalized.push("--timeout".to_owned());
            if let Some(value) = it.next() {
                normalized.push(value);
            }
            continue;
        }
        normalized.push(arg);
    }
    normalized
}
