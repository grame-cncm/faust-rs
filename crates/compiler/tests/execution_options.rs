//! End-to-end integration tests for the `-ec`/`-os` execution options on the
//! C and C++ backends (execution-options port plan §7.3, structural slice).
//!
//! Runtime equivalence (`control(); frame()×N` bit-exact against
//! `compute(N)`, and parity with the pinned C++ reference) is exercised by
//! the external differential harness; these tests lock the emitted
//! signatures and shapes so regressions surface in `cargo test`.

use codegen::backends::asc::AscOptions;
use codegen::backends::c::COptions;
use codegen::backends::cpp::CppOptions;
use codegen::backends::julia::JuliaOptions;
use codegen::backends::rust::RustOptions;
use compiler::{Compiler, ComputeMode, ControlRateMode, ProcessingApi};

const SLIDER_GAIN: &str = r#"process = _ * hslider("gain",0.5,0,1,0.01);"#;

fn compile_cpp(control: ControlRateMode, api: ProcessingApi) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(control)
        .with_processing_api(api);
    compiler
        .compile_source_to_cpp("exec_options_test.dsp", SLIDER_GAIN, &CppOptions::default())
        .expect("cpp compilation must succeed")
}

fn compile_c(control: ControlRateMode, api: ProcessingApi) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(control)
        .with_processing_api(api);
    compiler
        .compile_source_to_c("exec_options_test.dsp", SLIDER_GAIN, &COptions::default())
        .expect("c compilation must succeed")
}

#[test]
fn cpp_shapes_match_the_reference_contract() {
    // Classic: no execution entry points.
    let classic = compile_cpp(ControlRateMode::InlinePerBlock, ProcessingApi::Block);
    assert!(!classic.contains("void control()"));
    assert!(!classic.contains("void frame("));

    // -ec: plain (non-virtual) control(), block compute retained.
    let ec = compile_cpp(ControlRateMode::External, ProcessingApi::Block);
    assert!(ec.contains("void control() {"));
    assert!(!ec.contains("virtual void control"));
    assert!(!ec.contains("void frame("));
    assert!(ec.contains(
        "virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, \
         FAUSTFLOAT** RESTRICT outputs) {"
    ));

    // -os: virtual frame over flat arrays, canonical compute emitted empty.
    let os = compile_cpp(ControlRateMode::InlinePerBlock, ProcessingApi::OneSample);
    assert!(
        os.contains(
            "virtual void frame(FAUSTFLOAT* RESTRICT inputs, FAUSTFLOAT* RESTRICT outputs)"
        )
    );
    assert!(!os.contains("void control()"));
    let compute_pos = os
        .find("virtual void compute(")
        .expect("canonical compute retained");
    let after = &os[compute_pos..];
    let brace = after.find('{').expect("compute body");
    let close = after.find('}').expect("compute close");
    assert!(
        after[brace + 1..close].trim().is_empty(),
        "one-sample compute must be empty"
    );

    // -ec -os: both entry points.
    let ecos = compile_cpp(ControlRateMode::External, ProcessingApi::OneSample);
    assert!(ecos.contains("void control() {"));
    assert!(ecos.contains("virtual void frame("));
}

#[test]
fn c_shapes_match_the_reference_contract() {
    let ec = compile_c(ControlRateMode::External, ProcessingApi::Block);
    assert!(ec.contains("void controlmydsp(mydsp* dsp) {"));
    assert!(!ec.contains("void framemydsp("));

    let ecos = compile_c(ControlRateMode::External, ProcessingApi::OneSample);
    assert!(ecos.contains("void controlmydsp(mydsp* dsp) {"));
    assert!(ecos.contains(
        "void framemydsp(mydsp* dsp, FAUSTFLOAT* RESTRICT inputs, \
         FAUSTFLOAT* RESTRICT outputs) {"
    ));
    // Canonical compute retained and empty.
    let compute_pos = ecos
        .find("void computemydsp(")
        .expect("canonical compute retained");
    let after = &ecos[compute_pos..];
    let brace = after.find('{').expect("compute body");
    let close = after.find('}').expect("compute close");
    assert!(
        after[brace + 1..close].trim().is_empty(),
        "one-sample compute must be empty"
    );
}

fn compile_rust(control: ControlRateMode, api: ProcessingApi) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(control)
        .with_processing_api(api);
    compiler
        .compile_source_to_rust(
            "exec_options_test.dsp",
            SLIDER_GAIN,
            &RustOptions::default(),
        )
        .expect("rust compilation must succeed")
}

#[test]
fn rust_shapes_match_the_d3_contract() {
    // D3: public inherent methods; the FaustDsp trait stays unchanged.
    let ec = compile_rust(ControlRateMode::External, ProcessingApi::Block);
    assert!(ec.contains("pub fn control(&mut self)"));
    assert!(!ec.contains("pub fn frame("));
    assert!(ec.contains("impl FaustDsp for mydsp"));

    let ecos = compile_rust(ControlRateMode::External, ProcessingApi::OneSample);
    assert!(ecos.contains("pub fn control(&mut self)"));
    assert!(
        ecos.contains("pub fn frame(&mut self, inputs: &[FaustFloat], outputs: &mut [FaustFloat])")
    );
    // Canonical compute kept, empty, parameters underscored.
    let compute_pos = ecos
        .find("pub fn compute(&mut self, _count: usize")
        .expect("empty canonical compute retained with underscored params");
    let after = &ecos[compute_pos..];
    let brace = after.find('{').expect("compute body");
    let close = after.find('}').expect("compute close");
    assert!(
        after[brace + 1..close].trim().is_empty(),
        "one-sample compute must be empty"
    );
    // The host-facing trait surface is untouched (D3): the trait impl still
    // declares the canonical block compute and no frame/control.
    let trait_impl = &ecos[ecos.find("impl FaustDsp for mydsp").expect("trait impl")..];
    assert!(trait_impl.contains("fn compute(&mut self, count: i32"));
    assert!(!trait_impl.contains("fn frame"));
    assert!(!trait_impl.contains("fn control"));
}

fn compile_asc(control: ControlRateMode, api: ProcessingApi) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(control)
        .with_processing_api(api);
    compiler
        .compile_source_to_asc("exec_options_test.dsp", SLIDER_GAIN, &AscOptions::default())
        .expect("asc compilation must succeed")
}

#[test]
fn asc_shapes_match_the_one_sample_contract() {
    // Plan §5.7 (merged amendment): the AssemblyScript one-sample target.
    // Flat StaticArray channels, additive to the block compute contract.
    let ecos = compile_asc(ControlRateMode::External, ProcessingApi::OneSample);
    assert!(ecos.contains("control(): void {"));
    assert!(ecos.contains("frame(inputs: StaticArray<f32>, outputs: StaticArray<f32>): void {"));
    let compute_pos = ecos
        .find("compute(count: i32, inputs: Array<StaticArray<f32>>")
        .expect("canonical block compute retained");
    let after = &ecos[compute_pos..];
    let brace = after.find('{').expect("compute body");
    let close = after.find('}').expect("compute close");
    assert!(
        after[brace + 1..close].trim().is_empty(),
        "one-sample compute must be empty"
    );

    // Classic asc output keeps its block contract untouched.
    let classic = compile_asc(ControlRateMode::InlinePerBlock, ProcessingApi::Block);
    assert!(!classic.contains("control(): void {"));
    assert!(!classic.contains("frame(inputs:"));
}

#[test]
fn faustwasm_aux_files_honor_execution_flags_in_argv() {
    // The faustwasm `generateAuxFiles` surface receives its options as one
    // raw argv string; `-ec`/`-os` (and the `--` spellings) must configure
    // the derived compiler instead of being silently ignored.
    use compiler::GenerateAuxFilesRequest;

    for flags in ["--ec --os", "-ec -os", "--external-control --one-sample"] {
        let request = GenerateAuxFilesRequest {
            source_name: "exec_options_test.dsp".to_owned(),
            source: SLIDER_GAIN.to_owned(),
            args: format!("-lang asc -cn ExecTest {flags} -o /exec.out.ts"),
            ..GenerateAuxFilesRequest::default()
        };
        let artifacts = Compiler::new()
            .generate_aux_files(&request)
            .expect("asc aux generation must succeed");
        let asc = String::from_utf8(artifacts[0].content.clone()).expect("utf-8");
        assert!(asc.contains("control(): void {"), "flags {flags}: {asc}");
        assert!(
            asc.contains("frame(inputs: StaticArray<f32>, outputs: StaticArray<f32>): void {"),
            "flags {flags}"
        );
    }

    // Without the flags the classic block contract is untouched.
    let request = GenerateAuxFilesRequest {
        source_name: "exec_options_test.dsp".to_owned(),
        source: SLIDER_GAIN.to_owned(),
        args: "-lang asc -cn ExecTest -o /exec.out.ts".to_owned(),
        ..GenerateAuxFilesRequest::default()
    };
    let artifacts = Compiler::new()
        .generate_aux_files(&request)
        .expect("classic asc aux generation must succeed");
    let asc = String::from_utf8(artifacts[0].content.clone()).expect("utf-8");
    assert!(!asc.contains("control(): void {"));
    assert!(!asc.contains("frame(inputs:"));
}

#[test]
fn unsupported_backends_and_vector_mode_still_reject() {
    // -os stays a hard error in vector mode whatever the backend.
    let compiler = Compiler::new()
        .with_processing_api(ProcessingApi::OneSample)
        .with_compute_mode(ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        });
    let err = compiler
        .compile_source_to_cpp("exec_options_test.dsp", SLIDER_GAIN, &CppOptions::default())
        .expect_err("-os with -vec must fail");
    assert!(err.to_string().contains("scalar mode"), "{err}");

    // Julia keeps the capability rejection.
    let compiler = Compiler::new().with_processing_api(ProcessingApi::OneSample);
    let err = compiler
        .compile_source_to_julia(
            "exec_options_test.dsp",
            SLIDER_GAIN,
            &JuliaOptions::default(),
        )
        .expect_err("-os julia must fail");
    assert!(err.to_string().contains("'-os' option"), "{err}");
}
