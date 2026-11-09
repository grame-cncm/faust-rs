//! Emits the Mojo batch contract of one Faust program: one struct for N
//! voices, its state laid out as a structure of arrays
//! (`porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`, G3.5).
//!
//! The batch mode is an emitter option only (decision D3): `faust-rs` has no
//! command-line switch for it. This example is how the opt-in test lanes of
//! `tests/impulse-tests` (`make mojo-gpu-batch`, `make mojo-gpu-bench-batch`)
//! reach it, and the other emitter options with it: the outlining of large
//! methods and the packed tables
//! (`porting/mojo-large-program-outlining-plan-2026-11-09-en.md`), both on by
//! default, and, with `--scalar`, the scalar contract under the same options
//! (`make mojo-outline` forces every block out that way).
//!
//! ```text
//! cargo run --release -p compiler --example mojo_batch -- dsp/osc.dsp -I dsp -o osc.mojo
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use codegen::backends::mojo::{MojoOptions, MojoRealType};
use compiler::{Compiler, RealType};

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
    /// Keep every per-voice state access in the voice arenas, as before the
    /// state promotion (`MojoOptions::batch_registers`), for A/B timings.
    #[arg(long = "no-registers")]
    no_registers: bool,
    /// Emit the scalar contract (`-lang mojo`) with these emitter options
    /// instead of the batch one.
    #[arg(long)]
    scalar: bool,
    /// Keep every method whole (`MojoOptions::outline` off).
    #[arg(long = "no-outline", conflicts_with_all = ["outline_budget", "outline_min_block", "outline_split_budget"])]
    no_outline: bool,
    /// Statements above which a method is outlined (`OutlinePolicy::budget`).
    #[arg(long = "outline-budget")]
    outline_budget: Option<usize>,
    /// Smallest block outlined (`OutlinePolicy::min_block`).
    #[arg(long = "outline-min-block")]
    outline_min_block: Option<usize>,
    /// Statements above which a function's largest straight-line block is
    /// cut into runs (`OutlinePolicy::split_budget`).
    #[arg(long = "outline-split-budget")]
    outline_split_budget: Option<usize>,
    /// Write every table as an array literal (`MojoOptions::packed_tables`
    /// off).
    #[arg(long = "table-literals")]
    table_literals: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let outline = (!args.no_outline).then(|| {
        let default = fir::outline::OutlinePolicy::default();
        fir::outline::OutlinePolicy {
            budget: args.outline_budget.unwrap_or(default.budget),
            min_block: args.outline_min_block.unwrap_or(default.min_block),
            split_budget: args.outline_split_budget.unwrap_or(default.split_budget),
        }
    });
    let options = MojoOptions {
        class_name: Some(args.class_name),
        real_type: if args.double {
            MojoRealType::Float64
        } else {
            MojoRealType::Float32
        },
        batch: !args.scalar,
        batch_registers: !args.no_registers,
        outline,
        packed_tables: !args.table_literals,
        ..MojoOptions::default()
    };
    // `--double` is the compiler's precision as well as the emitter's (the
    // emitter alone would print a single-precision computation in `F64`).
    let real_type = if args.double {
        RealType::Float64
    } else {
        RealType::Float32
    };
    let compiler = Compiler::new().with_real_type(real_type);
    match compiler.compile_file_to_mojo(&args.input, &args.include, &options) {
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
