//! `impulse-runner` — faust-rs analogue of the C++ `tools/impulseinterp.cpp`.
//!
//! It compiles one DSP file through the faust-rs library to interpreter
//! bytecode and runs the **scalar impulse pass** of the reference impulse-test
//! protocol (see `controlTools.h::runDSP` in the C++ test suite):
//!
//! - sample rate 44100, block size 64 (`kFrames`),
//! - first frame of every input channel = 1.0 (impulse), all other inputs 0.0,
//! - every `button` zone held at 1.0 during the first block then 0.0
//!   (`FUI::setButtons` does not drive checkboxes),
//! - output samples printed as `"%6d :  %8.6f ..."` after the same
//!   `normalize()` zero-clamp (|x| < 1e-6 → 0) the C++ harness applies.
//!
//! The faust-rs interpreter runtime has no polyphonic / MIDI wrapper, so this
//! runner only reproduces the scalar pass (the first 15000
//! reference frames). The generated `.ir` is therefore compared against the
//! genuine 4-pass C++ reference with `filesCompare -part`, which compares only
//! the produced prefix — exactly how the C++ suite's own `Make.rust` tests a
//! scalar-only Rust architecture against the full reference.
//!
//! Usage:
//! ```text
//! impulse-runner <file.dsp> [-double] [-n <frames>] [-I <dir>]... [-ss <n>]
//! ```
//! The `.ir` text is written to stdout (the Makefile redirects it to a file).

use std::ffi::OsString;
use std::iter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use codegen::backends::interp::{
    FbcDspInstance, FbcOpcode, FbcReal, InterpOptions, Soundfile, generate_interp_module,
};
use compiler::{
    Compiler, ComputeMode, FirVerifyOptions, RealType, SchedulingStrategy, SignalFirLane,
    TableInitMode,
};
use fir::{FirId, FirStore};

/// Reference protocol constants (mirrors `controlTools.h`).
const SAMPLE_RATE: i32 = 44100;
const BLOCK_SIZE: usize = 64;
/// Default produced frame count: the scalar pass length of the C++ reference
/// (`nbsamples / 4` with `nbsamples == 60000`).
const DEFAULT_FRAMES: usize = 15000;

/// Parsed command-line options.
#[derive(Debug)]
struct Options {
    dsp: PathBuf,
    double: bool,
    frames: usize,
    import_dirs: Vec<PathBuf>,
    compute_mode: ComputeMode,
    scheduling_strategy: SchedulingStrategy,
    table_init: TableInitMode,
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(options) => options,
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(exit_code as u8);
        }
    };

    // Faust library expansion and structural lowering can recurse deeply even
    // when the final FIR is compact. Match the compiler CLI stack contract so
    // stdfaust-based inputs do not depend on the platform's main-thread stack.
    std::thread::Builder::new()
        .name("impulse-runner".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_main(options))
        .expect("failed to spawn impulse-runner thread")
        .join()
        .expect("impulse-runner thread panicked")
}

fn run_main(options: Options) -> ExitCode {
    match real_main(options) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("impulse-runner: {err}");
            ExitCode::FAILURE
        }
    }
}

fn real_main(options: Options) -> Result<String, String> {
    let real_type = if options.double {
        RealType::Float64
    } else {
        RealType::Float32
    };

    let search_paths = resolve_search_paths(&options);

    let compiler = Compiler::new()
        .with_real_type(real_type)
        .with_compute_mode(options.compute_mode)
        .with_scheduling_strategy(options.scheduling_strategy)
        .with_table_init_mode(options.table_init)
        .with_fir_verify_options(FirVerifyOptions {
            enabled: true,
            strict: false,
        });

    let fir = compiler
        .compile_file_to_fir_with_lane(
            &options.dsp,
            &search_paths,
            SignalFirLane::TransformFastLane,
        )
        .map_err(|e| format!("compilation failed for {}: {e}", options.dsp.display()))?;

    if options.double {
        run::<f64>(&fir.store, fir.module, options.frames)
    } else {
        run::<f32>(&fir.store, fir.module, options.frames)
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "impulse-runner",
    version,
    about = "Compile a Faust DSP and emit its scalar impulse response",
    args_override_self = true
)]
struct CliArgs {
    /// Faust DSP source file.
    #[arg(value_name = "FILE")]
    dsp: PathBuf,

    /// Compile and execute with double-precision samples.
    #[arg(long, overrides_with = "single")]
    double: bool,

    /// Compile and execute with single-precision samples.
    #[arg(long, overrides_with = "double")]
    single: bool,

    /// Number of impulse-response frames to emit.
    #[arg(short = 'n', long = "frames", default_value_t = DEFAULT_FRAMES)]
    frames: usize,

    /// Add a Faust library import directory.
    #[arg(short = 'I', long = "import-dir", value_name = "DIR")]
    import_dirs: Vec<PathBuf>,

    /// Enable vector compilation.
    #[arg(long = "vectorize")]
    vectorize: bool,

    /// Vector loop size.
    #[arg(
        long = "vector-size",
        default_value_t = ComputeMode::DEFAULT_VEC_SIZE
    )]
    vector_size: u32,

    /// Vector loop variant.
    #[arg(long = "loop-variant", default_value_t = 0)]
    loop_variant: u8,

    /// FIR scheduling strategy selector.
    #[arg(long = "scheduling-strategy", default_value_t = 0)]
    scheduling_strategy: u32,

    /// How a `rdtable`/`rwtable` initialization signal is computed:
    /// `const` folds it at compile time, `runtime` emits a generator
    /// sub-module that fills the table at initialization.
    #[arg(long = "table-init", value_name = "MODE", default_value = "const")]
    table_init: TableInitArg,
}

/// CLI spelling of [`TableInitMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum TableInitArg {
    Const,
    Runtime,
}

impl From<TableInitArg> for TableInitMode {
    fn from(value: TableInitArg) -> Self {
        match value {
            TableInitArg::Const => Self::Const,
            TableInitArg::Runtime => Self::Runtime,
        }
    }
}

/// Parses argv after mapping the legacy Faust one-dash spellings to the
/// canonical long options declared by [`CliArgs`].
fn parse_args() -> Result<Options, clap::Error> {
    parse_args_from(std::env::args_os().skip(1))
}

fn parse_args_from<I, T>(args: I) -> Result<Options, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let normalized = iter::once(OsString::from("impulse-runner"))
        .chain(args.into_iter().map(Into::into).map(normalize_legacy_arg));
    let args = CliArgs::try_parse_from(normalized)?;
    let compute_mode = if args.vectorize {
        ComputeMode::Vector {
            vec_size: args.vector_size,
            loop_variant: args.loop_variant,
        }
    } else {
        ComputeMode::Scalar
    };

    Ok(Options {
        dsp: args.dsp,
        double: args.double,
        frames: args.frames,
        import_dirs: args.import_dirs,
        compute_mode,
        scheduling_strategy: SchedulingStrategy::decode(args.scheduling_strategy),
        table_init: args.table_init.into(),
    })
}

fn normalize_legacy_arg(arg: OsString) -> OsString {
    match arg.to_str() {
        Some("-double") => OsString::from("--double"),
        Some("-single") => OsString::from("--single"),
        Some("-vec") => OsString::from("--vectorize"),
        Some("-vs") => OsString::from("--vector-size"),
        Some("-lv") => OsString::from("--loop-variant"),
        Some("-ss") => OsString::from("--scheduling-strategy"),
        _ => arg,
    }
}

/// Builds the import search path list: explicit `-I` dirs first, then the DSP's
/// own directory, then the system faust libraries when present.
fn resolve_search_paths(options: &Options) -> Vec<PathBuf> {
    let mut paths = options.import_dirs.clone();
    if let Some(parent) = options.dsp.parent()
        && !parent.as_os_str().is_empty()
    {
        paths.push(parent.to_path_buf());
    }
    let system_libs = PathBuf::from("/usr/local/share/faust");
    if system_libs.is_dir() {
        paths.push(system_libs);
    }
    paths
}

/// Runs the scalar impulse pass for one precision and renders the `.ir` text.
fn run<R: FbcReal>(store: &FirStore, module: FirId, frames: usize) -> Result<String, String> {
    let options = InterpOptions {
        opt_level: 0,
        module_name: None,
    };
    let mut factory = generate_interp_module::<R>(store, module, &options)
        .map_err(|e| format!("interp codegen failed: {e}"))?;
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(SAMPLE_RATE);

    let num_inputs = usize::try_from(instance.get_num_inputs())
        .map_err(|_| "negative input arity".to_string())?;
    let num_outputs = usize::try_from(instance.get_num_outputs())
        .map_err(|_| "negative output arity".to_string())?;

    // Discover button zones to drive like `FUI::setButtons`.
    let button_zones: Vec<i32> = instance
        .ui_instructions()
        .iter()
        .filter(|ui| ui.opcode == FbcOpcode::AddButton)
        .map(|ui| ui.offset)
        .collect();

    let soundfiles: Vec<(usize, Soundfile)> = instance
        .ui_instructions()
        .iter()
        .filter(|ui| ui.opcode == FbcOpcode::AddSoundfile)
        .filter_map(|ui| {
            let slot = usize::try_from(ui.offset).ok()?;
            Some((
                slot,
                Soundfile::impulse_test_memory_reader(soundfile_part_count(&ui.key)),
            ))
        })
        .collect();
    for (slot, soundfile) in soundfiles {
        if !instance.set_soundfile(slot, soundfile) {
            return Err(format!("invalid soundfile slot {slot}"));
        }
    }

    let mut out = String::new();
    out.push_str(&format!("number_of_inputs  : {num_inputs:3}\n"));
    out.push_str(&format!("number_of_outputs : {num_outputs:3}\n"));
    out.push_str(&format!("number_of_frames  : {frames:6}\n"));

    let mut in_buffer = vec![vec![R::default(); BLOCK_SIZE]; num_inputs];
    let mut out_buffer = vec![vec![R::default(); BLOCK_SIZE]; num_outputs];

    let zero = R::default();
    let one = R::from_f64(1.0);

    let mut written = 0usize;
    let mut cycle = 0usize;
    while written < frames {
        let n = BLOCK_SIZE.min(frames - written);

        // Impulse: first frame of every input channel is 1.0 on the very first
        // block, everything else is silence.
        for channel in &mut in_buffer {
            for sample in channel.iter_mut() {
                *sample = zero;
            }
            if written == 0 && !channel.is_empty() {
                channel[0] = one;
            }
        }

        // Buttons held high during the first block then released.
        let button_value = if cycle == 0 { one } else { zero };
        for &offset in &button_zones {
            instance.set_real_zone(offset, button_value);
        }

        let input_refs: Vec<&[R]> = in_buffer.iter().map(|c| &c[..n]).collect();
        let mut output_refs: Vec<&mut [R]> = out_buffer.iter_mut().map(|c| &mut c[..n]).collect();
        instance
            .try_compute(n as i32, &input_refs, &mut output_refs)
            .map_err(|e| format!("compute failed at frame {written}: {e}"))?;

        for j in 0..n {
            out.push_str(&format!("{written:6} : "));
            for channel in out_buffer.iter().take(num_outputs) {
                let value = normalize(channel[j].to_f64());
                out.push_str(&format!(" {value:8.6}"));
            }
            out.push('\n');
            written += 1;
        }
        cycle += 1;
    }

    Ok(out)
}

/// Zero-clamps tiny magnitudes exactly like `controlTools.h::normalize`.
///
/// The C++ harness aborts on NaN/Inf; here they are passed through so the
/// downstream `filesCompare` reports a concrete sample mismatch instead.
fn normalize(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() {
        value
    } else if value.abs() < 0.000_001 {
        0.0
    } else {
        value
    }
}

/// Counts the resource parts encoded in a Faust soundfile URL.
///
/// `SoundUI::addSoundfile` uses `parseMenuList2`: a menu list such as
/// `{'sound1';'sound2'}` creates one part per entry, otherwise the URL is a
/// single file. The exact names do not matter for the impulse tests because
/// `TestMemoryReader::checkFile` accepts every path and synthesizes data from
/// the part index.
fn soundfile_part_count(url: &str) -> usize {
    let trimmed = url.trim();
    let Some(open) = trimmed.find('{') else {
        return usize::from(!trimmed.is_empty()).max(1);
    };
    let Some(close) = trimmed[open + 1..].find('}') else {
        return 1;
    };
    let body = &trimmed[open + 1..open + 1 + close];
    let count = body
        .split(';')
        .filter(|part| !part.trim().trim_matches('\'').is_empty())
        .count();
    count.max(1)
}

/// Kept to document the runner's contract against a known-good reference path.
#[allow(dead_code)]
fn _reference_protocol_note(_: &Path) {}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::{CliArgs, ComputeMode, SchedulingStrategy, parse_args_from, soundfile_part_count};

    fn parse(args: &[&str]) -> Result<super::Options, clap::Error> {
        parse_args_from(args.iter().copied())
    }

    #[test]
    fn scheduling_strategy_is_independent_from_compute_mode() {
        let scalar = parse(&["test.dsp", "-ss", "1"]).expect("parse scalar options");
        assert_eq!(scalar.compute_mode, ComputeMode::Scalar);
        assert_eq!(scalar.scheduling_strategy, SchedulingStrategy::BreadthFirst);

        let vector = parse(&[
            "test.dsp",
            "-vec",
            "-lv",
            "1",
            "--scheduling-strategy",
            "42",
        ])
        .expect("parse vector options");
        assert_eq!(
            vector.compute_mode,
            ComputeMode::Vector {
                vec_size: ComputeMode::DEFAULT_VEC_SIZE,
                loop_variant: 1,
            }
        );
        assert_eq!(
            vector.scheduling_strategy,
            SchedulingStrategy::ReverseBreadthFirst
        );
    }

    #[test]
    fn clap_definition_is_consistent() {
        CliArgs::command().debug_assert();
    }

    #[test]
    fn malformed_scheduling_strategy_is_rejected_before_compilation() {
        assert!(parse(&["test.dsp", "-ss"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "-1"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "abc"]).is_err());
    }

    #[test]
    fn legacy_flags_work_before_the_dsp_and_last_precision_wins() {
        let options = parse(&["-double", "-I", "lib", "test.dsp", "-single", "-n", "8"])
            .expect("parse legacy options");
        assert!(!options.double);
        assert_eq!(options.frames, 8);
        assert_eq!(options.import_dirs, [std::path::PathBuf::from("lib")]);
    }

    #[test]
    fn soundfile_part_count_follows_sound_ui_menu_urls() {
        assert_eq!(soundfile_part_count("{'sound1';'sound2'}"), 2);
        assert_eq!(soundfile_part_count("sound1"), 1);
        assert_eq!(soundfile_part_count(""), 1);
    }
}
