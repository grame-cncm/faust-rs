//! Count impulse-test DSPs whose requested vector pipeline remains certified.
//!
//! Usage:
//! `cargo run -p compiler --example count_vector_corpus -- [lv] [ss] [--precision=f32|f64] [--json] [--filter=TEXT] [--shard=INDEX/COUNT] [--compare-scalar-time]`
//!
//! The default is `-vec -lv 0 -ss 0`. A DSP is counted as vector-capable only
//! when the FIR result reports `VectorPipelineStatus::Certified`; a generated
//! artifact alone is not sufficient because unsupported vector shapes may use
//! the scalar fallback path. `--json` emits a machine-readable report with the
//! effective mode and complete first-failure detail for every DSP.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use compiler::{
    Compiler, ComputeMode, RealType, SchedulingStrategy, SignalFirLane, VectorPipelineStatus,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Precision {
    #[value(aliases = ["float", "single"])]
    F32,
    #[value(aliases = ["double"])]
    F64,
}

impl From<Precision> for RealType {
    fn from(value: Precision) -> Self {
        match value {
            Precision::F32 => Self::Float32,
            Precision::F64 => Self::Float64,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "count_vector_corpus",
    about = "Measure checked-vector coverage over the impulse-test corpus"
)]
struct CliArgs {
    /// Vector loop variant.
    #[arg(default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
    loop_variant: u8,

    /// Scheduling strategy selector.
    #[arg(default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=3))]
    scheduling_strategy: u8,

    /// Floating-point precision.
    #[arg(long, value_enum, default_value = "f64")]
    precision: Precision,

    /// Emit the machine-readable coverage report.
    #[arg(long)]
    json: bool,

    /// Retain corpus paths containing this substring.
    #[arg(long)]
    filter: Option<String>,

    /// Process one INDEX/COUNT shard of the sorted corpus.
    #[arg(long, value_parser = parse_shard)]
    shard: Option<(usize, usize)>,

    /// Also measure a scalar compile for each selected DSP.
    #[arg(long)]
    compare_scalar_time: bool,
}

fn precision_name(real_type: RealType) -> &'static str {
    match real_type {
        RealType::Float32 => "f32",
        RealType::Float64 => "f64",
    }
}

fn portable_path(path: &std::path::Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn parse_shard(value: &str) -> Result<(usize, usize), String> {
    let (index, count) = value
        .split_once('/')
        .ok_or_else(|| format!("invalid shard: {value} (expected INDEX/COUNT)"))?;
    let index = index
        .parse::<usize>()
        .map_err(|_| format!("invalid shard index: {index}"))?;
    let count = count
        .parse::<usize>()
        .map_err(|_| format!("invalid shard count: {count}"))?;
    if count == 0 || index >= count {
        return Err(format!(
            "invalid shard: {value} (COUNT must be positive and INDEX < COUNT)"
        ));
    }
    Ok((index, count))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{CliArgs, parse_shard};

    #[test]
    fn shard_argument_is_checked() {
        assert_eq!(parse_shard("2/4"), Ok((2, 4)));
        assert!(parse_shard("4/4").is_err());
        assert!(parse_shard("0/0").is_err());
        assert!(parse_shard("bad").is_err());
    }

    #[test]
    fn clap_preserves_positional_and_equals_forms() {
        CliArgs::command().debug_assert();
        let args = CliArgs::try_parse_from([
            "count_vector_corpus",
            "1",
            "3",
            "--precision=f32",
            "--filter=delay",
            "--shard=2/4",
            "--json",
        ])
        .unwrap();
        assert_eq!(args.loop_variant, 1);
        assert_eq!(args.scheduling_strategy, 3);
        assert_eq!(args.shard, Some((2, 4)));
    }
}

fn main() {
    let args = CliArgs::parse();
    std::thread::Builder::new()
        .name("vector-corpus-scan".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run(args))
        .expect("spawn vector corpus scan thread")
        .join()
        .expect("vector corpus scan thread");
}

fn run(args: CliArgs) {
    let json = args.json;
    let compare_scalar_time = args.compare_scalar_time;
    let filter = args.filter.as_deref();
    let shard = args.shard;
    let real_type = args.precision.into();
    let loop_variant = args.loop_variant;
    let strategy = args.scheduling_strategy;
    let root = PathBuf::from("tests/impulse-tests/dsp");

    let mut files = std::fs::read_dir(&root)
        .expect("read impulse DSP directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dsp"))
        .collect::<Vec<_>>();
    files.sort();
    if let Some(filter) = filter {
        files.retain(|path| path.display().to_string().contains(filter));
    }
    if let Some((index, count)) = shard {
        files = files
            .into_iter()
            .enumerate()
            .filter_map(|(position, path)| (position % count == index).then_some(path))
            .collect();
    }

    let mut certified = Vec::new();
    let mut fallback = Vec::new();
    let mut errors = Vec::new();

    let file_count = files.len();
    for (index, path) in files.into_iter().enumerate() {
        eprintln!("[{}/{}] {}", index + 1, file_count, path.display());
        let scalar_elapsed = compare_scalar_time.then(|| {
            let started = std::time::Instant::now();
            let result = Compiler::new()
                .with_real_type(real_type)
                .with_compute_mode(ComputeMode::Scalar)
                .compile_file_to_fir_with_lane(
                    &path,
                    &[root.clone(), PathBuf::from("/usr/local/share/faust")],
                    SignalFirLane::TransformFastLane,
                );
            let elapsed = started.elapsed();
            match result {
                Ok(_) => eprintln!("  scalar baseline in {:.3}s", elapsed.as_secs_f64()),
                Err(error) => eprintln!(
                    "  scalar baseline failed in {:.3}s: {error}",
                    elapsed.as_secs_f64()
                ),
            }
            elapsed
        });
        let compiler = Compiler::new()
            .with_real_type(real_type)
            .with_compute_mode(ComputeMode::Vector {
                vec_size: ComputeMode::DEFAULT_VEC_SIZE,
                loop_variant,
            })
            .with_scheduling_strategy(match strategy {
                0 => SchedulingStrategy::DepthFirst,
                1 => SchedulingStrategy::BreadthFirst,
                2 => SchedulingStrategy::Special,
                _ => SchedulingStrategy::ReverseBreadthFirst,
            });

        let started = std::time::Instant::now();
        match compiler.compile_file_to_fir_with_lane(
            &path,
            &[root.clone(), PathBuf::from("/usr/local/share/faust")],
            SignalFirLane::TransformFastLane,
        ) {
            Ok(output) => match output.vector_pipeline_status {
                VectorPipelineStatus::Certified => certified.push(path),
                status => fallback.push((
                    path,
                    status,
                    output.vector_effective_mode,
                    output.vector_pipeline_detail,
                )),
            },
            Err(error) => errors.push((path, error.to_string())),
        }
        let vector_elapsed = started.elapsed();
        if let Some(scalar_elapsed) = scalar_elapsed {
            eprintln!(
                "  vector request in {:.3}s ({:+.3}s, {:.2}x scalar)",
                vector_elapsed.as_secs_f64(),
                vector_elapsed.as_secs_f64() - scalar_elapsed.as_secs_f64(),
                vector_elapsed.as_secs_f64() / scalar_elapsed.as_secs_f64()
            );
        } else {
            eprintln!("  completed in {:.3}s", vector_elapsed.as_secs_f64());
        }
    }

    let reason_counts = fallback.iter().fold(BTreeMap::new(), |mut counts, entry| {
        let code = match entry.1 {
            VectorPipelineStatus::Fallback(reason) => reason.code(),
            VectorPipelineStatus::NotRequested => "FRS-VEC-NOT-REQUESTED",
            VectorPipelineStatus::Certified => "FRS-VEC-CERTIFIED",
        };
        *counts.entry(code).or_insert(0_usize) += 1;
        counts
    });

    if json {
        let report = serde_json::json!({
            "mode": {
                "vector": true,
                "precision": precision_name(real_type),
                "loop_variant": loop_variant,
                "scheduling_strategy": strategy
            },
            "summary": {
                "total": certified.len() + fallback.len() + errors.len(),
                "certified": certified.len(),
                "fallback": fallback.len(),
                "error": errors.len(),
                "fallback_by_reason": reason_counts,
            },
            "certified_files": certified.iter().map(|path| portable_path(path)).collect::<Vec<_>>(),
            "fallback_files": fallback.iter().map(|(path, status, effective, detail)| serde_json::json!({
                "path": portable_path(path),
                "status": format!("{status:?}"),
                "reason_code": match status {
                    VectorPipelineStatus::Fallback(reason) => reason.code(),
                    VectorPipelineStatus::NotRequested => "FRS-VEC-NOT-REQUESTED",
                    VectorPipelineStatus::Certified => "FRS-VEC-CERTIFIED",
                },
                "effective_mode": format!("{effective:?}"),
                "detail": detail,
            })).collect::<Vec<_>>(),
            "error_files": errors.iter().map(|(path, error)| serde_json::json!({
                "path": portable_path(path),
                "error": error,
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("serialize report")
        );
    } else {
        println!(
            "MODE --precision={} -vec -lv {loop_variant} -ss {strategy}",
            precision_name(real_type)
        );
        println!("TOTAL {}", certified.len() + fallback.len() + errors.len());
        println!("CERTIFIED {}", certified.len());
        println!("FALLBACK {}", fallback.len());
        println!("ERROR {}", errors.len());
        println!("FALLBACK_BY_REASON");
        for (reason, count) in reason_counts {
            println!("{reason} {count}");
        }
        println!("CERTIFIED_FILES");
        for path in certified {
            println!("{}", path.display());
        }
        println!("FALLBACK_FILES");
        for (path, status, effective, detail) in fallback {
            println!(
                "{}: {status:?} effective={effective:?} detail={}",
                path.display(),
                detail.as_deref().unwrap_or("-")
            );
        }
        println!("ERROR_FILES");
        for (path, error) in errors {
            println!("{}: {error}", path.display());
        }
    }
}
