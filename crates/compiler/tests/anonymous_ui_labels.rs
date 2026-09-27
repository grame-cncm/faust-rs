//! Unlabelled groups and widgets, as the reference compiler names them.
//!
//! C++ `checkNullLabel` (`compiler/generator/uitree.cpp`) passes `0x00` for
//! a group or a widget whose label is empty, except at the root, which is
//! named after the program; `checkNullBargraphLabel` names an unlabelled
//! bargraph `hbargraph<n>` / `vbargraph<n>` (`global::getFreshID`, a counter
//! per prefix). Sibling groups with the same label and orientation are one
//! group of the UI tree. The expected names are what Faust 2.89.3 generates
//! for the same programs (2026-09-27).

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

/// The widget names and order of Faust 2.89.3 for this program, whose
/// `sortPropList` keeps equal labels in insertion order (`ed7c12606`).
/// Faust 2.89.2 declared the same three in another order (`vbargraph0`,
/// `hbargraph0`, `0x00`), left to its unstable `std::sort` (DIFF-BEH-015).
#[test]
fn unlabelled_widgets_are_0x00_and_numbered_bargraphs() {
    const ANON: &str = r#"
process = hslider("", 0, 0, 1, 0.1) + (1 : hbargraph("", 0, 1)) + (2 : vbargraph("", 0, 2));
"#;
    let cpp = Compiler::new()
        .compile_source_to_cpp("anon.dsp", ANON, &CppOptions::default())
        .expect("the program compiles");
    let labels: Vec<&str> = cpp
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("ui_interface->add"))
        .filter_map(|line| line.split('"').nth(1))
        .collect();
    assert_eq!(labels, ["0x00", "hbargraph0", "vbargraph0"]);
}

/// Faust 2.89.2 and 2.89.3 refuse this program: `ERROR : path '/anon2/0x00' is already
/// used`, exit status 1.
#[test]
fn two_unlabelled_sliders_in_one_group_are_refused_as_in_cpp() {
    const ANON2: &str = r#"
process = hslider("", 0, 0, 1, 0.1) + hslider("", 0.5, 0, 1, 0.1) + (1 : hbargraph("", 0, 1)) + (2 : hbargraph("", 0, 2));
"#;
    let error = Compiler::new()
        .compile_source_to_cpp("anon2.dsp", ANON2, &CppOptions::default())
        .expect_err("two sliders at one address are refused");
    assert!(
        matches!(error, compiler::CompilerError::UiLayout { .. }),
        "{error}"
    );
    let diagnostic = &error.diagnostic_bundle().as_slice()[0];
    assert_eq!(diagnostic.code.0, "FRS-UI-0001");
    assert!(
        diagnostic.message.contains("/anon2/0x00"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("empty label")),
        "{:?}",
        diagnostic.notes
    );
}
