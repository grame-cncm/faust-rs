//! `cinputs`, `cinput`, `coutputs`, `coutput` and the wildcard modulation
//! target `"*"`: faust-rs extensions, no C++ equivalent.
//!
//! Contract: `porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`
//! section 3. The programs are the fixtures `tests/corpus/cinputs_*.dsp`,
//! `tests/corpus/wildcard_*.dsp` and `tests/corpus/err_3{0,1}_*.dsp`, each
//! stating its expected outputs in its header; they are run here through the
//! interpreter fast lane:
//! - the list order is the order of the program's own interface (its JSON),
//!   groups merged by label and children sorted by raw label;
//! - `cinput` gives the widget and its evaluated default, range and step;
//! - `"*"` with a two-input modulator equals the same modulation written with
//!   one literal target per control in interface order, to the bit, and its
//!   i-th extra input drives the i-th control of `cinputs`;
//! - the errors carry their own codes (`FRS-EVAL-0009`, `FRS-EVAL-0010`).
//!
//! The C++ reference has none of this (it stops on `undefined symbol :
//! cinputs`, and parses `"*"` as a label that matches nothing), so there is no
//! differential oracle: the expected values are derived by hand in each
//! fixture's header.

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, CompilerError};
use serde_json::Value;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(format!("{name}.dsp"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Runs `source` for `frames` samples with constant inputs `inputs`.
fn run(source: &str, inputs: &[f32], frames: usize) -> Vec<Vec<f32>> {
    let fbc = Compiler::new()
        .compile_source_to_interp("control_inputs.dsp", source, &InterpOptions::default())
        .unwrap_or_else(|e| panic!("compilation failed: {e}\n{source}"));
    let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("parse the bytecode");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    assert_eq!(
        usize::try_from(instance.get_num_inputs()).expect("non-negative"),
        inputs.len(),
        "input count of\n{source}"
    );
    let input_buffers: Vec<Vec<f32>> = inputs.iter().map(|&v| vec![v; frames]).collect();
    let input_slices: Vec<&[f32]> = input_buffers.iter().map(Vec::as_slice).collect();
    let outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative");
    let mut buffers = vec![vec![0.0_f32; frames]; outputs];
    let mut slices: Vec<&mut [f32]> = buffers.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(
            i32::try_from(frames).expect("frames"),
            &input_slices,
            &mut slices,
        )
        .expect("run the program");
    buffers
}

/// The first sample of every output of fixture `name`.
fn first_samples(name: &str, inputs: &[f32]) -> Vec<f32> {
    run(&fixture(name), inputs, 1)
        .iter()
        .map(|out| out[0])
        .collect()
}

/// The addresses of the input controls of `source`'s interface, in order.
fn interface_inputs(source: &str) -> Vec<String> {
    fn walk(items: &[Value], out: &mut Vec<String>) {
        for item in items {
            match item["type"].as_str() {
                Some("hgroup" | "vgroup" | "tgroup") => {
                    walk(item["items"].as_array().expect("group items"), out);
                }
                Some("hslider" | "vslider" | "nentry" | "button" | "checkbox") => {
                    out.push(item["address"].as_str().expect("address").to_owned());
                }
                _ => {}
            }
        }
    }
    let json = Compiler::new()
        .compile_source_to_json("control_inputs.dsp", source)
        .unwrap_or_else(|e| panic!("json failed: {e}"));
    let value: Value = serde_json::from_str(&json).expect("valid json");
    let mut out = Vec::new();
    walk(value["ui"].as_array().expect("ui"), &mut out);
    out
}

/// Fixture `name` with its `process` replaced by `process`.
fn with_process(name: &str, process: &str) -> String {
    let source = fixture(name);
    let at = source.find("process =").expect("a process definition");
    format!("{}{process}", &source[..at])
}

fn codes_of(error: &CompilerError) -> Vec<String> {
    error
        .diagnostic_bundle()
        .as_slice()
        .iter()
        .map(|d| d.code.0.to_owned())
        .collect()
}

fn refusal_codes(name: &str, source: &str) -> Vec<String> {
    let error = Compiler::new()
        .compile_source_to_interp(name, source, &InterpOptions::default())
        .map(|_| ())
        .expect_err(source);
    codes_of(&error)
}

#[test]
fn cinputs_counts_the_controls_in_interface_order() {
    // y, a, b, g/d, go, z/c: the defaults in interface order
    assert_eq!(
        first_samples("cinputs_01_interface_order", &[]),
        vec![6.0, 1.0, 0.0, 0.25, 0.5, 1.0, 0.0, 3.0]
    );
    let interface = interface_inputs(&with_process("cinputs_01_interface_order", "process = e;"));
    let tails: Vec<&str> = interface
        .iter()
        .map(|a| a.strip_prefix("/control_inputs/").unwrap_or(a))
        .collect();
    assert_eq!(tails, ["y", "a", "b", "g/d", "go", "z/c"]);
}

#[test]
fn cinput_is_the_widget_its_default_its_range_and_its_step() {
    let values = first_samples("cinputs_02_entry_values", &[]);
    // b: the slider at its default, then 0.5, 0, 1, 0.1
    assert_eq!(&values[..5], &[0.5, 0.5, 0.0, 1.0, 0.1]);
    // go, a button: its signal, then 0, 0, 1, 1
    assert_eq!(&values[5..], &[0.0, 0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn coutput_is_the_bargraph_its_min_and_its_max() {
    assert_eq!(
        first_samples("cinputs_03_bargraph_entry", &[]),
        vec![0.25, -1.0, 1.0, 1.0]
    );
}

#[test]
fn a_program_without_controls_has_an_empty_list() {
    assert_eq!(
        first_samples("cinputs_04_no_controls", &[]),
        vec![0.0, 0.0, 0.0]
    );
}

#[test]
fn a_widget_under_several_groups_is_one_control_per_group() {
    let values = first_samples("cinputs_05_widget_under_three_groups", &[]);
    assert_eq!(values[0], 4.0);
    // Op 0/g, Op 1/g, Op 2/g, amp/vol
    assert_eq!(values[1], (10.0 + 20.0 * 2.0 + 30.0 * 3.0) * 40.0);
    // only Op 1/g rebound
    assert_eq!(values[2], 0.5 + 7.0 * 2.0 + 0.5 * 3.0);
}

#[test]
fn dead_widgets_are_control_inputs() {
    assert_eq!(first_samples("cinputs_06_dead_widget", &[]), vec![2.0]);
}

#[test]
fn fad_on_cinputs_equals_fad_on_the_widgets() {
    // primal, d/da = x b + 1, d/db = x a, then the differences with the
    // widgets written out, on four samples
    let outs = run(&fixture("cinputs_07_fad_seeds"), &[0.75], 4);
    let expected = [
        0.75 * 0.5 * 2.0 + 0.5,
        0.75 * 2.0 + 1.0,
        0.75 * 0.5,
        0.0,
        0.0,
        0.0,
    ];
    assert_eq!(outs.len(), expected.len());
    for (out, value) in outs.iter().zip(expected) {
        assert!(out.iter().all(|&sample| sample == value), "{outs:?}");
    }
}

#[test]
fn the_wildcard_equals_one_literal_target_per_control_in_interface_order() {
    // y, a (x100), b (x10), g/d (x10000), go (x100000), z/c (x1000)
    let expected = 1.0 + 200.0 + 30.0 + 40_000.0 + 500_000.0 + 6000.0;
    assert_eq!(
        first_samples("wildcard_01_every_control", &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        vec![expected, 0.0]
    );
    let rebound = with_process(
        "wildcard_01_every_control",
        "process = [\"*\": (!, _) -> e];",
    );
    assert!(
        interface_inputs(&rebound).is_empty(),
        "every rebound widget leaves the interface"
    );
}

#[test]
fn the_wildcard_with_one_input_transforms_every_control() {
    // the bargraph is never matched
    assert_eq!(
        first_samples("wildcard_02_one_input_modulator", &[]),
        vec![2.5, 0.5 + (0.25 - 1.0) + 1.0]
    );
}

#[test]
fn the_wildcard_rebinds_a_fad_seed_with_its_body() {
    // the seed occurrence of `g` is the same control as the body's: both
    // become the one extra input, so the tangent is still d/dg
    assert_eq!(
        first_samples("wildcard_03_fad_seed", &[3.0, 5.0]),
        vec![15.0, 5.0]
    );
}

#[test]
fn a_literal_star_inside_a_segment_is_not_a_wildcard() {
    // `stage*` is a label like any other: no match, the reference's dangling
    // input, not the wildcard's error
    assert_eq!(
        first_samples("wildcard_04_star_inside_a_segment", &[1.0]),
        vec![0.5]
    );
}

#[test]
fn an_index_past_the_count_is_frs_eval_0009() {
    let name = "err_30_cinput_index_out_of_range";
    assert_eq!(refusal_codes(name, &fixture(name)), ["FRS-EVAL-0009"]);
}

#[test]
fn a_wildcard_matching_nothing_is_frs_eval_0010() {
    let name = "err_31_wildcard_no_match";
    assert_eq!(refusal_codes(name, &fixture(name)), ["FRS-EVAL-0010"]);
}

/// The message and the help of the error `source` is refused with.
fn refusal(name: &str, source: &str) -> (String, Vec<String>) {
    let error = Compiler::new()
        .compile_source_to_interp(name, source, &InterpOptions::default())
        .map(|_| ())
        .expect_err(source);
    let bundle = error.diagnostic_bundle();
    let first = bundle.as_slice().first().expect("one diagnostic");
    (
        first.message.to_string(),
        first.help.iter().map(ToString::to_string).collect(),
    )
}

#[test]
fn swapped_arguments_name_the_index_and_suggest_the_order() {
    let name = "err_32_cinput_arguments_swapped";
    assert_eq!(refusal_codes(name, &fixture(name)), ["FRS-EVAL-0009"]);
    let (message, help) = refusal(name, &fixture(name));
    assert_eq!(
        message,
        "the index of `cinput` must be a compile-time integer, and `freq1` is not"
    );
    assert_eq!(help, ["write `cinput(0, freq1)`"]);
}

#[test]
fn a_signal_index_without_a_constant_expression_suggests_no_swap() {
    let source =
        "g = hslider(\"g\", 0, 0, 1, 0.1); process = coutput(g, g : hbargraph(\"m\", 0, 1));";
    assert_eq!(
        refusal_codes("control_inputs.dsp", source),
        ["FRS-EVAL-0009"]
    );
    let (message, help) = refusal("control_inputs.dsp", source);
    assert_eq!(
        message,
        "the index of `coutput` must be a compile-time integer, and `g` is not"
    );
    assert_eq!(
        help,
        [
            "use an integer constant, or iterate with `par(i, outputs(coutputs(e)), coutput(i, e) : ...)`"
        ]
    );
}

#[test]
fn a_negative_index_is_reported_as_written() {
    let (message, _) = refusal(
        "control_inputs.dsp",
        "process = cinput(-1, hslider(\"a\", 0, 0, 1, 0.1));",
    );
    assert_eq!(
        message,
        "`cinput` index -1 is out of range: the expression has 1 control input"
    );
}

#[test]
fn every_other_way_to_the_two_codes() {
    // coutput past the bargraphs, cinput on an expression without controls,
    // a wildcard over a bargraph only, "*" on a program without controls
    for (source, code) in [
        (
            "e = hbargraph(\"m\", 0, 1); process = coutput(1, e);",
            "FRS-EVAL-0009",
        ),
        ("process = cinput(0, _);", "FRS-EVAL-0009"),
        (
            "process = [\"*\": *(2) -> hbargraph(\"m\", 0, 1)];",
            "FRS-EVAL-0010",
        ),
        ("process = [\"*\": (!, _) -> _ + 1];", "FRS-EVAL-0010"),
    ] {
        assert_eq!(
            refusal_codes("control_inputs.dsp", source),
            [code],
            "{source}"
        );
    }
}
