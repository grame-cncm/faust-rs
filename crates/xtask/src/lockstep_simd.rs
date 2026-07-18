//! Native SIMD evidence gate for signal-level lockstep vectorization.
//!
//! Section 8 of the vector-mode port plan deliberately lowers each certified
//! bundle as one C++ sample loop containing unchanged scalar lane expressions.
//! This workflow first requires [`compiler::VectorPipelineStatus::Certified`]
//! and [`compiler::VectorEffectiveMode::CertifiedVector`], then checks the
//! final part of that adapted contract: Clang at
//! `-O3`, without fast-math or FMA contraction, must turn the representative
//! four-lane recursive expressions into LLVM vector floating-point operations.
//! Clang line-table provenance must attribute those operations to the generated
//! physical loop containing checked `vlock_*` state; vector operations in a
//! separate mixed-DSP loop do not count.
//! It complements the FIR certificate and bit-exact interpreter tests; it does
//! not make a target-specific instruction-set assumption and cannot accept
//! SIMD emitted from a scalar fallback module.

use std::ops::RangeInclusive;

use super::*;

const SIMD_CASES: [(&str, usize, usize); 3] = [
    ("tests/corpus/vector_lockstep_simd_quad.dsp", 10, 1),
    ("tests/corpus/vector_lockstep_mixed_reduce.dsp", 10, 2),
    ("tests/corpus/vector_lockstep_mixed_branch.dsp", 10, 2),
];

fn lockstep_source_range(
    cpp: &str,
    expected_physical_loops: usize,
) -> Result<RangeInclusive<u32>, String> {
    let lines = cpp.lines().collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.contains("for (int i0 = vindex;").then_some(index))
        .collect::<Vec<_>>();
    if starts.len() != expected_physical_loops {
        return Err(format!(
            "expected {expected_physical_loops} physical sample loops, found {}",
            starts.len()
        ));
    }
    let mut lockstep = Vec::new();
    for start in starts {
        let mut depth = 0_i32;
        let mut entered = false;
        let mut end = None;
        for (index, line) in lines.iter().enumerate().skip(start) {
            for byte in line.bytes() {
                match byte {
                    b'{' => {
                        depth += 1;
                        entered = true;
                    }
                    b'}' => depth -= 1,
                    _ => {}
                }
            }
            if entered && depth == 0 {
                end = Some(index);
                break;
            }
        }
        let end = end.ok_or_else(|| "unterminated physical sample loop".to_owned())?;
        if lines[start..=end]
            .iter()
            .any(|line| line.contains("vlock_b"))
        {
            let first = u32::try_from(start + 1).map_err(|_| "source line overflow")?;
            let last = u32::try_from(end + 1).map_err(|_| "source line overflow")?;
            lockstep.push(first..=last);
        }
    }
    let [range] = lockstep.as_slice() else {
        return Err(format!(
            "expected one register-carried lockstep sample loop, found {}",
            lockstep.len()
        ));
    };
    Ok(range.clone())
}

fn llvm_vector_op_counts(llvm: &str, lockstep_lines: &RangeInclusive<u32>) -> (usize, usize) {
    let locations = llvm
        .lines()
        .filter_map(|line| {
            let (id, body) = line.split_once(" = !DILocation(line: ")?;
            let id = id.strip_prefix('!')?.parse::<u64>().ok()?;
            let source_line = body.split(',').next()?.parse::<u32>().ok()?;
            Some((id, source_line))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut total = 0;
    let mut attributed = 0;
    for line in llvm.lines().filter(|line| {
        ["fadd <4 x float>", "fsub <4 x float>", "fmul <4 x float>"]
            .iter()
            .any(|operation| line.contains(operation))
    }) {
        total += 1;
        let debug_id = line
            .split("!dbg !")
            .nth(1)
            .and_then(|tail| {
                tail.split(|character: char| !character.is_ascii_digit())
                    .next()
            })
            .and_then(|id| id.parse::<u64>().ok());
        if debug_id
            .and_then(|id| locations.get(&id))
            .is_some_and(|line| lockstep_lines.contains(line))
        {
            attributed += 1;
        }
    }
    (total, attributed)
}

fn require_checked_vector_status(
    relative: &str,
    status: compiler::VectorPipelineStatus,
    effective: compiler::VectorEffectiveMode,
    detail: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if status != compiler::VectorPipelineStatus::Certified
        || effective != compiler::VectorEffectiveMode::CertifiedVector
    {
        return Err(format!(
            "{relative}: lockstep SIMD evidence requires checked vector FIR; status={status:?}, effective={effective:?}, detail={}",
            detail.unwrap_or("none")
        )
        .into());
    }
    if let Some(detail) = detail {
        return Err(format!(
            "{relative}: certified vector FIR unexpectedly retained fallback detail: {detail}"
        )
        .into());
    }
    Ok(())
}

/// Compiles the complex lockstep corpus through vector C++, then asks Clang for
/// optimized LLVM IR and requires profitable four-wide floating-point SLP
/// attributed to the generated lockstep source range.
pub(crate) fn lockstep_simd_check(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(option) = args.next() {
        return Err(format!("unknown lockstep-simd-check option: {option}").into());
    }

    let root = workspace_root();
    let driver = root.join("tests/bench/faust_cpp_compute_driver.cpp");
    let clang = std::env::var_os("CLANGXX").unwrap_or_else(|| "clang++".into());
    let temp_root =
        std::env::temp_dir().join(format!("faust-rs-lockstep-simd-{}", std::process::id()));
    fs::create_dir_all(&temp_root)?;

    let result = (|| {
        for (relative, minimum_vector_ops, expected_physical_loops) in SIMD_CASES {
            let source_path = root.join(relative);
            let source = fs::read_to_string(&source_path)?;
            let stem = source_path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("invalid corpus file name: {}", source_path.display()))?;
            let fir = compiler::Compiler::new()
                .with_compute_mode(compiler::ComputeMode::Vector {
                    vec_size: 32,
                    loop_variant: 1,
                })
                .compile_source_to_fir_with_lane(
                    relative,
                    &source,
                    compiler::SignalFirLane::TransformFastLane,
                )?;
            require_checked_vector_status(
                relative,
                fir.vector_pipeline_status,
                fir.vector_effective_mode,
                fir.vector_pipeline_detail.as_deref(),
            )?;
            let cpp = codegen::backends::cpp::generate_cpp_module(
                &fir.store,
                fir.module,
                &codegen::backends::cpp::CppOptions::default(),
            )?;
            let lockstep_lines = lockstep_source_range(&cpp, expected_physical_loops)
                .map_err(|error| format!("{relative}: {error}"))?;
            let cpp_path = temp_root.join(format!("{stem}.cpp"));
            let llvm_path = temp_root.join(format!("{stem}.ll"));
            fs::write(&cpp_path, cpp)?;

            // Preprocessor include paths use forward slashes on every host,
            // including Windows Clang, so the macro remains one C++ string.
            let include_path = cpp_path.to_string_lossy().replace('\\', "/");
            let output = Command::new(&clang)
                .arg("-std=c++17")
                .arg("-O3")
                .arg("-ffp-contract=off")
                .arg("-gline-tables-only")
                .arg(format!("-DFAUST_TEST_DSP=\"{include_path}\""))
                .arg("-S")
                .arg("-emit-llvm")
                .arg(&driver)
                .arg("-o")
                .arg(&llvm_path)
                .output()
                .map_err(|error| {
                    format!(
                        "failed to execute {} for {relative}: {error}",
                        Path::new(&clang).display()
                    )
                })?;
            if !output.status.success() {
                return Err(format!(
                    "{} failed for {relative}:\n{}",
                    Path::new(&clang).display(),
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }

            let llvm = fs::read_to_string(&llvm_path)?;
            let (total_vector_ops, attributed_vector_ops) =
                llvm_vector_op_counts(&llvm, &lockstep_lines);
            if attributed_vector_ops < minimum_vector_ops {
                return Err(format!(
                    "{relative}: expected at least {minimum_vector_ops} four-wide LLVM floating-point operations attributed to generated lockstep lines {lockstep_lines:?}, found {attributed_vector_ops} ({total_vector_ops} in the complete module)"
                )
                .into());
            }
            println!(
                "SIMD {relative}: {attributed_vector_ops} lockstep-attributed four-wide LLVM FP operations ({total_vector_ops} module total)"
            );
        }
        Ok(())
    })();

    let _ = fs::remove_dir_all(&temp_root);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_fallback_cannot_count_as_lockstep_simd_evidence() {
        let error = require_checked_vector_status(
            "fallback.dsp",
            compiler::VectorPipelineStatus::Fallback(
                compiler::VectorFallbackReason::EventCertificate,
            ),
            compiler::VectorEffectiveMode::Scalar,
            Some("bounded event table"),
        )
        .expect_err("scalar fallback must be rejected");
        assert!(error.to_string().contains("requires checked vector FIR"));
    }

    #[test]
    fn certified_vector_without_fallback_detail_is_accepted() {
        require_checked_vector_status(
            "certified.dsp",
            compiler::VectorPipelineStatus::Certified,
            compiler::VectorEffectiveMode::CertifiedVector,
            None,
        )
        .expect("checked vector status");
    }

    #[test]
    fn source_range_requires_one_lockstep_loop_and_retains_side_loop() {
        let cpp = "for (int i0 = vindex; i0 < n; ++i0) {\n  vlock_b0_l0_state = 1;\n}\nfor (int i0 = vindex; i0 < n; ++i0) {\n  side[i0] = 2;\n}\n";
        assert_eq!(lockstep_source_range(cpp, 2), Ok(1..=3));
        assert!(lockstep_source_range(cpp, 1).is_err());
    }

    #[test]
    fn llvm_vector_ops_outside_lockstep_source_range_are_not_attributed() {
        let llvm = "%a = fmul <4 x float> %x, %y, !dbg !7\n%b = fadd <4 x float> %a, %z, !dbg !8\n!7 = !DILocation(line: 12, column: 1, scope: !1)\n!8 = !DILocation(line: 40, column: 1, scope: !1)\n";
        assert_eq!(llvm_vector_op_counts(llvm, &(10..=20)), (2, 1));
    }
}
