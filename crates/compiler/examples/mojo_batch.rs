//! Emits the Mojo batch contract of one Faust program: one struct for N
//! voices, its state laid out as a structure of arrays
//! (`porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`, G3.5).
//!
//! The batch mode is an emitter option only (decision D3): `faust-rs` has no
//! command-line switch for it. This example is how the opt-in test lanes of
//! `tests/impulse-tests` (`make mojo-gpu-batch`, `make mojo-gpu-bench-batch`)
//! reach it.
//!
//! ```text
//! cargo run --release -p compiler --example mojo_batch -- dsp/osc.dsp -I dsp -o osc.mojo
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use codegen::backends::mojo::{MojoOptions, MojoRealType};
use compiler::Compiler;

#[derive(Debug, Parser)]
#[command(about = "Emit the Mojo batch contract (many voices, one struct) of a Faust program")]
struct Args {
    /// The Faust program.
    input: PathBuf,
    /// Import search paths, as `faust-rs -I`.
    #[arg(short = 'I')]
    include: Vec<PathBuf>,
    /// Output file.
    #[arg(short = 'o')]
    output: PathBuf,
    /// Double precision (the Apple GPU has none; the default is single).
    #[arg(long)]
    double: bool,
    /// Name of the generated struct.
    #[arg(long = "cn", default_value = "mydsp")]
    class_name: String,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let options = MojoOptions {
        class_name: Some(args.class_name),
        real_type: if args.double {
            MojoRealType::Float64
        } else {
            MojoRealType::Float32
        },
        batch: true,
        ..MojoOptions::default()
    };
    match Compiler::new().compile_file_to_mojo(&args.input, &args.include, &options) {
        Ok(text) => match std::fs::write(&args.output, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("mojo_batch: cannot write {}: {err}", args.output.display());
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("mojo_batch: {err}");
            ExitCode::FAILURE
        }
    }
}
