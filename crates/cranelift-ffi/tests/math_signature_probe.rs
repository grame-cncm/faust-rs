//! A math function declared by `ffunction` is called with the signature the
//! backend implements, or refused with both signatures (W1 of
//! `porting/foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md`).
//!
//! The C and C++ backends call the `<math.h>` function, whose declaration
//! decides: the arguments convert to its parameter types, the precision
//! included. The JIT does the same from its inventory, `HOST_MATH`. Before,
//! only FIR's own names and a few C symbols were recognised, so
//! `float sinf(float)` had "no Cranelift implementation", and a C symbol with
//! another shape (`int tanhf(int)`) failed as a "subset matcher drift". A
//! call is refused when its argument count differs (the generated C does not
//! compile either) or when it declares an `int` result for a real one or the
//! reverse (the generated C++ keeps the implementation's type there).

use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;
use std::rc::Rc;

fn source(declaration: &str, process: &str) -> String {
    format!("f = ffunction({declaration}, <math.h>, \"\");\nprocess = {process};\n")
}

/// The impulse response over 4 frames, first output.
fn impulse(source: &str, double: bool, opt_level: i32) -> Vec<f64> {
    let factory = Factory::compile_from_string("math", source, &[], double, opt_level)
        .unwrap_or_else(|error| panic!("{source}: {error}"));
    let probe = Probe::instantiate(&Rc::new(factory), 44_100).expect("instantiate");
    let spec = RenderSpec {
        frames: 4,
        input: InputMode::Impulse,
        ..RenderSpec::default()
    };
    let mut out = Vec::new();
    probe.render(&spec, |_, samples| out.push(samples[0]));
    out
}

/// A real result rounded to the output precision.
fn out(x: f64, double: bool) -> f64 {
    if double { x } else { f64::from(x as f32) }
}

#[test]
fn a_math_function_runs_with_its_arguments_and_precision_converted() {
    for double in [false, true] {
        // (declaration, process, first two frames of the impulse response).
        let cases: [(&str, &str, [f64; 2]); 6] = [
            // The `f32` symbol, in both precisions: under `-double` the
            // argument converts to `float` and the result back, as in C.
            (
                "float sinf(float)",
                "f",
                [out(f64::from(1.0_f32.sin()), double), 0.0],
            ),
            (
                "float tanhf(float)",
                "f",
                [out(f64::from(1.0_f32.tanh()), double), 0.0],
            ),
            // The `f64` symbol in single precision.
            ("float tanh(float)", "f", [out(1.0_f64.tanh(), double), 0.0]),
            // An `int` parameter: the argument is truncated, then converted
            // to the `float` `tanhf` takes.
            (
                "float tanhf(int)",
                "_ * 1.9 : f",
                [out(f64::from(1.0_f32.tanh()), double), 0.0],
            ),
            ("float copysignf(float, float)", "f(2, _ - 1)", [2.0, -2.0]),
            ("int isnanf(float)", "sqrt(_ - 1) : f", [0.0, 1.0]),
        ];
        for (declaration, process, expected) in cases {
            for opt_level in [0, 3] {
                let got = impulse(&source(declaration, process), double, opt_level);
                assert_eq!(
                    [got[0], got[1]],
                    expected,
                    "{declaration} [{process}] double={double} opt_level={opt_level}"
                );
            }
        }
    }
}

#[test]
fn a_math_function_with_another_shape_is_refused_with_both_signatures() {
    for (declaration, process, implemented, call) in [
        (
            "int tanhf(int)",
            "f",
            "float tanhf(float)",
            "1 argument and an `int` result",
        ),
        (
            "float tanhf(float, float)",
            "f",
            "float tanhf(float)",
            "2 arguments and a `float` result",
        ),
        (
            "float isnanf(float)",
            "f",
            "int isnanf(float)",
            "1 argument and a `float` result",
        ),
        // FIR's own names are checked too.
        (
            "float fmin(float)",
            "f",
            "float fmin(float, float)",
            "1 argument and a `float` result",
        ),
    ] {
        for double in [false, true] {
            let program = source(declaration, process);
            let message = match Factory::compile_from_string("math", &program, &[], double, 0) {
                Ok(_) => panic!("{declaration} compiled"),
                Err(message) => message,
            };
            let name = declaration.split([' ', '(']).nth(1).expect("a name");
            assert!(message.contains("[FRS-CGEN-CLIF-0002]"), "{message}");
            assert!(
                message.contains(&format!(
                    "`{name}` does not have the signature of the math function the Cranelift \
                     backend implements, `{implemented}`: the call has {call}"
                )),
                "{message}"
            );
            assert!(!message.contains("drift"), "{message}");
        }
    }
}

/// `name(args)` computed by the system's C math library, as the JIT calls it.
#[cfg(not(target_arch = "wasm32"))]
fn libm(name: &str, args: &[foreign_call::Value]) -> f64 {
    let function = foreign_call::libm_function(name)
        .unwrap_or_else(|| panic!("{name}"))
        .function;
    match foreign_call::invoke(function.address(), function.ret(), args) {
        Some(foreign_call::Value::Float32(x)) => f64::from(x),
        Some(foreign_call::Value::Float64(x)) => x,
        Some(foreign_call::Value::Int32(x)) => f64::from(x),
        other => panic!("{name}: {other:?}"),
    }
}

/// The `ffunction` declarations of `maths.lib` the backend has no function
/// for call the system's C math library: the `float` symbol in single
/// precision, the `double` one under `-double`, and a single-name `double`
/// function (`j0`) with its argument converted, as in C.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_c_math_library_function_is_called_from_its_maths_lib_declaration() {
    use foreign_call::Value::{Float32 as F, Float64 as D, Int32 as I};
    let as_float = |x: f64| f64::from(x as f32);
    for double in [false, true] {
        let real = |x: f32| if double { D(f64::from(x)) } else { F(x) };
        let precise = |x: f64| if double { x } else { as_float(x) };
        let symbol = |single: &'static str, double_name: &'static str| {
            if double { double_name } else { single }
        };
        let cases: Vec<(&str, &str, f64)> = vec![
            (
                "float cbrtf|cbrt|cbrtl (float)",
                "f(8.5)",
                libm(symbol("cbrtf", "cbrt"), &[real(8.5)]),
            ),
            (
                "float hypotf|hypot|hypotl (float, float)",
                "f(2.5, 4.5)",
                libm(symbol("hypotf", "hypot"), &[real(2.5), real(4.5)]),
            ),
            (
                "float erff|erf|erfl(float)",
                "f(0.5)",
                libm(symbol("erff", "erf"), &[real(0.5)]),
            ),
            (
                "float tgammaf|tgamma|tgammal(float)",
                "f(4.5)",
                libm(symbol("tgammaf", "tgamma"), &[real(4.5)]),
            ),
            (
                "float lgammaf|lgamma|lgammal(float)",
                "f(4.5)",
                libm(symbol("lgammaf", "lgamma"), &[real(4.5)]),
            ),
            ("float ldexpf|ldexp|ldexpl (float, int)", "f(1.5, 3)", 12.0),
            ("int ilogbf|ilogb|ilogbl (float)", "f(10)", 3.0),
            // `double` only: the argument converts, the result rounds.
            ("float j0(float)", "f(1.5)", precise(libm("j0", &[D(1.5)]))),
            (
                "float jn(int, float)",
                "f(2, 1.5)",
                precise(libm("jn", &[I(2), D(1.5)])),
            ),
            (
                "float nextafter(float, float)",
                "f(1, 2)",
                precise(libm("nextafter", &[D(1.0), D(2.0)])),
            ),
        ];
        for (declaration, process, expected) in cases {
            for opt_level in [0, 3] {
                let got = impulse(&source(declaration, process), double, opt_level);
                assert_eq!(
                    got[0], expected,
                    "{declaration} [{process}] double={double} opt_level={opt_level}"
                );
            }
        }
    }
}

extern "C" fn host_erfcf(_: f32) -> f32 {
    42.0
}

/// A host registration replaces a C math library function (one no other
/// test calls: the registry is process-global).
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_host_registration_replaces_a_c_math_library_function() {
    unsafe {
        cranelift_ffi::factory::registerCCraneliftForeignFunction(
            c"erfcf".as_ptr(),
            (host_erfcf as *const ()).cast_mut().cast(),
        );
    }
    let got = impulse(&source("float erfcf(float)", "f(0.5)"), false, 0);
    assert_eq!(got[0], 42.0);
}
