//! `-uim`: the user-interface macros of the C and C++ backends.
//!
//! Expected entries follow the reference (`faust -uim`, 2.84.3,
//! `master-dev-ocpp-od-fir-2-FIR19`): group paths keep their raw labels,
//! inline metadata included, the synthesized root and unnamed groups add no
//! segment, the order is the interface's, and a dead widget is left out.
//! Zone numbering and number spelling are faust-rs's own (`fEntry1`,
//! `440.0f` where the reference prints `fEntry0`, `4.4e+02f`), consistent
//! with the fields and `buildUserInterface` of the same file.

use codegen::backends::c::COptions;
use codegen::backends::cpp::CppOptions;
use compiler::Compiler;

const SOURCE: &str = r#"
g = vgroup("[1]Main [style:knob]", hslider("[0]gain [unit:dB]", 0.5, 0, 1, 0.01));
f = hgroup("", nentry("freq", 440, 20, 2000, 1));
d = hslider("dead", 0.2, 0, 1, 0.1) : !;
process = _*g*f*(button("gate")+checkbox("on")), d
    <: attach(_, abs : hbargraph("level", 0, 1)), attach(_, vbargraph("my v[style:led]", -1.5, 1e-3));
"#;

fn cpp(options: CppOptions) -> String {
    Compiler::new()
        .compile_source_to_cpp("uim.dsp", SOURCE, &options)
        .expect("C++ compilation")
}

#[test]
fn cpp_macros_list_the_reference_entries() {
    let out = cpp(CppOptions {
        ui_macros: true,
        ..CppOptions::default()
    });
    assert!(out.contains("struct mydsp : public dsp {"), "{out}");
    assert!(!out.contains("private:") && !out.contains("public:"));
    for line in [
        "#ifdef FAUST_UIMACROS",
        "    #define FAUST_FILE_NAME \"uim.dsp\"",
        "    #define FAUST_CLASS_NAME \"mydsp\"",
        "    #define FAUST_INPUTS 1",
        "    #define FAUST_OUTPUTS 2",
        "    #define FAUST_ACTIVES 4",
        "    #define FAUST_PASSIVES 2",
        "    #define FAUST_LIST_ACTIVES(p) \\",
        "    #define FAUST_LIST_PASSIVES(p) \\",
    ] {
        assert!(out.lines().any(|l| l == line), "missing {line:?} in\n{out}");
    }
    let adds: Vec<&str> = out
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("FAUST_ADD"))
        .map(|l| l.split(',').next().unwrap_or(l))
        .collect();
    assert_eq!(
        adds,
        [
            "NUMENTRY(\"freq\"",
            "HORIZONTALSLIDER(\"[1]Main [style:knob]/gain\"",
            "BUTTON(\"gate\"",
            "HORIZONTALBARGRAPH(\"level\"",
            "VERTICALBARGRAPH(\"my v\"",
            "CHECKBOX(\"on\"",
        ]
    );
    assert!(!out.contains("dead"), "a pruned widget has no macro");
    assert!(out.contains("p(VERTICALBARGRAPH, my_v, \"my v\", "));
    // Each list ends with a blank line, or its trailing `\` would swallow
    // the next one.
    assert!(out.contains(" \\\n\n#endif"));
}

#[test]
fn every_macro_names_a_declared_field() {
    let out = cpp(CppOptions {
        ui_macros: true,
        ..CppOptions::default()
    });
    let block = &out[out.find("#ifdef FAUST_UIMACROS").expect("macro block")..];
    let zones: Vec<&str> = block
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("FAUST_ADD"))
        .filter_map(|l| l.split(", ").nth(1))
        .map(|zone| zone.trim_end_matches(");"))
        .collect();
    assert_eq!(zones.len(), 6);
    for zone in zones {
        assert!(
            out.contains(&format!("FAUSTFLOAT {zone};")),
            "{zone} is not a field"
        );
    }
}

#[test]
fn without_uim_the_class_is_unchanged() {
    let out = cpp(CppOptions::default());
    assert!(out.contains("class mydsp : public dsp {"));
    assert!(out.contains("private:"));
    assert!(!out.contains("FAUST_UIMACROS"));
}

#[test]
fn c_macros_close_before_the_extern_c_footer() {
    let out = Compiler::new()
        .compile_source_to_c(
            "uim.dsp",
            SOURCE,
            &COptions {
                ui_macros: true,
                ..COptions::default()
            },
        )
        .expect("C compilation");
    let macros = out.find("#ifdef FAUST_UIMACROS").expect("macro block");
    let footer = out.rfind("#ifdef __cplusplus").expect("footer");
    assert!(macros < footer);
    assert!(out.contains("FAUST_ADDHORIZONTALSLIDER(\"[1]Main [style:knob]/gain\", "));
}
