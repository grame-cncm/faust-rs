//! C++ powprim.hh regression eb21fccc (#1340), with inline DSP definitions.
//! Integer subtraction must happen before conversion to the audio buffer;
//! comparing float(pow(25, 6)) to a float reference masks the lost unit.

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, MAX_OPT_LEVEL, read_fbc};
use compiler::{Compiler, ComputeMode, RealType, SignalFirLane};

const SOURCE: &str = r#"
process(x) = pow(250, s) - (1 + 249*s),
             pow(25, 6*s) - (1 + 244140624*s),
             pow(-3, 3*s) - (1 - 28*s),
             pow(int(x) + 65536, 2) - 131073*int(x),
             pow(2, -s) - (1-s)
with { s = x < 1; };
"#;

fn compiler(double: bool, vector: bool) -> Compiler {
    Compiler::new()
        .with_real_type(if double {
            RealType::Float64
        } else {
            RealType::Float32
        })
        .with_compute_mode(if vector {
            ComputeMode::Vector {
                vec_size: 4,
                loop_variant: 0,
            }
        } else {
            ComputeMode::Scalar
        })
}

#[test]
fn integer_pow_uses_double_math_and_mixed_real_calls_keep_their_precision() {
    let source = "process(x) = pow(25, 6*(x<1)), pow(25.0, x), rint(x);";
    for double in [false, true] {
        for vector in [false, true] {
            let output = compiler(double, vector)
                .compile_source_to_fir_with_lane(
                    "integer-pow",
                    source,
                    SignalFirLane::TransformFastLane,
                )
                .expect("mixed precision FIR must verify");
            let report = fir::checker::verify_fir_module(&output.store, output.module);
            assert!(
                !report.has_errors()
                    && report
                        .warnings()
                        .all(|d| !d.code.starts_with("FIR-FC") && !d.code.starts_with("FIR-MA")),
                "{report:?}"
            );
            if vector {
                assert_eq!(
                    output.vector_pipeline_status,
                    compiler::VectorPipelineStatus::Certified,
                    "{:?}",
                    output.vector_pipeline_detail
                );
            }
            let cpp = compiler(double, vector)
                .compile_source_to_cpp("integer-pow", source, &CppOptions::default())
                .expect("C++ codegen");
            assert!(cpp.contains("std::rint(std::pow("), "{cpp}");
            assert!(cpp.contains("double"), "{cpp}");
        }
    }
}

#[test]
fn integer_pow_double_interpreter_is_exact_before_and_after_optimization() {
    for vector in [false, true] {
        for opt_level in [0, i32::from(MAX_OPT_LEVEL)] {
            let fbc = compiler(true, vector)
                .compile_source_to_interp(
                    "integer-pow",
                    SOURCE,
                    &InterpOptions {
                        opt_level,
                        ..Default::default()
                    },
                )
                .expect("interpreter codegen");
            let mut factory = read_fbc::<f64>(&mut Cursor::new(fbc)).expect("read FBC");
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(44100);
            let input = [0., 1., 0., 1., 0., 1., 0., 1., 0.];
            let mut buffers = vec![vec![f64::NAN; input.len()]; 5];
            let mut outputs = buffers
                .iter_mut()
                .map(Vec::as_mut_slice)
                .collect::<Vec<_>>();
            instance
                .try_compute(input.len() as i32, &[&input], &mut outputs)
                .expect("compute");
            assert!(
                buffers.iter().flatten().all(|&x| x == 0.),
                "vec={vector} opt={opt_level}: {buffers:?}"
            );
        }
    }
}

#[test]
fn small_integer_power_evaluates_a_foreign_base_once() {
    let source = "f = ffunction(int integer_pow_side_effect(int), \"pow-test.h\", \"\"); process = pow(f(int(_)), 3);";
    let cpp = compiler(false, false)
        .compile_source_to_cpp("integer-pow", source, &CppOptions::default())
        .expect("C++ codegen");
    assert_eq!(cpp.matches("integer_pow_side_effect(").count(), 1, "{cpp}");
    assert!(cpp.contains("faust_wrap_mul"), "{cpp}");
}

#[test]
fn generated_cpp_integer_pow_is_exact_with_fast_math() {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    if !Command::new(&cxx)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipping native integer pow test: no C++ compiler");
        return;
    }
    let dir = std::env::temp_dir().join(format!("faust-rs-integer-pow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let architecture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/impulse-tests/archs");
    std::fs::write(
        dir.join("runner.cpp"),
        r#"#include "faust_minimal.h"
#include "generated.cpp"
int main() {
    mydsp d; d.init(44100);
    float input[9] = {0,1,0,1,0,1,0,1,0};
    float output[5][9] = {};
    float* inputs[] = {input};
    float* outputs[] = {output[0],output[1],output[2],output[3],output[4]};
    d.compute(9, inputs, outputs);
    for (int c=0; c<5; ++c) for (int i=0; i<9; ++i)
        if (output[c][i] != 0) return 1;
}
"#,
    )
    .expect("write runner");
    for double in [false, true] {
        for vector in [false, true] {
            let cpp = compiler(double, vector)
                .compile_source_to_cpp("integer-pow", SOURCE, &CppOptions::default())
                .expect("C++ codegen");
            std::fs::write(dir.join("generated.cpp"), cpp).expect("write generated code");
            for flags in [&["-O2"][..], &["-O3", "-ffast-math"][..]] {
                let exe = dir.join(format!("runner{}", std::env::consts::EXE_SUFFIX));
                let build = Command::new(&cxx)
                    .args(flags)
                    .arg("-std=c++14")
                    .arg("-I")
                    .arg(&architecture)
                    .arg(dir.join("runner.cpp"))
                    .arg("-o")
                    .arg(&exe)
                    .output()
                    .expect("compile runner");
                assert!(
                    build.status.success(),
                    "{}",
                    String::from_utf8_lossy(&build.stderr)
                );
                let run = Command::new(&exe).output().expect("run runner");
                assert!(
                    run.status.success(),
                    "double={double} vec={vector} flags={flags:?}"
                );
            }
        }
    }
    std::fs::remove_dir_all(dir).expect("remove temp dir");
}
