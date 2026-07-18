//! Persistent vector-corpus coverage reports and retention checks.
//!
//! The report is generated from the `count_vector_corpus` diagnostic example.
//! This checker treats effective vector structure as a separate invariant from
//! numerical parity: every baseline-certified DSP must still produce a checked
//! vector module containing the canonical `vindex`/`vcount` chunk driver.

use super::*;
use compiler::{
    Compiler, ComputeMode, RealType, SchedulingStrategy, SignalFirLane, VectorEffectiveMode,
    VectorPipelineStatus,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const VECTOR_COVERAGE_BASELINE: &str = "tests/vector-coverage/corpus-baseline.json";
const VECTOR_CERTIFIED_LIST: &str = "tests/vector-coverage/certified-dspfiles.txt";
const VECTOR_CORPUS_ROOT: &str = "tests/impulse-tests/dsp";
const VECTOR_COVERAGE_SCHEMA: u32 = 1;
const VECTOR_COVERAGE_WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VectorCoverageBaseline {
    schema_version: u32,
    corpus_root: String,
    modes: Vec<VectorModeReport>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VectorModeReport {
    mode: VectorMode,
    summary: VectorSummary,
    certified_files: Vec<String>,
    fallback_files: Vec<VectorFallbackEntry>,
    error_files: Vec<VectorErrorEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct VectorMode {
    vector: bool,
    precision: String,
    loop_variant: u8,
    scheduling_strategy: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VectorSummary {
    total: usize,
    certified: usize,
    fallback: usize,
    error: usize,
    fallback_by_reason: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VectorFallbackEntry {
    path: String,
    status: String,
    reason_code: String,
    effective_mode: String,
    detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VectorErrorEntry {
    path: String,
    error: String,
}

pub(crate) fn vector_coverage_merge(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut reports = None;
    let mut out = workspace_root().join(VECTOR_COVERAGE_BASELINE);
    let mut certified_list = workspace_root().join(VECTOR_CERTIFIED_LIST);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--reports" => reports = Some(PathBuf::from(required_arg(&mut args, "--reports")?)),
            "--out" => out = PathBuf::from(required_arg(&mut args, "--out")?),
            "--certified-list" => {
                certified_list = PathBuf::from(required_arg(&mut args, "--certified-list")?)
            }
            other => return Err(format!("unknown vector-coverage-merge option: {other}").into()),
        }
    }
    let reports = reports.ok_or("vector-coverage-merge requires --reports <directory>")?;
    let mut paths = fs::read_dir(&reports)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no JSON reports found in {}", reports.display()).into());
    }

    let mut modes = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path)?;
        let mut report: VectorModeReport = serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        sort_mode_report(&mut report);
        modes.push(report);
    }
    modes.sort_by(|left, right| left.mode.cmp(&right.mode));
    let baseline = VectorCoverageBaseline {
        schema_version: VECTOR_COVERAGE_SCHEMA,
        corpus_root: VECTOR_CORPUS_ROOT.to_owned(),
        modes,
    };
    let corpus = vector_corpus_files()?;
    validate_vector_coverage(&baseline, &corpus)?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(&baseline)?;
    json.push('\n');
    fs::write(&out, json)?;
    let certified = universally_certified(&baseline);
    if let Some(parent) = certified_list.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut list = certified.iter().cloned().collect::<Vec<_>>().join("\n");
    list.push('\n');
    fs::write(&certified_list, list)?;
    println!(
        "vector coverage baseline: {} modes, {} DSPs, {} universally certified -> {} and {}",
        baseline.modes.len(),
        corpus.len(),
        certified.len(),
        out.display(),
        certified_list.display()
    );
    Ok(())
}

pub(crate) fn vector_coverage_check(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut baseline_path = workspace_root().join(VECTOR_COVERAGE_BASELINE);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--baseline" => baseline_path = PathBuf::from(required_arg(&mut args, "--baseline")?),
            other => return Err(format!("unknown vector-coverage-check option: {other}").into()),
        }
    }
    let text = fs::read_to_string(&baseline_path)?;
    let baseline: VectorCoverageBaseline = serde_json::from_str(&text)?;
    let corpus = vector_corpus_files()?;
    validate_vector_coverage(&baseline, &corpus)?;
    validate_certified_list(&baseline)?;

    let root = workspace_root();
    let corpus_root = root.join(&baseline.corpus_root);
    let search_paths = vec![corpus_root, PathBuf::from("/usr/local/share/faust")];
    let worker_count = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(4)
        .min(baseline.modes.len());
    println!(
        "vector retention: checking {} modes with {} bounded worker(s)",
        baseline.modes.len(),
        worker_count
    );

    let next_mode = AtomicUsize::new(0);
    let mode_results = Mutex::new(vec![None; baseline.modes.len()]);
    std::thread::scope(|scope| -> Result<(), std::io::Error> {
        for worker_index in 0..worker_count {
            std::thread::Builder::new()
                .name(format!("vector-coverage-{worker_index}"))
                .stack_size(VECTOR_COVERAGE_WORKER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    loop {
                        let mode_index = next_mode.fetch_add(1, Ordering::Relaxed);
                        let Some(report) = baseline.modes.get(mode_index) else {
                            break;
                        };
                        let result = check_vector_retention_mode(&root, &search_paths, report);
                        mode_results
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)[mode_index] =
                            Some(result);
                    }
                })?;
        }
        Ok(())
    })?;

    let mode_results = mode_results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut checked = 0usize;
    for (mode_index, (report, result)) in baseline.modes.iter().zip(mode_results).enumerate() {
        println!(
            "vector retention [{}/{}]: precision={} -lv {} -ss {} ({} certified)",
            mode_index + 1,
            baseline.modes.len(),
            report.mode.precision,
            report.mode.loop_variant,
            report.mode.scheduling_strategy,
            report.certified_files.len()
        );
        checked += result
            .ok_or_else(|| format!("vector retention worker omitted mode {mode_index}"))?
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    }
    println!(
        "vector-coverage-check: OK ({} retained certified mode/DSP pairs, {} modes, {} corpus DSPs)",
        checked,
        baseline.modes.len(),
        corpus.len()
    );
    Ok(())
}

/// Recompiles and structurally verifies one complete vector-retention mode.
///
/// Mode-level isolation lets the command bound parallelism without sharing a
/// compiler instance or changing the fail-closed checks applied to each DSP.
fn check_vector_retention_mode(
    root: &Path,
    search_paths: &[PathBuf],
    report: &VectorModeReport,
) -> Result<usize, String> {
    let real_type = real_type(&report.mode.precision).map_err(|error| error.to_string())?;
    for relative in &report.certified_files {
        let path = root.join(relative);
        let output = Compiler::new()
            .with_real_type(real_type)
            .with_compute_mode(ComputeMode::Vector {
                vec_size: ComputeMode::DEFAULT_VEC_SIZE,
                loop_variant: report.mode.loop_variant,
            })
            .with_scheduling_strategy(SchedulingStrategy::decode(u32::from(
                report.mode.scheduling_strategy,
            )))
            .compile_file_to_fir_with_lane(&path, search_paths, SignalFirLane::TransformFastLane)
            .map_err(|error| format!("{relative}: vector retention compile failed: {error}"))?;
        if output.vector_pipeline_status != VectorPipelineStatus::Certified
            || output.vector_effective_mode != VectorEffectiveMode::CertifiedVector
            || output.vector_pipeline_detail.is_some()
        {
            return Err(format!(
                "{relative}: certified baseline regressed under precision={} -lv {} -ss {}: status={:?}, effective={:?}, detail={}",
                report.mode.precision,
                report.mode.loop_variant,
                report.mode.scheduling_strategy,
                output.vector_pipeline_status,
                output.vector_effective_mode,
                output.vector_pipeline_detail.as_deref().unwrap_or("-")
            ));
        }
        let dump = dump_fir(&output.store, output.module);
        if !has_checked_chunk_driver(&dump) {
            return Err(format!(
                "{relative}: claimed certified module lacks the canonical vindex/vcount chunk driver under precision={} -lv {} -ss {}",
                report.mode.precision, report.mode.loop_variant, report.mode.scheduling_strategy
            ));
        }
    }
    Ok(report.certified_files.len())
}

fn required_arg(
    args: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value").into())
}

fn real_type(precision: &str) -> Result<RealType, Box<dyn std::error::Error>> {
    match precision {
        "f32" => Ok(RealType::Float32),
        "f64" => Ok(RealType::Float64),
        other => Err(format!("unsupported vector coverage precision: {other}").into()),
    }
}

fn sort_mode_report(report: &mut VectorModeReport) {
    report.certified_files.sort();
    report
        .fallback_files
        .sort_by(|left, right| left.path.cmp(&right.path));
    report
        .error_files
        .sort_by(|left, right| left.path.cmp(&right.path));
}

fn vector_corpus_files() -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let root = workspace_root().join(VECTOR_CORPUS_ROOT);
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(&root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "dsp") {
            let relative = path.strip_prefix(workspace_root())?;
            files.insert(portable_path(relative)?);
        }
    }
    Ok(files)
}

fn portable_path(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| format!("non-UTF-8 path component in {}", path.display()))?,
            ),
            std::path::Component::CurDir => {}
            _ => {
                return Err(
                    format!("expected repository-relative path: {}", path.display()).into(),
                );
            }
        }
    }
    Ok(parts.join("/"))
}

fn expected_modes() -> BTreeSet<VectorMode> {
    let mut modes = BTreeSet::new();
    for precision in ["f32", "f64"] {
        for loop_variant in [0_u8, 1] {
            for scheduling_strategy in 0_u8..=3 {
                modes.insert(VectorMode {
                    vector: true,
                    precision: precision.to_owned(),
                    loop_variant,
                    scheduling_strategy,
                });
            }
        }
    }
    modes
}

fn universally_certified(baseline: &VectorCoverageBaseline) -> BTreeSet<String> {
    let mut modes = baseline.modes.iter();
    let Some(first) = modes.next() else {
        return BTreeSet::new();
    };
    let mut certified = first
        .certified_files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    for report in modes {
        let mode_files = report
            .certified_files
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        certified.retain(|path| mode_files.contains(path));
    }
    certified
}

fn validate_certified_list(
    baseline: &VectorCoverageBaseline,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = workspace_root().join(VECTOR_CERTIFIED_LIST);
    let listed = fs::read_to_string(&path)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let expected = universally_certified(baseline);
    if listed != expected {
        return Err(format!(
            "{} does not match the universally certified baseline intersection",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn validate_vector_coverage(
    baseline: &VectorCoverageBaseline,
    corpus: &BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if baseline.schema_version != VECTOR_COVERAGE_SCHEMA {
        return Err(format!(
            "unsupported vector coverage schema {} (expected {})",
            baseline.schema_version, VECTOR_COVERAGE_SCHEMA
        )
        .into());
    }
    if baseline.corpus_root != VECTOR_CORPUS_ROOT {
        return Err(format!("unexpected vector corpus root: {}", baseline.corpus_root).into());
    }
    let actual_modes = baseline
        .modes
        .iter()
        .map(|report| report.mode.clone())
        .collect::<BTreeSet<_>>();
    if actual_modes != expected_modes() || actual_modes.len() != baseline.modes.len() {
        return Err("vector coverage report is incomplete or contains duplicate modes".into());
    }

    for report in &baseline.modes {
        let mut listed = BTreeSet::new();
        let mut insert = |path: &str| -> Result<(), Box<dyn std::error::Error>> {
            if !listed.insert(path.to_owned()) {
                return Err(format!("duplicate DSP in vector mode report: {path}").into());
            }
            Ok(())
        };
        for path in &report.certified_files {
            insert(path)?;
        }
        for entry in &report.fallback_files {
            insert(&entry.path)?;
            if entry.effective_mode != "Scalar" {
                return Err(format!(
                    "fallback {} is not explicitly scalar: {}",
                    entry.path, entry.effective_mode
                )
                .into());
            }
        }
        for entry in &report.error_files {
            insert(&entry.path)?;
        }
        if &listed != corpus {
            return Err(format!(
                "vector mode precision={} -lv {} -ss {} does not cover the exact corpus",
                report.mode.precision, report.mode.loop_variant, report.mode.scheduling_strategy
            )
            .into());
        }
        if report.summary.total != corpus.len()
            || report.summary.certified != report.certified_files.len()
            || report.summary.fallback != report.fallback_files.len()
            || report.summary.error != report.error_files.len()
        {
            return Err("vector coverage summary counts do not match per-DSP entries".into());
        }
        let reasons = report.fallback_files.iter().fold(
            BTreeMap::<String, usize>::new(),
            |mut counts, entry| {
                *counts.entry(entry.reason_code.clone()).or_default() += 1;
                counts
            },
        );
        if reasons != report.summary.fallback_by_reason {
            return Err("vector fallback reason summary does not match per-DSP entries".into());
        }
    }
    Ok(())
}

fn has_checked_chunk_driver(dump: &str) -> bool {
    dump.contains("ForLoop { var: \"vindex\"") && dump.contains("DeclareVar { name: \"vcount\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_fixture() -> (VectorCoverageBaseline, BTreeSet<String>) {
        let path = "tests/impulse-tests/dsp/x.dsp".to_owned();
        let modes = expected_modes()
            .into_iter()
            .map(|mode| VectorModeReport {
                mode,
                summary: VectorSummary {
                    total: 1,
                    certified: 1,
                    fallback: 0,
                    error: 0,
                    fallback_by_reason: BTreeMap::new(),
                },
                certified_files: vec![path.clone()],
                fallback_files: Vec::new(),
                error_files: Vec::new(),
            })
            .collect();
        (
            VectorCoverageBaseline {
                schema_version: VECTOR_COVERAGE_SCHEMA,
                corpus_root: VECTOR_CORPUS_ROOT.to_owned(),
                modes,
            },
            BTreeSet::from([path]),
        )
    }

    #[test]
    fn complete_matrix_is_accepted() {
        let (baseline, corpus) = complete_fixture();
        validate_vector_coverage(&baseline, &corpus).unwrap();
    }

    #[test]
    fn missing_mode_is_rejected() {
        let (mut baseline, corpus) = complete_fixture();
        baseline.modes.pop();
        assert!(validate_vector_coverage(&baseline, &corpus).is_err());
    }

    #[test]
    fn scalar_fallback_is_required() {
        let (mut baseline, corpus) = complete_fixture();
        let report = &mut baseline.modes[0];
        report.certified_files.clear();
        report.fallback_files.push(VectorFallbackEntry {
            path: "tests/impulse-tests/dsp/x.dsp".to_owned(),
            status: "Fallback(VectorPlan)".to_owned(),
            reason_code: "FRS-VEC-FALLBACK-PLAN".to_owned(),
            effective_mode: "CertifiedVector".to_owned(),
            detail: Some("fixture".to_owned()),
        });
        report.summary.certified = 0;
        report.summary.fallback = 1;
        report.summary.fallback_by_reason =
            BTreeMap::from([("FRS-VEC-FALLBACK-PLAN".to_owned(), 1)]);
        assert!(validate_vector_coverage(&baseline, &corpus).is_err());
    }

    #[test]
    fn chunk_driver_shape_requires_both_markers() {
        assert!(has_checked_chunk_driver(
            "ForLoop { var: \"vindex\" DeclareVar { name: \"vcount\""
        ));
        assert!(!has_checked_chunk_driver("SimpleForLoop { var: \"i0\" }"));
    }
}
