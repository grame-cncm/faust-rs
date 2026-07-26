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

#[test]
fn faustwasm_aux_files_honor_mcd_dlt_vec_ss_in_argv() {
    // Same rationale as `faustwasm_aux_files_honor_execution_flags_in_argv`,
    // for the delay-strategy and scheduling flags: `-mcd`/`-dlt`/`-vec`/`-ss`
    // must configure the derived compiler, not be silently dropped.
    use compiler::GenerateAuxFilesRequest;

    // `mem` (delay 1) uses the Shift strategy by default (mcd 16): no
    // `fIOTA`-masked read/write for it. `@(2205)` is far above mcd, so it
    // always goes through the circular fIOTA strategy.
    const DELAY_SOURCE: &str = "process = _ <: _,(mem : @(2205) : *(0.35)) : +;";

    let cpp_of = |args: &str| {
        let artifacts = Compiler::new()
            .generate_aux_files(&GenerateAuxFilesRequest {
                source_name: "exec_options_test.dsp".to_owned(),
                source: DELAY_SOURCE.to_owned(),
                args: args.to_owned(),
                ..GenerateAuxFilesRequest::default()
            })
            .unwrap_or_else(|e| panic!("generate_aux_files({args}) must succeed: {e}"));
        String::from_utf8(artifacts[0].content.clone()).expect("utf-8 cpp")
    };

    // Default `-mcd` (16): delay 1 stays on the Shift strategy.
    let default_mcd = cpp_of("-cpp");
    assert!(!default_mcd.contains("[(fIOTA & 1)]"));

    // `-mcd 0`: even delay 1 must go through the fIOTA-masked strategy.
    let mcd0 = cpp_of("-cpp -mcd 0");
    assert!(
        mcd0.contains("[(fIOTA & 1)]"),
        "-mcd 0 must force delay 1 off the Shift strategy:\n{mcd0}"
    );

    // Default `-dlt` (disabled): the 2205 delay line stays circular-pow2.
    assert!(!default_mcd.contains("fIdx"));

    // `-dlt 8`: the 2205 delay line (well above 8) must switch to the
    // if-based wrapping strategy, using a per-line `fIdx` counter.
    let dlt8 = cpp_of("-cpp -dlt 8");
    assert!(
        dlt8.contains("fIdx"),
        "-dlt 8 must switch the long delay line to IfWrapping:\n{dlt8}"
    );

    // `-vec`: vector mode rejects `-os` regardless of backend (mirrors
    // `unsupported_backends_and_vector_mode_still_reject`); this only
    // triggers if `-vec` actually reached `ComputeMode::Vector`.
    let err = Compiler::new()
        .generate_aux_files(&GenerateAuxFilesRequest {
            source_name: "exec_options_test.dsp".to_owned(),
            source: SLIDER_GAIN.to_owned(),
            args: "-cpp -vec -os".to_owned(),
            ..GenerateAuxFilesRequest::default()
        })
        .expect_err("-vec -os must fail through generate_aux_files");
    assert!(err.to_string().contains("scalar mode"), "{err}");

    // `-ss`: every documented decode bucket (0/1/2/n>=3) must still compile.
    for ss in ["0", "1", "2", "3", "42"] {
        cpp_of(&format!("-cpp -ss {ss}"));
    }
}

// ── The canonical-block-`compute` contract, driven by the capability table ───

/// How to emit one backend under `-ec -os`, and how to recognise what it emits.
///
/// Both signature fragments are needed. `block_compute` alone cannot tell a
/// backend that correctly omits the canonical block entry point from a probe
/// that is simply looking for the wrong string — so `per_sample` is asserted
/// present on every backend, which makes a mis-typed probe fail loudly instead
/// of quietly agreeing with a `canonical_compute_required: false` row.
struct ComputeProbe {
    /// Emits this backend's `-ec -os` output.
    emit: fn() -> String,
    /// Appears exactly when the canonical *block* `compute` — the one taking a
    /// sample count and channel arrays — is emitted.
    block_compute: &'static str,
    /// Appears exactly when the per-sample entry point is emitted.
    per_sample: &'static str,
}

/// Returns the probe for a backend that accepts `-os`.
///
/// Panics on an unknown identifier, which is the fail-closed half of the
/// contract: a new capability row claiming `-os` support cannot reach the test
/// below without someone stating what its canonical `compute` looks like.
fn compute_probe(backend: &str) -> ComputeProbe {
    const ECOS: (ControlRateMode, ProcessingApi) =
        (ControlRateMode::External, ProcessingApi::OneSample);
    match backend {
        "cpp" => ComputeProbe {
            emit: || compile_cpp(ECOS.0, ECOS.1),
            block_compute: "virtual void compute(int count",
            per_sample: "virtual void frame(",
        },
        "c" => ComputeProbe {
            emit: || compile_c(ECOS.0, ECOS.1),
            block_compute: "void computemydsp(mydsp* dsp, int count",
            per_sample: "void framemydsp(",
        },
        "rust" => ComputeProbe {
            emit: || compile_rust(ECOS.0, ECOS.1),
            block_compute: "pub fn compute(&mut self, _count: usize",
            per_sample: "pub fn frame(&mut self,",
        },
        "asc" => ComputeProbe {
            emit: || compile_asc(ECOS.0, ECOS.1),
            block_compute: "compute(count: i32, inputs: Array<StaticArray<f32>>",
            per_sample: "frame(inputs: StaticArray<f32>",
        },
        "fir" => ComputeProbe {
            emit: || compile_fir_text(ECOS.0, ECOS.1),
            block_compute: r#"DeclareFun { name: "compute""#,
            per_sample: r#"DeclareFun { name: "frame""#,
        },
        "codebox" => ComputeProbe {
            emit: compile_codebox,
            // Codebox has no canonical block entry point at all. If it ever
            // grew one it would have to take a sample count, like every other
            // backend's does; nothing else in the emitted file mentions one.
            block_compute: "count",
            // RNBO's per-sample entry: one argument per input, no count.
            per_sample: "function compute(i0) {",
        },
        unknown => panic!(
            "backend '{unknown}' has a capability row accepting '-os' but no \
             canonical-`compute` probe. Add one: state the signature fragment \
             that appears when it emits the canonical block `compute`, and the \
             one that appears when it emits the per-sample entry point."
        ),
    }
}

fn compile_fir_text(control: ControlRateMode, api: ProcessingApi) -> String {
    let out = Compiler::new()
        .with_control_rate_mode(control)
        .with_processing_api(api)
        .compile_source_to_fir_with_lane(
            "exec_options_test.dsp",
            SLIDER_GAIN,
            compiler::SignalFirLane::TransformFastLane,
        )
        .expect("fir lowering must succeed");
    fir::dump_fir(&out.store, out.module)
}

fn compile_codebox() -> String {
    // No execution options passed on purpose: codebox forces external control
    // and one-sample itself, which is what its `Intrinsic` capability means.
    Compiler::new()
        .compile_source_to_codebox(
            "exec_options_test.dsp",
            SLIDER_GAIN,
            &codegen::backends::codebox::CodeboxOptions::default(),
        )
        .expect("codebox compilation must succeed")
}

/// `canonical_compute_required` must describe what the backend actually emits.
///
/// Until this test existed the field was set on every row and read by nobody:
/// the contract it describes was enforced only by the four hand-written
/// per-backend tests above, which name their signatures directly and never
/// consult the table. A new row could therefore claim
/// `canonical_compute_required: true` and get no coverage at all.
///
/// Driving the loop from [`all_backend_execution_caps`] fixes the direction of
/// the dependency: the table decides which backends are checked, and
/// [`compute_probe`] panics on a row it does not know.
///
/// Scope: this checks that the entry point is *present or absent* as the row
/// claims. That it is present *and empty* under `-os` is checked, with exact
/// signatures, by `cpp_shapes_match_the_reference_contract`,
/// `c_shapes_match_the_reference_contract`, `rust_shapes_match_the_d3_contract`
/// and `asc_shapes_match_the_one_sample_contract`.
#[test]
fn canonical_compute_matches_every_capability_row() {
    let mut checked = Vec::new();
    for caps in compiler::execution::all_backend_execution_caps() {
        if !caps.one_sample.is_supported() {
            continue;
        }
        let probe = compute_probe(caps.backend);
        let text = (probe.emit)();
        assert!(
            text.contains(probe.per_sample),
            "`{}` emitted no per-sample entry point matching {:?} — the probe \
             is wrong or the backend regressed, and either way its \
             `block_compute` verdict cannot be trusted:\n{text}",
            caps.backend,
            probe.per_sample
        );
        assert_eq!(
            text.contains(probe.block_compute),
            caps.canonical_compute_required,
            "`{}` disagrees with its capability row: \
             canonical_compute_required = {}, but {:?} was {} in the output",
            caps.backend,
            caps.canonical_compute_required,
            probe.block_compute,
            if caps.canonical_compute_required {
                "absent"
            } else {
                "present"
            }
        );
        checked.push(caps.backend);
    }
    assert!(
        checked.contains(&"codebox"),
        "the only row with canonical_compute_required = false must be covered, \
         otherwise this test only ever asserts presence: {checked:?}"
    );
    assert!(
        checked.len() >= 5,
        "expected every `-os` backend to be probed, got {checked:?}"
    );
}
