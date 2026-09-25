//! `faustprobe` command-line entry point.
//!
//! See the crate documentation for why this exists alongside the two impulse
//! runners, and `porting/faustprobe-generic-test-tool-design-2026-08-14-en.md`
//! for the full design.
//!
//! The binary is one module per concern:
//!
//! | module | holds |
//! |---|---|
//! | [`cli`] | the command line: `Args`, its enums, what a mode refuses |
//! | [`setup`] | what the command line means: excitation, assignments, schedule, compilation |
//! | [`writes`] | the writes of a run, checked against their ranges before any render |
//! | [`report`] | how results are said: lines, JSON, `--time`, where they go |
//! | [`failure`] | a render that went wrong, and the context it is explained with |
//! | [`verify`] | `--compare`, `--ref`, `--check` |
//! | [`render`] | the plain render and the sweep |
//! | [`poly`] | `--nvoices` |
//! | [`freqresp`] | `--freqresp` |
//! | [`train`] | `--train`, `--fd-check` |
//! | [`determinism`] | the second process of `--check determinism` |

use std::process::ExitCode;
use std::thread;

use clap::Parser;

use cranelift_ffi::probe::engine::last_compile_failure;
use cranelift_ffi::probe::protocol;

mod cli;
mod determinism;
mod failure;
mod freqresp;
mod poly;
mod render;
mod report;
mod setup;
mod train;
mod verify;
mod writes;

use cli::{Args, ErrorFormat, Format, Protocol, reject_protocol_conflicts};

/// Chooses the mode the command line asks for, after the checks that concern
/// more than one of them.
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
    args.compile.require_block_compute()?;
    if args.compile.memory_manager {
        return Err(
            "-mem0 needs a memory manager from the host, which a probe does not supply".to_owned(),
        );
    }
    if args.compile.process_name != "process" && !args.evals.is_empty() {
        return Err(
            "--process-name cannot be combined with --eval: the expressions are the program"
                .to_owned(),
        );
    }
    if args.error_format == ErrorFormat::Json && !args.evals.is_empty() {
        return Err(
            "--error-format json cannot be combined with --eval: the report's offsets would be \
             those of the wrapped source, not of FILE"
                .to_owned(),
        );
    }
    if args.nvoices > 0 {
        return poly::run_poly(&args);
    }
    if !args.train.is_empty() || args.fd_check.is_some() {
        return train::run_train(&args);
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
        return freqresp::run_freqresp(&args);
    }
    render::run_render(&args, impulse_test)
}

fn main() -> ExitCode {
    // `-pn NAME` is two short flags to Clap: the `faust-rs` single-dash
    // spellings are rewritten to their long flags first, by `faust-rs`'s own
    // table.
    let args = Args::parse_from(compiler::normalize_legacy_args(std::env::args()));
    // Cranelift JIT plus the faust-rs front end recurse deeply; run on a large
    // stack, as `impulse-cranelift` and the differential tests do.
    let error_format = args.error_format;
    let result = thread::Builder::new()
        .name("faustprobe".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let result = if args.determinism_worker {
                determinism::worker(&args)
            } else {
                run(args)
            };
            result.map_err(|error| compile_failure_report(error, error_format))
        })
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
pub(crate) fn compile_failure_report(error: String, format: ErrorFormat) -> String {
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
