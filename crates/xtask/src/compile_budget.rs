//! Release-build compilation-cost retention gate for representative DSPs.
//!
//! Named `compile-budget-check` rather than `vector-...`: it covers the scalar
//! front end and both scalar and vector codegen, so scoping the name to vector
//! mode misdescribed what it gates.
//!
//! The gate has two independent dimensions.
//!
//! **Codegen basket** (`codegen_cases`) measures the complete file-to-C++ path
//! in scalar and checked vector modes. Absolute per-case ceilings catch large
//! regressions while a vector/scalar ratio plus a fixed noise allowance catches
//! vector-only cost growth without treating normal runner jitter as a failure.
//!
//! **Front-end basket** (`frontend_cases`) measures the `--check` path only
//! (`parse -> eval -> propagate -> type -> FIR verify`, no codegen). Absolute
//! wall-clock ceilings are useless here: they must be loosened until they
//! survive the slowest CI runner, at which point they no longer catch a 2x
//! regression -- exactly how the 2026-07-30 provenance blow-up reached `main`
//! through a green `scalar_max_ms: 45000` ceiling. This basket therefore
//! normalizes every measurement against a calibration DSP measured in the same
//! process, and enforces a tight tolerance on the resulting dimensionless
//! *units*. Machine speed cancels out; a real algorithmic regression does not.
//!
//! See `porting/compile-time-provenance-regression-analysis-and-plan-2026-07-30-en.md`.

use super::*;
use codegen::backends::cpp::CppOptions;
use compiler::{Compiler, ComputeMode, SchedulingStrategy, SignalFirLane};
use std::hint::black_box;
use std::time::Instant;

const COMPILE_BUDGET_BASELINE: &str = "tests/compile-budget/release-baseline.json";
const COMPILE_BUDGET_SCHEMA: u32 = 2;

/// Basket entries that must never silently disappear from the codegen budget.
const REQUIRED_CODEGEN_CASES: [&str; 5] = [
    "APF",
    "karplus",
    "cubic_distortion",
    "spectral_level",
    "reverb_designer",
];

/// Basket entries that must never silently disappear from the front-end budget.
///
/// Every name here regressed measurably in the diagnostics-provenance arc, so
/// dropping one would reopen a known hole rather than merely reduce coverage.
/// `ja_smoothed_parameter` is the separate propagation-sharing sentinel from
/// `ja-smoothed-parameter-propagation-analysis-and-optimization-plan-2026-08-08-en.md`;
/// it is required for the same reason: aggregate corpora hid its stage shape.
const REQUIRED_FRONTEND_CASES: [&str; 7] = [
    "bells",
    "ja_smoothed_parameter",
    "parametric_eq",
    "reverb_designer",
    "spectral_level",
    "vcf_wah_pedals",
    "virtual_analog_oscillators",
];

#[derive(Debug, Deserialize, Serialize)]
struct CompileBudgetBaseline {
    schema_version: u32,
    profile: CompileBudgetProfile,
    codegen_cases: Vec<CompileBudgetCase>,
    frontend_cases: Vec<FrontendBudgetCase>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CompileBudgetProfile {
    vec_size: u32,
    loop_variant: u8,
    scheduling_strategy: u32,
    max_vector_to_scalar_ratio_milli: u64,
    fixed_noise_margin_ms: u64,
    /// DSP whose front-end cost defines one *unit*, cancelling machine speed.
    ///
    /// It must be cheap enough to keep the gate fast and structurally unrelated
    /// to what the front-end basket is watching, so that a regression shows up
    /// as a larger ratio instead of inflating the divisor too.
    calibration_path: String,
    /// Timed runs per measurement; the minimum is retained.
    ///
    /// The minimum is the robust estimator for "this run met no interference":
    /// scheduler noise, page faults, and neighbouring CI jobs can only add time.
    repeats: u32,
    /// Timed runs per codegen measurement; the minimum is retained.
    ///
    /// Separate from `repeats` because the two baskets have very different cost
    /// profiles: a codegen pass is seconds where a front-end pass is
    /// milliseconds. Recording a codegen baseline from a single run let a fast
    /// outlier become the reference and made the gate flaky.
    codegen_repeats: u32,
    /// Timed runs for the calibration DSP specifically.
    ///
    /// The calibration divides every other measurement, so its noise is the
    /// noise floor of the whole basket. It is also two orders of magnitude
    /// cheaper than the cases, which makes extra repeats essentially free.
    calibration_repeats: u32,
    /// Permitted growth of a front-end case, in percent of its baseline units.
    frontend_tolerance_percent: u64,
    /// Refuse to normalize against a calibration measurement below this floor.
    ///
    /// Guards the ratio against timer granularity on a very fast machine.
    min_calibration_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CompileBudgetCase {
    name: String,
    path: String,
    /// Coarse absolute backstop, kept for a catastrophic blow-up.
    ///
    /// These have to be loose enough for the slowest runner, so they are not
    /// the real gate -- `scalar_units_milli` is. Retained because a case that
    /// somehow escapes normalization (a calibration that moved with it) should
    /// still hit a wall.
    scalar_max_ms: u64,
    vector_max_ms: u64,
    /// Scalar file-to-C++ cost in calibration units, times 1000.
    #[serde(default)]
    scalar_units_milli: u64,
    /// Vector file-to-C++ cost in calibration units, times 1000.
    #[serde(default)]
    vector_units_milli: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct FrontendBudgetCase {
    name: String,
    path: String,
    /// Front-end cost in calibration units, times 1000.
    ///
    /// Stored as an integer so the versioned baseline stays byte-stable across
    /// platforms and float formatters.
    units_milli: u64,
    /// Why this entry currently sits above where it should, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    /// Set to `false` for a case whose current cost makes the gate unusable.
    ///
    /// A disabled case is still parsed and reported, so the debt stays visible
    /// instead of being deleted and forgotten.
    #[serde(default = "default_true")]
    enabled: bool,
}

const fn default_true() -> bool {
    true
}

/// One measured front-end case.
struct FrontendMeasurement {
    name: String,
    /// Cost in calibration units, times 1000.
    units_milli: u64,
}

/// One measured codegen case, in both compute modes.
struct CodegenMeasurement {
    name: String,
    /// Scalar cost in calibration units, times 1000.
    scalar_units_milli: u64,
    /// Vector cost in calibration units, times 1000.
    vector_units_milli: u64,
}

pub(crate) fn compile_budget_check(
    args: CompileBudgetArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err(
            "compile-budget-check must run with `cargo run --release -p xtask -- compile-budget-check`"
                .into(),
        );
    }
    let baseline_path = args
        .baseline
        .unwrap_or_else(|| workspace_root().join(COMPILE_BUDGET_BASELINE));
    let mut baseline: CompileBudgetBaseline =
        serde_json::from_str(&fs::read_to_string(&baseline_path)?)?;
    validate_baseline(&baseline)?;

    // Warm parser, import, lowering, and backend code paths before recording
    // the fixed basket. The warm-up is deliberately outside all budgets.
    let warmup = workspace_root().join("tests/corpus/rep_01_passthrough.dsp");
    compile_cpp(
        &warmup,
        ComputeMode::Scalar,
        baseline.profile.scheduling_strategy,
    )?;
    compile_cpp(
        &warmup,
        ComputeMode::Vector {
            vec_size: baseline.profile.vec_size,
            loop_variant: baseline.profile.loop_variant,
        },
        baseline.profile.scheduling_strategy,
    )?;

    // The front-end basket runs first, and identically in both modes. Its
    // calibration divides every recorded unit, so `--update` and a later
    // enforcing run must measure it under the same machine conditions --
    // measuring it after the codegen basket in one mode and before it in the
    // other moved the calibration by 44% and shifted every ratio with it.
    let (calibration_ms, measured) = measure_frontend_basket(&baseline)?;

    let codegen = measure_codegen_basket(&baseline, calibration_ms)?;
    if args.update {
        return write_updated_baseline(&baseline_path, &mut baseline, &measured, &codegen);
    }
    check_frontend_basket(&baseline, &measured)?;
    check_codegen_basket(&baseline, &codegen)?;

    println!(
        "compile-budget-check: OK ({} codegen cases scalar + vector, {} front-end cases normalized)",
        baseline.codegen_cases.len(),
        measured.len()
    );
    Ok(())
}

fn validate_baseline(baseline: &CompileBudgetBaseline) -> Result<(), Box<dyn std::error::Error>> {
    if baseline.schema_version != COMPILE_BUDGET_SCHEMA {
        return Err(format!(
            "unsupported vector compile budget schema {}, expected {}",
            baseline.schema_version, COMPILE_BUDGET_SCHEMA
        )
        .into());
    }
    if baseline.profile.vec_size == 0
        || baseline.profile.loop_variant > 1
        || baseline.profile.max_vector_to_scalar_ratio_milli == 0
        || baseline.profile.repeats == 0
        || baseline.profile.frontend_tolerance_percent == 0
        || baseline.profile.min_calibration_ms == 0
    {
        return Err("invalid vector compile budget profile".into());
    }
    if !workspace_root()
        .join(&baseline.profile.calibration_path)
        .is_file()
    {
        return Err(format!(
            "calibration DSP {} is missing",
            baseline.profile.calibration_path
        )
        .into());
    }
    require_cases(
        "codegen",
        &REQUIRED_CODEGEN_CASES,
        baseline.codegen_cases.iter().map(|case| case.name.as_str()),
    )?;
    require_cases(
        "front-end",
        &REQUIRED_FRONTEND_CASES,
        baseline
            .frontend_cases
            .iter()
            .map(|case| case.name.as_str()),
    )?;
    for case in &baseline.codegen_cases {
        if case.scalar_max_ms == 0
            || case.vector_max_ms == 0
            || !workspace_root().join(&case.path).is_file()
        {
            return Err(format!("invalid compile budget case {}", case.name).into());
        }
    }
    for case in &baseline.frontend_cases {
        if case.units_milli == 0 || !workspace_root().join(&case.path).is_file() {
            return Err(format!("invalid front-end budget case {}", case.name).into());
        }
    }
    Ok(())
}

/// Rejects a basket that lost a required entry.
///
/// Extra entries are welcome; a missing one silently reopens a known hole, so
/// it is an error rather than a warning.
fn require_cases<'a>(
    basket: &str,
    required: &[&str],
    actual: impl Iterator<Item = &'a str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let present = actual.collect::<BTreeSet<_>>();
    let missing = required
        .iter()
        .filter(|name| !present.contains(*name))
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!("{basket} compile budget basket is missing {missing:?}").into())
}

/// Measures every enabled front-end case plus the calibration DSP.
///
/// Calibration is re-measured in the same process and immediately before the
/// basket so that CPU frequency state, allocator warmth, and runner load are as
/// close as possible to the conditions of the measurements it normalizes.
fn measure_frontend_basket(
    baseline: &CompileBudgetBaseline,
) -> Result<(u64, Vec<FrontendMeasurement>), Box<dyn std::error::Error>> {
    let calibration_path = workspace_root().join(&baseline.profile.calibration_path);
    let calibration_ms = measure_frontend_calibration(
        &calibration_path,
        baseline.profile.calibration_repeats.max(1),
        baseline.profile.min_calibration_ms,
    )?;
    if calibration_ms < baseline.profile.min_calibration_ms {
        return Err(format!(
            "calibration {} took {calibration_ms} ms, below the {} ms floor needed for a meaningful ratio",
            baseline.profile.calibration_path, baseline.profile.min_calibration_ms
        )
        .into());
    }
    println!("front-end calibration {calibration_ms:>6} ms = 1.000 units");

    let mut measured = Vec::new();
    for case in &baseline.frontend_cases {
        if !case.enabled {
            println!(
                "front-end budget {:>26}: SKIPPED ({})",
                case.name,
                case.note.as_deref().unwrap_or("disabled in baseline")
            );
            continue;
        }
        let path = workspace_root().join(&case.path);
        let case_ms = measure_frontend(&path, baseline.profile.repeats)?;
        let units_milli = case_ms.saturating_mul(1000) / calibration_ms;
        measured.push(FrontendMeasurement {
            name: case.name.clone(),
            units_milli,
        });
    }
    Ok((calibration_ms, measured))
}

/// Measures the codegen basket, normalized against the same calibration.
///
/// Returns `(name, scalar_units_milli, vector_units_milli)` per case.
fn measure_codegen_basket(
    baseline: &CompileBudgetBaseline,
    calibration_ms: u64,
) -> Result<Vec<CodegenMeasurement>, Box<dyn std::error::Error>> {
    let mut measured = Vec::new();
    for case in &baseline.codegen_cases {
        let path = workspace_root().join(&case.path);
        let scalar_ms = measure_compile(
            &path,
            ComputeMode::Scalar,
            baseline.profile.scheduling_strategy,
            baseline.profile.codegen_repeats,
        )?;
        let vector_ms = measure_compile(
            &path,
            ComputeMode::Vector {
                vec_size: baseline.profile.vec_size,
                loop_variant: baseline.profile.loop_variant,
            },
            baseline.profile.scheduling_strategy,
            baseline.profile.codegen_repeats,
        )?;
        check_case_budget(case, &baseline.profile, scalar_ms, vector_ms)?;
        println!(
            "codegen budget {:>26}: scalar={scalar_ms:>6} ms vector={vector_ms:>6} ms",
            case.name
        );
        measured.push(CodegenMeasurement {
            name: case.name.clone(),
            scalar_units_milli: scalar_ms.saturating_mul(1000) / calibration_ms,
            vector_units_milli: vector_ms.saturating_mul(1000) / calibration_ms,
        });
    }
    Ok(measured)
}

/// Enforces the normalized codegen budget.
///
/// The absolute ceilings in [`check_case_budget`] have to survive the slowest
/// runner, which currently leaves 4.7x to 638x of headroom -- the same width
/// that let the 2026-07-30 front-end regression through a green CI. These
/// normalized ceilings are the real gate on the codegen path.
fn check_codegen_basket(
    baseline: &CompileBudgetBaseline,
    measured: &[CodegenMeasurement],
) -> Result<(), Box<dyn std::error::Error>> {
    for CodegenMeasurement {
        name,
        scalar_units_milli: scalar_units,
        vector_units_milli: vector_units,
    } in measured
    {
        let case = baseline
            .codegen_cases
            .iter()
            .find(|case| &case.name == name)
            .ok_or_else(|| format!("measured unknown codegen case {name}"))?;
        for (mode, measured_units, baseline_units) in [
            ("scalar", *scalar_units, case.scalar_units_milli),
            ("vector", *vector_units, case.vector_units_milli),
        ] {
            if baseline_units == 0 {
                return Err(format!(
                    "codegen case {name} has no recorded {mode} units; regenerate the baseline \
                     with `cargo run --release -p xtask -- compile-budget-check --update`"
                )
                .into());
            }
            let ceiling = frontend_ceiling_milli(baseline_units, &baseline.profile);
            println!(
                "codegen budget {:>26}: {mode} {:.3} units (baseline {:.3}, ceiling {:.3})",
                case.name,
                measured_units as f64 / 1000.0,
                baseline_units as f64 / 1000.0,
                ceiling as f64 / 1000.0,
            );
            if measured_units > ceiling {
                return Err(format!(
                    "{name} {mode} codegen cost is {:.3} calibration units; baseline is {:.3} \
                     and the {}% tolerance permits {:.3}.",
                    measured_units as f64 / 1000.0,
                    baseline_units as f64 / 1000.0,
                    baseline.profile.frontend_tolerance_percent,
                    ceiling as f64 / 1000.0,
                )
                .into());
            }
        }
    }
    Ok(())
}

fn check_frontend_basket(
    baseline: &CompileBudgetBaseline,
    measured: &[FrontendMeasurement],
) -> Result<(), Box<dyn std::error::Error>> {
    for FrontendMeasurement { name, units_milli } in measured {
        let case = baseline
            .frontend_cases
            .iter()
            .find(|case| &case.name == name)
            .ok_or_else(|| format!("measured unknown front-end case {name}"))?;
        let ceiling = frontend_ceiling_milli(case.units_milli, &baseline.profile);
        println!(
            "front-end budget {:>26}: {:.3} units (baseline {:.3}, ceiling {:.3})",
            case.name,
            *units_milli as f64 / 1000.0,
            case.units_milli as f64 / 1000.0,
            ceiling as f64 / 1000.0,
        );
        if *units_milli > ceiling {
            return Err(format!(
                "{name} front-end cost is {:.3} calibration units; baseline is {:.3} and the \
                 {}% tolerance permits {:.3}. Either the change made compilation slower, or the \
                 baseline needs an explicit, justified update via \
                 `cargo run --release -p xtask -- compile-budget-check --update`.",
                *units_milli as f64 / 1000.0,
                case.units_milli as f64 / 1000.0,
                baseline.profile.frontend_tolerance_percent,
                ceiling as f64 / 1000.0,
            )
            .into());
        }
    }
    Ok(())
}

fn frontend_ceiling_milli(baseline_units_milli: u64, profile: &CompileBudgetProfile) -> u64 {
    baseline_units_milli
        .saturating_mul(100 + profile.frontend_tolerance_percent)
        .saturating_div(100)
}

/// Rewrites the versioned baseline with freshly measured front-end units.
///
/// Deliberately a separate, explicit invocation: an automatic refresh would
/// turn the gate into a recorder that ratifies whatever the last commit did.
fn write_updated_baseline(
    baseline_path: &Path,
    baseline: &mut CompileBudgetBaseline,
    measured: &[FrontendMeasurement],
    codegen: &[CodegenMeasurement],
) -> Result<(), Box<dyn std::error::Error>> {
    for FrontendMeasurement { name, units_milli } in measured {
        let Some(case) = baseline
            .frontend_cases
            .iter_mut()
            .find(|case| &case.name == name)
        else {
            continue;
        };
        let previous = case.units_milli;
        case.units_milli = *units_milli;
        let direction = if *units_milli > previous {
            "SLOWER"
        } else {
            "faster"
        };
        println!(
            "front-end baseline {:>26}: {:.3} -> {:.3} units ({direction})",
            case.name,
            previous as f64 / 1000.0,
            *units_milli as f64 / 1000.0,
        );
    }
    for CodegenMeasurement {
        name,
        scalar_units_milli: scalar_units,
        vector_units_milli: vector_units,
    } in codegen
    {
        let Some(case) = baseline
            .codegen_cases
            .iter_mut()
            .find(|case| &case.name == name)
        else {
            continue;
        };
        println!(
            "codegen baseline {:>26}: scalar {:.3} -> {:.3}, vector {:.3} -> {:.3} units",
            case.name,
            case.scalar_units_milli as f64 / 1000.0,
            *scalar_units as f64 / 1000.0,
            case.vector_units_milli as f64 / 1000.0,
            *vector_units as f64 / 1000.0,
        );
        case.scalar_units_milli = *scalar_units;
        case.vector_units_milli = *vector_units;
    }
    let mut json = serde_json::to_string_pretty(baseline)?;
    json.push('\n');
    fs::write(baseline_path, json)?;
    println!(
        "compile-budget-check: baseline updated at {}",
        baseline_path.display()
    );
    println!("review the diff and justify every increase in the commit message");
    Ok(())
}

/// Returns the fastest of `repeats` timed codegen runs, in milliseconds.
fn measure_compile(
    path: &Path,
    compute_mode: ComputeMode,
    scheduling_strategy: u32,
    repeats: u32,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut best = u64::MAX;
    for _ in 0..repeats.max(1) {
        let started = Instant::now();
        let bytes = compile_cpp(path, compute_mode, scheduling_strategy)?;
        black_box(bytes);
        best = best.min(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    }
    Ok(best)
}

/// Returns the fastest of `repeats` timed front-end runs, in milliseconds.
fn measure_frontend(path: &Path, repeats: u32) -> Result<u64, Box<dyn std::error::Error>> {
    let mut best = u64::MAX;
    for _ in 0..repeats {
        let started = Instant::now();
        let nodes = compile_frontend(path)?;
        black_box(nodes);
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        best = best.min(elapsed);
    }
    Ok(best)
}

/// Measures a cheap calibration DSP in batches, avoiding whole-millisecond
/// timer quantization without weakening the configured per-run floor.
fn measure_frontend_calibration(
    path: &Path,
    repeats: u32,
    batch_runs: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut best_micros = u128::MAX;
    let batch_runs = batch_runs.max(1);
    for _ in 0..repeats {
        let started = Instant::now();
        for _ in 0..batch_runs {
            black_box(compile_frontend(path)?);
        }
        best_micros = best_micros.min(started.elapsed().as_micros());
    }
    let per_run_micros = best_micros.div_ceil(u128::from(batch_runs));
    Ok(u64::try_from(per_run_micros.div_ceil(1_000)).unwrap_or(u64::MAX))
}

fn compile_cpp(
    path: &Path,
    compute_mode: ComputeMode,
    scheduling_strategy: u32,
) -> Result<usize, Box<dyn std::error::Error>> {
    let output = Compiler::new()
        .with_compute_mode(compute_mode)
        .with_scheduling_strategy(SchedulingStrategy::decode(scheduling_strategy))
        .compile_file_default_to_cpp(path, &CppOptions::default())?;
    Ok(output.len())
}

/// Runs exactly what `faust-rs --check` runs: the full front end plus FIR
/// verification, and no backend.
fn compile_frontend(path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let output = Compiler::new()
        .compile_file_default_to_fir_with_lane(path, SignalFirLane::TransformFastLane)?;
    Ok(output.store.len())
}

fn check_case_budget(
    case: &CompileBudgetCase,
    profile: &CompileBudgetProfile,
    scalar_ms: u64,
    vector_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    if scalar_ms > case.scalar_max_ms {
        return Err(format!(
            "{} scalar compile took {scalar_ms} ms, ceiling is {} ms",
            case.name, case.scalar_max_ms
        )
        .into());
    }
    if vector_ms > case.vector_max_ms {
        return Err(format!(
            "{} vector compile took {vector_ms} ms, ceiling is {} ms",
            case.name, case.vector_max_ms
        )
        .into());
    }
    let ratio_budget = scalar_ms
        .saturating_mul(profile.max_vector_to_scalar_ratio_milli)
        .saturating_div(1000)
        .saturating_add(profile.fixed_noise_margin_ms);
    if vector_ms > ratio_budget {
        return Err(format!(
            "{} vector compile took {vector_ms} ms; scalar {scalar_ms} ms permits {ratio_budget} ms including noise margin",
            case.name
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fe(name: &str, units_milli: u64) -> FrontendMeasurement {
        FrontendMeasurement {
            name: name.to_owned(),
            units_milli,
        }
    }

    fn cg(name: &str, scalar_units_milli: u64, vector_units_milli: u64) -> CodegenMeasurement {
        CodegenMeasurement {
            name: name.to_owned(),
            scalar_units_milli,
            vector_units_milli,
        }
    }

    fn profile() -> CompileBudgetProfile {
        CompileBudgetProfile {
            vec_size: 32,
            loop_variant: 0,
            scheduling_strategy: 0,
            max_vector_to_scalar_ratio_milli: 2000,
            fixed_noise_margin_ms: 100,
            calibration_path: "tests/impulse-tests/dsp/karplus.dsp".to_owned(),
            repeats: 2,
            codegen_repeats: 2,
            calibration_repeats: 8,
            frontend_tolerance_percent: 30,
            min_calibration_ms: 5,
        }
    }

    fn case() -> CompileBudgetCase {
        CompileBudgetCase {
            name: "fixture".to_owned(),
            path: "unused".to_owned(),
            scalar_max_ms: 1000,
            vector_max_ms: 2000,
            scalar_units_milli: 10_000,
            vector_units_milli: 20_000,
        }
    }

    fn frontend_baseline(units_milli: u64) -> CompileBudgetBaseline {
        CompileBudgetBaseline {
            schema_version: COMPILE_BUDGET_SCHEMA,
            profile: profile(),
            codegen_cases: Vec::new(),
            frontend_cases: vec![FrontendBudgetCase {
                name: "fixture".to_owned(),
                path: "unused".to_owned(),
                units_milli,
                note: None,
                enabled: true,
            }],
        }
    }

    #[test]
    fn budget_accepts_fixed_noise_margin() {
        check_case_budget(&case(), &profile(), 10, 120).unwrap();
    }

    #[test]
    fn budget_rejects_absolute_and_relative_regressions() {
        assert!(check_case_budget(&case(), &profile(), 1001, 100).is_err());
        assert!(check_case_budget(&case(), &profile(), 100, 2001).is_err());
        assert!(check_case_budget(&case(), &profile(), 100, 301).is_err());
    }

    #[test]
    fn frontend_tolerance_accepts_jitter_and_rejects_regressions() {
        let baseline = frontend_baseline(10_000);
        // +29% is runner jitter under a 30% tolerance; +31% is not.
        check_frontend_basket(&baseline, &[fe("fixture", 12_400)]).unwrap();
        check_frontend_basket(&baseline, &[fe("fixture", 12_900)]).unwrap();
        assert!(check_frontend_basket(&baseline, &[fe("fixture", 13_100)]).is_err());
    }

    #[test]
    fn frontend_tolerance_rejects_the_2026_07_30_provenance_regression() {
        // `spectral_level` went from 902 ms to 10 899 ms against a calibration
        // that did not move: any tolerance short of +1100% must reject it.
        let baseline = frontend_baseline(27_400);
        assert!(check_frontend_basket(&baseline, &[fe("fixture", 331_000)]).is_err());
        // The residual left by the PR #15 caps alone (x2.5) must also fail.
        assert!(check_frontend_basket(&baseline, &[fe("fixture", 69_100)]).is_err());
    }

    #[test]
    fn codegen_normalization_rejects_what_the_absolute_ceiling_waves_through() {
        // spectral_level scalar measured 1531 ms against a 20 000 ms ceiling:
        // 13x of headroom. A 2x codegen regression clears the ceiling and must
        // be caught by the normalized budget instead.
        let baseline = CompileBudgetBaseline {
            schema_version: COMPILE_BUDGET_SCHEMA,
            profile: profile(),
            codegen_cases: vec![case()],
            frontend_cases: Vec::new(),
        };
        check_codegen_basket(&baseline, &[cg("fixture", 12_400, 24_000)]).unwrap();
        assert!(check_codegen_basket(&baseline, &[cg("fixture", 20_000, 24_000)]).is_err());
        assert!(check_codegen_basket(&baseline, &[cg("fixture", 12_400, 40_000)]).is_err());
    }

    #[test]
    fn codegen_case_without_recorded_units_is_rejected() {
        let mut only = case();
        only.scalar_units_milli = 0;
        let baseline = CompileBudgetBaseline {
            schema_version: COMPILE_BUDGET_SCHEMA,
            profile: profile(),
            codegen_cases: vec![only],
            frontend_cases: Vec::new(),
        };
        assert!(check_codegen_basket(&baseline, &[cg("fixture", 1, 1)]).is_err());
    }

    #[test]
    fn missing_required_case_is_rejected() {
        require_cases("front-end", &["a", "b"], ["a", "c"].into_iter()).unwrap_err();
        require_cases("front-end", &["a"], ["a", "extra"].into_iter()).unwrap();
    }

    #[test]
    fn ceiling_applies_the_configured_tolerance() {
        assert_eq!(frontend_ceiling_milli(10_000, &profile()), 13_000);
    }
}
