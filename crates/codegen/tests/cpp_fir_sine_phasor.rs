//! Integration tests for `cpp_fir_sine_phasor`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use codegen::backends::cpp::{CppOptions, generate_cpp_module};
use codegen::fixtures::build_sine_phasor_test_module;

#[test]
fn fir_module_sine_phasor_with_freq_and_gain_sliders_generates_cpp() {
    let (store, module) = build_sine_phasor_test_module();
    let cpp = generate_cpp_module(&store, module, &CppOptions::default())
        .expect("synthetic FIR module should generate C++");

    assert!(cpp.contains("class mydsp : public dsp"));
    assert!(cpp.contains("FAUSTFLOAT fFreq = 440.0;"));
    assert!(cpp.contains("FAUSTFLOAT fGain = 0.2;"));
    assert!(cpp.contains("double fPhase = 0.0;"));
    assert!(cpp.contains("void buildUserInterface(UI* ui_interface)"));
    // Slider numeric arguments are FAUSTFLOAT(...)-wrapped, matching the
    // upstream C++ compiler's `cast2FAUSTFLOAT` (C-family plan §2.5, DRIFT 5).
    assert!(cpp.contains(
        "ui_interface->addHorizontalSlider(\"freq\", &fFreq, FAUSTFLOAT(440.0), FAUSTFLOAT(20.0), FAUSTFLOAT(3000.0), FAUSTFLOAT(1.0));"
    ));
    assert!(cpp.contains(
        "ui_interface->addHorizontalSlider(\"gain\", &fGain, FAUSTFLOAT(0.2), FAUSTFLOAT(0.0), FAUSTFLOAT(1.0), FAUSTFLOAT(0.001));"
    ));
    assert!(cpp.contains(
        "void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs)"
    ));
    assert!(cpp.contains("fPhase = "));
    assert!(cpp.contains("(fFreq / 48000.0)"));
    assert!(cpp.contains("for (int i0 = 0; i0 < count; ++i0) {"));
    assert!(cpp.contains("output0[i0] = "));
    assert!(cpp.contains(" ? "));
    assert!(cpp.contains("std::sin"));
    assert!(cpp.contains("(fGain * std::sin"));
}
