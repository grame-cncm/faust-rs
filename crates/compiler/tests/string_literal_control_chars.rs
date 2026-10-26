//! Multi-line string literals, as the reference compiler prints them.
//!
//! C++ `Symbol::get` (`compiler/tlib/symbol.cpp`) replaces every control
//! character of a string with a space when it becomes a symbol, so a newline
//! inside a label, a tooltip or a `declare` value reaches
//! `buildUserInterface`, `metadata()` and the JSON as a space. The expected
//! strings are what Faust 2.84.3 (the pinned reference) and 2.90.15 generate
//! for the same program (2026-10-26).

use codegen::backends::cpp::CppOptions;
use compiler::Compiler;
use serde_json::Value;

const SOURCE: &str = "declare author \"A\nB\";\n\
process = vgroup(\"Grp\n  [tooltip: two\n lines]\", \
hslider(\"my\nfreq[unit:Hz]\", 1, 0, 2, 1));\n";

#[test]
fn newlines_in_labels_and_metadata_are_spaces_in_the_cpp_output() {
    let cpp = Compiler::new()
        .compile_source_to_cpp("multiline.dsp", SOURCE, &CppOptions::default())
        .expect("the program compiles");
    for expected in [
        r#"m->declare("author", "A B");"#,
        r#"ui_interface->declare(0, "tooltip", "two  lines");"#,
        r#"ui_interface->openVerticalBox("Grp");"#,
        r#"ui_interface->addHorizontalSlider("my freq", "#,
    ] {
        assert!(cpp.contains(expected), "`{expected}` missing in:\n{cpp}");
    }
}

#[test]
fn newlines_in_labels_and_metadata_are_spaces_in_the_json() {
    let json = Compiler::new()
        .compile_source_to_json("multiline.dsp", SOURCE)
        .expect("the program compiles");
    let json: Value = serde_json::from_str(&json).expect("valid JSON");
    assert!(
        json["meta"]
            .as_array()
            .expect("meta is an array")
            .iter()
            .any(|entry| entry["author"] == "A B"),
        "author metadata in {json}"
    );
    let group = &json["ui"][0];
    assert_eq!(group["label"], "Grp");
    assert_eq!(group["meta"][0]["tooltip"], "two  lines");
    let slider = &group["items"][0];
    assert_eq!(slider["label"], "my freq");
    assert_eq!(slider["address"], "/Grp/my_freq");
}
