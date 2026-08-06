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
//! - Command dispatch and validation are declared once in the typed Clap tree;
//!   workflow modules receive validated option values.

use clap::Parser;
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
use wasmparser::{ExternalKind, Parser as WasmParser, Payload};

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
    let cli = XtaskCli::parse();
    let exit_code = std::thread::Builder::new()
        .name("xtask".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            if let Err(err) = run(cli) {
                eprintln!("xtask error: {err}");
                1
            } else {
                0
            }
        })
        .expect("failed to spawn xtask worker thread")
        .join()
        .expect("xtask worker thread panicked");
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// Dispatches one `xtask` subcommand.
fn run(cli: XtaskCli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        XtaskCommand::GoldenCheck => golden_check(None)?,
        XtaskCommand::GoldenCheckCpp => golden_check(Some(GoldenRef::Cpp))?,
        XtaskCommand::GoldenGenRust => golden_gen_rust()?,
        XtaskCommand::GoldenGenCpp(args) => golden_gen_cpp(&args.extra_args)?,
        XtaskCommand::InterpTraceDump(args) => interp_trace_dump(args)?,
        XtaskCommand::InterpTraceDumpCppfbc(args) => interp_trace_dump_cppfbc(args)?,
        XtaskCommand::InterpTraceGenCppfbc(args) => interp_trace_gen_cppfbc(args)?,
        XtaskCommand::InterpTraceGen(args) => interp_trace_gen(args)?,
        XtaskCommand::InterpTraceCheck(args) => interp_trace_check(args)?,
        XtaskCommand::FirDumpScan(args) => fir_dump_scan(args)?,
        XtaskCommand::BuildFaustwasmCompilerModule(args) => build_faustwasm_compiler_module(args)?,
        XtaskCommand::BuildLibfaust(args) => build_libfaust_distribution_command(args)?,
        XtaskCommand::BackendAlignSmoke(args) => backend_align_smoke(args)?,
        XtaskCommand::BackendAlignNightly(args) => backend_align_nightly(args)?,
        XtaskCommand::CodeGraphs(args) => code_graphs(args)?,
        XtaskCommand::ParserParityReport => parser_parity_report()?,
        XtaskCommand::CorpusStatusReport => corpus_status_report()?,
        XtaskCommand::CorpusStatusQuery(args) => corpus_status_query(args)?,
        XtaskCommand::CppBackendDiffReport => cpp_backend_diff_report()?,
        XtaskCommand::CFastlaneDiffReport => c_fastlane_diff_report()?,
        XtaskCommand::BackendFullCorpusDiffReport => backend_full_corpus_diff_report()?,
        XtaskCommand::TableFastlaneDiffReport => table_fastlane_diff_report()?,
        XtaskCommand::LibfaustApiMatrix(args) => libfaust_api_matrix(args)?,
        XtaskCommand::LibfaustExportCheck(args) => libfaust_export_check(args)?,
        XtaskCommand::P7MatrixReport(args) => p7_matrix_report(args)?,
        XtaskCommand::VectorCoverageMerge(args) => vector_coverage_merge(args)?,
        XtaskCommand::VectorCoverageCheck(args) => vector_coverage_check(args)?,
        XtaskCommand::VectorInterpOptCheck => vector_interp_opt_check()?,
        XtaskCommand::CompileBudgetCheck(args) => compile_budget_check(args)?,
        XtaskCommand::LockstepSimdCheck => lockstep_simd_check()?,
        XtaskCommand::FfiBoundaryCheck => ffi_boundary_check()?,
        XtaskCommand::CliParserCheck => cli_parser_check()?,
        XtaskCommand::ErrorModelCheck => error_model_check()?,
        XtaskCommand::DiagnosticsQualityCheck => diagnostics_quality_check()?,
        XtaskCommand::DiagnosticsProvenanceProbe(args) => diagnostics_provenance_probe(args)?,
        XtaskCommand::StructureCheck => structure_check()?,
        XtaskCommand::CliTranscriptGen => cli_transcript_gen()?,
        XtaskCommand::CliTranscriptCheck => cli_transcript_check()?,
        XtaskCommand::EmissionDeterminism(args) => emission_determinism(args)?,
        XtaskCommand::CompileProfile(args) => compile_profile(args)?,
        XtaskCommand::LexerDifferential(args) => lexer_differential(args)?,
    }

    Ok(())
}

mod backend_align;
mod cli;
mod cli_parser_check;
mod cli_transcript;
mod code_graphs;
mod compile_budget;
mod compile_profile;
mod lexer_differential;
mod corpus_status_query;
mod diagnostics_provenance;
mod diagnostics_quality_check;
mod emission_determinism;
mod error_model_check;
mod ffi_boundary_check;
mod fir_dump;
mod golden;
mod libfaust_api_matrix;
mod libfaust_export_check;
mod lockstep_simd;
mod p7_matrix;
mod reports;
mod runtime_trace;
mod shared;
mod structure_check;
mod vector_coverage;
mod wasm;

pub(crate) use backend_align::*;
pub(crate) use cli::*;
pub(crate) use cli_parser_check::*;
pub(crate) use cli_transcript::*;
pub(crate) use code_graphs::*;
pub(crate) use compile_budget::*;
pub(crate) use compile_profile::*;
pub(crate) use lexer_differential::*;
pub(crate) use corpus_status_query::*;
pub(crate) use diagnostics_provenance::*;
pub(crate) use diagnostics_quality_check::*;
pub(crate) use emission_determinism::*;
pub(crate) use error_model_check::*;
pub(crate) use ffi_boundary_check::*;
pub(crate) use fir_dump::*;
pub(crate) use golden::*;
pub(crate) use libfaust_api_matrix::*;
pub(crate) use libfaust_export_check::*;
pub(crate) use lockstep_simd::*;
pub(crate) use p7_matrix::*;
pub(crate) use reports::*;
pub(crate) use runtime_trace::*;
pub(crate) use shared::*;
pub(crate) use structure_check::*;
pub(crate) use vector_coverage::*;
pub(crate) use wasm::*;

#[cfg(test)]
mod tests;
