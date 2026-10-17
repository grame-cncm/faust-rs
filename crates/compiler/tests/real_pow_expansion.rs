//! Real powers with a small integral literal exponent are products.
//!
//! C++ `powprim.hh` expands `pow(x, n)` for a literal `n` from 0 to 8 into a
//! `<Klass>_faustpower<n>_f` helper whose body is `((x·x)·x)…`, on the
//! backends where `gNeedManualPow` holds, and keeps `pow` elsewhere. Rust
//! writes that product inline on every backend; FIR common-subexpression
//! materialization evaluates a non-trivial base once. Compatibility registry:
//! DIFF-BEH-022.

use std::io::Cursor;

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, MAX_OPT_LEVEL, read_fbc};
use compiler::{Compiler, ComputeMode, RealType, SignalFirLane};

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

const SMALL_POWERS: &str = "process(x) = pow(x + 0.25, 2), pow(x + 0.5, 3.0), pow(x - 0.25, 8), pow(x, 0.0), pow(int(4 * x), 2.0);";

#[test]
fn small_integral_real_powers_are_products_in_scalar_and_vector_code() {
    for double in [false, true] {
        for vector in [false, true] {
            let output = compiler(double, vector)
                .compile_source_to_fir_with_lane(
                    "real-pow",
                    SMALL_POWERS,
                    SignalFirLane::TransformFastLane,
                )
                .expect("FIR");
            let report = fir::checker::verify_fir_module(&output.store, output.module);
            assert!(!report.has_errors(), "{report:?}");
            if vector {
                assert_eq!(
                    output.vector_pipeline_status,
                    compiler::VectorPipelineStatus::Certified,
                    "{:?}",
                    output.vector_pipeline_detail
                );
            }
            let cpp = compiler(double, vector)
                .compile_source_to_cpp("real-pow", SMALL_POWERS, &CppOptions::default())
                .expect("C++ codegen");
            assert!(
                !cpp.contains("pow("),
                "double={double} vector={vector}\n{cpp}"
            );
            // The base of the eighth power is bound once and multiplied by
            // itself seven times, left-associated as in the C++ helper.
            let temp = if vector { "fVecL0Temp" } else { "fTemp" };
            let eighth = cpp
                .lines()
                .find(|line| line.contains("output2["))
                .expect("third output");
            assert_eq!(eighth.matches(" * ").count(), 7, "{eighth}");
            assert!(eighth.contains(&format!("((((((({temp}")), "{eighth}");
        }
    }
}

/// `pow(c, 1)` is the base itself. When `c` is an init-local `fConst*` (shared
/// by constants only) and the power is read by the sample loop, the power still
/// gets a struct field; the local must not leak into `compute` (faustlibraries
/// `an.spectral_centroid`, `moment(…, K)` with `K = 1` and `0`).
#[test]
fn an_identity_power_of_an_init_local_constant_is_stored_for_compute() {
    let source = "SR = fconstant(int fSamplingFreq, <math.h>);\n\
                  c = sqrt(SR * 4000.0);\n\
                  process = _ <: *(pow(c, 1)), (@(1) * (1.0 / (c + 1.0))), (@(2) * pow(c, 0)) :> _;";
    for double in [false, true] {
        for vector in [false, true] {
            let output = compiler(double, vector)
                .compile_source_to_fir_with_lane(
                    "real-pow",
                    source,
                    SignalFirLane::TransformFastLane,
                )
                .expect("FIR");
            let report = fir::checker::verify_fir_module(&output.store, output.module);
            assert!(
                !report.has_errors(),
                "double={double} vector={vector}: {report:?}"
            );
            compiler(double, vector)
                .compile_source_to_cpp("real-pow", source, &CppOptions::default())
                .unwrap_or_else(|e| panic!("double={double} vector={vector}: {e:?}"));
        }
    }
}

/// The struct copy of the init-local carries its value into `compute`: the
/// interpreter output is `x[n]·c + x[n-1]/(c + 1) + x[n-2]` with
/// `c = sqrt(SR·4000)`, scalar and vector, at optimization levels 0 and max.
#[test]
fn an_identity_power_of_an_init_local_constant_computes_its_value() {
    let source = "SR = fconstant(int fSamplingFreq, <math.h>);\n\
                  c = sqrt(SR * 4000.0);\n\
                  process = _ <: *(pow(c, 1)), (@(1) * (1.0 / (c + 1.0))), (@(2) * pow(c, 0)) :> _;";
    let sample_rate = 44100;
    let input = [0.1f32, -0.7, 0.3333, 0.9, -0.05, 0.61, -0.99, 0.0];
    let c = (sample_rate as f32 * 4000.0).sqrt();
    for vector in [false, true] {
        for opt_level in [0, i32::from(MAX_OPT_LEVEL)] {
            let fbc = compiler(false, vector)
                .compile_source_to_interp(
                    "real-pow",
                    source,
                    &InterpOptions {
                        opt_level,
                        ..Default::default()
                    },
                )
                .expect("interpreter codegen");
            let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("read FBC");
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(sample_rate);
            let mut output = vec![f32::NAN; input.len()];
            instance
                .try_compute(input.len() as i32, &[&input], &mut [output.as_mut_slice()])
                .expect("compute");
            for (n, &got) in output.iter().enumerate() {
                let at = |k: usize| if n >= k { input[n - k] } else { 0.0 };
                let want = at(0) * c + at(1) / (c + 1.0) + at(2);
                assert!(
                    (got - want).abs() <= 1e-6 * want.abs().max(1.0),
                    "vec={vector} opt={opt_level} n={n}: {got} != {want}"
                );
            }
        }
    }
}

#[test]
fn a_real_power_evaluates_a_foreign_base_once() {
    let source = "f = ffunction(float real_pow_side_effect(float), \"pow-test.h\", \"\"); process = pow(f(_), 3.0);";
    let cpp = compiler(false, false)
        .compile_source_to_cpp("real-pow", source, &CppOptions::default())
        .expect("C++ codegen");
    assert_eq!(cpp.matches("real_pow_side_effect(").count(), 1, "{cpp}");
    assert!(!cpp.contains("powf"), "{cpp}");
}

#[test]
fn other_real_exponents_keep_pow() {
    // Non-integral, beyond 8, negative (C++ expands a negative integer
    // literal to the base itself, a defect not ported), and not a literal.
    for source in [
        "process(x) = pow(x, 2.5);",
        "process(x) = pow(x, 9.0);",
        "process(x) = pow(x, -2);",
        "process(x, y) = pow(x, y);",
    ] {
        let cpp = compiler(false, false)
            .compile_source_to_cpp("real-pow", source, &CppOptions::default())
            .expect("C++ codegen");
        assert!(cpp.contains("powf("), "{source}\n{cpp}");
    }
}

#[test]
fn interpreted_powers_are_the_left_associated_products_at_every_optimization_level() {
    let source = "process(x) = pow(x + 0.25, 2), pow(x + 0.5, 3.0), pow(x - 0.25, 8);";
    let input = [0.1, -0.7, 0.3333, 0.9, -0.05, 0.61, -0.99, 0.0];
    for vector in [false, true] {
        for opt_level in [0, i32::from(MAX_OPT_LEVEL)] {
            let fbc = compiler(false, vector)
                .compile_source_to_interp(
                    "real-pow",
                    source,
                    &InterpOptions {
                        opt_level,
                        ..Default::default()
                    },
                )
                .expect("interpreter codegen");
            let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("read FBC");
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(44100);
            let input: Vec<f32> = input.to_vec();
            let mut buffers = vec![vec![f32::NAN; input.len()]; 3];
            let mut outputs = buffers
                .iter_mut()
                .map(Vec::as_mut_slice)
                .collect::<Vec<_>>();
            instance
                .try_compute(input.len() as i32, &[&input], &mut outputs)
                .expect("compute");
            for (i, &x) in input.iter().enumerate() {
                let a = x + 0.25;
                let b = x + 0.5;
                let c = x - 0.25;
                let eighth = ((((((c * c) * c) * c) * c) * c) * c) * c;
                let expected = [a * a, (b * b) * b, eighth];
                for (out, want) in expected.iter().enumerate() {
                    assert_eq!(
                        buffers[out][i].to_bits(),
                        want.to_bits(),
                        "vec={vector} opt={opt_level} out={out} x={x}"
                    );
                }
            }
        }
    }
}
