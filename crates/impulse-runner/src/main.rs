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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use codegen::backends::interp::{
    FbcDspInstance, FbcOpcode, FbcReal, InterpOptions, Soundfile, generate_interp_module,
};
use compiler::{
    Compiler, ComputeMode, FirVerifyOptions, RealType, SchedulingStrategy, SignalFirLane,
};
use fir::{FirId, FirStore};

/// Reference protocol constants (mirrors `controlTools.h`).
const SAMPLE_RATE: i32 = 44100;
const BLOCK_SIZE: usize = 64;
/// Default produced frame count: the scalar pass length of the C++ reference
/// (`nbsamples / 4` with `nbsamples == 60000`).
const DEFAULT_FRAMES: usize = 15000;

/// Parsed command-line options.
struct Options {
    dsp: PathBuf,
    double: bool,
    frames: usize,
    import_dirs: Vec<PathBuf>,
    compute_mode: ComputeMode,
    scheduling_strategy: SchedulingStrategy,
}

fn main() -> ExitCode {
    match real_main() {
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

fn real_main() -> Result<String, String> {
    let options = parse_args()?;
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

/// Parses argv into [`Options`], accepting the Faust-style flags the Makefile
/// passes through (`-double`, `-I <dir>`), plus the runner-specific `-n`.
fn parse_args() -> Result<Options, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut dsp: Option<PathBuf> = None;
    let mut double = false;
    let mut frames = DEFAULT_FRAMES;
    let mut import_dirs = Vec::new();
    // Vector-mode flags, mirroring the faust-rs CLI (`-vec`, `-vs N`, `-lv N`).
    // Accumulated separately so their order does not matter; folded into a
    // `ComputeMode` at the end.
    let mut vec_mode = false;
    let mut vec_size = ComputeMode::DEFAULT_VEC_SIZE;
    let mut loop_variant = 0u8;
    let mut scheduling_strategy = SchedulingStrategy::DepthFirst;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-double" => double = true,
            "-single" => double = false,
            "-n" => {
                let value = args.next().ok_or("missing value after -n")?;
                frames = value
                    .parse::<usize>()
                    .map_err(|e| format!("bad -n value: {e}"))?;
            }
            "-I" => {
                let value = args.next().ok_or("missing value after -I")?;
                import_dirs.push(PathBuf::from(value));
            }
            "-vec" => vec_mode = true,
            "-vs" => {
                let value = args.next().ok_or("missing value after -vs")?;
                vec_size = value
                    .parse::<u32>()
                    .map_err(|e| format!("bad -vs value: {e}"))?;
            }
            "-lv" => {
                let value = args.next().ok_or("missing value after -lv")?;
                loop_variant = value
                    .parse::<u8>()
                    .map_err(|e| format!("bad -lv value: {e}"))?;
            }
            "-ss" | "--scheduling-strategy" => {
                let value = args
                    .next()
                    .ok_or("missing value after scheduling-strategy option")?;
                let value = value
                    .parse::<u32>()
                    .map_err(|e| format!("bad scheduling-strategy value: {e}"))?;
                scheduling_strategy = SchedulingStrategy::decode(value);
            }
            // Reject unknown flags loudly: an option taking an argument we do not
            // model would desync parsing.
            other if other.starts_with('-') => {
                return Err(format!("unknown option: {other}"));
            }
            other => {
                if dsp.is_some() {
                    return Err(format!("unexpected extra argument: {other}"));
                }
                dsp = Some(PathBuf::from(other));
            }
        }
    }

    let compute_mode = if vec_mode {
        ComputeMode::Vector {
            vec_size,
            loop_variant,
        }
    } else {
        ComputeMode::Scalar
    };

    Ok(Options {
        dsp: dsp.ok_or("missing <file.dsp> argument")?,
        double,
        frames,
        import_dirs,
        compute_mode,
        scheduling_strategy,
    })
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
    use super::{ComputeMode, SchedulingStrategy, parse_args_from, soundfile_part_count};

    fn parse(args: &[&str]) -> Result<super::Options, String> {
        parse_args_from(args.iter().map(|arg| (*arg).to_owned()))
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
    fn malformed_scheduling_strategy_is_rejected_before_compilation() {
        assert!(parse(&["test.dsp", "-ss"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "-1"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "abc"]).is_err());
    }

    #[test]
    fn soundfile_part_count_follows_sound_ui_menu_urls() {
        assert_eq!(soundfile_part_count("{'sound1';'sound2'}"), 2);
        assert_eq!(soundfile_part_count("sound1"), 1);
        assert_eq!(soundfile_part_count(""), 1);
    }
}
