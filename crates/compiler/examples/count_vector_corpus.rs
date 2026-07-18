//! Count impulse-test DSPs whose requested vector pipeline remains certified.
//!
//! Usage:
//! `cargo run -p compiler --example count_vector_corpus -- [lv] [ss] [--precision=f32|f64] [--json] [--filter=TEXT] [--compare-scalar-time]`
//!
//! The default is `-vec -lv 0 -ss 0`. A DSP is counted as vector-capable only
//! when the FIR result reports `VectorPipelineStatus::Certified`; a generated
//! artifact alone is not sufficient because unsupported vector shapes may use
//! the scalar fallback path. `--json` emits a machine-readable report with the
//! effective mode and complete first-failure detail for every DSP.

use std::collections::BTreeMap;
use std::path::PathBuf;

use compiler::{
    Compiler, ComputeMode, RealType, SchedulingStrategy, SignalFirLane, VectorPipelineStatus,
};

fn parse_precision(args: &[String]) -> RealType {
    let Some(value) = args.iter().find_map(|arg| arg.strip_prefix("--precision=")) else {
        return RealType::Float64;
    };
    match value {
        "f32" | "float" | "single" => RealType::Float32,
        "f64" | "double" => RealType::Float64,
        _ => {
            eprintln!("invalid precision: {value} (expected f32 or f64)");
            std::process::exit(2);
        }
    }
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

fn parse_arg(args: &[String], index: usize, default: u8, name: &str) -> u8 {
    args.get(index)
        .map_or(Ok(default), |value| {
            value
                .parse::<u8>()
                .map_err(|_| format!("invalid {name}: {value}"))
        })
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2);
        })
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let positional = std::iter::once(args[0].clone())
        .chain(
            args.iter()
                .skip(1)
                .filter(|arg| !arg.starts_with("--"))
                .cloned(),
        )
        .collect::<Vec<_>>();
    let json = args.iter().any(|arg| arg == "--json");
    let compare_scalar_time = args.iter().any(|arg| arg == "--compare-scalar-time");
    let filter = args.iter().find_map(|arg| arg.strip_prefix("--filter="));
    let real_type = parse_precision(&args);
    let loop_variant = parse_arg(&positional, 1, 0, "loop variant");
    let strategy = parse_arg(&positional, 2, 0, "scheduling strategy");
    if loop_variant > 1 || strategy > 3 {
        eprintln!("loop variant must be 0..1 and scheduling strategy must be 0..3");
        std::process::exit(2);
    }
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
