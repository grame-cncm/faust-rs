//! `cinputs`, `cinput`, `coutputs`, `coutput` and the wildcard modulation
//! target `"*"`: faust-rs extensions, no C++ equivalent.
//!
//! Contract: `porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`
//! section 3. The checks are self-contained programs run through the
//! interpreter fast lane:
//! - the list order is the order of the program's own interface (its JSON),
//!   groups merged by label and children sorted by raw label;
//! - `cinput` gives the widget and its evaluated default, range and step;
//! - `"*"` with a two-input modulator equals the same modulation written with
//!   one literal target per control in interface order, to the bit, and its
//!   i-th extra input drives the i-th control of `cinputs`;
//! - the errors carry their own codes (`FRS-EVAL-0009`, `FRS-EVAL-0010`).

use std::io::Cursor;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, CompilerError};
use serde_json::Value;

/// A program whose six controls are declared out of interface order: two
/// groups, a `[0]` ordering prefix, a numentry and a button, plus a bargraph.
const SIX_CONTROLS: &str = r#"
e = hslider("b", 0.5, 0, 1, 0.1) * 10 + hslider("a", 0.25, 0, 2, 0.1) * 100
  + hgroup("z", hslider("c", 3, 0, 10, 1)) * 1000 + hslider("[0]y", 0, 0, 1, 0.1)
  + hgroup("g", nentry("d", 1, 0, 4, 1)) * 10000 + button("go") * 100000
  : hbargraph("m", -1, 1);
"#;

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

/// The first sample of every output.
fn first_samples(source: &str, inputs: &[f32]) -> Vec<f32> {
    run(source, inputs, 1).iter().map(|out| out[0]).collect()
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

fn eval_error(source: &str) -> CompilerError {
    match Compiler::new().compile_source_to_interp(
        "control_inputs.dsp",
        source,
        &InterpOptions::default(),
    ) {
        Ok(_) => panic!("should be refused:\n{source}"),
        Err(e) => e,
    }
}

fn codes(error: &CompilerError) -> Vec<String> {
    error
        .diagnostic_bundle()
        .as_slice()
        .iter()
        .map(|d| d.code.0.to_owned())
        .collect()
}

#[test]
fn cinputs_counts_the_controls_in_interface_order() {
    let source = format!(
        "{SIX_CONTROLS}process = outputs(cinputs(e)), outputs(coutputs(e)), \
         par(i, outputs(cinputs(e)), cinput(i, e) : !, _, !, !, !);"
    );
    // y, a, b, g/d, go, z/c: the defaults in interface order
    assert_eq!(
        first_samples(&source, &[]),
        vec![6.0, 1.0, 0.0, 0.25, 0.5, 1.0, 0.0, 3.0]
    );
    let interface = interface_inputs(&format!("{SIX_CONTROLS}process = e;"));
    let tails: Vec<&str> = interface
        .iter()
        .map(|a| a.strip_prefix("/control_inputs/").unwrap_or(a))
        .collect();
    assert_eq!(tails, ["y", "a", "b", "g/d", "go", "z/c"]);
}

#[test]
fn cinput_is_the_widget_its_default_its_range_and_its_step() {
    let source = format!("{SIX_CONTROLS}process = cinput(2, e), cinput(4, e);");
    let values = first_samples(&source, &[]);
    // b: the slider at its default, then 0.5, 0, 1, 0.1
    assert_eq!(&values[..5], &[0.5, 0.5, 0.0, 1.0, 0.1]);
    // go, a button: its signal, then 0, 0, 1, 1
    assert_eq!(&values[5..], &[0.0, 0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn coutput_is_the_bargraph_its_min_and_its_max() {
    let source = format!("{SIX_CONTROLS}process = 0.25 : coutput(0, e), outputs(coutputs(e));");
    assert_eq!(first_samples(&source, &[]), vec![0.25, -1.0, 1.0, 1.0]);
}

#[test]
fn a_program_without_controls_has_an_empty_list() {
    let source =
        "e = _ + 1; process = outputs(cinputs(e)), inputs(cinputs(e)), outputs(coutputs(e));";
    assert_eq!(first_samples(source, &[]), vec![0.0, 0.0, 0.0]);
}

#[test]
fn a_widget_under_several_groups_is_one_control_per_group() {
    let source = r#"
g = hslider("g", 0.5, 0, 1, 0.1);
e = par(i, 3, vgroup("Op %i", g * (i + 1))) :> _ : hgroup("amp", *(hslider("vol", 1, 0, 2, 0.1)));
process = outputs(cinputs(e)), (10, 20, 30, 40 : ["*": (!, _) -> e]), (7 : ["Op 1/*": (!, _) -> e]);
"#;
    let values = first_samples(source, &[]);
    assert_eq!(values[0], 4.0);
    // Op 0/g, Op 1/g, Op 2/g, amp/vol
    assert_eq!(values[1], (10.0 + 20.0 * 2.0 + 30.0 * 3.0) * 40.0);
    // only Op 1/g rebound
    assert_eq!(values[2], 0.5 + 7.0 * 2.0 + 0.5 * 3.0);
}

#[test]
fn dead_widgets_are_control_inputs() {
    let source = r#"
e = hslider("dead", 0.5, 0, 1, 0.1) : !, hslider("live", 0.25, 0, 1, 0.1);
process = outputs(cinputs(e));
"#;
    assert_eq!(first_samples(source, &[]), vec![2.0]);
}

#[test]
fn the_wildcard_equals_one_literal_target_per_control_in_interface_order() {
    let wildcard = format!("{SIX_CONTROLS}process = [\"*\": (!, _) -> e];");
    let literal = format!(
        "{SIX_CONTROLS}process = [\"y\": (!, _), \"a\": (!, _), \"b\": (!, _), \
         \"g/d\": (!, _), \"go\": (!, _), \"z/c\": (!, _) -> e];"
    );
    let inputs = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    // y, a (x100), b (x10), g/d (x10000), go (x100000), z/c (x1000)
    let expected = 1.0 + 200.0 + 30.0 + 40_000.0 + 500_000.0 + 6000.0;
    let from_wildcard = first_samples(&wildcard, &inputs);
    assert_eq!(from_wildcard, first_samples(&literal, &inputs));
    assert_eq!(from_wildcard, vec![expected]);
    assert!(
        interface_inputs(&wildcard).is_empty(),
        "every rebound widget leaves the interface"
    );
}

#[test]
fn the_wildcard_with_one_input_transforms_every_control() {
    let source = r#"
e = hslider("a", 0.5, 0, 1, 0.1) + hgroup("g", hslider("b", 0.25, 0, 1, 0.1)) + hbargraph("m", 0, 1)(1);
process = ["*": *(2) -> e], ["g/*": -(1) -> e];
"#;
    // the bargraph is never matched
    assert_eq!(
        first_samples(source, &[]),
        vec![2.5, 0.5 + (0.25 - 1.0) + 1.0]
    );
}

#[test]
fn fad_on_cinputs_equals_fad_on_the_widgets() {
    let widgets = r#"
a = hslider("a", 0.5, 0, 1, 0.1);
b = hslider("b", 2, 0, 4, 0.1);
f = _ * a * b + a;
"#;
    let listed = format!("{widgets}process = fad(f, cinputs(f));");
    let spelled = format!("{widgets}process = fad(f, (a, b));");
    let listed = run(&listed, &[0.75], 4);
    assert_eq!(listed, run(&spelled, &[0.75], 4));
    // primal, d/da = x b + 1, d/db = x a
    assert_eq!(
        listed.iter().map(|out| out[0]).collect::<Vec<_>>(),
        vec![0.75 * 0.5 * 2.0 + 0.5, 0.75 * 2.0 + 1.0, 0.75 * 0.5]
    );
}

#[test]
fn the_wildcard_rebinds_a_fad_seed_with_its_body() {
    // the seed occurrence of `g` is the same control as the body's: both
    // become the one extra input, so the tangent is still d/dg
    let source = r#"
g = hslider("g", 2, 0, 4, 0.1);
e = fad(_ * g, g);
process = ["*": (!, _) -> e];
"#;
    assert_eq!(first_samples(source, &[3.0, 5.0]), vec![15.0, 5.0]);
}

#[test]
fn an_index_past_the_count_is_frs_eval_0009() {
    for source in [
        format!("{SIX_CONTROLS}process = cinput(6, e);"),
        format!("{SIX_CONTROLS}process = coutput(1, e);"),
        "process = cinput(0, _);".to_owned(),
    ] {
        let error = eval_error(&source);
        assert_eq!(codes(&error), ["FRS-EVAL-0009"], "{error}");
    }
}

#[test]
fn a_wildcard_matching_nothing_is_frs_eval_0010() {
    for source in [
        "process = [\"*\": (!, _) -> _ + 1];",
        "process = [\"amp/*\": (!, _) -> hslider(\"x\", 0, 0, 1, 0.1)];",
        // a bargraph is not a control input
        "process = [\"*\": *(2) -> hbargraph(\"m\", 0, 1)];",
    ] {
        let error = eval_error(source);
        assert_eq!(codes(&error), ["FRS-EVAL-0010"], "{error}");
    }
}

#[test]
fn a_literal_star_inside_a_segment_is_not_a_wildcard() {
    // `stage*` is a label like any other: no match, the reference's dangling
    // slot and warning, not the wildcard's error
    let source = "process = 1 : [\"stage*\": (!, _) -> hslider(\"x\", 0.5, 0, 1, 0.1)];";
    assert_eq!(first_samples(source, &[]), vec![0.5]);
}
