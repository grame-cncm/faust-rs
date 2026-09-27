//! A group with an empty label, as the reference compiler names it.
//!
//! C++ `checkNullLabel` (`compiler/generator/uitree.cpp`) passes `0x00` to
//! `open*Box` for a group whose label is empty, except at the root, which is
//! named after the program; sibling groups with the same label and
//! orientation are one group of the UI tree. The expected text is what
//! Faust 2.89.2 generates for `SOURCE` (`faust empty.dsp`, 2026-09-27).

use codegen::backends::cpp::CppOptions;
use compiler::Compiler;
use serde_json::Value;

const SOURCE: &str = r#"
declare name "empty";
process = hgroup("", hslider("g", 0, 0, 1, 0.1)) + hgroup("", hslider("h", 0, 0, 1, 0.1)) + vgroup("0x00", hslider("k", 0, 0, 1, 0.1));
"#;

#[test]
fn an_empty_group_label_is_0x00_in_the_generated_user_interface() {
    let cpp = Compiler::new()
        .compile_source_to_cpp("empty.dsp", SOURCE, &CppOptions::default())
        .expect("the program compiles");
    let ui: Vec<&str> = cpp
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("ui_interface->open") || line.starts_with("ui_interface->close")
        })
        .collect();
    assert_eq!(
        ui,
        [
            r#"ui_interface->openVerticalBox("empty");"#,
            // the two `hgroup("")`: one group, named `0x00`
            r#"ui_interface->openHorizontalBox("0x00");"#,
            "ui_interface->closeBox();",
            r#"ui_interface->openVerticalBox("0x00");"#,
            "ui_interface->closeBox();",
            "ui_interface->closeBox();",
        ]
    );
}

#[test]
fn an_empty_group_label_is_0x00_in_the_json_addresses() {
    let json = Compiler::new()
        .compile_source_to_json("empty.dsp", SOURCE)
        .expect("the program compiles");
    let json: Value = serde_json::from_str(&json).expect("valid JSON");
    let mut addresses = Vec::new();
    collect_addresses(&json["ui"], &mut addresses);
    assert_eq!(
        addresses,
        ["/empty/0x00/g", "/empty/0x00/h", "/empty/0x00/k"]
    );
}

fn collect_addresses(items: &Value, out: &mut Vec<String>) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(address) = item["address"].as_str() {
            out.push(address.to_owned());
        }
        collect_addresses(&item["items"], out);
    }
}
