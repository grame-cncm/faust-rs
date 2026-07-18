//! Interpreter runtime trace workflows.
//!
//! Runtime traces compile DSP corpus cases, execute deterministic input
//! scenarios through the interpreter backend, serialize output samples, and
//! compare snapshots across lanes or optimization levels.

use super::*;

// ---------------------------------------------------------------------------
// Runtime trace workflows
// ---------------------------------------------------------------------------

/// Input scenario used by runtime-trace generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TraceScenario {
    /// All input channels receive zero.
    Zeros,
    /// The first sample of each input channel is one, followed by zeroes.
    Impulse,
    /// Input channels receive a deterministic increasing ramp.
    Ramp,
    /// Input channels receive a deterministic sine wave.
    Sine,
}

impl TraceScenario {
    /// Returns the stable CLI / snapshot string for this scenario.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Zeros => "zeros",
            Self::Impulse => "impulse",
            Self::Ramp => "ramp",
            Self::Sine => "sine",
        }
    }

    /// Parses a CLI/runtime-trace scenario name.
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        match s {
            "zeros" => Ok(Self::Zeros),
            "impulse" => Ok(Self::Impulse),
            "ramp" => Ok(Self::Ramp),
            "sine" => Ok(Self::Sine),
            _ => Err(format!(
                "unknown scenario '{s}' (expected: zeros|impulse|ramp|sine)"
            )),
        }
    }
}

/// Interpreter lane used by runtime-trace workflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TraceLane {
    /// Active transform fast lane.
    Fast,
}

impl TraceLane {
    /// Returns the stable textual label used in logs and snapshots.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast-lane",
        }
    }

    /// Parses the accepted CLI aliases for one interpreter lane.
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        match s {
            "fast" | "fast-lane" | "transform" => Ok(Self::Fast),
            _ => Err(format!("unknown lane '{s}' (expected: fast)")),
        }
    }

    /// Maps the CLI/runtime-trace lane to the compiler's signal-to-FIR lane.
    pub(crate) fn to_signal_fir_lane(self) -> compiler::SignalFirLane {
        match self {
            Self::Fast => compiler::SignalFirLane::TransformFastLane,
        }
    }
}

/// Parsed options for `interp-trace-dump`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterpTraceDumpOptions {
    /// DSP source file to compile and execute.
    pub(crate) case: PathBuf,
    /// Deterministic input pattern.
    pub(crate) scenario: TraceScenario,
    /// Lowering lane used before interpreter bytecode generation.
    pub(crate) lane: TraceLane,
    /// Runtime sample rate.
    pub(crate) sample_rate: usize,
    /// Number of frames per compute block.
    pub(crate) block_size: usize,
    /// Number of compute blocks to execute.
    pub(crate) num_blocks: usize,
    /// Whether FIR type diagnostics should reject the trace.
    pub(crate) strict_fir_types: bool,
    /// Optional JSON output path. When absent, JSON is printed to stdout.
    pub(crate) out: Option<PathBuf>,
}

/// Parsed options for `interp-trace-dump-cppfbc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterpTraceCppFbcDumpOptions {
    /// Shared trace execution options.
    trace: InterpTraceDumpOptions,
    /// Optional C++ Faust executable override.
    faust_bin: Option<PathBuf>,
}

/// Parsed options for batch generation from C++ `.fbc` files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterpTraceCppFbcBatchOptions {
    /// Optional single compile corpus case; absent means all default corpus
    /// cases.
    case: Option<PathBuf>,
    /// Deterministic input pattern.
    scenario: TraceScenario,
    /// Runtime sample rate.
    sample_rate: usize,
    /// Number of frames per compute block.
    block_size: usize,
    /// Number of compute blocks to execute.
    num_blocks: usize,
    /// Output root for persisted JSON traces.
    out_dir: PathBuf,
    /// Optional C++ Faust executable override.
    faust_bin: Option<PathBuf>,
}

impl Default for InterpTraceCppFbcBatchOptions {
    /// Returns default batch settings for generating C++ `.fbc` traces.
    fn default() -> Self {
        Self {
            case: None,
            scenario: TraceScenario::Impulse,
            sample_rate: 48_000,
            block_size: 64,
            num_blocks: 1,
            out_dir: workspace_root().join("tests/runtime_traces").join("cppfbc"),
            faust_bin: None,
        }
    }
}

impl Default for InterpTraceDumpOptions {
    /// Returns default options for one interpreter trace run.
    fn default() -> Self {
        Self {
            case: PathBuf::new(),
            scenario: TraceScenario::Zeros,
            lane: TraceLane::Fast,
            sample_rate: 48_000,
            block_size: 64,
            num_blocks: 4,
            strict_fir_types: false,
            out: None,
        }
    }
}

/// Persisted runtime trace payload used by snapshot workflows.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct RuntimeTrace {
    /// Repository-relative DSP source path.
    pub(crate) dsp_path: String,
    /// Signal-to-FIR lane label.
    pub(crate) lane: String,
    /// Input scenario label.
    pub(crate) scenario: String,
    /// Runtime sample rate.
    pub(crate) sample_rate: usize,
    /// Number of frames per compute block.
    pub(crate) block_size: usize,
    /// Number of compute blocks executed.
    pub(crate) num_blocks: usize,
    /// Number of DSP input channels.
    pub(crate) num_inputs: usize,
    /// Number of DSP output channels.
    pub(crate) num_outputs: usize,
    /// Output samples by channel.
    pub(crate) outputs: Vec<Vec<f32>>,
}

/// Numeric tolerances used when comparing runtime traces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TraceCompareTolerances {
    /// Absolute tolerance.
    abs_tol: f32,
    /// Relative tolerance.
    rel_tol: f32,
}

impl Default for TraceCompareTolerances {
    /// Returns the default absolute/relative float tolerances for trace diffing.
    fn default() -> Self {
        Self {
            abs_tol: 1.0e-6,
            rel_tol: 1.0e-5,
        }
    }
}

/// One concrete runtime-trace mismatch entry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TraceMismatch {
    /// Field or payload area that mismatched.
    pub(crate) field: String,
    /// Optional output channel index for sample mismatches.
    pub(crate) channel: Option<usize>,
    /// Optional sample index for sample mismatches.
    pub(crate) sample: Option<usize>,
    /// Expected float value for sample mismatches.
    pub(crate) expected: Option<f32>,
    /// Actual float value for sample mismatches.
    pub(crate) actual: Option<f32>,
}

/// Shared batch options for runtime-trace generation/checking flows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterpTraceBatchOptions {
    /// Optional single runtime corpus case; absent means all runtime corpus
    /// cases.
    pub(crate) case: Option<PathBuf>,
    /// Lowering lane used before interpreter bytecode generation.
    pub(crate) lane: TraceLane,
    /// Runtime sample rate.
    pub(crate) sample_rate: usize,
    /// Number of frames per compute block.
    pub(crate) block_size: usize,
    /// Number of compute blocks to execute.
    pub(crate) num_blocks: usize,
    /// Whether FIR type diagnostics should reject traces.
    pub(crate) strict_fir_types: bool,
}

impl Default for InterpTraceBatchOptions {
    /// Returns default options for runtime-trace batch generation/checking.
    fn default() -> Self {
        Self {
            case: None,
            lane: TraceLane::Fast,
            sample_rate: 48_000,
            block_size: 64,
            num_blocks: 4,
            strict_fir_types: false,
        }
    }
}

/// Executes one Rust interpreter trace run and writes/prints the JSON payload.
pub(crate) fn interp_trace_dump(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_interp_trace_dump_options(&mut args)?;
    let trace = run_interp_trace_case(&options)?;
    let json = render_runtime_trace_json(&trace);
    if let Some(path) = &options.out {
        fs::write(path, json)?;
    } else {
        print!("{json}");
    }
    Ok(())
}

/// Executes one C++ `.fbc`-backed trace run and writes/prints the JSON payload.
pub(crate) fn interp_trace_dump_cppfbc(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_interp_trace_dump_cppfbc_options(&mut args)?;
    let trace = run_interp_trace_case_from_cpp_fbc(&options)?;
    let json = render_runtime_trace_json(&trace);
    if let Some(path) = &options.trace.out {
        fs::write(path, json)?;
    } else {
        print!("{json}");
    }
    Ok(())
}

/// Generates C++ `.fbc` trace snapshots for one case or the default corpus.
pub(crate) fn interp_trace_gen_cppfbc(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_interp_trace_gen_cppfbc_options(&mut args)?;
    let mut cases = if let Some(case) = &options.case {
        vec![case.clone()]
    } else {
        corpus_files()?
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n.starts_with("rep_"))
            })
            .collect()
    };
    cases.sort();
    if cases.is_empty() {
        return Err("no corpus cases found for interp-trace-gen-cppfbc".into());
    }

    let mut generated = 0usize;
    for case in cases {
        let case_id = case_name(&case)?;
        let trace = run_interp_trace_case_from_cpp_fbc(&InterpTraceCppFbcDumpOptions {
            trace: InterpTraceDumpOptions {
                case: case.clone(),
                scenario: options.scenario,
                lane: TraceLane::Fast,
                sample_rate: options.sample_rate,
                block_size: options.block_size,
                num_blocks: options.num_blocks,
                strict_fir_types: false,
                out: None,
            },
            faust_bin: options.faust_bin.clone(),
        })?;
        let case_dir = options.out_dir.join(&case_id);
        fs::create_dir_all(&case_dir)?;
        let path = case_dir.join(format!("{}.json", options.scenario.as_str()));
        fs::write(&path, render_runtime_trace_json(&trace))?;
        println!("generated {}", path.display());
        generated += 1;
    }
    println!(
        "interp-trace-gen-cppfbc: generated {generated} trace snapshot(s) in {}",
        options.out_dir.display()
    );
    Ok(())
}

/// Parses CLI options for `interp-trace-dump`.
pub(crate) fn parse_interp_trace_dump_options(
    args: &mut impl Iterator<Item = String>,
) -> Result<InterpTraceDumpOptions, Box<dyn std::error::Error>> {
    let mut options = InterpTraceDumpOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --case".into());
                };
                options.case = PathBuf::from(path);
            }
            "--scenario" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --scenario".into());
                };
                options.scenario = TraceScenario::parse(&value)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }
            "--lane" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --lane".into());
                };
                options.lane = TraceLane::parse(&value)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }
            "--sample-rate" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --sample-rate".into());
                };
                options.sample_rate = value.parse::<usize>()?;
            }
            "--block-size" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --block-size".into());
                };
                options.block_size = value.parse::<usize>()?;
            }
            "--num-blocks" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --num-blocks".into());
                };
                options.num_blocks = value.parse::<usize>()?;
            }
            "--strict-fir-types" => {
                options.strict_fir_types = true;
            }
            "--out" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --out".into());
                };
                options.out = Some(PathBuf::from(path));
            }
            "--help" | "-h" => {
                return Err("usage: cargo run -p xtask -- interp-trace-dump --case <path> [--scenario zeros|impulse|ramp|sine] [--lane fast] [--sample-rate N] [--block-size N] [--num-blocks N] [--strict-fir-types] [--out path]".into());
            }
            other => {
                return Err(format!("unknown interp-trace-dump option: {other}").into());
            }
        }
    }

    if options.case.as_os_str().is_empty() {
        return Err("interp-trace-dump requires --case <path>".into());
    }
    if options.block_size == 0 || options.num_blocks == 0 {
        return Err("block-size and num-blocks must be > 0".into());
    }
    Ok(options)
}

/// Parses CLI options for `interp-trace-dump-cppfbc`.
///
/// The lane is fixed to the C++ `.fbc` runtime path, so flags that would alter
/// FIR-lane semantics are rejected here instead of ignored.
pub(crate) fn parse_interp_trace_dump_cppfbc_options(
    args: &mut impl Iterator<Item = String>,
) -> Result<InterpTraceCppFbcDumpOptions, Box<dyn std::error::Error>> {
    let mut options = InterpTraceCppFbcDumpOptions {
        trace: InterpTraceDumpOptions::default(),
        faust_bin: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --case".into());
                };
                options.trace.case = PathBuf::from(path);
            }
            "--scenario" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --scenario".into());
                };
                options.trace.scenario = TraceScenario::parse(&value)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }
            "--faust-bin" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --faust-bin".into());
                };
                options.faust_bin = Some(PathBuf::from(path));
            }
            "--sample-rate" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --sample-rate".into());
                };
                options.trace.sample_rate = value.parse::<usize>()?;
            }
            "--block-size" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --block-size".into());
                };
                options.trace.block_size = value.parse::<usize>()?;
            }
            "--num-blocks" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --num-blocks".into());
                };
                options.trace.num_blocks = value.parse::<usize>()?;
            }
            "--out" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --out".into());
                };
                options.trace.out = Some(PathBuf::from(path));
            }
            "--lane" => {
                return Err(
                    "--lane is not supported for interp-trace-dump-cppfbc (source is C++ .fbc)"
                        .into(),
                );
            }
            "--strict-fir-types" => {
                return Err(
                    "--strict-fir-types is not applicable to interp-trace-dump-cppfbc".into(),
                );
            }
            "--help" | "-h" => {
                return Err("usage: cargo run -p xtask -- interp-trace-dump-cppfbc --case <path> [--scenario zeros|impulse|ramp|sine] [--faust-bin /path/to/faust] [--sample-rate N] [--block-size N] [--num-blocks N] [--out path]".into());
            }
            other => {
                return Err(format!("unknown interp-trace-dump-cppfbc option: {other}").into());
            }
        }
    }
    if options.trace.case.as_os_str().is_empty() {
        return Err("interp-trace-dump-cppfbc requires --case <path>".into());
    }
    if options.trace.block_size == 0 || options.trace.num_blocks == 0 {
        return Err("block-size and num-blocks must be > 0".into());
    }
    options.trace.lane = TraceLane::Fast;
    Ok(options)
}

/// Parses CLI options for `interp-trace-gen-cppfbc`.
pub(crate) fn parse_interp_trace_gen_cppfbc_options(
    args: &mut impl Iterator<Item = String>,
) -> Result<InterpTraceCppFbcBatchOptions, Box<dyn std::error::Error>> {
    let mut options = InterpTraceCppFbcBatchOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --case".into());
                };
                options.case = Some(PathBuf::from(path));
            }
            "--scenario" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --scenario".into());
                };
                options.scenario = TraceScenario::parse(&value)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }
            "--faust-bin" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --faust-bin".into());
                };
                options.faust_bin = Some(PathBuf::from(path));
            }
            "--sample-rate" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --sample-rate".into());
                };
                options.sample_rate = value.parse::<usize>()?;
            }
            "--block-size" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --block-size".into());
                };
                options.block_size = value.parse::<usize>()?;
            }
            "--num-blocks" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --num-blocks".into());
                };
                options.num_blocks = value.parse::<usize>()?;
            }
            "--out-dir" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --out-dir".into());
                };
                options.out_dir = PathBuf::from(path);
            }
            "--help" | "-h" => {
                return Err("usage: cargo run -p xtask -- interp-trace-gen-cppfbc [--case <path>] [--scenario zeros|impulse|ramp|sine] [--faust-bin /path/to/faust] [--sample-rate N] [--block-size N] [--num-blocks N] [--out-dir <dir>]".into());
            }
            other => return Err(format!("unknown interp-trace-gen-cppfbc option: {other}").into()),
        }
    }
    if options.block_size == 0 || options.num_blocks == 0 {
        return Err("block-size and num-blocks must be > 0".into());
    }
    Ok(options)
}

/// Generates Rust runtime-trace snapshots for the selected runtime corpus cases.
pub(crate) fn interp_trace_gen(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_interp_trace_batch_options(&mut args)?;
    let cases = runtime_trace_cases(&options)?;
    let mut generated = 0usize;
    for case in cases {
        let case_id = case_name(&case)?;
        fs::create_dir_all(runtime_trace_snapshot_root().join(&case_id))?;
        let scenarios = trace_scenarios_for_runtime_case(&case)?;
        if scenarios.is_empty() {
            println!(
                "skip {} (no snapshot-enabled scenarios yet)",
                case.display()
            );
            continue;
        }
        for scenario in scenarios {
            let trace = run_interp_trace_case(&InterpTraceDumpOptions {
                case: case.clone(),
                scenario,
                lane: options.lane,
                sample_rate: options.sample_rate,
                block_size: options.block_size,
                num_blocks: options.num_blocks,
                strict_fir_types: options.strict_fir_types,
                out: None,
            })?;
            let path = runtime_trace_snapshot_path(&case_id, scenario);
            fs::write(&path, render_runtime_trace_json(&trace))?;
            println!("generated {}", path.display());
            generated += 1;
        }
    }
    println!("interp-trace-gen: generated {generated} trace snapshot(s)");
    Ok(())
}

/// Recomputes Rust runtime traces and compares them against checked-in snapshots.
pub(crate) fn interp_trace_check(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_interp_trace_batch_options(&mut args)?;
    let tol = TraceCompareTolerances::default();
    let cases = runtime_trace_cases(&options)?;
    let mut checked = 0usize;
    for case in cases {
        let case_id = case_name(&case)?;
        let scenarios = trace_scenarios_for_runtime_case(&case)?;
        if scenarios.is_empty() {
            println!(
                "skip {} (no snapshot-enabled scenarios yet)",
                case.display()
            );
            continue;
        }
        for scenario in scenarios {
            let expected_path = runtime_trace_snapshot_path(&case_id, scenario);
            let expected_text = fs::read_to_string(&expected_path).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!(
                        "missing runtime trace snapshot {}: {err} (run interp-trace-gen)",
                        expected_path.display()
                    ),
                )
            })?;
            let expected = parse_runtime_trace_json(&expected_text)?;
            let trace = run_interp_trace_case(&InterpTraceDumpOptions {
                case: case.clone(),
                scenario,
                lane: options.lane,
                sample_rate: options.sample_rate,
                block_size: options.block_size,
                num_blocks: options.num_blocks,
                strict_fir_types: options.strict_fir_types,
                out: None,
            })?;
            let actual = render_runtime_trace_json(&trace);
            let actual_parsed = parse_runtime_trace_json(&actual)?;
            if let Err(mismatch) = compare_runtime_traces(&expected, &actual_parsed, tol) {
                return Err(format!(
                    "interp-trace-check failed for {} [{}]: mismatch {:?} ({})",
                    case.display(),
                    scenario.as_str(),
                    mismatch,
                    expected_path.display()
                )
                .into());
            }
            println!("ok {} [{}]", case.display(), scenario.as_str());
            checked += 1;
        }
    }
    println!("interp-trace-check: {checked} trace snapshot(s) matched");
    Ok(())
}

/// Compares `opt_level=0` and `opt_level=max` interpreter traces on selected cases.
///
/// This is a low-cost metamorphic guardrail: the bytecode optimizer may change
/// execution strategy but must not change observable sample outputs.
pub(crate) fn interp_trace_diff_opt_levels_cases(
    cases: &[PathBuf],
    strict_fir_types: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let tol = TraceCompareTolerances::default();
    let default_options = InterpTraceBatchOptions::default();
    let mut compared = 0usize;

    for case in cases {
        let scenarios = trace_scenarios_for_runtime_case(case)?;
        if scenarios.is_empty() {
            println!(
                "skip {} (no snapshot-enabled scenarios yet)",
                case.display()
            );
            continue;
        }

        for scenario in scenarios {
            let base = InterpTraceDumpOptions {
                case: case.clone(),
                scenario,
                lane: TraceLane::Fast,
                sample_rate: default_options.sample_rate,
                block_size: default_options.block_size,
                num_blocks: default_options.num_blocks,
                strict_fir_types,
                out: None,
            };
            let unoptimized = run_interp_trace_case_with_opt_level(&base, 0)?;
            let optimized = run_interp_trace_case_with_opt_level(
                &base,
                codegen::backends::interp::MAX_OPT_LEVEL.into(),
            )?;
            if let Err(mismatch) = compare_runtime_traces(&unoptimized, &optimized, tol) {
                return Err(format!(
                    "interp opt-level diff failed for {} [{}]: mismatch {:?}",
                    case.display(),
                    scenario.as_str(),
                    mismatch
                )
                .into());
            }
            println!(
                "match {} [{}] (interp opt_level=0 vs opt_level=max)",
                case.display(),
                scenario.as_str()
            );
            compared += 1;
        }
    }

    println!("interp opt-level diff: {compared} trace(s) matched");
    Ok(())
}

/// Representative checked-vector corpus cases for the interpreter optimizer
/// metamorphic gate. They cover delayed recursion, structural recursion,
/// checked prefix state, and the vector UI lifecycle respectively.
const VECTOR_INTERP_OPT_CASES: &[&str] = &[
    "tests/corpus/vector_lockstep_pair.dsp",
    "tests/impulse-tests/dsp/APF.dsp",
    "tests/impulse-tests/dsp/capture.dsp",
    "tests/impulse-tests/dsp/prefix.dsp",
    "tests/impulse-tests/dsp/UITester.dsp",
];

/// Compares interpreter `opt_level=0` and maximum optimization for a compact
/// vector subset across both loop variants and every scheduling strategy.
///
/// This is intentionally separate from the scalar runtime-trace snapshots:
/// the guard requires the checked vector module to remain certified before its
/// optimized and unoptimized bytecode executions are compared.
pub(crate) fn vector_interp_opt_check(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(option) = args.next() {
        return Err(format!("unknown vector-interp-opt-check option: {option}").into());
    }
    let defaults = InterpTraceBatchOptions::default();
    let mut compared = 0usize;
    for relative in VECTOR_INTERP_OPT_CASES {
        let case = workspace_root().join(relative);
        for loop_variant in [0_u8, 1] {
            for scheduling_strategy in [
                compiler::SchedulingStrategy::DepthFirst,
                compiler::SchedulingStrategy::BreadthFirst,
                compiler::SchedulingStrategy::Special,
                compiler::SchedulingStrategy::ReverseBreadthFirst,
            ] {
                let base = InterpTraceDumpOptions {
                    case: case.clone(),
                    scenario: TraceScenario::Impulse,
                    lane: TraceLane::Fast,
                    sample_rate: defaults.sample_rate,
                    block_size: defaults.block_size,
                    num_blocks: defaults.num_blocks,
                    strict_fir_types: false,
                    out: None,
                };
                let compute_mode = compiler::ComputeMode::Vector {
                    vec_size: compiler::ComputeMode::DEFAULT_VEC_SIZE,
                    loop_variant,
                };
                let unoptimized = run_interp_trace_case_with_configuration(
                    &base,
                    0,
                    compute_mode,
                    scheduling_strategy,
                    true,
                )?;
                let optimized = run_interp_trace_case_with_configuration(
                    &base,
                    codegen::backends::interp::MAX_OPT_LEVEL.into(),
                    compute_mode,
                    scheduling_strategy,
                    true,
                )?;
                if let Err(mismatch) = compare_runtime_traces(
                    &unoptimized,
                    &optimized,
                    TraceCompareTolerances::default(),
                ) {
                    return Err(format!(
                        "vector interp opt-level diff failed for {} [-lv {} {:?}]: mismatch {:?}",
                        case.display(),
                        loop_variant,
                        scheduling_strategy,
                        mismatch
                    )
                    .into());
                }
                println!(
                    "match {} [-lv {} {:?}] (checked vector interp opt_level=0 vs max)",
                    case.display(),
                    loop_variant,
                    scheduling_strategy
                );
                compared += 1;
            }
        }
    }
    println!("vector-interp-opt-check: {compared} checked vector trace(s) matched");
    Ok(())
}

/// Parses shared batch options for `interp-trace-gen` and `interp-trace-check`.
pub(crate) fn parse_interp_trace_batch_options(
    args: &mut impl Iterator<Item = String>,
) -> Result<InterpTraceBatchOptions, Box<dyn std::error::Error>> {
    let mut options = InterpTraceBatchOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => {
                let Some(path) = args.next() else {
                    return Err("missing value after --case".into());
                };
                options.case = Some(PathBuf::from(path));
            }
            "--lane" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --lane".into());
                };
                options.lane = TraceLane::parse(&value)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }
            "--sample-rate" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --sample-rate".into());
                };
                options.sample_rate = value.parse::<usize>()?;
            }
            "--block-size" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --block-size".into());
                };
                options.block_size = value.parse::<usize>()?;
            }
            "--num-blocks" => {
                let Some(value) = args.next() else {
                    return Err("missing value after --num-blocks".into());
                };
                options.num_blocks = value.parse::<usize>()?;
            }
            "--strict-fir-types" => {
                options.strict_fir_types = true;
            }
            "--help" | "-h" => {
                return Err("usage: cargo run -p xtask -- interp-trace-gen [--case <path>] [--lane fast] [--sample-rate N] [--block-size N] [--num-blocks N] [--strict-fir-types]".into());
            }
            other => return Err(format!("unknown interp-trace batch option: {other}").into()),
        }
    }
    if options.block_size == 0 || options.num_blocks == 0 {
        return Err("block-size and num-blocks must be > 0".into());
    }
    Ok(options)
}

/// Resolves the case list for a batch runtime-trace workflow.
pub(crate) fn runtime_trace_cases(
    options: &InterpTraceBatchOptions,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if let Some(case) = &options.case {
        return Ok(vec![case.clone()]);
    }
    let cases = runtime_corpus_files()?;
    if cases.is_empty() {
        return Err("no runtime trace corpus files found in tests/runtime_corpus".into());
    }
    Ok(cases)
}

/// Returns the enabled runtime-trace scenarios for one runtime corpus case.
///
/// The mapping is intentionally explicit so newly added runtime cases must opt
/// in with scenario choices instead of silently inheriting an arbitrary default.
pub(crate) fn trace_scenarios_for_runtime_case(
    case: &Path,
) -> Result<Vec<TraceScenario>, Box<dyn std::error::Error>> {
    let name = case_name(case)?;
    let scenarios = match name.as_str() {
        "trace_01_passthrough" => vec![TraceScenario::Impulse, TraceScenario::Ramp],
        "trace_02_gain_bias_typed" => vec![],
        "trace_03_stereo_mix" => vec![],
        "trace_07_nonlinear_clip" => vec![],
        "trace_09_ui_slider" => vec![TraceScenario::Impulse],
        "trace_22_parallel_mix" => vec![],
        "trace_31_extended_primitives_typed" => vec![TraceScenario::Zeros],
        "trace_38_sine_phasor" => vec![],
        "trace_40_int_plus_one" => vec![TraceScenario::Ramp],
        other => {
            return Err(format!(
                "no runtime trace scenario mapping defined for {other} (update xtask)"
            )
            .into());
        }
    };
    Ok(scenarios)
}

/// Returns the checked-in snapshot path for one runtime trace case/scenario.
pub(crate) fn runtime_trace_snapshot_path(case_id: &str, scenario: TraceScenario) -> PathBuf {
    runtime_trace_snapshot_root()
        .join(case_id)
        .join(format!("{}.json", scenario.as_str()))
}

/// Runs one DSP through the Rust interpreter backend and captures the outputs.
pub(crate) fn run_interp_trace_case(
    options: &InterpTraceDumpOptions,
) -> Result<RuntimeTrace, Box<dyn std::error::Error>> {
    run_interp_trace_case_with_opt_level(options, 0)
}

/// Runs one DSP through the Rust interpreter backend with an explicit optimizer level.
pub(crate) fn run_interp_trace_case_with_opt_level(
    options: &InterpTraceDumpOptions,
    opt_level: i32,
) -> Result<RuntimeTrace, Box<dyn std::error::Error>> {
    run_interp_trace_case_with_configuration(
        options,
        opt_level,
        compiler::ComputeMode::Scalar,
        compiler::SchedulingStrategy::DepthFirst,
        false,
    )
}

fn run_interp_trace_case_with_configuration(
    options: &InterpTraceDumpOptions,
    opt_level: i32,
    compute_mode: compiler::ComputeMode,
    scheduling_strategy: compiler::SchedulingStrategy,
    require_checked_vector: bool,
) -> Result<RuntimeTrace, Box<dyn std::error::Error>> {
    let compiler = compiler::Compiler::new()
        .with_fir_verify_options(compiler::FirVerifyOptions {
            enabled: true,
            strict: false,
        })
        .with_compute_mode(compute_mode)
        .with_scheduling_strategy(scheduling_strategy);

    let signals = compiler.compile_file_default_to_signals(&options.case)?;
    let fir = compiler
        .compile_file_default_to_fir_with_lane(&options.case, options.lane.to_signal_fir_lane())?;
    if require_checked_vector
        && (fir.vector_pipeline_status != compiler::VectorPipelineStatus::Certified
            || fir.vector_effective_mode != compiler::VectorEffectiveMode::CertifiedVector
            || fir.vector_pipeline_detail.is_some())
    {
        return Err(format!(
            "{} did not retain checked vector FIR: status={:?}, effective={:?}, detail={}",
            options.case.display(),
            fir.vector_pipeline_status,
            fir.vector_effective_mode,
            fir.vector_pipeline_detail.as_deref().unwrap_or("-")
        )
        .into());
    }
    if options.strict_fir_types {
        enforce_strict_fir_type_diagnostics(&fir.store, fir.module, &options.case)?;
    }

    let interp_options = codegen::backends::interp::InterpOptions {
        opt_level,
        module_name: None,
    };
    let mut factory = codegen::backends::interp::generate_interp_module::<f32>(
        &fir.store,
        fir.module,
        &interp_options,
    )?;
    let mut instance = codegen::backends::interp::FbcDspInstance::new(&mut factory);
    instance.init(options.sample_rate as i32);

    let total_samples = options.block_size * options.num_blocks;
    let input_channels = generate_trace_inputs(
        options.scenario,
        signals.process_arity.inputs,
        total_samples,
        options.sample_rate,
    );
    let mut output_channels = vec![vec![0.0f32; total_samples]; signals.process_arity.outputs];

    for block_idx in 0..options.num_blocks {
        let start = block_idx * options.block_size;
        let end = start + options.block_size;
        let input_refs: Vec<&[f32]> = input_channels.iter().map(|ch| &ch[start..end]).collect();
        let mut output_refs: Vec<&mut [f32]> = output_channels
            .iter_mut()
            .map(|ch| &mut ch[start..end])
            .collect();
        instance
            .try_compute(options.block_size as i32, &input_refs, &mut output_refs)
            .map_err(|e| {
                format!(
                    "interp runtime execution failed in compute block (block_idx={}): {e}",
                    block_idx
                )
            })?;
    }

    Ok(RuntimeTrace {
        dsp_path: workspace_relative_path(&options.case),
        lane: options.lane.as_str().to_string(),
        scenario: options.scenario.as_str().to_string(),
        sample_rate: options.sample_rate,
        block_size: options.block_size,
        num_blocks: options.num_blocks,
        num_inputs: signals.process_arity.inputs,
        num_outputs: signals.process_arity.outputs,
        outputs: output_channels,
    })
}

/// Resolves the Faust C++ compiler binary used to generate `.fbc` fixtures.
pub(crate) fn resolve_faust_cpp_bin(
    explicit: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("FAUST_CPP_BIN") {
        return Ok(PathBuf::from(path));
    }
    Ok(PathBuf::from("faust"))
}

/// Invokes the Faust C++ compiler to produce an interpreter `.fbc` file.
pub(crate) fn compile_dsp_to_cpp_fbc(
    faust_bin: &Path,
    dsp_case: &Path,
    fbc_out: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new(faust_bin);
    cmd.arg("-lang").arg("interp");
    for inc in default_import_search_paths(dsp_case) {
        cmd.arg("-I").arg(inc);
    }
    cmd.arg(dsp_case);
    cmd.arg("-o").arg(fbc_out);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = cmd.output().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "failed to spawn Faust C++ binary {}: {e}",
                faust_bin.display()
            ),
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "faust -lang interp failed for {} with status {}\nstdout:\n{}\nstderr:\n{}",
            dsp_case.display(),
            output.status,
            stdout.trim(),
            stderr.trim()
        )
        .into());
    }
    if !fbc_out.is_file() {
        return Err(format!(
            "faust reported success but did not produce .fbc output: {}",
            fbc_out.display()
        )
        .into());
    }
    Ok(())
}

/// Runs one trace case by first compiling the DSP through the C++ `.fbc` path.
pub(crate) fn run_interp_trace_case_from_cpp_fbc(
    options: &InterpTraceCppFbcDumpOptions,
) -> Result<RuntimeTrace, Box<dyn std::error::Error>> {
    let faust_bin = resolve_faust_cpp_bin(options.faust_bin.as_deref())?;
    let case_id = case_name(&options.trace.case)?;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let fbc_path = std::env::temp_dir().join(format!("faust_rs_xtask_{case_id}_{pid}_{nanos}.fbc"));
    compile_dsp_to_cpp_fbc(&faust_bin, &options.trace.case, &fbc_path)?;

    let trace_result = (|| -> Result<RuntimeTrace, Box<dyn std::error::Error>> {
        let file = fs::File::open(&fbc_path)?;
        let mut reader = io::BufReader::new(file);
        let mut factory: codegen::backends::interp::FbcDspFactory<f32> =
            codegen::backends::interp::read_fbc(&mut reader).map_err(|e| {
                format!(
                    "failed to read C++ generated .fbc {}: {e}",
                    fbc_path.display()
                )
            })?;
        let num_inputs = factory.num_inputs.max(0) as usize;
        let num_outputs = factory.num_outputs.max(0) as usize;

        let mut instance = codegen::backends::interp::FbcDspInstance::new(&mut factory);
        instance.init(options.trace.sample_rate as i32);

        let total_samples = options.trace.block_size * options.trace.num_blocks;
        let input_channels = generate_trace_inputs(
            options.trace.scenario,
            num_inputs,
            total_samples,
            options.trace.sample_rate,
        );
        let mut output_channels = vec![vec![0.0f32; total_samples]; num_outputs];
        for block_idx in 0..options.trace.num_blocks {
            let start = block_idx * options.trace.block_size;
            let end = start + options.trace.block_size;
            let input_refs: Vec<&[f32]> = input_channels.iter().map(|ch| &ch[start..end]).collect();
            let mut output_refs: Vec<&mut [f32]> = output_channels
                .iter_mut()
                .map(|ch| &mut ch[start..end])
                .collect();
            instance
                .try_compute(
                    options.trace.block_size as i32,
                    &input_refs,
                    &mut output_refs,
                )
                .map_err(|e| {
                    format!(
                        "Rust interp runtime failed on C++ .fbc (block_idx={}): {e}",
                        block_idx
                    )
                })?;
        }

        Ok(RuntimeTrace {
            dsp_path: workspace_relative_path(&options.trace.case),
            lane: "cpp-fbc".to_string(),
            scenario: options.trace.scenario.as_str().to_string(),
            sample_rate: options.trace.sample_rate,
            block_size: options.trace.block_size,
            num_blocks: options.trace.num_blocks,
            num_inputs,
            num_outputs,
            outputs: output_channels,
        })
    })();

    let _ = fs::remove_file(&fbc_path);
    trace_result
}

/// Rejects traces when FIR verification reported type-focused diagnostics.
///
/// The filter intentionally keeps only typing/layout families so runtime-trace
/// workflows can opt into stronger type hygiene without failing on unrelated
/// structural warnings.
pub(crate) fn enforce_strict_fir_type_diagnostics(
    store: &fir::FirStore,
    module: fir::FirId,
    case: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let report = fir::checker::verify_fir_module(store, module);
    let type_diags: Vec<&fir::checker::FirDiagnostic> = report
        .diagnostics
        .iter()
        .filter(|d| is_fir_type_diagnostic_code(d.code))
        .collect();
    if type_diags.is_empty() {
        return Ok(());
    }

    let mut msg = format!(
        "strict FIR type diagnostics present for {}: {} diagnostic(s)",
        case.display(),
        type_diags.len()
    );
    for d in type_diags.iter().take(4) {
        let sev = match d.severity {
            fir::checker::Severity::Error => "error",
            fir::checker::Severity::Warning => "warning",
        };
        let fn_ctx = d
            .context
            .function_name
            .as_deref()
            .map(|f| format!(" (fn={f})"))
            .unwrap_or_default();
        msg.push_str(&format!("\n- {sev} [{}] {}{}", d.code, d.message, fn_ctx));
    }
    if type_diags.len() > 4 {
        msg.push_str(&format!("\n- ... {} more", type_diags.len() - 4));
    }
    Err(msg.into())
}

/// Returns `true` when a FIR diagnostic code belongs to the strict type subset.
pub(crate) fn is_fir_type_diagnostic_code(code: &str) -> bool {
    code.starts_with("FIR-B")
        || code.starts_with("FIR-U")
        || code.starts_with("FIR-C")
        || code.starts_with("FIR-FC")
        || code.starts_with("FIR-T")
        || code.starts_with("FIR-MA")
        || matches!(code, "FIR-R01" | "FIR-L03" | "FIR-SW01")
}

/// Generates deterministic numeric input channels for one trace scenario.
pub(crate) fn generate_trace_inputs(
    scenario: TraceScenario,
    num_inputs: usize,
    total_samples: usize,
    sample_rate: usize,
) -> Vec<Vec<f32>> {
    let mut inputs = vec![vec![0.0f32; total_samples]; num_inputs];
    match scenario {
        TraceScenario::Zeros => {}
        TraceScenario::Impulse => {
            if total_samples > 0 {
                for channel in &mut inputs {
                    channel[0] = 1.0;
                }
            }
        }
        TraceScenario::Ramp => {
            if total_samples == 0 {
                return inputs;
            }
            let denom = (total_samples.saturating_sub(1)).max(1) as f32;
            for channel in &mut inputs {
                for (i, sample) in channel.iter_mut().enumerate() {
                    *sample = (i as f32) / denom;
                }
            }
        }
        TraceScenario::Sine => {
            let sr = sample_rate.max(1) as f32;
            let freq_hz = 440.0f32;
            let w = core::f32::consts::TAU * freq_hz / sr;
            for (ch_idx, channel) in inputs.iter_mut().enumerate() {
                let phase = (ch_idx as f32) * 0.25 * core::f32::consts::TAU;
                for (i, sample) in channel.iter_mut().enumerate() {
                    *sample = (w * (i as f32) + phase).sin();
                }
            }
        }
    }
    inputs
}

/// Renders a checked-in runtime-trace JSON payload.
///
/// The structure is kept stable and explicit instead of deriving `Serialize`
/// directly from [`RuntimeTrace`] so snapshot formatting stays deterministic.
pub(crate) fn render_runtime_trace_json(trace: &RuntimeTrace) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "  \"schema_version\": 1,");
    let _ = writeln!(out, "  \"dsp\": \"{}\",", json_escape(&trace.dsp_path));
    let _ = writeln!(out, "  \"backend\": \"interp\",");
    let _ = writeln!(out, "  \"pipeline\": {{");
    let _ = writeln!(out, "    \"signal_fir_lane\": \"{}\"", trace.lane);
    let _ = writeln!(out, "  }},");
    let _ = writeln!(out, "  \"runtime\": {{");
    let _ = writeln!(out, "    \"sample_rate\": {},", trace.sample_rate);
    let _ = writeln!(out, "    \"block_size\": {},", trace.block_size);
    let _ = writeln!(out, "    \"num_blocks\": {}", trace.num_blocks);
    let _ = writeln!(out, "  }},");
    let _ = writeln!(out, "  \"scenario\": {{");
    let _ = writeln!(out, "    \"name\": \"{}\",", trace.scenario);
    let _ = writeln!(out, "    \"inputs\": {},", trace.num_inputs);
    let _ = writeln!(out, "    \"outputs\": {}", trace.num_outputs);
    let _ = writeln!(out, "  }},");
    let _ = writeln!(out, "  \"outputs\": [");
    for (ch_idx, channel) in trace.outputs.iter().enumerate() {
        let _ = write!(out, "    [");
        for (i, sample) in channel.iter().enumerate() {
            if i > 0 {
                let _ = write!(out, ", ");
            }
            let _ = write!(out, "{:.9}", sample);
        }
        let _ = writeln!(
            out,
            "]{}",
            if ch_idx + 1 == trace.outputs.len() {
                ""
            } else {
                ","
            }
        );
    }
    let _ = writeln!(out, "  ]");
    let _ = writeln!(out, "}}");
    out
}

/// Escapes a string for inclusion in the hand-written runtime-trace JSON output.
pub(crate) fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Serde-facing schema used to parse persisted runtime-trace snapshots.
#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeTraceJson {
    dsp: String,
    pipeline: RuntimeTracePipelineJson,
    runtime: RuntimeTraceRuntimeJson,
    scenario: RuntimeTraceScenarioJson,
    outputs: Vec<Vec<f32>>,
}

/// Nested `pipeline` section of a persisted runtime-trace snapshot.
#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeTracePipelineJson {
    signal_fir_lane: String,
}

/// Nested `runtime` section of a persisted runtime-trace snapshot.
#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeTraceRuntimeJson {
    sample_rate: usize,
    block_size: usize,
    num_blocks: usize,
}

/// Nested `scenario` section of a persisted runtime-trace snapshot.
#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeTraceScenarioJson {
    name: String,
    inputs: usize,
    outputs: usize,
}

/// Parses one runtime-trace snapshot JSON payload into [`RuntimeTrace`].
pub(crate) fn parse_runtime_trace_json(
    text: &str,
) -> Result<RuntimeTrace, Box<dyn std::error::Error>> {
    let parsed: RuntimeTraceJson = serde_json::from_str(text)?;
    Ok(RuntimeTrace {
        dsp_path: parsed.dsp,
        lane: parsed.pipeline.signal_fir_lane,
        scenario: parsed.scenario.name,
        sample_rate: parsed.runtime.sample_rate,
        block_size: parsed.runtime.block_size,
        num_blocks: parsed.runtime.num_blocks,
        num_inputs: parsed.scenario.inputs,
        num_outputs: parsed.scenario.outputs,
        outputs: parsed.outputs,
    })
}

/// Compares two runtime traces field-by-field with float tolerances on samples.
pub(crate) fn compare_runtime_traces(
    expected: &RuntimeTrace,
    actual: &RuntimeTrace,
    tol: TraceCompareTolerances,
) -> Result<(), TraceMismatch> {
    if expected.dsp_path != actual.dsp_path {
        return Err(TraceMismatch {
            field: "dsp".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.lane != actual.lane {
        return Err(TraceMismatch {
            field: "pipeline.signal_fir_lane".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.scenario != actual.scenario {
        return Err(TraceMismatch {
            field: "scenario.name".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.sample_rate != actual.sample_rate {
        return Err(TraceMismatch {
            field: "runtime.sample_rate".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.block_size != actual.block_size {
        return Err(TraceMismatch {
            field: "runtime.block_size".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.num_blocks != actual.num_blocks {
        return Err(TraceMismatch {
            field: "runtime.num_blocks".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.num_inputs != actual.num_inputs {
        return Err(TraceMismatch {
            field: "scenario.inputs".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.num_outputs != actual.num_outputs {
        return Err(TraceMismatch {
            field: "scenario.outputs".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    if expected.outputs.len() != actual.outputs.len() {
        return Err(TraceMismatch {
            field: "outputs.channel_count".into(),
            channel: None,
            sample: None,
            expected: None,
            actual: None,
        });
    }
    for (ch_idx, (exp_ch, act_ch)) in expected.outputs.iter().zip(&actual.outputs).enumerate() {
        if exp_ch.len() != act_ch.len() {
            return Err(TraceMismatch {
                field: "outputs.sample_count".into(),
                channel: Some(ch_idx),
                sample: None,
                expected: None,
                actual: None,
            });
        }
        for (i, (&e, &a)) in exp_ch.iter().zip(act_ch.iter()).enumerate() {
            if !trace_sample_equal(e, a, tol) {
                return Err(TraceMismatch {
                    field: "outputs".into(),
                    channel: Some(ch_idx),
                    sample: Some(i),
                    expected: Some(e),
                    actual: Some(a),
                });
            }
        }
    }
    Ok(())
}

/// Compares two floating-point samples using mixed absolute/relative tolerance.
pub(crate) fn trace_sample_equal(expected: f32, actual: f32, tol: TraceCompareTolerances) -> bool {
    if expected.is_nan() || actual.is_nan() {
        return expected.is_nan() && actual.is_nan();
    }
    if expected.is_infinite() || actual.is_infinite() {
        return expected == actual;
    }
    let diff = (expected - actual).abs();
    let scale = expected.abs().max(actual.abs());
    diff <= tol.abs_tol + tol.rel_tol * scale
}

/// Renders the Rust golden snapshot text for one corpus input.
pub(crate) fn render_rust_snapshot(input: &Path) -> Result<String, io::Error> {
    let source = fs::read_to_string(input)?;
    let name = input
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid input filename"))?;
    Ok(compiler::golden_snapshot(name, &source))
}

/// Returns the default import search paths used for corpus/golden compilation.
pub(crate) fn default_import_search_paths(input: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(parent) = input.parent() {
        paths.push(parent.to_path_buf());
    }
    {
        let path = PathBuf::from("/usr/local/share/faust");
        if path.is_dir() {
            paths.push(path);
        }
    }
    paths
}

/// Compiles one DSP through the Rust C++ backend and returns the rendered source.
pub(crate) fn render_rust_cpp_output(input: &Path) -> Result<String, compiler::CompilerError> {
    let compiler = compiler::Compiler::new();
    let options = codegen::backends::cpp::CppOptions::default();
    let search_paths = default_import_search_paths(input);
    compiler.compile_file_to_cpp(input, &search_paths, &options)
}

/// Regenerates all Rust golden snapshots from `tests/corpus`.
pub(crate) fn golden_gen_rust() -> Result<(), Box<dyn std::error::Error>> {
    let files = corpus_files()?;
    for file in files {
        if !is_rust_golden_eligible(&file) {
            continue;
        }
        let case = case_name(&file)?;
        let output = golden_file_for_ref(&case, GoldenRef::Rust);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let snapshot = normalize(&render_rust_snapshot(&file)?);
        fs::write(&output, snapshot)?;
        println!("updated {}", output.display());
    }
    Ok(())
}

/// Regenerates C++ golden snapshots using the external Faust reference binary.
pub(crate) fn golden_gen_cpp(extra_args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let cpp_bin = std::env::var_os("FAUST_CPP_BIN").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "FAUST_CPP_BIN is not set. Example: FAUST_CPP_BIN=/path/to/faust",
        )
    })?;

    let files = corpus_files()?;
    for file in files {
        let case = case_name(&file)?;
        let output = golden_file_for_ref(&case, GoldenRef::Cpp);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut cmd = Command::new(&cpp_bin);
        cmd.arg(&file);
        for arg in extra_args {
            cmd.arg(arg);
        }

        let result = cmd.output()?;
        if !result.status.success() {
            return Err(format!(
                "C++ reference command failed for {} with status {}",
                file.display(),
                result.status
            )
            .into());
        }

        let stdout = String::from_utf8(result.stdout)?;
        fs::write(&output, normalize(&stdout))?;
        println!("updated {}", output.display());
    }

    Ok(())
}

/// Resolves the active golden reference family from `GOLDEN_REF` or defaults.
pub(crate) fn golden_ref_from_env() -> Result<GoldenRef, Box<dyn std::error::Error>> {
    let Some(raw) = std::env::var_os("GOLDEN_REF") else {
        return Ok(GoldenRef::Rust);
    };
    let value = raw
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid GOLDEN_REF value"))?;
    match value {
        "rust" => Ok(GoldenRef::Rust),
        "cpp" => Ok(GoldenRef::Cpp),
        _ => Err(format!("invalid GOLDEN_REF={value}; expected rust or cpp").into()),
    }
}

/// Validates generated snapshots against the selected Rust or C++ golden family.
pub(crate) fn golden_check(forced: Option<GoldenRef>) -> Result<(), Box<dyn std::error::Error>> {
    let golden_ref = match forced {
        Some(value) => value,
        None => golden_ref_from_env()?,
    };

    let files = golden_cases_for_check(golden_ref)?;
    if files.is_empty() {
        return Err(format!(
            "no golden cases found for reference `{}`",
            golden_ref.as_dir_name()
        )
        .into());
    }
    let mut failures = 0usize;

    for (case, file) in files {
        if !file.exists() {
            return Err(format!(
                "missing corpus file for golden case `{case}`: {}",
                file.display()
            )
            .into());
        }
        let expected_path = golden_file_for_ref(&case, golden_ref);
        let expected = fs::read_to_string(&expected_path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "missing golden file {} (run golden-gen-rust or golden-gen-cpp): {err}",
                    expected_path.display()
                ),
            )
        })?;

        let actual = match golden_ref {
            GoldenRef::Rust => normalize(&render_rust_snapshot(&file)?),
            GoldenRef::Cpp => match render_rust_cpp_output(&file) {
                Ok(output) => normalize(&output),
                Err(error) => format!("__RUST_CPP_ERROR__\n{error}\n"),
            },
        };
        let expected = normalize(&expected);

        if actual != expected {
            failures += 1;
            println!("[FAIL] {case}");
            println!("  expected: {}", expected_path.display());
            println!("  first diff:");
            print_first_diff(&expected, &actual);
        } else {
            println!("[OK] {case}");
        }
    }

    if failures > 0 {
        return Err(format!("golden-check failed: {failures} case(s) differ").into());
    }

    Ok(())
}

/// Prints the first differing line between two normalized snapshot texts.
pub(crate) fn print_first_diff(expected: &str, actual: &str) {
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let max = expected_lines.len().max(actual_lines.len());

    for idx in 0..max {
        let e = expected_lines.get(idx).copied().unwrap_or("<missing>");
        let a = actual_lines.get(idx).copied().unwrap_or("<missing>");
        if e != a {
            println!("    line {}", idx + 1);
            println!("      expected: {e}");
            println!("      actual:   {a}");
            return;
        }
    }
}
