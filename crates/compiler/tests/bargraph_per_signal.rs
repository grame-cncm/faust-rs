//! One bargraph per bargraph signal, as in the reference compiler.
//!
//! A bargraph box is a function of its input: `process = hmeter, hmeter;`
//! applies one `hbargraph` box to each channel
//! (`tests/impulse-tests/dsp/vumeter.dsp`). The reference makes one widget per
//! bargraph signal it compiles (`InstructionsCompiler::generateHBargraph` calls
//! `fUITree.addUIWidget`, `compiler/generator/instructions_compiler.cpp`), so
//! the program shows two meters, each written by its own channel. faust-rs
//! used to key the control by the box and its group context only: one meter,
//! written by both channels, the second level overwriting the first.
//!
//! Expected counts were taken from faust 2.84.3 and 2.90.15 (2026-10-26).

use compiler::Compiler;
use serde_json::Value;

/// The zone of every bargraph declared by `buildUserInterface`, in order.
fn declared_bargraph_zones(cpp: &str) -> Vec<String> {
    cpp.lines()
        .filter(|line| {
            line.contains("addHorizontalBargraph") || line.contains("addVerticalBargraph")
        })
        .map(|line| {
            let start = line.find('&').expect("a bargraph declaration takes a zone") + 1;
            let end = start
                + line[start..]
                    .find(',')
                    .expect("the zone is followed by the range");
            line[start..end].to_owned()
        })
        .collect()
}

/// The right-hand sides written to `zone` in `compute`.
fn writes_to<'a>(cpp: &'a str, zone: &str) -> Vec<&'a str> {
    let compute = &cpp[cpp.find("virtual void compute(").expect("a compute method")..];
    let assignment = format!("{zone} = ");
    compute
        .lines()
        .filter_map(|line| line.trim().strip_prefix(assignment.as_str()))
        .collect()
}

fn cpp(name: &str, source: &str) -> String {
    Compiler::new()
        .compile_source_to_cpp(name, source, &codegen::backends::cpp::CppOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The bargraphs of the JSON description, in interface order.
fn json_bargraphs(name: &str, source: &str) -> Vec<Value> {
    fn walk(items: &[Value], out: &mut Vec<Value>) {
        for item in items {
            if let Some(children) = item.get("items").and_then(Value::as_array) {
                walk(children, out);
            } else if item["type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("bargraph"))
            {
                out.push(item.clone());
            }
        }
    }
    let json = Compiler::new()
        .compile_source_to_json(name, source)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let json: Value = serde_json::from_str(&json).expect("valid JSON");
    let mut out = Vec::new();
    walk(json["ui"].as_array().expect("a ui array"), &mut out);
    out
}

/// `expected` bargraphs are declared, each with a zone of its own written
/// exactly once in `compute`.
fn assert_one_zone_per_signal(cpp: &str, expected: usize) {
    let zones = declared_bargraph_zones(cpp);
    assert_eq!(zones.len(), expected, "{cpp}");
    let mut sorted = zones.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), zones.len(), "a zone declared twice:\n{cpp}");
    for zone in &zones {
        assert_eq!(
            writes_to(cpp, zone).len(),
            1,
            "{zone} is not written once:\n{cpp}"
        );
    }
}

const VUMETER: &str = r#"
envelop = abs : max ~ -(0.001) : max(0.0003);
hmeter(x) = attach(x, envelop(x) : hbargraph("[2][unit:dB]", -70, +5));
process = hmeter, hmeter;
"#;

#[test]
fn one_bargraph_box_on_two_channels_is_two_meters() {
    let cpp = cpp("vumeter", VUMETER);
    assert_one_zone_per_signal(&cpp, 2);
    let zones = declared_bargraph_zones(&cpp);
    assert_ne!(
        writes_to(&cpp, &zones[0]),
        writes_to(&cpp, &zones[1]),
        "each meter displays its own channel:\n{cpp}"
    );
    // The anonymous labels are numbered in interface order, as C++
    // `checkNullBargraphLabel` does.
    assert!(
        cpp.contains("addHorizontalBargraph(\"hbargraph0\""),
        "{cpp}"
    );
    assert!(
        cpp.contains("addHorizontalBargraph(\"hbargraph1\""),
        "{cpp}"
    );

    let bargraphs = json_bargraphs("vumeter", VUMETER);
    let addresses: Vec<&str> = bargraphs
        .iter()
        .map(|item| item["address"].as_str().expect("an address"))
        .collect();
    assert_eq!(addresses, ["/vumeter/hbargraph0", "/vumeter/hbargraph1"]);
}

#[test]
fn one_bargraph_box_on_one_signal_stays_one_meter() {
    // The same box displaying the same signal twice is one bargraph signal,
    // hence one widget in the reference.
    let source = r#"
m(x) = attach(x, abs(x) : hbargraph("m", 0, 1));
process = _ <: m, m;
"#;
    assert_one_zone_per_signal(&cpp("shared", source), 1);
}

#[test]
fn bargraphs_split_through_a_recursion_and_across_groups() {
    // Three inputs reach the one `m` box: the feedback, the sum and the
    // second channel; the reference shows three meters.
    let recursive = r#"
m(x) = attach(x, abs(x) : vbargraph("m", 0, 1));
process = (+ : m) ~ (m : *(0.5)), m;
"#;
    assert_one_zone_per_signal(&cpp("recursive", recursive), 3);

    // Two channels in each of two group contexts: four meters, two per group.
    let grouped = r#"
m(x) = attach(x, abs(x) : vbargraph("m", 0, 1));
process = m, m, hgroup("g", m, m);
"#;
    let cpp_text = cpp("grouped", grouped);
    assert_one_zone_per_signal(&cpp_text, 4);
    let bargraphs = json_bargraphs("grouped", grouped);
    let addresses: Vec<&str> = bargraphs
        .iter()
        .map(|item| item["address"].as_str().expect("an address"))
        .collect();
    assert_eq!(
        addresses,
        ["/grouped/g/m", "/grouped/g/m", "/grouped/m", "/grouped/m"]
    );
}

#[test]
fn bargraph_inputs_equal_only_after_simplification_stay_apart() {
    // DIFF-BEH-027: `x + x` and `2 * x` are two inputs as propagated, one
    // after simplification. C++ compares bargraph inputs after
    // simplification and shows two meters (`2x`, `x`); faust-rs compares
    // them as propagated and shows three, two of them the same value.
    let source = r#"
m(x) = attach(x, x : hbargraph("m", 0, 1));
process = _ <: (_ <: + : m), (*(2) : m), (+(1) : -(1) : m);
"#;
    let cpp_text = cpp("simplified", source);
    assert_one_zone_per_signal(&cpp_text, 3);
    let zones = declared_bargraph_zones(&cpp_text);
    assert_eq!(
        writes_to(&cpp_text, &zones[0]),
        writes_to(&cpp_text, &zones[1])
    );
    assert_ne!(
        writes_to(&cpp_text, &zones[0]),
        writes_to(&cpp_text, &zones[2])
    );
}
