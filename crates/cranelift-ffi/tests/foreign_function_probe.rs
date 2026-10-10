//! A foreign function the JIT cannot call is a compile error, not silence.
//!
//! The C factory constructors gave a `compute` the Cranelift backend cannot
//! lower a no-op body. That body writes no output, so a probe of it rendered
//! zeros as a successful result (the 2026-10-08 audit, W1). The constructors
//! now refuse such a program and name the function; every probe compiles
//! through one boundary, `Factory::create`, which checks it again.
//!
//! The registry of host functions is process-global: each test registers its
//! own names and never clears it.

use cranelift_ffi::factory::registerCCraneliftForeignFunction;
use cranelift_ffi::probe::engine::{Factory, PolyProbe, Probe, RenderSpec, last_compile_failure};
use cranelift_ffi::probe::render::InputMode;
use std::ffi::c_void;
use std::process::Command;
use std::rc::Rc;

/// A foreign function no host registers, called at the sample rate.
const UNBOUND: &str = "f = ffunction(float w1_unbound(float), \"w1.h\", \"\");\nprocess = f;\n";

/// The same function on one of two lanes: the whole `compute` is the no-op,
/// so the valid lane would go silent too.
const UNBOUND_ONE_LANE: &str =
    "f = ffunction(float w1_unbound(float), \"w1.h\", \"\");\nprocess = _, f;\n";

/// A math function the backend implements itself.
const TANH: &str = "t = ffunction(float tanhf|tanh|tanhl(float), <math.h>, \"\");\nprocess = t;\n";

extern "C" fn w1_add_one_f(x: f32) -> f32 {
    x + 1.0
}

extern "C" fn w1_add_one_d(x: f64) -> f64 {
    x + 1.0
}

/// Registered under both precisions' names: `w1_add_one_f` in single,
/// `w1_add_one_d` under `-double`.
const REGISTERED: &str = "f = ffunction(float w1_add_one_f|w1_add_one_d|w1_add_one_l(float), \"w1.h\", \"\");\n\
     process = f;\n";

fn register_add_one() {
    unsafe {
        registerCCraneliftForeignFunction(
            c"w1_add_one_f".as_ptr(),
            (w1_add_one_f as *const ()).cast_mut().cast::<c_void>(),
        );
        registerCCraneliftForeignFunction(
            c"w1_add_one_d".as_ptr(),
            (w1_add_one_d as *const ()).cast_mut().cast::<c_void>(),
        );
    }
}

/// The impulse response over `frames` frames, first output.
fn impulse(source: &str, double: bool, opt_level: i32, frames: usize) -> Vec<f64> {
    let factory = Factory::compile_from_string("w1", source, &[], double, opt_level)
        .unwrap_or_else(|error| panic!("compile: {error}"));
    let probe = Probe::instantiate(&Rc::new(factory), 44_100).expect("instantiate");
    let spec = RenderSpec {
        frames,
        input: InputMode::Impulse,
        ..RenderSpec::default()
    };
    let mut out = Vec::new();
    probe.render(&spec, |_, samples| out.push(samples[0]));
    out
}

fn compile_error(source: &str, double: bool) -> String {
    match Factory::compile_from_string("w1", source, &[], double, 0) {
        Ok(_) => panic!("a no-op factory was accepted"),
        Err(message) => message,
    }
}

#[test]
fn an_unbound_foreign_call_is_refused_and_named() {
    for double in [false, true] {
        let message = compile_error(UNBOUND, double);
        assert!(message.contains("[FRS-CGEN-CLIF-0002]"), "{message}");
        assert!(message.contains("cannot lower `compute`"), "{message}");
        assert!(
            message.contains("`w1_unbound` has no Cranelift implementation"),
            "{message}"
        );
        // The failure is the thread's last one, as for a front-end error.
        // The backend reports no typed diagnostics through the C API.
        let failure = last_compile_failure().expect("recorded");
        assert_eq!(failure.text, message);
        assert_eq!(failure.diagnostics_json, None);
    }
}

#[test]
fn an_unbound_call_on_one_lane_does_not_silence_the_other_one_quietly() {
    let message = compile_error(UNBOUND_ONE_LANE, false);
    assert!(message.contains("`w1_unbound`"), "{message}");
}

#[test]
fn programs_the_backend_lowers_still_render() {
    // A genuine silence is not mistaken for a no-op body.
    assert_eq!(impulse("process = 0;", false, 0, 4), [0.0; 4]);
    for (double, peak) in [(false, f64::from(1.0_f32.tanh())), (true, 1.0_f64.tanh())] {
        for opt_level in [0, 3] {
            assert_eq!(
                impulse(TANH, double, opt_level, 4),
                [peak, 0.0, 0.0, 0.0],
                "double={double} opt_level={opt_level}"
            );
        }
    }
}

#[test]
fn a_registered_host_function_is_called() {
    register_add_one();
    for double in [false, true] {
        for opt_level in [0, 3] {
            assert_eq!(
                impulse(REGISTERED, double, opt_level, 4),
                [2.0, 1.0, 1.0, 1.0],
                "double={double} opt_level={opt_level}"
            );
        }
    }
}

/// A voice from primitives only (as in `poly_probe.rs`), and an `effect` the
/// inline extraction finds.
fn instrument_with_effect(effect: &str) -> String {
    format!(
        "freq = hslider(\"freq\", 440, 20, 20000, 0.01);\n\
         gain = hslider(\"gain\", 0.8, 0, 1, 0.001);\n\
         gate = button(\"gate\");\n\
         process = (gate * 0.002) : (+ ~ *(0.998)) : *(gain) : *(freq > 0);\n\
         effect = {effect};\n"
    )
}

fn poly_compile(source: &str, tag: &str) -> Result<PolyProbe, String> {
    let path = std::env::temp_dir().join(format!(
        "faustprobe_w1_poly_{tag}_{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write dsp");
    let result = PolyProbe::compile(
        &path.to_string_lossy(),
        &[],
        44_100,
        false,
        0,
        2,
        None,
        1e-4,
    );
    let _ = std::fs::remove_file(&path);
    result
}

#[test]
fn an_inline_effect_the_jit_cannot_run_is_an_error_not_a_missing_effect() {
    // Control: an ordinary effect is extracted.
    assert!(poly_compile(&instrument_with_effect("*(0.5)"), "ok").is_ok());
    let error = match poly_compile(
        &instrument_with_effect("ffunction(float w1_unbound_fx(float), \"w1.h\", \"\")"),
        "unbound",
    ) {
        Ok(_) => panic!("the bus was built without its effect"),
        Err(error) => error,
    };
    assert!(error.contains("`effect`"), "{error}");
    assert!(error.contains("`w1_unbound_fx`"), "{error}");
}

#[test]
fn the_binary_renders_nothing_and_fails() {
    let path = std::env::temp_dir().join(format!("faustprobe_w1_{}.dsp", std::process::id()));
    std::fs::write(&path, UNBOUND).expect("write dsp");
    let out = Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .args(["-n", "4"])
        .arg(&path)
        .output()
        .expect("run faustprobe");
    let _ = std::fs::remove_file(&path);

    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "no statistics for a refused factory");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("`w1_unbound`"), "{stderr}");
}
