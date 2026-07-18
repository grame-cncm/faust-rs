//! Release-build compilation-cost retention gate for representative DSPs.
//!
//! The gate measures the complete file-to-C++ path in scalar and checked
//! vector modes. Absolute per-case ceilings catch large regressions while a
//! vector/scalar ratio plus a fixed noise allowance catches vector-only cost
//! growth without treating normal runner jitter as a failure.

use super::*;
use codegen::backends::cpp::CppOptions;
use compiler::{Compiler, ComputeMode, SchedulingStrategy};
use std::hint::black_box;
use std::time::Instant;

const VECTOR_COMPILE_BUDGET_BASELINE: &str = "tests/vector-compile-budget/release-baseline.json";
const VECTOR_COMPILE_BUDGET_SCHEMA: u32 = 1;

#[derive(Debug, Deserialize)]
struct CompileBudgetBaseline {
    schema_version: u32,
    profile: CompileBudgetProfile,
    cases: Vec<CompileBudgetCase>,
}

#[derive(Debug, Deserialize)]
struct CompileBudgetProfile {
    vec_size: u32,
    loop_variant: u8,
    scheduling_strategy: u32,
    max_vector_to_scalar_ratio_milli: u64,
    fixed_noise_margin_ms: u64,
}

#[derive(Debug, Deserialize)]
struct CompileBudgetCase {
    name: String,
    path: String,
    scalar_max_ms: u64,
    vector_max_ms: u64,
}

pub(crate) fn vector_compile_budget_check(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err(
            "vector-compile-budget-check must run with `cargo run --release -p xtask -- vector-compile-budget-check`"
                .into(),
        );
    }
    let mut baseline_path = workspace_root().join(VECTOR_COMPILE_BUDGET_BASELINE);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--baseline" => baseline_path = PathBuf::from(required_arg(&mut args, "--baseline")?),
            other => {
                return Err(format!("unknown vector-compile-budget-check option: {other}").into());
            }
        }
    }
    let baseline: CompileBudgetBaseline =
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

    for case in &baseline.cases {
        let path = workspace_root().join(&case.path);
        let scalar_ms = measure_compile(
            &path,
            ComputeMode::Scalar,
            baseline.profile.scheduling_strategy,
        )?;
        let vector_ms = measure_compile(
            &path,
            ComputeMode::Vector {
                vec_size: baseline.profile.vec_size,
                loop_variant: baseline.profile.loop_variant,
            },
            baseline.profile.scheduling_strategy,
        )?;
        check_case_budget(case, &baseline.profile, scalar_ms, vector_ms)?;
        println!(
            "vector compile budget {:>18}: scalar={scalar_ms:>6} ms vector={vector_ms:>6} ms",
            case.name
        );
    }
    println!(
        "vector-compile-budget-check: OK ({} release cases, scalar + vector)",
        baseline.cases.len()
    );
    Ok(())
}

fn required_arg(
    args: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value").into())
}

fn validate_baseline(baseline: &CompileBudgetBaseline) -> Result<(), Box<dyn std::error::Error>> {
    if baseline.schema_version != VECTOR_COMPILE_BUDGET_SCHEMA {
        return Err(format!(
            "unsupported vector compile budget schema {}, expected {}",
            baseline.schema_version, VECTOR_COMPILE_BUDGET_SCHEMA
        )
        .into());
    }
    if baseline.profile.vec_size == 0
        || baseline.profile.loop_variant > 1
        || baseline.profile.max_vector_to_scalar_ratio_milli == 0
    {
        return Err("invalid vector compile budget profile".into());
    }
    let expected = [
        "APF",
        "karplus",
        "cubic_distortion",
        "spectral_level",
        "reverb_designer",
    ];
    let actual = baseline
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(format!(
            "compile budget basket mismatch: expected {expected:?}, found {actual:?}"
        )
        .into());
    }
    for case in &baseline.cases {
        if case.scalar_max_ms == 0
            || case.vector_max_ms == 0
            || !workspace_root().join(&case.path).is_file()
        {
            return Err(format!("invalid compile budget case {}", case.name).into());
        }
    }
    Ok(())
}

fn measure_compile(
    path: &Path,
    compute_mode: ComputeMode,
    scheduling_strategy: u32,
) -> Result<u64, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let bytes = compile_cpp(path, compute_mode, scheduling_strategy)?;
    black_box(bytes);
    Ok(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
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

    fn profile() -> CompileBudgetProfile {
        CompileBudgetProfile {
            vec_size: 32,
            loop_variant: 0,
            scheduling_strategy: 0,
            max_vector_to_scalar_ratio_milli: 2000,
            fixed_noise_margin_ms: 100,
        }
    }

    fn case() -> CompileBudgetCase {
        CompileBudgetCase {
            name: "fixture".to_owned(),
            path: "unused".to_owned(),
            scalar_max_ms: 1000,
            vector_max_ms: 2000,
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
}
