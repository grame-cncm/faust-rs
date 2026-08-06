//! Per-stage compile-time profile over the DSP corpus.
//!
//! Answers "which compiler stage is the time in", which no existing gate does:
//! `compile-budget-check` measures *how much* compiling costs and defends a
//! ceiling, while this measures *where* the cost sits and defends nothing by
//! itself. The distinction matters because the two failure modes are different
//! — a uniform 20 % slowdown moves the budget and leaves this table unchanged,
//! and a stage that doubles while another halves does the reverse.
//!
//! # Why shares rather than seconds
//!
//! Absolute wall-clock is not comparable between machines, and
//! `compile_budget`'s header already records where that road ends: ceilings get
//! loosened until they survive the slowest runner, at which point they no
//! longer catch a 2× regression. Stage *shares* are dimensionless and survive a
//! machine change unaltered, so they are what `--baseline` compares. Seconds
//! are printed because they are what a human wants to read, and recorded in
//! JSON because a same-machine before/after is exactly how a performance change
//! is justified.
//!
//! # Provenance
//!
//! Written for phase P0 of
//! `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`,
//! whose §1.1 table this command reproduces. That analysis was assembled by
//! hand from `faust-rs -time` output; the point of P0 is that nobody should
//! have to do that again, and that P1's claim can be checked rather than
//! believed.

use super::*;
use compiler::{Compiler, FirVerifyOptions, RealType, SignalFirLane};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const COMPILE_PROFILE_CORPUS_ROOT: &str = "tests/impulse-tests/dsp";
const COMPILE_PROFILE_SCHEMA: u32 = 1;

/// The compiler's recursive traversals need more than a default thread stack on
/// library-heavy inputs; the CLI spawns 64 MiB for the same reason.
const COMPILE_PROFILE_STACK_BYTES: usize = 64 * 1024 * 1024;

/// One stage's contribution, as recorded by the compiler's timing sink.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct StageEntry {
    stage: String,
    seconds: f64,
    /// Fraction of `total_seconds`, in percent. This is the machine-independent
    /// quantity and the only one `--baseline` compares.
    share_pct: f64,
}

/// One DSP's total, for the slowest-first listing.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct CaseEntry {
    dsp: String,
    seconds: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CompileProfile {
    schema_version: u32,
    corpus_root: String,
    compiled: usize,
    /// DSPs the compiler rejected. Recorded rather than skipped silently: a
    /// profile over a shrinking corpus is not comparable with its own baseline,
    /// and a compile error appearing here is worth seeing even though this
    /// command is not a correctness gate.
    failed: Vec<String>,
    total_seconds: f64,
    stages: Vec<StageEntry>,
    slowest: Vec<CaseEntry>,
}

/// Compiles one file, returning its per-stage durations.
///
/// Timings come from the compiler's own sink — the same one `-time` prints —
/// rather than from parsing a subprocess's stderr, so the stage names cannot
/// drift out of sync with the compiler and process startup is not counted.
fn profile_one(path: &Path, search_paths: &[PathBuf]) -> Result<Vec<(String, Duration)>, String> {
    let collected: Arc<Mutex<Vec<(String, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    // Mirror the CLI's defaults, not the library's: the profile is meant to
    // explain what `faust-rs -lang cpp -double` costs, and the library default
    // leaves FIR verification off, which silently drops a stage the CLI runs.
    let compiler = Compiler::new()
        .with_real_type(RealType::Float64)
        .with_fir_verify_options(FirVerifyOptions {
            enabled: true,
            strict: false,
        })
        .with_timing_sink(move |name, duration| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((name.to_owned(), duration));
        });
    compiler
        .compile_file_to_cpp_with_lane(
            path,
            search_paths,
            &codegen::backends::cpp::CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .map_err(|error| error.to_string())?;
    let out = collected
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Ok(out)
}

/// Sums per-stage durations over the corpus.
///
/// Nesting is deliberately preserved rather than flattened: `signal-fir` spans
/// several of the `fir-*` stages, so the shares do not sum to 100 %. Reporting
/// them separately is what makes it visible that `fir-prepare-normalize` is
/// most of `signal-fir`, which a flattened tree would hide.
fn run_profile(root: &Path, filter: Option<&str>, top: usize) -> Result<CompileProfile, String> {
    let corpus_root = root.join(COMPILE_PROFILE_CORPUS_ROOT);
    let search_paths = vec![corpus_root.clone(), PathBuf::from("/usr/local/share/faust")];

    let mut files: Vec<PathBuf> = fs::read_dir(&corpus_root)
        .map_err(|error| format!("cannot read {}: {error}", corpus_root.display()))?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "dsp").then_some(path)
        })
        .collect();
    files.sort();
    if let Some(needle) = filter {
        files.retain(|p| {
            p.file_stem()
                .is_some_and(|s| s.to_string_lossy().contains(needle))
        });
    }
    if files.is_empty() {
        return Err("compile-profile: no DSP matched".to_owned());
    }

    let mut stage_totals: BTreeMap<String, f64> = BTreeMap::new();
    let mut cases: Vec<CaseEntry> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut total = 0.0_f64;

    for path in &files {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match profile_one(path, &search_paths) {
            Ok(stages) => {
                let mut case_total = 0.0;
                for (stage, duration) in stages {
                    let secs = duration.as_secs_f64();
                    // `signal-fir` and the `fir-*` stages it contains are both
                    // reported; only top-level stages may contribute to the
                    // total, or nested time would be counted twice.
                    if !stage.starts_with("fir-") {
                        case_total += secs;
                    }
                    *stage_totals.entry(stage).or_default() += secs;
                }
                total += case_total;
                cases.push(CaseEntry {
                    dsp: name,
                    seconds: case_total,
                });
            }
            Err(error) => {
                failed.push(format!("{name}: {error}"));
            }
        }
    }

    let mut stages: Vec<StageEntry> = stage_totals
        .into_iter()
        .map(|(stage, seconds)| StageEntry {
            share_pct: if total > 0.0 {
                seconds / total * 100.0
            } else {
                0.0
            },
            stage,
            seconds,
        })
        .collect();
    stages.sort_by(|a, b| {
        b.seconds
            .partial_cmp(&a.seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.stage.cmp(&b.stage))
    });

    cases.sort_by(|a, b| {
        b.seconds
            .partial_cmp(&a.seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.dsp.cmp(&b.dsp))
    });
    let slowest = cases.iter().take(top).cloned().collect();

    Ok(CompileProfile {
        schema_version: COMPILE_PROFILE_SCHEMA,
        corpus_root: COMPILE_PROFILE_CORPUS_ROOT.to_owned(),
        compiled: cases.len(),
        failed,
        total_seconds: total,
        stages,
        slowest,
    })
}

fn print_human(profile: &CompileProfile) {
    println!(
        "compile-profile: {} DSP(s) compiled, total {:.2}s",
        profile.compiled, profile.total_seconds
    );
    if !profile.failed.is_empty() {
        println!("  {} did not compile:", profile.failed.len());
        for f in &profile.failed {
            println!("    {f}");
        }
    }
    println!();
    println!("{:<28}{:>10}{:>9}", "stage", "seconds", "share");
    for s in &profile.stages {
        println!("{:<28}{:>10.3}{:>8.1}%", s.stage, s.seconds, s.share_pct);
    }
    println!();
    println!("(`signal-fir` contains the `fir-*` stages, so shares exceed 100%;");
    println!(" only top-level stages contribute to the total.)");
    if !profile.slowest.is_empty() {
        println!();
        println!("slowest DSPs:");
        for c in &profile.slowest {
            println!("  {:<28}{:>8.3}s", c.dsp, c.seconds);
        }
    }
}

/// Reports stage-share drift against a recorded profile.
///
/// Compares shares, not seconds: a baseline recorded on another machine is
/// still meaningful, and a uniform speed change must not read as drift. The
/// total is printed for context but never enforced — that is
/// `compile-budget-check`'s job, and duplicating it here would give two gates
/// that fail together for unrelated reasons.
fn compare(profile: &CompileProfile, baseline: &CompileProfile, tolerance_pct: f64) -> Vec<String> {
    let mut findings = Vec::new();
    let base: BTreeMap<&str, &StageEntry> = baseline
        .stages
        .iter()
        .map(|s| (s.stage.as_str(), s))
        .collect();
    for stage in &profile.stages {
        let Some(old) = base.get(stage.stage.as_str()) else {
            findings.push(format!(
                "{}: {:.1}% of compile time, absent from the baseline",
                stage.stage, stage.share_pct
            ));
            continue;
        };
        let delta = stage.share_pct - old.share_pct;
        if delta.abs() > tolerance_pct {
            findings.push(format!(
                "{}: share {:.1}% -> {:.1}% ({delta:+.1} points, tolerance {tolerance_pct:.1})",
                stage.stage, old.share_pct, stage.share_pct
            ));
        }
    }
    for stage in &baseline.stages {
        if !profile.stages.iter().any(|s| s.stage == stage.stage) {
            findings.push(format!(
                "{}: was {:.1}% of compile time, now absent",
                stage.stage, stage.share_pct
            ));
        }
    }
    findings
}

pub(crate) fn compile_profile(args: CompileProfileArgs) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let top = args.top.unwrap_or(8);
    let filter = args.filter.as_deref();

    // The corpus is compiled on one worker with the compiler's stack contract.
    // Single-threaded on purpose: this measures wall time per stage, and
    // sharing cores between compilations would make the numbers depend on the
    // scheduler rather than on the compiler.
    let root_for_worker = root.clone();
    let filter_owned = filter.map(str::to_owned);
    let profile = std::thread::Builder::new()
        .name("compile-profile".to_owned())
        .stack_size(COMPILE_PROFILE_STACK_BYTES)
        .spawn(move || run_profile(&root_for_worker, filter_owned.as_deref(), top))?
        .join()
        .map_err(|_| "compile-profile worker panicked")?
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    if let Some(path) = &args.write {
        let target = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, serde_json::to_string_pretty(&profile)? + "\n")?;
        println!("compile-profile: wrote {}", target.display());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&profile)?);
    } else {
        print_human(&profile);
    }

    if let Some(path) = &args.baseline {
        let target = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        let baseline: CompileProfile = serde_json::from_str(&fs::read_to_string(&target)?)?;
        let findings = compare(&profile, &baseline, args.tolerance.unwrap_or(3.0));
        println!();
        println!(
            "baseline {}: total {:.2}s -> {:.2}s",
            target.display(),
            baseline.total_seconds,
            profile.total_seconds
        );
        if findings.is_empty() {
            println!("compile-profile: OK (no stage share moved beyond tolerance)");
        } else {
            for f in &findings {
                println!("compile-profile: {f}");
            }
            return Err(
                format!("compile-profile: {} stage share finding(s)", findings.len()).into(),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(total: f64, stages: &[(&str, f64, f64)]) -> CompileProfile {
        CompileProfile {
            schema_version: COMPILE_PROFILE_SCHEMA,
            corpus_root: COMPILE_PROFILE_CORPUS_ROOT.to_owned(),
            compiled: 1,
            failed: Vec::new(),
            total_seconds: total,
            stages: stages
                .iter()
                .map(|(stage, seconds, share_pct)| StageEntry {
                    stage: (*stage).to_owned(),
                    seconds: *seconds,
                    share_pct: *share_pct,
                })
                .collect(),
            slowest: Vec::new(),
        }
    }

    /// A machine twice as fast must not read as drift.
    ///
    /// This is the property that made shares the compared quantity rather than
    /// seconds: halving every measurement leaves the distribution identical,
    /// and a gate that fired here would have to be loosened until it caught
    /// nothing — the failure `compile_budget`'s own header records.
    #[test]
    fn a_uniformly_faster_machine_is_not_drift() {
        let base = profile(20.0, &[("evaluation", 14.0, 70.0), ("parser", 6.0, 30.0)]);
        let fast = profile(10.0, &[("evaluation", 7.0, 70.0), ("parser", 3.0, 30.0)]);
        assert!(compare(&fast, &base, 3.0).is_empty());
    }

    /// A stage that grows at another's expense must be reported even when the
    /// total is unchanged — the case an absolute budget cannot see.
    #[test]
    fn a_share_shift_at_constant_total_is_reported() {
        let base = profile(20.0, &[("evaluation", 14.0, 70.0), ("parser", 6.0, 30.0)]);
        let shifted = profile(20.0, &[("evaluation", 8.0, 40.0), ("parser", 12.0, 60.0)]);
        let findings = compare(&shifted, &base, 3.0);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().any(|f| f.contains("evaluation")));
    }

    /// Movement inside the tolerance is noise, not a finding.
    #[test]
    fn small_movement_stays_within_tolerance() {
        let base = profile(20.0, &[("evaluation", 14.0, 70.0)]);
        let jittered = profile(20.0, &[("evaluation", 14.4, 72.0)]);
        assert!(compare(&jittered, &base, 3.0).is_empty());
    }

    /// A stage appearing or vanishing is structural, not a share change, and
    /// must be reported however small it is: a renamed stage would otherwise
    /// silently drop out of the profile.
    #[test]
    fn an_appearing_or_vanishing_stage_is_reported() {
        let base = profile(10.0, &[("evaluation", 10.0, 100.0)]);
        let renamed = profile(10.0, &[("eval", 10.0, 100.0)]);
        let findings = compare(&renamed, &base, 3.0);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(
            findings
                .iter()
                .any(|f| f.contains("absent from the baseline"))
        );
        assert!(findings.iter().any(|f| f.contains("now absent")));
    }
}
