//! `xtask` CLI entry point for repository maintenance workflows.
//!
//! # Role
//! - Hosts developer/CI automation that should not be part of runtime compiler
//!   crates (golden generation/checks, parity reports, differential reports).
//!
//! # Primary workflows
//! - Golden snapshots:
//!   - `golden-check`, `golden-check-cpp`
//!   - `golden-gen-rust`, `golden-gen-cpp`
//! - Runtime trace validation (interp backend):
//!   - `interp-trace-dump` (Phase 1 harness prototype)
//!   - `interp-trace-gen`, `interp-trace-check` (Phase 2 snapshot scaffold)
//!   - `interp-trace-dump-cppfbc` (C++ Faust `.fbc` -> Rust interp runtime)
//!   - `interp-trace-gen-cppfbc` (batch-generate persisted traces from C++ `.fbc`)
//!   - `fir-dump-scan` (structural scan of `dump_fir` loop body expansion)
//! - Backend alignment:
//!   - `backend-align-smoke` (CI-friendly smoke alignment orchestration,
//!     including `opt_level=0` vs `opt_level=max` interpreter drift checks)
//!   - `backend-align-nightly` (broader alignment orchestration)
//! - Developer navigation:
//!   - `code-graphs` (Mermaid/DOT/SVG crate graphs, curated IR overview, and a
//!     public API source-scan index)
//! - Wasm integration:
//!   - `build-faustwasm-compiler-module` (`wasm-ffi` -> verified `.wasm`)
//! - Differential reports:
//!   - parser parity report
//!   - corpus status report
//!   - backend diff reports
//!   - P7 executable backend scheduling matrix report
//!
//! # Design invariants
//! - Deterministic corpus file ordering.
//! - Normalized output text before snapshot comparison.
//! - Fail-fast behavior when one case diverges to preserve CI signal quality.
//! - Generated documentation uses repository-relative paths where practical.
//! - The command surface stays intentionally simple: argument parsing is local to
//!   each workflow instead of adding a runtime CLI dependency to this helper
//!   crate.

use fir::dump_fir;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use wasmparser::{ExternalKind, Parser, Payload};

/// Human-readable command summary printed when no command or an unknown command
/// is provided.
///
/// The project intentionally keeps `xtask` argument parsing lightweight. This
/// string is the canonical short-form help for the dispatcher below; the longer
/// workflow documentation lives in `crates/xtask/README.md`.
const USAGE: &str = "\
Usage:
  cargo run -p xtask -- golden-check
  cargo run -p xtask -- golden-check-cpp
  cargo run -p xtask -- golden-gen-rust
  cargo run -p xtask -- golden-gen-cpp [-- <extra args passed to FAUST_CPP_BIN>]
  cargo run -p xtask -- interp-trace-dump --case <tests/corpus/foo.dsp> [--scenario zeros|impulse|ramp|sine] [--lane fast] [--strict-fir-types]
  cargo run -p xtask -- interp-trace-dump-cppfbc --case <tests/corpus/foo.dsp> [--scenario zeros|impulse|ramp|sine] [--faust-bin /path/to/faust]
  cargo run -p xtask -- interp-trace-gen-cppfbc [--case <tests/corpus/foo.dsp>] [--scenario zeros|impulse|ramp|sine] [--out-dir <dir>] [--faust-bin /path/to/faust]
  cargo run -p xtask -- interp-trace-gen [--case <tests/runtime_corpus/foo.dsp>] [--lane fast] [--strict-fir-types]
  cargo run -p xtask -- interp-trace-check [--case <tests/runtime_corpus/foo.dsp>] [--lane fast] [--strict-fir-types]
  cargo run -p xtask -- fir-dump-scan [--case <tests/corpus/foo.dsp> ...] [--lane fast]
  cargo run -p xtask -- build-faustwasm-compiler-module [--debug]
  cargo run -p xtask -- backend-align-smoke [--case <tests/runtime_corpus/foo.dsp> ...] [--strict-fir-types] [--skip-golden] [--skip-fir-dump-scan]
  cargo run -p xtask -- backend-align-nightly [--strict-fir-types] [--skip-golden] [--skip-fir-dump-scan]
  cargo run -p xtask -- code-graphs [--out-dir <dir>]
  cargo run -p xtask -- parser-parity-report
  cargo run -p xtask -- corpus-status-report
  cargo run -p xtask -- cpp-backend-diff-report
  cargo run -p xtask -- c-fastlane-diff-report
  cargo run -p xtask -- backend-full-corpus-diff-report
  cargo run -p xtask -- table-fastlane-diff-report
  cargo run -p xtask -- libfaust-api-matrix [--cpp-root /path/to/faust] [--out porting/generated]
  cargo run -p xtask -- libfaust-export-check
  cargo run -p xtask -- p7-matrix-report [--artifact-root tests/impulse-tests/ir] [--out porting/generated/p7-executable-backend-matrix-2026-07-14-en.md]
  cargo run -p xtask -- vector-coverage-merge --reports <dir> [--out tests/vector-coverage/corpus-baseline.json] [--certified-list tests/vector-coverage/certified-dspfiles.txt]
  cargo run -p xtask -- vector-coverage-check [--baseline tests/vector-coverage/corpus-baseline.json]
  cargo run -p xtask -- vector-interp-opt-check
  cargo run --release -p xtask -- vector-compile-budget-check [--baseline tests/vector-compile-budget/release-baseline.json]
  cargo run -p xtask -- lockstep-simd-check
\nEnvironment for golden-gen-cpp:
  FAUST_CPP_BIN   Path to reference C++ faust binary
\nEnvironment for golden-check:
  GOLDEN_REF      rust (default) or cpp
";

/// Local checkout of the reference C++ Faust source tree used by static parser
/// report generation.
///
/// Runtime workflows that need a C++ Faust executable use `FAUST_CPP_BIN` or an
/// explicit `--faust-bin` instead.
const CPP_SOURCE_ROOT: &str = "/Users/letz/Developpements/RUST/faust";

/// Parser parity report output path, relative to the workspace root.
const PARITY_REPORT_REL_PATH: &str = "porting/phases/phase-3-parser-parity-report-en.md";

/// Corpus accept/reject diff report output path, relative to the workspace root.
const CORPUS_STATUS_REPORT_REL_PATH: &str =
    "porting/phases/phase-4-corpus-status-diff-report-en.md";

/// C++ backend differential report output path, relative to the workspace root.
const CPP_BACKEND_DIFF_REPORT_REL_PATH: &str =
    "porting/phases/phase-6-cpp-backend-diff-report-en.md";

/// C fast-lane differential report output path, relative to the workspace root.
const C_FASTLANE_DIFF_REPORT_REL_PATH: &str = "porting/phases/phase-6-c-fastlane-diff-report-en.md";

/// Full backend corpus diff report output path, relative to the workspace root.
const BACKEND_FULL_CORPUS_DIFF_REPORT_REL_PATH: &str =
    "porting/phases/phase-6-backend-full-corpus-diff-report-en.md";

/// Table lowering fast-lane report output path, relative to the workspace root.
const TABLE_FASTLANE_DIFF_REPORT_REL_PATH: &str =
    "porting/phases/phase-6-table-fastlane-diff-report-en.md";

/// `xtask` process entry point.
fn main() {
    if let Err(err) = run() {
        eprintln!("xtask error: {err}");
        std::process::exit(1);
    }
}

/// Dispatches one `xtask` subcommand.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print!("{USAGE}");
        return Ok(());
    };

    match command.as_str() {
        "golden-check" => golden_check(None)?,
        "golden-check-cpp" => golden_check(Some(GoldenRef::Cpp))?,
        "golden-gen-rust" => golden_gen_rust()?,
        "golden-gen-cpp" => {
            let mut passthrough: Vec<OsString> = Vec::new();
            let mut separator_seen = false;
            for arg in args {
                if separator_seen {
                    passthrough.push(OsString::from(arg));
                } else if arg == "--" {
                    separator_seen = true;
                }
            }
            golden_gen_cpp(&passthrough)?;
        }
        "interp-trace-dump" => interp_trace_dump(args)?,
        "interp-trace-dump-cppfbc" => interp_trace_dump_cppfbc(args)?,
        "interp-trace-gen-cppfbc" => interp_trace_gen_cppfbc(args)?,
        "interp-trace-gen" => interp_trace_gen(args)?,
        "interp-trace-check" => interp_trace_check(args)?,
        "fir-dump-scan" => fir_dump_scan(args)?,
        "build-faustwasm-compiler-module" => build_faustwasm_compiler_module(args)?,
        "backend-align-smoke" => backend_align_smoke(args)?,
        "backend-align-nightly" => backend_align_nightly(args)?,
        "code-graphs" => code_graphs(args)?,
        "parser-parity-report" => parser_parity_report()?,
        "corpus-status-report" => corpus_status_report()?,
        "cpp-backend-diff-report" => cpp_backend_diff_report()?,
        "c-fastlane-diff-report" => c_fastlane_diff_report()?,
        "backend-full-corpus-diff-report" => backend_full_corpus_diff_report()?,
        "table-fastlane-diff-report" => table_fastlane_diff_report()?,
        "libfaust-api-matrix" => libfaust_api_matrix(args)?,
        "libfaust-export-check" => libfaust_export_check()?,
        "p7-matrix-report" => p7_matrix_report(args)?,
        "vector-coverage-merge" => vector_coverage_merge(args)?,
        "vector-coverage-check" => vector_coverage_check(args)?,
        "vector-interp-opt-check" => vector_interp_opt_check(args)?,
        "vector-compile-budget-check" => vector_compile_budget_check(args)?,
        "lockstep-simd-check" => lockstep_simd_check(args)?,
        _ => {
            print!("{USAGE}");
        }
    }

    Ok(())
}

mod backend_align;
mod code_graphs;
mod fir_dump;
mod golden;
mod libfaust_api_matrix;
mod libfaust_export_check;
mod lockstep_simd;
mod p7_matrix;
mod reports;
mod runtime_trace;
mod shared;
mod vector_compile_budget;
mod vector_coverage;
mod wasm;

pub(crate) use backend_align::*;
pub(crate) use code_graphs::*;
pub(crate) use fir_dump::*;
pub(crate) use golden::*;
pub(crate) use libfaust_api_matrix::*;
pub(crate) use libfaust_export_check::*;
pub(crate) use lockstep_simd::*;
pub(crate) use p7_matrix::*;
pub(crate) use reports::*;
pub(crate) use runtime_trace::*;
pub(crate) use shared::*;
pub(crate) use vector_compile_budget::*;
pub(crate) use vector_coverage::*;
pub(crate) use wasm::*;

#[cfg(test)]
mod tests;
