//! Integration tests for `signal_fir_lane`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use codegen::backends::cranelift::{CraneliftOptions, generate_cranelift_module};
use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, RealType, SignalFirLane};
use std::path::PathBuf;

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn impulse_dsp_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("impulse-tests")
        .join("dsp")
        .join(file)
}

/// Compiles one full-library impulse fixture on an explicit test stack.
///
/// The library expansion is intentionally exercised here; the larger stack is
/// a test-runner accommodation, not a compiler execution requirement.
fn compile_impulse_cpp(file: &str) -> String {
    let file = file.to_owned();
    std::thread::Builder::new()
        .name(format!("{file}-load-cse"))
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .compile_file_default_to_cpp_with_lane(
                    &impulse_dsp_path(&file),
                    &codegen::backends::cpp::CppOptions::default(),
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|error| panic!("{file} scalar C++ compilation failed: {error}"))
        })
        .expect("impulse load-CSE test thread must start")
        .join()
        .expect("impulse load-CSE test thread must not panic")
}

#[test]
fn dump_cpp_fastlane_compiles_fixture() {
    let compiler = Compiler::new();
    let path = corpus_path("rep_01_passthrough.dsp");
    let cpp = compiler
        .compile_file_default_to_cpp_with_lane(
            &path,
            &codegen::backends::cpp::CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("fast-lane C++ compilation failed: {e}"));
    assert!(cpp.contains("class mydsp : public dsp"));
}

#[test]
fn fastlane_cpp_honors_explicit_class_name_option() {
    let cpp = compile_cpp_with_class_name(
        "rep_56_noise_smoo_slider.dsp",
        SignalFirLane::TransformFastLane,
        "customdsp",
    );
    assert!(cpp.contains("class customdsp : public dsp"));
    assert!(cpp.contains("#define FAUSTCLASS customdsp"));
    assert!(!cpp.contains("class mydsp : public dsp"));
}

#[test]
fn fastlane_cpp_honors_explicit_super_class_name_option() {
    let cpp = compile_cpp_with_names(
        "rep_56_noise_smoo_slider.dsp",
        SignalFirLane::TransformFastLane,
        "customdsp",
        "faust_dsp",
    );
    assert!(cpp.contains("class customdsp : public faust_dsp"));
    assert!(!cpp.contains("class customdsp : public dsp"));
}

#[test]
fn fastlane_c_honors_explicit_class_name_option() {
    let c_code = compile_c_with_class_name(
        "rep_56_noise_smoo_slider.dsp",
        SignalFirLane::TransformFastLane,
        "customdsp",
    );
    assert!(c_code.contains("} customdsp;"));
    assert!(c_code.contains("void computecustomdsp(customdsp* dsp"));
    assert!(!c_code.contains("} mydsp;"));
}

#[test]
fn fastlane_julia_emits_faust_style_shell() {
    let compiler = Compiler::new();
    let path = corpus_path("rep_01_passthrough.dsp");
    let julia = compiler
        .compile_file_default_to_julia_with_lane(
            &path,
            &codegen::backends::julia::JuliaOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("fast-lane Julia compilation failed: {e}"));

    assert!(julia.contains("mutable struct mydsp{T} <: dsp"));
    assert!(julia.contains("getNumInputs(dsp::mydsp{T}) where {T} = Int32(1)"));
    assert!(julia.contains("getNumOutputs(dsp::mydsp{T}) where {T} = Int32(1)"));
    assert!(julia.contains("function compute!(dsp::mydsp{T}, count::Int32"));
    assert!(julia.contains("outputs::AbstractMatrix{FAUSTFLOAT}"));
}

#[test]
fn fastlane_julia_honors_double_precision_real_alias() {
    let compiler = Compiler::new().with_real_type(RealType::Float64);
    let path = corpus_path("rep_01_passthrough.dsp");
    let julia = compiler
        .compile_file_default_to_julia_with_lane(
            &path,
            &codegen::backends::julia::JuliaOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("fast-lane Julia double compilation failed: {e}"));

    assert!(julia.contains("const REAL = Float64"));
}

fn compile_cpp_with_lane(file: &str, lane: SignalFirLane) -> String {
    let compiler = Compiler::new();
    let path = corpus_path(file);
    compiler
        .compile_file_default_to_cpp_with_lane(
            &path,
            &codegen::backends::cpp::CppOptions::default(),
            lane,
        )
        .unwrap_or_else(|e| panic!("{file} C++ compilation failed for lane {lane:?}: {e}"))
}

fn compile_cpp_source_with_lane(source_name: &str, source: &str, lane: SignalFirLane) -> String {
    let compiler = Compiler::new();
    compiler
        .compile_source_to_cpp_with_lane(
            source_name,
            source,
            &codegen::backends::cpp::CppOptions::default(),
            lane,
        )
        .unwrap_or_else(|e| panic!("{source_name} C++ compilation failed for lane {lane:?}: {e}"))
}

fn compile_c_with_lane(file: &str, lane: SignalFirLane) -> String {
    let compiler = Compiler::new();
    let path = corpus_path(file);
    compiler
        .compile_file_default_to_c_with_lane(
            &path,
            &codegen::backends::c::COptions::default(),
            lane,
        )
        .unwrap_or_else(|e| panic!("{file} C compilation failed for lane {lane:?}: {e}"))
}

fn compile_c_source_with_lane(source_name: &str, source: &str, lane: SignalFirLane) -> String {
    let compiler = Compiler::new();
    compiler
        .compile_source_to_c_with_lane(
            source_name,
            source,
            &codegen::backends::c::COptions::default(),
            lane,
        )
        .unwrap_or_else(|e| panic!("{source_name} C compilation failed for lane {lane:?}: {e}"))
}

fn compile_cpp_with_class_name(file: &str, lane: SignalFirLane, class_name: &str) -> String {
    compile_cpp_with_names(file, lane, class_name, "dsp")
}

fn compile_cpp_with_names(
    file: &str,
    lane: SignalFirLane,
    class_name: &str,
    super_class_name: &str,
) -> String {
    let compiler = Compiler::new();
    let path = corpus_path(file);
    let options = codegen::backends::cpp::CppOptions {
        class_name: Some(class_name.to_owned()),
        super_class_name: Some(super_class_name.to_owned()),
        ..codegen::backends::cpp::CppOptions::default()
    };
    compiler
        .compile_file_default_to_cpp_with_lane(&path, &options, lane)
        .unwrap_or_else(|e| {
            panic!(
                "{file} C++ compilation failed for lane {lane:?}, class name {class_name}, super class name {super_class_name}: {e}"
            )
        })
}

fn compile_c_with_class_name(file: &str, lane: SignalFirLane, class_name: &str) -> String {
    let compiler = Compiler::new();
    let path = corpus_path(file);
    let options = codegen::backends::c::COptions {
        class_name: Some(class_name.to_owned()),
        ..codegen::backends::c::COptions::default()
    };
    compiler
        .compile_file_default_to_c_with_lane(&path, &options, lane)
        .unwrap_or_else(|e| {
            panic!("{file} C compilation failed for lane {lane:?} and class name {class_name}: {e}")
        })
}

fn compile_cpp_with_lane_and_real_type(
    file: &str,
    lane: SignalFirLane,
    real_type: RealType,
) -> String {
    let compiler = Compiler::new().with_real_type(real_type);
    let path = corpus_path(file);
    compiler
        .compile_file_default_to_cpp_with_lane(
            &path,
            &codegen::backends::cpp::CppOptions::default(),
            lane,
        )
        .unwrap_or_else(|e| {
            panic!(
                "{file} C++ compilation failed for lane {lane:?} and real type {real_type:?}: {e}"
            )
        })
}

fn compile_c_with_lane_and_real_type(
    file: &str,
    lane: SignalFirLane,
    real_type: RealType,
) -> String {
    let compiler = Compiler::new().with_real_type(real_type);
    let path = corpus_path(file);
    compiler
        .compile_file_default_to_c_with_lane(
            &path,
            &codegen::backends::c::COptions::default(),
            lane,
        )
        .unwrap_or_else(|e| {
            panic!("{file} C compilation failed for lane {lane:?} and real type {real_type:?}: {e}")
        })
}

#[test]
fn fastlane_compiles_lowpass_feedback_fixture() {
    let fast = compile_cpp_with_lane(
        "rep_05_one_pole_lowpass.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void compute("));
}

#[test]
fn fastlane_cpp_and_interp_accept_forward_ad_delay_fixture() {
    let path = corpus_path("fad_delay.dsp");
    let compiler = Compiler::new();

    let cpp = compiler
        .compile_file_default_to_cpp_with_lane(
            &path,
            &codegen::backends::cpp::CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("fad_delay.dsp fast-lane C++ compilation failed: {e}"));
    assert!(cpp.contains("class mydsp : public dsp"));

    let fbc = compiler
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("fad_delay.dsp fast-lane interp compilation failed: {e}"));
    assert!(
        !fbc.is_empty(),
        "fad_delay.dsp fast-lane interp compilation should produce bytecode"
    );
}

#[test]
fn fastlane_delay_echo_uses_circular_delay_line_and_iota_in_c_and_cpp() {
    let fast_cpp = compile_cpp_with_lane("rep_04_delay_echo.dsp", SignalFirLane::TransformFastLane);
    assert!(fast_cpp.contains("class mydsp : public dsp"));
    assert!(fast_cpp.contains("int fIOTA;"));
    assert!(fast_cpp.contains("fVec"));
    assert!(
        fast_cpp.contains("[(fIOTA & 4095)]"),
        "C++ fast-lane should mask the delay-line write index"
    );
    // `mem` (Delay1) now uses the Shift strategy (2-element buffer, no fIOTA masking)
    // when max_copy_delay >= 1 (default 16). The large @(2205) line still uses fIOTA.
    assert!(
        !fast_cpp.contains("[(fIOTA & 1)]"),
        "C++ fast-lane should use 2-element shift buffer for delay1, not fIOTA & 1"
    );
    assert!(
        fast_cpp.contains("[((fIOTA - 2205) & 4095)]"),
        "C++ fast-lane should read the delay line through a masked circular index"
    );
    assert!(
        fast_cpp.contains("fIOTA = (fIOTA + 1);"),
        "C++ fast-lane should increment fIOTA once per sample"
    );
    assert!(
        fast_cpp.contains("for (int lDelay") && fast_cpp.contains("< 4096; ++lDelay"),
        "C++ fast-lane should zero the fixed-size delay line in instanceClear"
    );

    let fast_c = compile_c_with_lane("rep_04_delay_echo.dsp", SignalFirLane::TransformFastLane);
    assert!(fast_c.contains("int fIOTA;"));
    assert!(fast_c.contains("fVec"));
    assert!(
        fast_c.contains("[(dsp->fIOTA & 4095)]"),
        "C fast-lane should mask the delay-line write index"
    );
    // `mem` (Delay1) now uses the Shift strategy (2-element buffer, no fIOTA masking).
    assert!(
        !fast_c.contains("[(dsp->fIOTA & 1)]"),
        "C fast-lane should use 2-element shift buffer for delay1, not fIOTA & 1"
    );
    assert!(
        fast_c.contains("[((dsp->fIOTA - 2205) & 4095)]"),
        "C fast-lane should read the delay line through a masked circular index"
    );
    assert!(
        fast_c.contains("dsp->fIOTA = (dsp->fIOTA + 1);"),
        "C fast-lane should increment fIOTA once per sample"
    );
    assert!(
        fast_c.contains("for (int lDelay") && fast_c.contains("< 4096;") && fast_c.contains("= 0;"),
        "C fast-lane should zero the fixed-size delay line in instanceClear"
    );
}

#[test]
fn fastlane_interp_delay_lines_do_not_overrun_after_ring_wrap() {
    let compiler = Compiler::new();
    let path = corpus_path("rep_55_sine_phasor_echo_feedback.dsp");
    let fbc = compiler
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("fast-lane interp compilation failed: {e}"));

    let mut reader = std::io::Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader)
        .unwrap_or_else(|e| panic!("interp bytecode parse failed: {e}"));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);

    let frame_count = 5_000;
    let mut out0 = vec![0.0_f32; frame_count];
    let mut out1 = vec![0.0_f32; frame_count];
    let mut outputs: [&mut [f32]; 2] = [&mut out0, &mut out1];

    instance
        .try_compute(frame_count as i32, &[], &mut outputs)
        .unwrap_or_else(|e| panic!("interp execution should survive delay-ring wrap: {e}"));

    assert!(
        outputs[0].iter().all(|sample| sample.is_finite()),
        "output0 should stay finite across the delay-ring wrap"
    );
    assert!(
        outputs[1].iter().all(|sample| sample.is_finite()),
        "output1 should stay finite across the delay-ring wrap"
    );
}

#[test]
fn fastlane_compiles_feedback_projection_fixture() {
    let fast = compile_cpp_with_lane(
        "rep_23_feedback_simple.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void compute("));
    assert!(
        !fast.contains("frs_proj"),
        "Step 2C.2 should remove proj placeholder names from fast-lane output"
    );
    assert!(
        !fast.contains("frs_rec"),
        "Step 2C.2 should remove rec placeholder names from fast-lane output"
    );
}

#[test]
fn fastlane_compiles_environment_waveform_fixture() {
    let fast = compile_cpp_with_lane(
        "rep_20_environment_waveform.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void compute("));
    assert!(
        !fast.contains("frs_"),
        "Step 2G fast-lane output should not contain frs_* shims"
    );
}

#[test]
fn fastlane_compiles_extended_primitives_fixture() {
    let fast = compile_cpp_with_lane(
        "rep_31_extended_primitives.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void compute("));
    assert!(
        !fast.contains("frs_"),
        "Step 2F fast-lane output should not contain frs_* shims"
    );
}

#[test]
fn fastlane_compiles_nonlinear_clip_fixture() {
    let fast = compile_cpp_with_lane(
        "rep_07_nonlinear_clip.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void compute("));
    assert!(
        !fast.contains("frs_"),
        "Step 2F fast-lane output should not contain frs_* shims"
    );
}

#[test]
fn fastlane_ui_fixture_uses_native_ui_path_without_slider_shims() {
    let fast = compile_cpp_with_lane(
        "rep_10_two_in_two_out_ui.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void buildUserInterface("));
    assert!(
        !fast.contains("frs_hslider"),
        "UI sliders should use native FIR UI instructions, not frs_* shims"
    );
    assert!(
        !fast.contains("frs_vslider"),
        "UI sliders should use native FIR UI instructions, not frs_* shims"
    );
    assert!(
        !fast.contains("frs_"),
        "Step 2F fast-lane output should not contain frs_* shims"
    );
}

#[test]
fn fastlane_compiles_table_fixtures() {
    for file in [
        "rep_34_table_rdtable_readonly_const.dsp",
        "rep_35_table_rwtable_runtime_write.dsp",
        "rep_36_table_rdtable_negative_index.dsp",
        "rep_37_table_rwtable_negative_indices.dsp",
    ] {
        let fast = compile_cpp_with_lane(file, SignalFirLane::TransformFastLane);
        assert!(
            fast.contains("class "),
            "fast lane should compile table fixture {file}"
        );
        assert!(
            !fast.contains("frs_"),
            "fast lane output should not contain frs_* shim names for {file}"
        );
    }
}

#[test]
fn fastlane_compiles_sine_phasor_fixture() {
    let fast = compile_cpp_with_lane("rep_38_sine_phasor.dsp", SignalFirLane::TransformFastLane);
    assert!(fast.contains("class mydsp : public dsp"));
    assert!(fast.contains("void compute("));
    assert!(!fast.contains("frs_"));
    assert!(fast.contains("fHslider"));
    assert!(!fast.contains("fUiCtl"));
    assert!(fast.contains("ui_interface->openVerticalBox(\"rep_38_sine_phasor\");"));
    assert!(fast.contains("ui_interface->closeBox();"));
    assert_eq!(
        fast.matches("void instanceResetUserInterface() {").count(),
        1,
        "instanceResetUserInterface should be emitted once"
    );
    assert_eq!(
        fast.matches("void instanceClear() {").count(),
        1,
        "instanceClear should be emitted once"
    );
    assert!(
        fast.contains("float fRec") && fast.contains("float fRecCur"),
        "fast lane should lower simple phasor recursion to scalar state plus current-sample binding"
    );
    let has_scalar_path =
        fast.contains("float fRecCur") && fast.contains(" = fRecCur") && !fast.contains("[2];");
    let has_inline_circ =
        fast.contains("[(fIOTA & 1)] = (fRec") && fast.contains("[((fIOTA - 1) & 1)] +");
    let has_cse_circ =
        fast.contains("fIOTA & 1") && fast.contains("(fIOTA - 1) & 1") && fast.contains("fTemp");
    assert!(
        has_scalar_path || has_inline_circ || has_cse_circ,
        "fast lane should lower phasor recursion to either scalar or circular-buffer form"
    );

    let fast_c = compile_c_with_lane("rep_38_sine_phasor.dsp", SignalFirLane::TransformFastLane);
    assert!(fast_c.contains("void computemydsp("));
    assert!(!fast_c.contains("frs_"));
    assert!(fast_c.contains("fHslider"));
    assert!(!fast_c.contains("fUiCtl"));
    assert!(
        fast_c.contains("float fRec") && fast_c.contains("float fRecCur"),
        "fast lane C backend should lower simple recursion to scalar state plus current-sample binding"
    );
    assert!(fast_c.contains(
        "ui_interface->openVerticalBox(ui_interface->uiInterface, \"rep_38_sine_phasor\");"
    ));
    assert!(fast_c.contains("ui_interface->closeBox(ui_interface->uiInterface);"));
}

#[test]
fn scalar_cpp_reuses_non_aliasing_recursive_state_load_without_hiding_shift() {
    let cpp = compile_impulse_cpp("APF.dsp");
    let compute = cpp
        .find("void compute(")
        .map(|start| &cpp[start..])
        .unwrap_or_else(|| panic!("APF generated no compute method:\n{cpp}"));

    assert!(
        compute.contains("float fTemp0 = fRec204[1];"),
        "the first recursive-state load remains materialized:\n{compute}"
    );
    assert!(
        !compute.contains("float fTemp1 = fRec204[1];"),
        "the duplicate direct state load must reuse fTemp0:\n{compute}"
    );
    assert!(
        compute.contains("fRec204[2] = fTemp0;") && compute.contains("fRec204[1] = fRec204[0];"),
        "the required ordered recursive history shift must remain explicit and reuse the proven prior state:\n{compute}"
    );
}

#[test]
fn scalar_cpp_reuses_karplus_state_load_across_unrelated_scalar_store() {
    let cpp = compile_impulse_cpp("karplus.dsp");
    let compute = cpp
        .find("void compute(")
        .map(|start| &cpp[start..])
        .unwrap_or_else(|| panic!("Karplus generated no compute method:\n{cpp}"));

    assert!(
        compute.contains("float fTemp0 = fRec214[1];") && compute.contains("fRec214[2] = fTemp0;"),
        "an unrelated scalar state store must not invalidate the table-load proof:\n{compute}"
    );
    assert!(
        compute.contains("fRec205 = fRecCur205;") && compute.contains("fRec214[1] = fRec214[0];"),
        "Karplus scalar and recursive state commits must remain explicit:\n{compute}"
    );
}

#[test]
fn fastlane_cpp_root_group_prefers_declared_name_metadata() {
    let fast = compile_cpp_with_lane(
        "rep_40_metadata_master.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("ui_interface->openVerticalBox(\"main\");"));
}

#[test]
fn fastlane_cpp_preserves_metadata_bearing_ui_labels() {
    let fast = compile_cpp_with_lane(
        "rep_56_noise_smoo_slider.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("ui_interface->openVerticalBox(\"rep_56_noise_smoo_slider\");"));
    assert!(fast.contains("ui_interface->declare(&fHslider0, \"style\", \"knob\");"));
    assert!(fast.contains("ui_interface->addHorizontalSlider(\"gain\", &fHslider0"));
    assert!(fast.contains("ui_interface->closeBox();"));
}

#[test]
fn fastlane_c_preserves_metadata_bearing_ui_labels() {
    let fast = compile_c_with_lane(
        "rep_56_noise_smoo_slider.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains(
        "ui_interface->openVerticalBox(ui_interface->uiInterface, \"rep_56_noise_smoo_slider\");"
    ));
    assert!(fast.contains(
        "ui_interface->declare(ui_interface->uiInterface, &dsp->fHslider0, \"style\", \"knob\");"
    ));
    assert!(fast.contains(
        "ui_interface->addHorizontalSlider(ui_interface->uiInterface, \"gain\", &dsp->fHslider0"
    ));
    assert!(fast.contains("ui_interface->closeBox(ui_interface->uiInterface);"));
}

#[test]
fn fastlane_cpp_lifecycle_order_matches_faust_instance_init_flow() {
    let fast = compile_cpp_with_lane(
        "rep_10_two_in_two_out_ui.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(fast.contains("void instanceInit(int sample_rate)"));
    let init_sig = "virtual void init(int sample_rate) {";
    let init_start = fast
        .find(init_sig)
        .expect("init signature should be present");
    let init_body = &fast[init_start..];
    let init_class_i = init_body
        .find("classInit(sample_rate);")
        .expect("init should call classInit");
    let init_instance_i = init_body
        .find("instanceInit(sample_rate);")
        .expect("init should call instanceInit");
    assert!(
        init_class_i < init_instance_i,
        "init should call classInit before instanceInit"
    );

    let instance_init_sig = "virtual void instanceInit(int sample_rate) {";
    let instance_init_start = fast
        .find(instance_init_sig)
        .expect("instanceInit signature should be present");
    let instance_init_body = &fast[instance_init_start..];
    let instance_init_end = instance_init_body
        .find("\n    }")
        .expect("instanceInit body should close");
    let instance_init_body = &instance_init_body[..instance_init_end];
    assert!(
        !instance_init_body.contains("classInit(sample_rate);"),
        "instanceInit must not call classInit"
    );
    let constants_i = instance_init_body
        .find("instanceConstants(sample_rate);")
        .expect("instanceConstants call should be present");
    let reset_i = instance_init_body
        .find("instanceResetUserInterface();")
        .expect("instanceResetUserInterface call should be present");
    let clear_i = instance_init_body
        .find("instanceClear();")
        .expect("instanceClear call should be present");
    assert!(
        constants_i < reset_i && reset_i < clear_i,
        "instanceInit should call constants -> resetUI -> clear in order"
    );
}

#[test]
fn dump_c_fastlane_compiles_fixture() {
    let fast = compile_c_with_lane("rep_01_passthrough.dsp", SignalFirLane::TransformFastLane);
    assert!(fast.contains("typedef struct {"));
    assert!(fast.contains("void computemydsp("));
}

#[test]
fn fastlane_cpp_double_keeps_faustfloat_interface_and_uses_double_internal_ops() {
    let single = compile_cpp_with_lane_and_real_type(
        "rep_01_passthrough.dsp",
        SignalFirLane::TransformFastLane,
        RealType::Float32,
    );
    let double = compile_cpp_with_lane_and_real_type(
        "rep_01_passthrough.dsp",
        SignalFirLane::TransformFastLane,
        RealType::Float64,
    );

    assert!(single.contains("#define FAUSTFLOAT float"));
    assert!(double.contains("#define FAUSTFLOAT float"));
    assert!(single.contains("output0[i0] = ((FAUSTFLOAT)(((float)(input0[i0]))));"));
    assert!(double.contains("output0[i0] = ((FAUSTFLOAT)(((double)(input0[i0]))));"));
}

#[test]
fn fastlane_c_double_keeps_faustfloat_interface_and_uses_double_internal_ops() {
    let single = compile_c_with_lane_and_real_type(
        "rep_01_passthrough.dsp",
        SignalFirLane::TransformFastLane,
        RealType::Float32,
    );
    let double = compile_c_with_lane_and_real_type(
        "rep_01_passthrough.dsp",
        SignalFirLane::TransformFastLane,
        RealType::Float64,
    );

    assert!(single.contains("#define FAUSTFLOAT float"));
    assert!(double.contains("#define FAUSTFLOAT float"));
    assert!(single.contains("output0[i0] = ((FAUSTFLOAT)(((float)(input0[i0]))));"));
    assert!(double.contains("output0[i0] = ((FAUSTFLOAT)(((double)(input0[i0]))));"));
}

#[test]
fn fastlane_compiles_c_table_fixtures_without_shims() {
    for file in [
        "rep_34_table_rdtable_readonly_const.dsp",
        "rep_35_table_rwtable_runtime_write.dsp",
        "rep_36_table_rdtable_negative_index.dsp",
        "rep_37_table_rwtable_negative_indices.dsp",
    ] {
        let fast = compile_c_with_lane(file, SignalFirLane::TransformFastLane);
        assert!(
            fast.contains("void compute"),
            "fast lane should compile C fixture {file}"
        );
        assert!(
            !fast.contains("frs_"),
            "fast lane C output should not contain frs_* shim names for {file}"
        );
    }
}

#[test]
fn fastlane_cpp_compiles_noise_smoo_slider_fixture() {
    let cpp = compile_cpp_with_lane(
        "rep_56_noise_smoo_slider.dsp",
        SignalFirLane::TransformFastLane,
    );
    assert!(cpp.contains("class mydsp : public dsp"));
    assert!(cpp.contains("void compute("));
    assert!(cpp.contains("int iRec") && cpp.contains("int iRecCur"));
    assert!(cpp.contains("fSampleRate"));
    assert!(
        !cpp.contains("float fRec"),
        "fast-lane C++ should keep the recursive noise carrier in integer state"
    );
}

#[test]
fn fastlane_cpp_keeps_integer_recursive_min_feedback_in_int_state() {
    let cpp = compile_cpp_source_with_lane(
        "rec_int_min.dsp",
        "process = 1 : (+ : min(3)) ~ _;",
        SignalFirLane::TransformFastLane,
    );
    assert!(cpp.contains("class mydsp : public dsp"));
    assert!(cpp.contains("int iRec") && cpp.contains("int iRecCur"));
    assert!(
        !cpp.contains("float fRec") && !cpp.contains("double fRec"),
        "integer recursive min should keep recursion state in integer storage"
    );
    assert!(
        cpp.contains("std::min<int>("),
        "integer recursive min should stay an explicit integer min function call"
    );
}

#[test]
fn fastlane_cpp_keeps_integer_recursive_abs_feedback_in_int_state() {
    let cpp = compile_cpp_source_with_lane(
        "rec_int_abs.dsp",
        "process = 1 : (+ : abs) ~ _;",
        SignalFirLane::TransformFastLane,
    );
    assert!(cpp.contains("class mydsp : public dsp"));
    assert!(cpp.contains("int iRec") && cpp.contains("int iRecCur"));
    assert!(
        !cpp.contains("float fRec") && !cpp.contains("double fRec"),
        "integer recursive abs should keep recursion state in integer storage"
    );
    assert!(
        cpp.contains("std::abs("),
        "integer recursive abs should stay an explicit integer abs function call"
    );
}

#[test]
fn fastlane_cpp_emits_valid_infinity_literal_for_overflowed_float_constant() {
    let cpp = compile_cpp_source_with_lane(
        "min_overflow_inf.dsp",
        "process = 1.175494351e-38 * 1e307;",
        SignalFirLane::TransformFastLane,
    );
    assert!(
        cpp.contains("INFINITY"),
        "overflowed float constants should lower to a valid C++ infinity literal"
    );
    assert!(
        !cpp.contains("inf.0f"),
        "backend must not emit invalid `inf.0f` C++ literals"
    );
}

#[test]
fn fastlane_c_emits_valid_infinity_literal_for_overflowed_float_constant() {
    let c = compile_c_source_with_lane(
        "min_overflow_inf.dsp",
        "process = 1.175494351e-38 * 1e307;",
        SignalFirLane::TransformFastLane,
    );
    assert!(
        c.contains("INFINITY"),
        "overflowed float constants should lower to a valid C infinity literal"
    );
    assert!(
        !c.contains("inf.0f"),
        "backend must not emit invalid `inf.0f` C literals"
    );
}

#[test]
fn fastlane_backends_accept_lti_recursive_rad_state_space() {
    let path = corpus_path("rad_lti_recursive_state_space.dsp");
    let compiler = Compiler::new();

    let cpp = compiler
        .compile_file_default_to_cpp_with_lane(
            &path,
            &codegen::backends::cpp::CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("rad_lti_state_space.dsp C++ lowering failed: {e}"));
    assert!(cpp.contains("virtual int getNumInputs()"));
    assert!(cpp.contains("virtual int getNumOutputs()"));
    assert!(cpp.contains("return 2;"));
    assert!(cpp.contains("return 4;"));
    assert!(
        cpp.contains("for (int i0 = (count) - 1; i0 >= 0; i0 = i0 - 1)"),
        "C++ backend should emit the reverse-time adjoint loop"
    );
    assert!(
        cpp.contains("output2[i0]") && cpp.contains("output3[i0]"),
        "C++ backend should expose both per-sample seed-gradient contribution lanes"
    );

    let c = compiler
        .compile_file_default_to_c_with_lane(
            &path,
            &codegen::backends::c::COptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("rad_lti_state_space.dsp C lowering failed: {e}"));
    assert!(c.contains("int getNumInputsmydsp("));
    assert!(c.contains("int getNumOutputsmydsp("));
    assert!(c.contains("return 2;"));
    assert!(c.contains("return 4;"));
    assert!(
        c.contains("for (int i0 = (count) - 1; i0 >= 0; i0 = i0 - 1)"),
        "C backend should emit the reverse-time adjoint loop"
    );
    assert!(
        c.contains("output2[i0]") && c.contains("output3[i0]"),
        "C backend should expose both per-sample seed-gradient contribution lanes"
    );

    let fir = compiler
        .compile_file_to_fir_with_lane(&path, &[], SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("rad_lti_state_space.dsp FIR lowering failed: {e}"));
    let options = CraneliftOptions {
        fail_on_subset_gap: true,
        ..CraneliftOptions::default()
    };
    let module = generate_cranelift_module(&fir.store, fir.module, &options)
        .unwrap_or_else(|e| panic!("rad_lti_state_space.dsp Cranelift lowering failed: {e}"));
    assert!(
        module.compute_body_lowered(),
        "Cranelift backend should lower the reverse-time adjoint loop"
    );
}

#[test]
fn fastlane_interp_compiles_noise_smoo_slider_fixture() {
    let compiler = Compiler::new();
    let path = corpus_path("rep_56_noise_smoo_slider.dsp");
    let fbc = compiler
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| {
            panic!("rep_56_noise_smoo_slider.dsp fast-lane interp compilation failed: {e}")
        });
    assert!(
        !fbc.is_empty(),
        "rep_56_noise_smoo_slider.dsp fast-lane interp compilation should produce bytecode"
    );
}

#[test]
fn default_interp_api_uses_fastlane_runtime_lowering() {
    let compiler = Compiler::new();
    let path = corpus_path("rep_56_noise_smoo_slider.dsp");

    let default_fbc = compiler
        .compile_file_default_to_interp(&path, &InterpOptions::default())
        .unwrap_or_else(|e| panic!("default interp compilation failed: {e}"));
    let explicit_fast_fbc = compiler
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("explicit fast-lane interp compilation failed: {e}"));

    assert_eq!(
        default_fbc, explicit_fast_fbc,
        "default interp API should follow the executable fast-lane lowering"
    );
}

#[test]
fn fastlane_c_lifecycle_order_matches_faust_instance_init_flow() {
    let fast = compile_c_with_lane(
        "rep_10_two_in_two_out_ui.dsp",
        SignalFirLane::TransformFastLane,
    );
    let public_init_sig = "void initmydsp(mydsp* dsp, int sample_rate) {";
    let public_init_start = fast
        .find(public_init_sig)
        .expect("init signature should be present");
    let public_init_body = &fast[public_init_start..];
    let init_class_i = public_init_body
        .find("classInitmydsp(sample_rate);")
        .expect("init should call classInit");
    let init_instance_i = public_init_body
        .find("instanceInitmydsp(dsp, sample_rate);")
        .expect("init should call instanceInit");
    assert!(
        init_class_i < init_instance_i,
        "init should call classInit before instanceInit"
    );

    let instance_init_sig = "void instanceInitmydsp(mydsp* dsp, int sample_rate) {";
    let instance_init_start = fast
        .find(instance_init_sig)
        .expect("instanceInit signature should be present");
    let instance_init_body = &fast[instance_init_start..];
    let instance_init_end = instance_init_body
        .find("\n}")
        .expect("instanceInit body should close");
    let instance_init_body = &instance_init_body[..instance_init_end];
    assert!(
        !instance_init_body.contains("classInitmydsp(sample_rate);"),
        "instanceInit must not call classInit"
    );
    let constants_i = instance_init_body
        .find("instanceConstantsmydsp(dsp, sample_rate);")
        .expect("instanceConstants call should be present");
    let reset_i = instance_init_body
        .find("instanceResetUserInterfacemydsp(dsp);")
        .expect("instanceResetUserInterface call should be present");
    let clear_i = instance_init_body
        .find("instanceClearmydsp(dsp);")
        .expect("instanceClear call should be present");
    assert!(
        constants_i < reset_i && reset_i < clear_i,
        "instanceInit should call constants -> resetUI -> clear in order"
    );
}
