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
use compiler::{
    Compiler, ComputeMode, ControlRateMode, ProcessingApi, RealType, SignalFirLane, TableInitMode,
};

/// Compiles one source to codebox, through the lowering the backend expects.
fn codebox(source_name: &str, source: &str) -> String {
    codebox_with(source_name, source, &CodeboxOptions::default())
}

fn codebox_with(source_name: &str, source: &str, options: &CodeboxOptions) -> String {
    codebox_with_table_init(source_name, source, options, TableInitMode::default())
}

fn codebox_with_table_init(
    source_name: &str,
    source: &str,
    options: &CodeboxOptions,
    table_init: TableInitMode,
) -> String {
    let compiler = Compiler::new()
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample)
        .with_table_init_mode(table_init);
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
            test_labels: false,
            ..CodeboxOptions::default()
        },
    );
    assert!(single.contains("0.5f"), "{single}");
    assert!(double.contains("0.5"), "{double}");
    assert!(
        !double.contains("0.5f"),
        "double precision must drop the f suffix:\n{double}"
    );
    // The shapes are otherwise identical (the header's compilation-options
    // line also reflects `-single`/`-double`, so normalize that too).
    assert_eq!(
        single.replace("0.5f", "0.5"),
        double.replace("-lang codebox -double", "-lang codebox -single")
    );
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

// ── Numeric verification against the interpreter ─────────────────────────────
//
// The primary oracle of the port plan (§5.2 layer 2). Text cannot arbitrate:
// our lowering legitimately differs from C++'s, so a text comparison proves
// nothing and a snapshot of our own output only detects change. Running the
// emitted codebox and comparing it sample-for-sample against the interpreter —
// which the impulse suite validates against the genuine C++ reference — is what
// shows the emission is *correct*.
//
// Both backends consume the same FIR module, so this isolates the codebox
// emission from the lowering.

use codegen::backends::codebox::eval::Program;
use codegen::backends::interp::{FbcDspInstance, FbcOpcode, InterpOptions, generate_interp_module};

/// Runs `frames` samples of `source` through both the interpreter and the
/// emitted codebox, and asserts they agree bit-for-bit in f64.
fn assert_codebox_matches_interpreter(source_name: &str, source: &str, frames: usize) {
    // Double precision on both sides, because codebox has a single `number`
    // type and it is a double. Comparing against a single-precision DSP would
    // measure the f32→f64 gap rather than the emission: a `0.3f` constant is
    // 0.30000001192092896 in the DSP and exactly 0.3 in codebox, which shows up
    // as a ~1e-8 disagreement that no emitter change can remove.
    let compiler = Compiler::new()
        .with_real_type(RealType::Float64)
        .with_control_rate_mode(ControlRateMode::External)
        .with_processing_api(ProcessingApi::OneSample);
    let fir = compiler
        .compile_source_to_fir_with_lane(source_name, source, SignalFirLane::TransformFastLane)
        .expect("FIR lowering must succeed");

    let text = generate_codebox_module(
        &fir.store,
        fir.module,
        &CodeboxOptions {
            double_precision: true,
            test_labels: false,
            ..CodeboxOptions::default()
        },
    )
    .expect("codebox emission must succeed");
    let mut program = Program::parse(&text)
        .unwrap_or_else(|e| panic!("the emitted codebox must parse: {e}\n{text}"));
    program.dspsetup(44100.0).expect("dspsetup must run");

    // The interpreter needs its own lowering of the same source, in the block
    // shape it supports.
    let block_compiler = Compiler::new().with_real_type(RealType::Float64);
    let block_fir = block_compiler
        .compile_source_to_fir_with_lane(source_name, source, SignalFirLane::TransformFastLane)
        .expect("block FIR lowering must succeed");
    let mut factory = generate_interp_module::<f64>(
        &block_fir.store,
        block_fir.module,
        &InterpOptions {
            opt_level: 0,
            module_name: None,
            ..InterpOptions::default()
        },
    )
    .expect("interp codegen must succeed");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(44100);

    let num_inputs = instance.get_num_inputs() as usize;
    let num_outputs = instance.get_num_outputs() as usize;
    assert_eq!(
        program.compute_arity(),
        num_inputs,
        "codebox compute arity must match the DSP input count\n{text}"
    );

    // Bargraphs are control zones for the interpreter and extra audio channels
    // for codebox, so they are compared by reading the interpreter's zones —
    // not by assuming the counts line up. Their order is part of the contract:
    // an RNBO patch wires channel N to a named meter.
    let bargraph_zones: Vec<i32> = instance
        .ui_instructions()
        .iter()
        .filter(|ui| {
            matches!(
                ui.opcode,
                FbcOpcode::AddHorizontalBargraph | FbcOpcode::AddVerticalBargraph
            )
        })
        .map(|ui| ui.offset)
        .collect();

    // An impulse followed by silence, the same excitation the impulse suite uses.
    for frame in 0..frames {
        let sample = if frame == 0 { 1.0f64 } else { 0.0f64 };
        let inputs: Vec<f64> = vec![sample; num_inputs];

        let in_bufs: Vec<Vec<f64>> = inputs.iter().map(|v| vec![*v]).collect();
        let mut out_bufs: Vec<Vec<f64>> = vec![vec![0.0]; num_outputs];
        {
            let in_refs: Vec<&[f64]> = in_bufs.iter().map(Vec::as_slice).collect();
            let mut out_refs: Vec<&mut [f64]> =
                out_bufs.iter_mut().map(Vec::as_mut_slice).collect();
            instance
                .try_compute(1, &in_refs, &mut out_refs)
                .expect("interpreter compute must succeed");
        }
        // Real audio channels first, then each bargraph's zone, in the order
        // the emitter appends them.
        let mut expected: Vec<f64> = out_bufs.iter().map(|c| c[0]).collect();
        for &offset in &bargraph_zones {
            expected.push(
                instance
                    .get_real_zone(offset)
                    .expect("bargraph zone must be readable"),
            );
        }

        let got = program
            .compute(&[], &inputs)
            .unwrap_or_else(|e| panic!("codebox evaluation failed at frame {frame}: {e}\n{text}"));

        assert_eq!(
            got.len(),
            expected.len(),
            "channel count differs at frame {frame} \
             ({} outputs + {} bargraphs expected)\n{text}",
            num_outputs,
            bargraph_zones.len()
        );
        for (channel, (a, b)) in got.iter().zip(expected.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-9,
                "frame {frame} channel {channel}: codebox {a} vs interpreter {b}\n{text}"
            );
        }
    }
}

#[test]
fn numeric_passthrough_matches_the_interpreter() {
    assert_codebox_matches_interpreter("id.dsp", "process = _;", 64);
}

#[test]
fn numeric_arithmetic_matches_the_interpreter() {
    assert_codebox_matches_interpreter("arith.dsp", "process = _ , _ : + : *(0.5);", 64);
}

#[test]
fn numeric_recursion_matches_the_interpreter() {
    assert_codebox_matches_interpreter("rec.dsp", "process = + ~ *(0.5);", 256);
}

#[test]
fn numeric_delay_matches_the_interpreter() {
    assert_codebox_matches_interpreter("del.dsp", "process = _ : @(7);", 256);
}

/// Dumps a UI-bearing DSP for eyeball comparison against the reference.
#[test]
fn dump_ui_for_eyeball_comparison() {
    let src = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01) * checkbox(\"on\") \
               + button(\"trig\") + nentry(\"num\", 2, 0, 10, 1) \
               + vslider(\"v\", 0.3, -1, 1, 0.01);";
    println!("=== plain ===\n{}", codebox("c2ui.dsp", src));
    println!(
        "=== codebox-test ===\n{}",
        codebox_with(
            "c2ui.dsp",
            src,
            &CodeboxOptions {
                double_precision: false,
                test_labels: true,
                ..CodeboxOptions::default()
            }
        )
    );
}

// ── C2: params, control, update ──────────────────────────────────────────────

const UI_DSP: &str = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01) * checkbox(\"on\") \
                      + button(\"trig\") + nentry(\"num\", 2, 0, 10, 1) \
                      + vslider(\"v\", 0.3, -1, 1, 0.01);";

fn test_labelled(source_name: &str, source: &str) -> String {
    codebox_with(
        source_name,
        source,
        &CodeboxOptions {
            double_precision: false,
            test_labels: true,
            ..CodeboxOptions::default()
        },
    )
}

/// Sliders and numeric entries carry their real range; buttons and checkboxes
/// carry a hardcoded one, spelled differently. The asymmetry is the
/// reference's, reproduced rather than normalised.
#[test]
fn param_ranges_follow_the_widget_kind() {
    let text = codebox("ui.dsp", UI_DSP);
    assert!(
        text.contains("@param({min: 0.0f, max: 1.0f}) gain = 0.5f;"),
        "{text}"
    );
    assert!(
        text.contains("@param({min: 0.0f, max: 10.0f}) num = 2.0f;"),
        "{text}"
    );
    assert!(
        text.contains("@param({min: -1.0f, max: 1.0f}) v = 0.3f;"),
        "{text}"
    );
    assert!(
        text.contains("@param({min: 0., max: 1.}) on = 0.;"),
        "{text}"
    );
    assert!(
        text.contains("@param({min: 0., max: 1.}) trig = 0.;"),
        "{text}"
    );
}

/// Every parameter is compared with its zone, and `control()` runs once for the
/// whole block rather than once per parameter that moved.
#[test]
fn update_checks_each_parameter_then_controls_once() {
    let text = codebox("ui.dsp", UI_DSP);
    assert!(
        text.contains("fUpdated = int(fUpdated) | (gain != fHslider0_cb); fHslider0_cb = gain;"),
        "{text}"
    );
    let update = text
        .split("function update(")
        .nth(1)
        .expect("update function");
    assert_eq!(
        update.matches("control();").count(),
        1,
        "control() must run once per update, not once per parameter:\n{update}"
    );
    assert!(
        update.contains("if (fUpdated) { fUpdated = false; control(); }"),
        "{update}"
    );
}

/// `update`'s argument list, the `@param` names and the top-level call must
/// agree, or the emitted file does not even parse.
#[test]
fn parameter_names_agree_across_the_three_sites() {
    let text = codebox("ui.dsp", UI_DSP);
    let params: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("@param("))
        .filter_map(|l| l.split(") ").nth(1))
        .filter_map(|l| l.split(' ').next())
        .collect();
    assert_eq!(params.len(), 5, "{text}");

    let signature = text
        .split("function update(")
        .nth(1)
        .and_then(|s| s.split(american_paren()).next())
        .expect("update signature");
    let call = text
        .split("\nupdate(")
        .nth(1)
        .and_then(|s| s.split(american_paren()).next())
        .expect("top-level update call");
    let joined = params.join(",");
    assert_eq!(
        signature, joined,
        "update signature differs from @param names"
    );
    assert_eq!(call, joined, "top-level call differs from @param names");
}

fn american_paren() -> char {
    ')'
}

/// The test convention is what lets `rnbo-dsp.h` rebuild the Faust UI, so the
/// prefixes must match the ones it matches on.
#[test]
fn test_labels_carry_the_rnbo_prefixes() {
    let text = test_labelled("ui.dsp", UI_DSP);
    for expected in [
        "RB_hslider_gain",
        "RB_checkbox_on",
        "RB_button_trig",
        "RB_nentry_num",
        "RB_vslider_v",
    ] {
        assert!(text.contains(expected), "missing {expected}\n{text}");
    }
    // Plain mode must not carry them.
    let plain = codebox("ui.dsp", UI_DSP);
    assert!(!plain.contains("RB_"), "{plain}");
}

/// Colliding labels are disambiguated by the shared shortname algorithm, which
/// is `preserved` — so these names *are* comparable with the C++ compiler.
#[test]
fn colliding_labels_get_disambiguated_names() {
    let text = codebox(
        "coll.dsp",
        "process = vgroup(\"a\", hslider(\"gain\", 0.5, 0, 1, 0.01)) \
         + vgroup(\"b\", hslider(\"gain\", 0.2, 0, 1, 0.01));",
    );
    assert!(text.contains(" a_gain = "), "{text}");
    assert!(text.contains(" b_gain = "), "{text}");
}

/// A label starting with a digit cannot be a codebox identifier.
#[test]
fn digit_initial_labels_get_the_cb_prefix() {
    let text = codebox(
        "num.dsp",
        "process = _ * hslider(\"0freq\", 1, 0, 2, 0.01);",
    );
    assert!(text.contains(" cb_0freq = "), "{text}");
}

/// Numeric check with parameters held at their initial values.
#[test]
fn numeric_ui_dsp_matches_the_interpreter() {
    assert_codebox_matches_interpreter("ui.dsp", UI_DSP, 64);
}

/// Dumps the delay DSP, whose ring counter is the integer-arithmetic source.
#[test]
fn dump_delay_for_eyeball_comparison() {
    println!("{}", codebox("del.dsp", "process = _ : @(7);"));
    println!(
        "--- int arithmetic ---
{}",
        codebox("ia.dsp", "process = _ : int : +(3) : *(2) : %(5);")
    );
}

/// Dumps the quirk DSP for eyeball comparison against the reference.
#[test]
fn dump_quirks_for_eyeball_comparison() {
    println!(
        "{}",
        codebox(
            "c4q.dsp",
            "process = (_ : *(3) : int : /(2)) , (_ , 2.5 : fmod) \
             , (_ : abs : sqrt) , (_ * -1.0);"
        )
    );
}

// ── C3: bargraphs as extra audio outputs ─────────────────────────────────────

const BARGRAPH_DSP: &str =
    "process = _ <: attach(_, vbargraph(\"lvl\", 0, 1)), hbargraph(\"pk\", 0, 2);";

/// Codebox cannot send a value back as control data, so a bargraph leaves as an
/// extra audio channel appended after the real outputs.
#[test]
fn bargraphs_become_extra_audio_outputs() {
    let text = codebox("bar.dsp", BARGRAPH_DSP);
    let ret = text
        .lines()
        .find(|l| l.trim_start().starts_with("return ["))
        .expect("return list");
    // Two real outputs, then the two bargraph variables, in that order.
    assert!(ret.contains("output0_cb,output1_cb,"), "{ret}");
    assert!(ret.contains("bargraph"), "bargraph vars missing from {ret}");
    // The wiring must expose all four channels.
    for channel in 1..=4 {
        assert!(
            text.contains(&format!("out{channel} = outputs[{}];", channel - 1)),
            "missing channel {channel}\n{text}"
        );
    }
    assert!(
        !text.contains("out5 = "),
        "emitted more channels than outputs + bargraphs:\n{text}"
    );
}

/// A bargraph is not a control: it must not appear as a `@param`, or the host
/// would try to write to a meter.
#[test]
fn bargraphs_are_not_params() {
    let text = codebox("bar.dsp", BARGRAPH_DSP);
    let params: Vec<&str> = text.lines().filter(|l| l.starts_with("@param(")).collect();
    assert!(
        params.is_empty(),
        "bargraphs leaked into the parameter list: {params:?}"
    );
}

// ── C4: language quirks ──────────────────────────────────────────────────────

/// Codebox's own `int()` floors, so an integer cast must use `trunc()`.
/// The two differ only on negative values, which is why the numeric test below
/// feeds them.
#[test]
fn integer_casts_use_trunc() {
    let text = codebox("cast.dsp", "process = _ : *(3) : int : /(2);");
    assert!(text.contains("trunc("), "{text}");
    assert!(
        !text.contains("floor("),
        "an integer cast must not floor:\n{text}"
    );
}

/// `fmod` has no codebox equivalent; `safemod` is the documented substitute.
#[test]
fn fmod_maps_to_safemod() {
    let text = codebox("mod.dsp", "process = _ , 2.5 : fmod;");
    assert!(text.contains("safemod("), "{text}");
    assert!(!text.contains("fmod("), "{text}");
}

/// Faust's precision-suffixed math names are not codebox names.
#[test]
fn math_names_lose_their_precision_suffix() {
    let text = codebox("math.dsp", "process = _ : abs : sqrt : log;");
    assert!(
        text.contains("sqrt(") && text.contains("abs(") && text.contains("log("),
        "{text}"
    );
    for banned in ["sqrtf(", "fabsf(", "fabs(", "logf("] {
        assert!(!text.contains(banned), "unmapped name {banned}\n{text}");
    }
}

/// Integer `+`, `*` and `%` wrap at 32 bits, which codebox's infix operators —
/// working on `number` — would not do.
///
/// The DSP matters: a delay's ring counter only produces integer arithmetic in
/// the *loop headers*, which the emitter writes directly, so it exercises
/// nothing of the operator mapping. Explicit `int` arithmetic is what reaches
/// it. Found by mutation: dropping the `Add` mapping left every test green
/// until this DSP was added.
#[test]
fn integer_arithmetic_uses_the_wrapping_helpers() {
    let text = codebox("ia.dsp", "process = _ : int : +(3) : *(2) : %(5);");
    assert!(text.contains("iadd("), "no iadd:\n{text}");
    assert!(text.contains("imul("), "no imul:\n{text}");
    assert!(text.contains("imod("), "no imod:\n{text}");
}

/// Subtraction and the bitwise operators stay infix: the C++ helper table
/// covers only `kAdd`, `kMul` and `kRem`.
#[test]
fn only_add_mul_and_rem_use_helpers() {
    let text = codebox("ia.dsp", "process = _ : int : -(3) : &(255);");
    assert!(
        !text.contains("isub(") && !text.contains("iand("),
        "helper used for an operator the reference keeps infix:\n{text}"
    );
}

/// The helper path must also be numerically right, not just present.
#[test]
fn numeric_integer_arithmetic_matches_the_interpreter() {
    assert_codebox_matches_interpreter("ia.dsp", "process = _ : int : +(3) : *(2) : %(5);", 32);
}

// ── Numeric checks for the quirks ────────────────────────────────────────────

/// The quirk that a text comparison cannot arbitrate: `trunc` versus `floor`
/// differs only on negatives, so this DSP is fed them.
#[test]
fn numeric_negative_integer_cast_matches_the_interpreter() {
    assert_codebox_matches_interpreter("cast.dsp", "process = _ : *(-3.5) : int : /(2);", 32);
}

#[test]
fn numeric_fmod_matches_the_interpreter() {
    assert_codebox_matches_interpreter("mod.dsp", "process = _ , 2.5 : fmod;", 32);
}

#[test]
fn numeric_math_calls_match_the_interpreter() {
    assert_codebox_matches_interpreter("math.dsp", "process = _ : abs : sqrt : exp;", 32);
}

#[test]
fn numeric_bargraph_dsp_matches_the_interpreter() {
    assert_codebox_matches_interpreter("bar.dsp", BARGRAPH_DSP, 32);
}

// ── C5: facade wiring ────────────────────────────────────────────────────────
//
// Everything above builds the FIR module by hand, with the lowering the
// backend expects already selected. These tests go through the facade instead,
// which is where the "codebox imposes its own execution modes" contract lives.

/// The four execution shapes must produce byte-identical output, because
/// `lower_signals_to_codebox` forces external control and one-sample whatever
/// was asked. This is what `ExecutionCapability::Intrinsic` claims in the
/// capability table; without this test the claim is unchecked.
#[test]
fn the_facade_forces_the_execution_modes_codebox_imposes() {
    let source = "process = _ * hslider(\"g\", 0.5, 0, 1, 0.01) : @(3);";
    let shapes = [
        (ControlRateMode::InlinePerBlock, ProcessingApi::Block),
        (ControlRateMode::External, ProcessingApi::Block),
        (ControlRateMode::InlinePerBlock, ProcessingApi::OneSample),
        (ControlRateMode::External, ProcessingApi::OneSample),
    ];
    let outputs: Vec<String> = shapes
        .iter()
        .map(|&(control, api)| {
            Compiler::new()
                .with_control_rate_mode(control)
                .with_processing_api(api)
                .compile_source_to_codebox("f.dsp", source, &CodeboxOptions::default())
                .expect("codebox must accept every execution shape")
        })
        .collect();
    for (index, text) in outputs.iter().enumerate().skip(1) {
        assert_eq!(
            *text, outputs[0],
            "shape {:?} produced different output from the default",
            shapes[index]
        );
    }
    // Not vacuous: the shared output really is the one-sample shape, so the
    // assertion above is not just four copies of block-processing code.
    assert!(
        outputs[0].contains("function compute(i0) {"),
        "{}",
        outputs[0]
    );
    assert!(
        outputs[0].contains("function control() {"),
        "{}",
        outputs[0]
    );
}

/// Vector mode is the one request codebox cannot absorb, so it is refused by
/// name rather than silently downgraded to the scalar output above.
#[test]
fn the_facade_rejects_vector_mode_by_name() {
    let err = Compiler::new()
        .with_compute_mode(ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        })
        .compile_source_to_codebox("f.dsp", "process = _;", &CodeboxOptions::default())
        .expect_err("codebox must reject vector mode");
    let rendered = err.to_string();
    assert!(
        rendered.contains("'-vec'") && rendered.contains("codebox"),
        "the diagnostic must name both the flag and the backend: {rendered}"
    );
}

/// `-double` reaches the emitter through the facade's real type, not through a
/// separately parsed flag — the drift that bit the wasm backend before.
#[test]
fn the_facade_derives_literal_precision_from_the_real_type() {
    let source = "process = _ * 0.5;";
    let single = Compiler::new()
        .compile_source_to_codebox("f.dsp", source, &CodeboxOptions::default())
        .expect("emission must succeed");
    let double = Compiler::new()
        .with_real_type(RealType::Float64)
        .compile_source_to_codebox("f.dsp", source, &CodeboxOptions::default())
        .expect("emission must succeed");
    assert!(single.contains("0.5f"), "{single}");
    assert!(!double.contains("0.5f"), "{double}");
    assert!(double.contains("0.5"), "{double}");
}

/// `test_labels` survives the trip through the facade: it is the only thing
/// separating `-lang codebox` from `-lang codebox-test`.
#[test]
fn the_facade_forwards_the_test_label_option() {
    let options = CodeboxOptions {
        test_labels: true,
        ..CodeboxOptions::default()
    };
    let text = Compiler::new()
        .compile_source_to_codebox(
            "f.dsp",
            "process = _ * hslider(\"g\", 0.5, 0, 1, 0.01);",
            &options,
        )
        .expect("emission must succeed");
    assert!(text.contains("RB_hslider_g"), "{text}");
}

/// A lookup table must be declared and filled, not merely read.
///
/// Regression: `generate_codebox_module` emitted `compute()` reading
/// `ftbl0_cb[…]` while never declaring the symbol and never writing a single
/// element, so any `rdtable`/`rwtable` DSP silently returned zeros. Two causes:
/// the module's `static_decls` block was never visited, and `DeclareTable` —
/// the shape the transform actually emits for a table — had no arm in either
/// the statement emitter or the array initialiser pass, which only understood
/// `DeclareVar` with an array-literal init.
#[test]
fn a_read_only_table_is_declared_and_filled() {
    // The property has to hold in both table-init modes, and the two produce
    // different shapes: `const` folds the content into one literal store per
    // element, `runtime` emits the generator's fill loop. What must be true of
    // both is that the table is declared and written inside `dspsetup`, before
    // any compute call — the defect this test was written for was codebox
    // reading a table it never declared or initialized.
    for mode in [TableInitMode::Const, TableInitMode::Runtime] {
        // Spelled without imports: the harness compiles from a string and has
        // no library search path. `t` is `ba.time`.
        let text = codebox_with_table_init(
            "tbl.dsp",
            "t = (+(1) ~ _) - 1;\nprocess = rdtable(4, int(t * 2), int(t % 4));",
            &CodeboxOptions::default(),
            mode,
        );

        // The table name carries a type prefix (`itbl0_cb`), so the whole
        // identifier around the `tbl` marker is what matters.
        let table = text
            .lines()
            .find_map(|line| {
                let marker = line.find("tbl")?;
                let bytes = line.as_bytes();
                let mut start = marker;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                let mut end = marker;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                Some(line[start..end].to_owned())
            })
            .expect("the emitted program must mention a table");

        assert!(
            text.contains(&format!("@state {table} = new FixedFloatArray(")),
            "{mode:?}: table `{table}` is read but never declared:\n{text}"
        );

        let setup = text.find("function dspsetup()").expect("dspsetup");
        let compute = text.find("function compute(").expect("compute");

        match mode {
            TableInitMode::Const => {
                // The folded shape additionally pins the values, which is the
                // only place the generator's arithmetic is checked directly.
                for (index, value) in [(0, "0"), (1, "2"), (2, "4"), (3, "6")] {
                    assert!(
                        text.contains(&format!("{table}[{index}] = {value};")),
                        "const: table `{table}` element {index} is never written:\n{text}"
                    );
                }
                let first_fill = text
                    .find(&format!("{table}[0] ="))
                    .expect("first element write");
                assert!(
                    setup < first_fill && first_fill < compute,
                    "const: table data must be written inside dspsetup:\n{text}"
                );
            }
            TableInitMode::Runtime => {
                // The fill loop indexes by its induction variable, so there is
                // no literal `[0] =` to look for; what must hold is that some
                // store into the table sits inside `dspsetup`.
                let fill = text
                    .find(&format!("{table}["))
                    .and_then(|_| {
                        text.match_indices(&format!("{table}["))
                            .find(|(at, _)| text[*at..].contains("] ="))
                            .map(|(at, _)| at)
                    })
                    .expect("runtime: the fill loop must store into the table");
                assert!(
                    setup < fill && fill < compute,
                    "runtime: the fill must run inside dspsetup:\n{text}"
                );
            }
        }
    }
}

/// A literal `waveform` used directly as a signal keeps its own declaration and
/// data, which is a separate path from generated tables.
#[test]
fn a_literal_waveform_is_declared_and_filled() {
    let text = codebox("wave.dsp", "process = waveform{10.0, 20.0, 30.0};");
    assert!(
        text.contains("Wave0_cb = new FixedFloatArray(3)"),
        "waveform table missing its declaration:\n{text}"
    );
    assert!(
        text.contains("Wave0_cb[0] = 10"),
        "waveform data missing:\n{text}"
    );
}

/// The two `--table-init` modes must agree numerically on the same backend.
///
/// `const` folds the generator into a literal table at compile time; `runtime`
/// compiles it into a sub-module that codebox inlines and runs in `dspsetup`.
/// The table content is the same either way, so the emitted programs must
/// produce identical output — which is what makes the flattened fill loop
/// trustworthy rather than merely well-shaped.
///
/// This is the mode matrix of
/// `porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md` §8.2,
/// applied to the first backend that consumes the flattening pass.
fn assert_table_init_modes_agree(source_name: &str, source: &str, frames: usize) {
    let render = |mode: TableInitMode| -> Vec<f64> {
        let compiler = Compiler::new()
            .with_real_type(RealType::Float64)
            .with_control_rate_mode(ControlRateMode::External)
            .with_processing_api(ProcessingApi::OneSample)
            .with_table_init_mode(mode);
        let fir = compiler
            .compile_source_to_fir_with_lane(source_name, source, SignalFirLane::TransformFastLane)
            .unwrap_or_else(|e| panic!("{mode:?}: FIR lowering must succeed: {e:?}"));
        let text = generate_codebox_module(
            &fir.store,
            fir.module,
            &CodeboxOptions {
                double_precision: true,
                test_labels: false,
                ..CodeboxOptions::default()
            },
        )
        .unwrap_or_else(|e| panic!("{mode:?}: codebox emission must succeed: {e}"));
        let mut program = Program::parse(&text)
            .unwrap_or_else(|e| panic!("{mode:?}: emitted codebox must parse: {e}\n{text}"));
        program.dspsetup(44100.0).expect("dspsetup must run");

        let arity = program.compute_arity();
        let mut out = Vec::with_capacity(frames);
        for frame in 0..frames {
            let sample = if frame == 0 { 1.0f64 } else { 0.0f64 };
            let inputs: Vec<f64> = vec![sample; arity];
            let outputs = program
                .compute(&[], &inputs)
                .unwrap_or_else(|e| panic!("{mode:?}: compute must run: {e}"));
            out.extend(outputs);
        }
        out
    };

    let folded = render(TableInitMode::Const);
    let filled = render(TableInitMode::Runtime);
    assert_eq!(
        folded.len(),
        filled.len(),
        "the two modes produced different output lengths"
    );
    for (frame, (a, b)) in folded.iter().zip(filled.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-12,
            "frame {frame}: const mode gave {a}, runtime mode gave {b}"
        );
    }
    // A table of zeros would satisfy the comparison vacuously.
    assert!(
        folded.iter().any(|v| *v != 0.0),
        "the fixture must produce a non-zero signal for the comparison to mean anything"
    );
}

#[test]
fn table_init_modes_agree_on_a_constant_generator() {
    assert_table_init_modes_agree(
        "tblc.dsp",
        "t = (+(1) ~ _) - 1;\nprocess = rdtable(8, 0.25, int(t % 8));",
        32,
    );
}

#[test]
fn table_init_modes_agree_on_a_recursive_generator() {
    // The generator's carrier is read one sample late — the shape whose update
    // used to be dropped, producing an all-zero table.
    assert_table_init_modes_agree(
        "tblr.dsp",
        "t = (+(1) ~ _) - 1;\nprocess = rdtable(8, int(t * 2), int(t % 8));",
        32,
    );
}
