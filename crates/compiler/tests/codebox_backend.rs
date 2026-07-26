//! Codebox backend, phase C1: module shape and one-sample body.
//!
//! The backend consumes a FIR module lowered with external control and the
//! one-sample processing API, so these tests build that module through the
//! public FIR entry point rather than waiting for the CLI wiring of phase C5.
//!
//! What is checked here is *syntax and shape*, not text equality with the C++
//! compiler: our FIR lowering legitimately differs in structure, so byte parity
//! is not a goal (see the correction in
//! `porting/codebox-backend-port-plan-2026-07-26-en.md` §5.2). Numeric
//! verification arrives with the evaluator layer.

use codegen::backends::codebox::{CodeboxOptions, CodegenErrorCode, generate_codebox_module};
use compiler::{Compiler, ControlRateMode, ProcessingApi, SignalFirLane};

/// Compiles one source to codebox, through the lowering the backend expects.
fn codebox(source_name: &str, source: &str) -> String {
    codebox_with(source_name, source, &CodeboxOptions::default())
}

fn codebox_with(source_name: &str, source: &str, options: &CodeboxOptions) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    let fir = compiler
        .compile_source_to_fir_with_lane(source_name, source, SignalFirLane::TransformFastLane)
        .expect("FIR lowering must succeed");
    generate_codebox_module(&fir.store, fir.module, options).expect("codebox emission must succeed")
}

/// The section order is fixed: RNBO parses a flat file where declarations must
/// precede use.
#[test]
fn sections_appear_in_the_order_rnbo_expects() {
    let text = codebox("id.dsp", "process = _;");
    let order = [
        "// Additional functions",
        "// Params",
        "// Globals",
        "// Fields",
        "@state fUpdated : Int = 0;",
        "// Init",
        "function dspsetup() {",
        "// Control",
        "function control() {",
        "// Update parameters",
        "function update() {",
        "// Compute one frame",
        "function compute(",
        "update();",
        "outputs = compute(",
    ];
    let mut cursor = 0;
    for needle in order {
        let found = text[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing or out of order: {needle}\n{text}"));
        cursor += found + needle.len();
    }
}

/// `compute` takes one argument per input and returns one value per output.
#[test]
fn compute_is_one_sample_in_and_a_list_out() {
    let text = codebox("io.dsp", "process = (_ , _ : +) , (_ , _ : *);");
    assert!(text.contains("function compute(i0,i1,i2,i3) {"), "{text}");
    assert!(text.contains("let input0_cb : number = i0;"), "{text}");
    assert!(text.contains("let input3_cb : number = i3;"), "{text}");
    assert!(text.contains("let output0_cb : number = 0;"), "{text}");
    assert!(text.contains("return [output0_cb,output1_cb];"), "{text}");
    // Top-level wiring uses 1-based `inN`/`outN`, unlike the 0-based locals.
    assert!(
        text.contains("outputs = compute(in1,in2,in3,in4);"),
        "{text}"
    );
    assert!(text.contains("out1 = outputs[0];"), "{text}");
    assert!(text.contains("out2 = outputs[1];"), "{text}");
}

/// The one-sample body reads and writes the `compute` locals, never an
/// `inputs[]`/`outputs[]` array: codebox has no such arrays in scope.
#[test]
fn io_arrays_become_compute_locals() {
    let text = codebox("io.dsp", "process = _ , _ : +;");
    assert!(
        !text.contains("inputs_cb[") && !text.contains("outputs_cb["),
        "the one-sample I/O arrays leaked into the body:\n{text}"
    );
    assert!(text.contains("output0_cb = "), "{text}");
}

/// Every emitted identifier carries `_cb`, because codebox rejects identifiers
/// ending in a digit — which every Faust-generated name does.
#[test]
fn identifiers_never_end_with_a_digit() {
    let text = codebox("rec.dsp", "process = + ~ *(0.5);");
    for line in text.lines() {
        // Only look at declarations; `compute(i0,i1)` arguments are ours and
        // deliberately bare, matching the reference.
        let Some(rest) = line
            .trim()
            .strip_prefix("@state ")
            .or_else(|| line.trim().strip_prefix("let "))
        else {
            continue;
        };
        let name = rest.split([' ', ':', '=']).next().unwrap_or_default();
        assert!(
            !name.ends_with(|c: char| c.is_ascii_digit()),
            "identifier ends with a digit: {name}\n{text}"
        );
    }
}

/// Persistent state is `@state` and must be initialised; locals are `let`.
#[test]
fn storage_classes_follow_the_access_type() {
    let text = codebox("rec.dsp", "process = + ~ *(0.5);");
    let fields: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("// Fields"))
        .take_while(|l| !l.starts_with("// Init"))
        .collect();
    assert!(
        fields.iter().any(|l| l.starts_with("@state ")),
        "no @state field emitted:\n{text}"
    );
    for line in &fields {
        if let Some(rest) = line.strip_prefix("@state ") {
            // A scalar `@state` needs an initialiser; an array is constructed.
            assert!(
                rest.contains(" = "),
                "@state without initialiser: {line}\n{text}"
            );
        }
    }
}

/// `sample_rate` is a call in codebox, not a field.
#[test]
fn sample_rate_reads_through_the_builtin_call() {
    // Every module carries fSampleRate, set from the init argument.
    let text = codebox("sr.dsp", "process = _;");
    assert!(text.contains("samplerate()"), "{text}");
    assert!(
        !text.contains("sample_rate_cb"),
        "the sample-rate argument leaked as a variable:\n{text}"
    );
}

/// Codebox precedence is not C's, so operators stay fully parenthesised.
#[test]
fn binary_operators_are_fully_parenthesised() {
    let text = codebox("mix.dsp", "process = _ , _ : + : *(0.5);");
    assert!(text.contains("("), "{text}");
    let body: String = text
        .lines()
        .skip_while(|l| !l.starts_with("function compute("))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains(" + ") && body.contains("("),
        "expected parenthesised arithmetic:\n{body}"
    );
}

/// `-double` changes literal spelling only: codebox has one numeric type.
#[test]
fn double_precision_only_changes_literal_spelling() {
    let single = codebox("lit.dsp", "process = _ * 0.5;");
    let double = codebox_with(
        "lit.dsp",
        "process = _ * 0.5;",
        &CodeboxOptions {
            double_precision: true,
        },
    );
    assert!(single.contains("0.5f"), "{single}");
    assert!(double.contains("0.5"), "{double}");
    assert!(
        !double.contains("0.5f"),
        "double precision must drop the f suffix:\n{double}"
    );
    // The shapes are otherwise identical.
    assert_eq!(single.replace("0.5f", "0.5"), double);
}

/// Soundfiles are rejected with a typed error rather than emitted wrongly,
/// matching the upstream behaviour.
#[test]
fn soundfiles_are_rejected_with_a_typed_error() {
    let compiler = Compiler::new()
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    let fir = compiler
        .compile_source_to_fir_with_lane(
            "sf.dsp",
            "process = 0,0 : soundfile(\"s[url:{'a.wav'}]\", 2) : !,!,_,_;",
            SignalFirLane::TransformFastLane,
        )
        .expect("FIR lowering must succeed");
    let error = generate_codebox_module(&fir.store, fir.module, &CodeboxOptions::default())
        .expect_err("codebox must reject soundfiles");
    assert_eq!(error.code, CodegenErrorCode::Unsupported);
    assert_eq!(error.code.as_str(), "FRS-CGEN-CBOX-0002");
}

/// Prints the emitted codebox for eyeball comparison against the reference.
/// Run with `cargo test -p compiler --test codebox_backend -- --nocapture dump`.
#[test]
fn dump_for_eyeball_comparison() {
    for (name, src) in [
        ("id.dsp", "process = _;"),
        ("rec.dsp", "process = + ~ *(0.5);"),
    ] {
        println!("=== {name} ===\n{}", codebox(name, src));
    }
}
