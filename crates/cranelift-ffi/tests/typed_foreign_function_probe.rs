//! A host function registered with its C signature is called with it, or
//! refused with both signatures (W1 of
//! `porting/foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md`).
//!
//! `registerCCraneliftForeignFunction` takes a name and an address, as C++
//! `registerForeignFunction` takes a name: the JIT calls the function with
//! the types of the `ffunction` declaration, and a host function of other
//! types is called wrongly, unchecked. With
//! `registerCCraneliftForeignFunctionWithSignature`, the arguments and the
//! result convert to the registered types, as a C prototype makes C do, and
//! a declaration of another shape fails the factory.
//!
//! The registry is process-global: each test registers its own names.

use cranelift_ffi::factory::{
    registerCCraneliftForeignFunction, registerCCraneliftForeignFunctionWithSignature,
};
use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;
use std::ffi::{CString, c_void};
use std::rc::Rc;

extern "C" fn typed_twice_d(x: f64) -> f64 {
    2.0 * x + 0.25
}

extern "C" fn typed_mix_d(n: i32, a: f64) -> f64 {
    f64::from(n) + 10.0 * a
}

fn address(f: *const ()) -> *mut c_void {
    f.cast_mut().cast::<c_void>()
}

fn register_typed(name: &str, f: *const (), signature: &str) -> bool {
    let name = CString::new(name).expect("name");
    let signature = CString::new(signature).expect("signature");
    unsafe {
        registerCCraneliftForeignFunctionWithSignature(
            name.as_ptr(),
            address(f),
            signature.as_ptr(),
        )
    }
}

fn source(declaration: &str, process: &str) -> String {
    format!("f = ffunction({declaration}, \"typed.h\", \"\");\nprocess = {process};\n")
}

/// The first two frames of the impulse response, first output.
fn impulse(source: &str, double: bool, opt_level: i32) -> [f64; 2] {
    let factory = Factory::compile_from_string("typed", source, &[], double, opt_level)
        .unwrap_or_else(|error| panic!("{source}: {error}"));
    let probe = Probe::instantiate(&Rc::new(factory), 44_100).expect("instantiate");
    let spec = RenderSpec {
        frames: 2,
        input: InputMode::Impulse,
        ..RenderSpec::default()
    };
    let mut out = Vec::new();
    probe.render(&spec, |_, samples| out.push(samples[0]));
    [out[0], out[1]]
}

fn compile_error(source: &str, double: bool) -> String {
    match Factory::compile_from_string("typed", source, &[], double, 0) {
        Ok(_) => panic!("{source} compiled"),
        Err(message) => message,
    }
}

#[test]
fn a_typed_host_function_gets_its_arguments_and_result_converted() {
    assert!(register_typed(
        "typed_twice_d",
        typed_twice_d as *const (),
        "double(double)"
    ));
    assert!(register_typed(
        "typed_mix_d",
        typed_mix_d as *const (),
        "double typed_mix_d(int, double)"
    ));
    for double in [false, true] {
        for opt_level in [0, 3] {
            // A `double` function declared `float`: in single precision the
            // argument converts to `double` and the result back to `float`.
            assert_eq!(
                impulse(
                    &source("float typed_twice_d(float)", "f"),
                    double,
                    opt_level
                ),
                [2.25, 0.25],
                "double={double} opt_level={opt_level}"
            );
            // A real argument of the `int` parameter is truncated.
            assert_eq!(
                impulse(
                    &source("float typed_mix_d(float, float)", "f(_ * 1.9, 2)"),
                    double,
                    opt_level
                ),
                [21.0, 20.0],
                "double={double} opt_level={opt_level}"
            );
        }
    }
}

#[test]
fn a_declaration_of_another_shape_is_refused_with_both_signatures() {
    assert!(register_typed(
        "typed_shape_d",
        typed_twice_d as *const (),
        "double(double)"
    ));
    for (declaration, process, call) in [
        (
            "float typed_shape_d(float, float)",
            "f",
            "2 arguments and a `float` result",
        ),
        (
            "int typed_shape_d(float)",
            "f",
            "1 argument and an `int` result",
        ),
    ] {
        for double in [false, true] {
            let message = compile_error(&source(declaration, process), double);
            assert!(message.contains("[FRS-CGEN-CLIF-0002]"), "{message}");
            assert!(
                message.contains(&format!(
                    "`typed_shape_d` does not have the signature of the host function \
                     registered under that name, `double typed_shape_d(double)`: the call \
                     has {call}; declare it `float typed_shape_d(float)` in the `ffunction`"
                )),
                "{message}"
            );
        }
    }
}

#[test]
fn a_malformed_signature_registers_nothing() {
    for signature in [
        "float(long)",
        "float",
        "float(float",
        "void(float)",
        "double other_name(double)",
        "double typed_bad_d double(double)",
    ] {
        assert!(
            !register_typed("typed_bad_d", typed_twice_d as *const (), signature),
            "{signature}"
        );
    }
    let message = compile_error(&source("float typed_bad_d(float)", "f"), false);
    assert!(
        message.contains("`typed_bad_d` has no Cranelift implementation"),
        "{message}"
    );
}

/// The factory cache key holds the signatures: a name registered again with
/// another signature, at the same address, compiles to another factory, not
/// to the cached one, which is still alive.
#[test]
fn a_new_signature_is_not_served_a_cached_factory() {
    let program = source("float typed_again_d(float)", "f");
    let compile = || {
        Factory::compile_from_string("typed", &program, &[], true, 0)
            .unwrap_or_else(|error| panic!("{error}"))
    };
    assert!(register_typed(
        "typed_again_d",
        typed_twice_d as *const (),
        "double(double)"
    ));
    let typed = compile();
    assert_eq!(compile().sha_key(), typed.sha_key());
    // Registered again without a signature, it is trusted.
    unsafe {
        registerCCraneliftForeignFunction(
            c"typed_again_d".as_ptr(),
            address(typed_twice_d as *const ()),
        );
    }
    let untyped = compile();
    assert_ne!(untyped.sha_key(), typed.sha_key());
    assert_eq!(impulse(&program, true, 0), [2.25, 0.25]);
    assert!(register_typed(
        "typed_again_d",
        typed_twice_d as *const (),
        "double(double, double)"
    ));
    let message = compile_error(&program, true);
    assert!(
        message.contains("registered under that name, `double typed_again_d(double, double)`"),
        "{message}"
    );
}
