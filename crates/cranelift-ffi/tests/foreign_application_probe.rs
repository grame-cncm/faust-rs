//! A foreign function block applied to fewer arguments than it has inputs
//! gets implicit wires for the rest, as C++ `applyList` does (W4 of
//! `porting/foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md`).
//!
//! The evaluator's arity inference had no case for `FFun`, so `f(1)` on a
//! three-input function was the bare `1 : f`, which propagation rejected
//! (`FRS-PROP-0002`). Two more defects showed here: the arguments were not
//! cast to their declared types, and the prototype listed them last first.
//! The function is a registered host function, whose
//! asymmetric arithmetic shows a swapped argument or wire: n + 10·a + 100·b.
//! The expected values were checked against the pinned C++ compiler's
//! generated code, input counts and samples.

use cranelift_ffi::factory::registerCCraneliftForeignFunction;
use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;
use std::ffi::c_void;
use std::rc::Rc;

extern "C" fn w4_three_f(n: i32, a: f32, b: f32) -> f32 {
    n as f32 + 10.0 * a + 100.0 * b
}

extern "C" fn w4_three_d(n: i32, a: f64, b: f64) -> f64 {
    f64::from(n) + 10.0 * a + 100.0 * b
}

fn register() {
    unsafe {
        registerCCraneliftForeignFunction(
            c"w4_three_f".as_ptr(),
            (w4_three_f as *const ()).cast_mut().cast::<c_void>(),
        );
        registerCCraneliftForeignFunction(
            c"w4_three_d".as_ptr(),
            (w4_three_d as *const ()).cast_mut().cast::<c_void>(),
        );
    }
}

const DESCRIPTOR: &str =
    "ffunction(float w4_three_f|w4_three_d|w4_three_l(int, float, float), \"w4.h\", \"\")";

fn source(process: &str) -> String {
    format!("f = {DESCRIPTOR};\nprocess = {process};\n")
}

/// The input count, and the first two frames of the output with an impulse
/// on each input in turn (one output only, with no input).
fn responses(process: &str, double: bool, opt_level: i32) -> (usize, Vec<[f64; 2]>) {
    let factory = Factory::compile_from_string("w4", &source(process), &[], double, opt_level)
        .unwrap_or_else(|error| panic!("{process}: {error}"));
    let probe = Probe::instantiate(&Rc::new(factory), 44_100).expect("instantiate");
    let inputs = probe.inputs();
    let modes: Vec<InputMode> = if inputs == 0 {
        vec![InputMode::Zero]
    } else {
        (0..inputs).map(InputMode::ImpulseChannel).collect()
    };
    let responses = modes
        .into_iter()
        .map(|input| {
            let spec = RenderSpec {
                frames: 2,
                input,
                ..RenderSpec::default()
            };
            let mut out = Vec::new();
            probe.render(&spec, |_, samples| out.push(samples[0]));
            [out[0], out[1]]
        })
        .collect();
    (inputs, responses)
}

#[test]
fn a_partially_applied_foreign_function_gets_wires_for_its_last_inputs() {
    register();
    // (application, inputs, [first two frames with an impulse on input k]).
    let cases: [(&str, usize, &[[f64; 2]]); 9] = [
        // n = 1, a and b from the inputs.
        ("f(1)", 2, &[[11.0, 1.0], [101.0, 1.0]]),
        ("(1, _, _) : f", 2, &[[11.0, 1.0], [101.0, 1.0]]),
        (
            &format!("{DESCRIPTOR}(1)") as &str,
            2,
            &[[11.0, 1.0], [101.0, 1.0]],
        ),
        // n = 1, a = 2, b from the input.
        ("f(1, 2)", 1, &[[121.0, 21.0]]),
        // One argument with two outputs fills two inputs.
        ("f((1, 2))", 1, &[[121.0, 21.0]]),
        ("f(1, 2, 3)", 0, &[[321.0, 321.0]]),
        // The inner application is n = 321.
        ("f(f(1, 2, 3))", 2, &[[331.0, 321.0], [421.0, 321.0]]),
        // A real argument of the `int` parameter is truncated, an `int`
        // argument of a `float` one converted (`2`, `3` above): C++
        // `SignalPromotion` casts each argument to its declared type.
        ("f(1.7, 2, 3)", 0, &[[321.0, 321.0]]),
        ("f(_ * 2.6, 2, 3)", 1, &[[322.0, 320.0]]),
    ];
    for (process, inputs, expected) in cases {
        for double in [false, true] {
            for opt_level in [0, 3] {
                let (got_inputs, got) = responses(process, double, opt_level);
                assert_eq!(got_inputs, inputs, "{process}");
                assert_eq!(
                    got, expected,
                    "{process} double={double} opt_level={opt_level}"
                );
            }
        }
    }
}

#[test]
fn an_over_applied_foreign_function_is_an_evaluation_error() {
    register();
    let error = match Factory::compile_from_string("w4", &source("f(1, 2, 3, 4)"), &[], false, 0) {
        Ok(_) => panic!("four arguments to a three-input function compiled"),
        Err(error) => error,
    };
    assert!(error.contains("[FRS-EVAL-0003]"), "{error}");
    assert!(
        error.contains("too many arguments: expected at most 3, got 4"),
        "{error}"
    );
}
