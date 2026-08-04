//! Cmajor backend integration tests.
//!
//! These tests deliberately compile self-contained Faust definitions through
//! the production signal-to-FIR lane before invoking the source emitter. They
//! do not depend on an installed Faust compiler, Faust libraries, or Cmajor
//! SDK. Optional Cmajor frontend/runtime validation is layered separately.
//!
//! Source provenance and acceptance contract:
//! `porting/cmajor-backend-port-and-test-plan-2026-08-04-en.md` C1-C6.

use codegen::backends::cmajor::{CmajorOptions, CodegenErrorCode, generate_cmajor_module};
use compiler::{Compiler, ControlRateMode, ProcessingApi, SignalFirLane};

/// Compiles a Faust source with the execution shape Cmajor intrinsically uses.
fn cmajor(source_name: &str, source: &str) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    let fir = compiler
        .compile_source_to_fir_with_lane(source_name, source, SignalFirLane::TransformFastLane)
        .expect("Cmajor FIR lowering must succeed");
    generate_cmajor_module(&fir.store, fir.module, &CmajorOptions::default())
        .expect("Cmajor emission must succeed")
}

#[test]
fn one_sample_io_uses_cmajor_streams() {
    let text = cmajor("io.dsp", "process = _ , _ : +;");
    assert!(text.contains("input stream float32 input1;"), "{text}");
    assert!(text.contains("input stream float32 input0;"), "{text}");
    assert!(text.contains("output stream float32 output0;"), "{text}");
    assert!(text.contains("output0 <-"), "{text}");
    assert_eq!(text.matches("advance();").count(), 1, "{text}");
    assert!(!text.contains("inputs["), "{text}");
    assert!(!text.contains("outputs["), "{text}");
}

#[test]
fn recurrence_and_delay_emit_state_and_dynamic_access() {
    let recurrence = cmajor("rec.dsp", "process = + ~ *(0.5);");
    assert!(
        recurrence.contains("fRec") || recurrence.contains("fVec"),
        "{recurrence}"
    );
    assert!(recurrence.contains("loop"), "{recurrence}");

    let delay = cmajor("delay.dsp", "process = _ : @(7);");
    assert!(delay.contains(".at("), "{delay}");
    assert!(delay.contains("output0 <-"), "{delay}");
}

#[test]
fn generated_lifecycle_obeys_backend_contract() {
    let text = cmajor("life.dsp", "process = + ~ *(0.5);");
    let instance = text
        .split("void instanceInit(int sample_rate)")
        .nth(1)
        .and_then(|tail| tail.split("void init()").next())
        .expect("instanceInit section");
    let constants = instance.find("instanceConstants").expect("constants");
    let reset = instance
        .find("instanceResetUserInterface")
        .expect("reset UI");
    let clear = instance.find("instanceClear").expect("clear");
    assert!(constants < reset && reset < clear, "{text}");
    assert!(!instance.contains("classInit"), "{text}");

    let init = text
        .split("void init()")
        .nth(1)
        .and_then(|tail| tail.split("void control()").next())
        .expect("init section");
    assert!(
        init.find("classInit(sample_rate)") < init.find("instanceInit(sample_rate)"),
        "{text}"
    );
}

#[test]
fn unsupported_soundfile_is_a_typed_error() {
    let compiler = Compiler::new()
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    let fir = compiler
        .compile_source_to_fir_with_lane(
            "sound.dsp",
            "process = 0,0 : soundfile(\"sound[url:{'x.wav'}]\", 2) : !,!,_,_;",
            SignalFirLane::TransformFastLane,
        )
        .expect("soundfile reaches FIR so the backend can reject it");
    let error = generate_cmajor_module(&fir.store, fir.module, &CmajorOptions::default())
        .expect_err("Cmajor must reject soundfiles");
    assert_eq!(error.code, CodegenErrorCode::Unsupported);
    assert_eq!(error.code.as_str(), "FRS-CGEN-CMAJ-0002");
}
