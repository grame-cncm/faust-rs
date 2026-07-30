//! Declarative command-line surface for repository maintenance workflows.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

/// Top-level `xtask` command line.
#[derive(Debug, Parser)]
#[command(
    name = "xtask",
    version,
    about = "Repository maintenance and porting workflows",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub(crate) struct XtaskCli {
    #[command(subcommand)]
    pub(crate) command: XtaskCommand,
}

/// All supported repository-maintenance workflows.
#[derive(Debug, Subcommand)]
pub(crate) enum XtaskCommand {
    GoldenCheck,
    GoldenCheckCpp,
    GoldenGenRust,
    GoldenGenCpp(GoldenGenCppArgs),
    InterpTraceDump(InterpTraceDumpArgs),
    InterpTraceDumpCppfbc(InterpTraceCppFbcDumpArgs),
    InterpTraceGenCppfbc(InterpTraceCppFbcBatchArgs),
    InterpTraceGen(InterpTraceBatchArgs),
    InterpTraceCheck(InterpTraceBatchArgs),
    FirDumpScan(FirDumpScanArgs),
    BuildFaustwasmCompilerModule(FaustwasmCompilerModuleArgs),
    BuildLibfaust(BuildLibfaustArgs),
    BackendAlignSmoke(BackendAlignSmokeArgs),
    BackendAlignNightly(BackendAlignNightlyArgs),
    CodeGraphs(CodeGraphArgs),
    ParserParityReport,
    CorpusStatusReport,
    CorpusStatusQuery(CorpusStatusQueryArgs),
    CppBackendDiffReport,
    CFastlaneDiffReport,
    BackendFullCorpusDiffReport,
    TableFastlaneDiffReport,
    LibfaustApiMatrix(LibfaustApiMatrixArgs),
    LibfaustExportCheck(LibfaustExportCheckArgs),
    P7MatrixReport(P7MatrixReportArgs),
    VectorCoverageMerge(VectorCoverageMergeArgs),
    VectorCoverageCheck(VectorCoverageCheckArgs),
    VectorInterpOptCheck,
    VectorCompileBudgetCheck(VectorCompileBudgetArgs),
    LockstepSimdCheck,
    FfiBoundaryCheck,
    CliParserCheck,
    ErrorModelCheck,
    DiagnosticsQualityCheck,
    DiagnosticsProvenanceProbe(DiagnosticsProvenanceProbeArgs),
    StructureCheck,
    CliTranscriptGen,
    CliTranscriptCheck,
    EmissionDeterminism(EmissionDeterminismArgs),
}

/// Options for comparing provenance storage representations.
#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct DiagnosticsProvenanceProbeArgs {
    /// Number of written source occurrences to simulate.
    #[arg(long, default_value_t = 250_000, value_parser = positive_usize)]
    pub(crate) iterations: usize,
    /// Number of distinct hash-consed semantic nodes shared by the occurrences.
    #[arg(long, default_value_t = 4_096, value_parser = positive_usize)]
    pub(crate) semantic_nodes: usize,
}

/// Extra arguments forwarded verbatim to the reference C++ Faust executable.
#[derive(Debug, Args)]
pub(crate) struct GoldenGenCppArgs {
    #[arg(last = true, allow_hyphen_values = true, value_name = "FAUST_ARG")]
    pub(crate) extra_args: Vec<OsString>,
}

/// Deterministic input scenario for interpreter traces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TraceScenarioArg {
    Zeros,
    Impulse,
    Ramp,
    Sine,
}

/// Signal-to-FIR lane accepted by trace and FIR-dump workflows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TraceLaneArg {
    #[value(name = "fast", aliases = ["fast-lane", "transform"])]
    Fast,
}

/// Options shared by one Rust interpreter trace.
#[derive(Clone, Debug, Args)]
pub(crate) struct InterpTraceDumpArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: PathBuf,
    #[arg(long, value_enum, default_value = "zeros")]
    pub(crate) scenario: TraceScenarioArg,
    #[arg(long, value_enum, default_value = "fast")]
    pub(crate) lane: TraceLaneArg,
    #[arg(long, default_value_t = 48_000)]
    pub(crate) sample_rate: usize,
    #[arg(long, default_value_t = 64, value_parser = positive_usize)]
    pub(crate) block_size: usize,
    #[arg(long, default_value_t = 4, value_parser = positive_usize)]
    pub(crate) num_blocks: usize,
    #[arg(long)]
    pub(crate) strict_fir_types: bool,
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
}

/// Options for a trace executed from C++ Faust interpreter bytecode.
#[derive(Clone, Debug, Args)]
pub(crate) struct InterpTraceCppFbcDumpArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: PathBuf,
    #[arg(long, value_enum, default_value = "zeros")]
    pub(crate) scenario: TraceScenarioArg,
    #[arg(long, value_name = "PATH")]
    pub(crate) faust_bin: Option<PathBuf>,
    #[arg(long, default_value_t = 48_000)]
    pub(crate) sample_rate: usize,
    #[arg(long, default_value_t = 64, value_parser = positive_usize)]
    pub(crate) block_size: usize,
    #[arg(long, default_value_t = 4, value_parser = positive_usize)]
    pub(crate) num_blocks: usize,
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
}

/// Options for batch trace generation from C++ Faust bytecode.
#[derive(Clone, Debug, Args)]
pub(crate) struct InterpTraceCppFbcBatchArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "impulse")]
    pub(crate) scenario: TraceScenarioArg,
    #[arg(long, value_name = "PATH")]
    pub(crate) faust_bin: Option<PathBuf>,
    #[arg(long, default_value_t = 48_000)]
    pub(crate) sample_rate: usize,
    #[arg(long, default_value_t = 64, value_parser = positive_usize)]
    pub(crate) block_size: usize,
    #[arg(long, default_value_t = 1, value_parser = positive_usize)]
    pub(crate) num_blocks: usize,
    #[arg(long, value_name = "DIR")]
    pub(crate) out_dir: Option<PathBuf>,
}

/// Shared options for Rust trace generation and checking.
#[derive(Clone, Debug, Args)]
pub(crate) struct InterpTraceBatchArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "fast")]
    pub(crate) lane: TraceLaneArg,
    #[arg(long, default_value_t = 48_000)]
    pub(crate) sample_rate: usize,
    #[arg(long, default_value_t = 64, value_parser = positive_usize)]
    pub(crate) block_size: usize,
    #[arg(long, default_value_t = 4, value_parser = positive_usize)]
    pub(crate) num_blocks: usize,
    #[arg(long)]
    pub(crate) strict_fir_types: bool,
}

/// Options for structural FIR dump scanning.
#[derive(Clone, Debug, Args)]
pub(crate) struct FirDumpScanArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: Vec<PathBuf>,
    #[arg(long, value_enum, default_value = "fast")]
    pub(crate) lane: TraceLaneArg,
}

/// Options for building the embedded compiler WebAssembly module.
#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct FaustwasmCompilerModuleArgs {
    #[arg(long)]
    pub(crate) debug: bool,
}

/// Options for building the native libfaust distribution.
#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct BuildLibfaustArgs {
    #[arg(long)]
    pub(crate) release: bool,
}

/// Options for the CI-sized backend alignment workflow.
#[derive(Clone, Debug, Args)]
pub(crate) struct BackendAlignSmokeArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) strict_fir_types: bool,
    #[arg(long)]
    pub(crate) skip_golden: bool,
    #[arg(long)]
    pub(crate) skip_fir_dump_scan: bool,
}

/// Options for the full backend alignment workflow.
#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct BackendAlignNightlyArgs {
    #[arg(long)]
    pub(crate) strict_fir_types: bool,
    #[arg(long)]
    pub(crate) skip_golden: bool,
    #[arg(long)]
    pub(crate) skip_fir_dump_scan: bool,
}

/// Options for code-graph generation.
#[derive(Clone, Debug, Args)]
pub(crate) struct CodeGraphArgs {
    #[arg(long, value_name = "DIR")]
    pub(crate) out_dir: Option<PathBuf>,
}

/// Output format for corpus status queries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum QueryFormatArg {
    #[default]
    Json,
    Human,
}

/// Options for a bounded or full corpus status query.
#[derive(Clone, Debug, Args)]
#[command(group(
    ArgGroup::new("selection")
        .required(true)
        .multiple(false)
        .args(["case", "all"])
))]
pub(crate) struct CorpusStatusQueryArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) case: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) all: bool,
    #[arg(long, value_enum, default_value = "json")]
    pub(crate) format: QueryFormatArg,
}

/// Options for generating libfaust C API matrices.
#[derive(Clone, Debug, Args)]
pub(crate) struct LibfaustApiMatrixArgs {
    #[arg(long, value_name = "DIR")]
    pub(crate) cpp_root: Option<PathBuf>,
    #[arg(long = "out", value_name = "DIR")]
    pub(crate) out_dir: Option<PathBuf>,
}

/// Options for libfaust exported-symbol validation.
#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct LibfaustExportCheckArgs {
    #[arg(long)]
    pub(crate) bless: bool,
}

/// Options for the P7 executable backend matrix report.
#[derive(Clone, Debug, Args)]
pub(crate) struct P7MatrixReportArgs {
    #[arg(long, default_value = "tests/impulse-tests/ir", value_name = "DIR")]
    pub(crate) artifact_root: PathBuf,
    #[arg(
        long = "out",
        default_value = "porting/generated/p7-executable-backend-matrix-2026-07-14-en.md",
        value_name = "PATH"
    )]
    pub(crate) output: PathBuf,
}

/// Options for merging sharded vector-coverage reports.
#[derive(Clone, Debug, Args)]
pub(crate) struct VectorCoverageMergeArgs {
    #[arg(long, value_name = "DIR")]
    pub(crate) reports: PathBuf,
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub(crate) certified_list: Option<PathBuf>,
}

/// Options for validating the vector-coverage baseline.
#[derive(Clone, Debug, Args)]
pub(crate) struct VectorCoverageCheckArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) baseline: Option<PathBuf>,
}

/// Options for validating release compilation budgets.
#[derive(Clone, Debug, Args)]
pub(crate) struct VectorCompileBudgetArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) baseline: Option<PathBuf>,
    /// Rewrite the front-end baseline units from this run instead of enforcing
    /// them. Every increase must be justified in the commit message.
    #[arg(long)]
    pub(crate) update: bool,
}

/// Options for run-to-run emission determinism.
#[derive(Clone, Debug, Args)]
pub(crate) struct EmissionDeterminismArgs {
    #[arg(long, value_parser = at_least_two)]
    pub(crate) passes: Option<usize>,
    #[arg(long, value_name = "PATH")]
    pub(crate) allowlist: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub(crate) write_unstable: Option<PathBuf>,
    #[arg(long, value_name = "STEM")]
    pub(crate) case: Vec<String>,
}

fn positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("invalid integer {value:?}: {error}"))?;
    if parsed == 0 {
        Err("value must be greater than zero".to_owned())
    } else {
        Ok(parsed)
    }
}

fn at_least_two(value: &str) -> Result<usize, String> {
    let parsed = positive_usize(value)?;
    if parsed < 2 {
        Err("value must be at least 2".to_owned())
    } else {
        Ok(parsed)
    }
}
