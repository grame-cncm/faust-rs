//! Dead widgets against the reference compiler.
//!
//! The reference builds its interface while generating code (`generateHSlider`
//! and its siblings call `fUITree.addUIWidget` in
//! `compiler/generator/compile_scal.cpp`), so a widget whose signal the
//! simplified graph no longer reaches never appears: cut by `!`, absorbed by a
//! folded zero, left in the dead branch of a `select2` with a constant
//! selector, a bargraph whose output is dropped. faust-rs registers every
//! widget of the box tree during propagation and prunes them in the fast
//! lane, once the final signals are known (`UiProgram::pruned`).
//!
//! The `tests/corpus/ui_dead_*.dsp` fixtures freeze the reference's arity and
//! interface (faust 2.88.1, `-json`, 2026-09-23) and are compared live with
//! the `faust` binary when one is installed (`FAUST_CPP_BIN`, else
//! `/usr/local/bin/faust`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use compiler::{Compiler, SignalFirLane};
use serde_json::Value;

#[derive(Debug, PartialEq)]
struct Control {
    address: &'static str,
    kind: &'static str,
    init: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
}

/// One control as read from a JSON description: address, kind, init, min,
/// max, step.
type ControlRow = (
    String,
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

#[derive(Debug, PartialEq)]
struct Observed {
    inputs: u64,
    outputs: u64,
    controls: Vec<ControlRow>,
}

struct Case {
    fixture: &'static str,
    inputs: u64,
    outputs: u64,
    controls: &'static [Control],
}

const CASES: &[Case] = &[
    Case {
        fixture: "ui_dead_01_cut.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[],
    },
    Case {
        fixture: "ui_dead_02_folded_zero.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[],
    },
    Case {
        fixture: "ui_dead_03_select2_constant.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[Control {
            address: "/ui_dead_03_select2_constant/kept",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
    },
    Case {
        fixture: "ui_dead_04_bargraph_cut.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[],
    },
    Case {
        fixture: "ui_dead_05_group_emptied.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[Control {
            address: "/live/g",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
    },
    Case {
        fixture: "ui_dead_06_attach_keeps.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/ui_dead_06_attach_keeps/kept_by_attach",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/ui_dead_06_attach_keeps/lev",
                kind: "hbargraph",
                init: None,
                min: Some(0.0),
                max: Some(1.0),
                step: None,
            },
        ],
    },
    Case {
        fixture: "ui_dead_07_nested_groups_emptied.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[Control {
            address: "/ui_dead_07_nested_groups_emptied/top",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
    },
];

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn cpp_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FAUST_CPP_BIN") {
        return Some(PathBuf::from(path));
    }
    let default = PathBuf::from("/usr/local/bin/faust");
    default.exists().then_some(default)
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flatten_ui(items: &[Value], out: &mut Vec<ControlRow>) {
    for item in items {
        let kind = item["type"].as_str().unwrap_or_default();
        if matches!(kind, "vgroup" | "hgroup" | "tgroup") {
            if let Some(children) = item["items"].as_array() {
                flatten_ui(children, out);
            }
        } else {
            out.push((
                item["address"].as_str().unwrap_or_default().to_owned(),
                kind.to_owned(),
                number(item.get("init")),
                number(item.get("min")),
                number(item.get("max")),
                number(item.get("step")),
            ));
        }
    }
}

fn observe(json: &str) -> Observed {
    let value: Value = serde_json::from_str(json).expect("a JSON description");
    let mut controls = Vec::new();
    if let Some(items) = value["ui"].as_array() {
        flatten_ui(items, &mut controls);
    }
    Observed {
        inputs: value["inputs"].as_u64().expect("inputs"),
        outputs: value["outputs"].as_u64().expect("outputs"),
        controls,
    }
}

fn expected(case: &Case) -> Observed {
    Observed {
        inputs: case.inputs,
        outputs: case.outputs,
        controls: case
            .controls
            .iter()
            .map(|c| {
                (
                    c.address.to_owned(),
                    c.kind.to_owned(),
                    c.init,
                    c.min,
                    c.max,
                    c.step,
                )
            })
            .collect(),
    }
}

fn rust_json(compiler: &Compiler, fixture: &str) -> String {
    compiler
        .compile_file_to_json(&corpus_path(fixture), &[], SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("{fixture}: faust-rs failed: {e}"))
}

fn cpp_json(cpp_bin: &Path, fixture: &str) -> Result<String, String> {
    let out_dir = std::env::temp_dir().join(format!(
        "faust-rs-dead-widgets-json-{}-{}",
        std::process::id(),
        fixture.trim_end_matches(".dsp")
    ));
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let output = Command::new(cpp_bin)
        .arg("-json")
        .arg(corpus_path(fixture))
        .arg("-O")
        .arg(&out_dir)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", cpp_bin.display()))?;
    let json_path = out_dir.join(format!("{fixture}.json"));
    let text = fs::read_to_string(&json_path);
    let _ = fs::remove_dir_all(&out_dir);
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    text.map_err(|e| format!("cannot read {}: {e}", json_path.display()))
}

#[test]
fn dead_widgets_leave_the_interface_as_in_the_frozen_reference() {
    let compiler = Compiler::new();
    let mut failures = Vec::new();
    for case in CASES {
        let observed = observe(&rust_json(&compiler, case.fixture));
        let wanted = expected(case);
        if observed != wanted {
            failures.push(format!(
                "{}\n  expected {wanted:?}\n  observed {observed:?}",
                case.fixture
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) differ from the frozen reference:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn dead_widgets_leave_the_interface_as_in_the_live_reference_compiler() {
    let Some(cpp_bin) = cpp_bin() else {
        eprintln!(
            "Skipping live dead-widget differential: FAUST_CPP_BIN not set and /usr/local/bin/faust not found"
        );
        return;
    };
    let compiler = Compiler::new();
    let mut failures = Vec::new();
    for case in CASES {
        let rust = observe(&rust_json(&compiler, case.fixture));
        match cpp_json(&cpp_bin, case.fixture) {
            Ok(json) => {
                let cpp = observe(&json);
                if rust != cpp {
                    failures.push(format!(
                        "{}\n  reference {cpp:?}\n  faust-rs  {rust:?}",
                        case.fixture
                    ));
                }
            }
            Err(e) => failures.push(format!("{}: reference compiler failed: {e}", case.fixture)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) differ from the reference compiler:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a_dead_widget_gets_no_field_and_no_ui_entry_in_the_generated_code() {
    // The reference declares no member for a widget it does not show; the
    // generated C++ of the cut slider must not mention it either.
    let compiler = Compiler::new();
    let cpp = compiler
        .compile_file_default_to_cpp_with_lane(
            &corpus_path("ui_dead_01_cut.dsp"),
            &codegen::backends::cpp::CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .expect("the cut slider compiles");
    assert!(!cpp.contains("fHslider"), "{cpp}");
    assert!(!cpp.contains("addHorizontalSlider"), "{cpp}");
    assert!(cpp.contains("openVerticalBox(\"ui_dead_01_cut\")"), "{cpp}");
}
