//! What `faustprobe` says when the program does not compile.
//!
//! The probe compiles through the C API, whose 4096-byte `error_msg` carries
//! the one-line summary: "parse failed for x.dsp: errors=1, recoveries=0,
//! diagnostics=1" tells that there is an error and not where. The probe reads
//! `getCCompleteCraneliftDSPFactoryError` instead, and prints what `faust-rs`
//! prints: the summary, then location, source snippet, notes and fixes.

use cranelift_ffi::probe::engine::Factory;
use std::process::Command;

/// An unclosed parenthesis on line 2, column 21.
const UNCLOSED: &str = "// a comment line\nprocess = _ : *(0.5 ;\n";

/// A symbol that does not exist, used on line 2.
const UNDEFINED: &str = "gain = 0.5;\nprocess = _ * gian;\n";

fn compile_error(source: &str) -> String {
    match Factory::compile_from_string("compile_error_test", source, &[], true, 0) {
        Ok(_) => panic!("the program compiled"),
        Err(message) => message,
    }
}

#[test]
fn a_syntax_error_has_its_location_snippet_and_fix() {
    let message = compile_error(UNCLOSED);
    assert!(message.starts_with("parse failed for"), "{message}");
    assert!(
        message.contains(":2:21: error [FRS-PARSE-0001]"),
        "{message}"
    );
    assert!(message.contains("  2 | process = _ : *(0.5 ;"), "{message}");
    assert!(message.contains("insert `)`"), "{message}");
}

#[test]
fn an_undefined_symbol_has_its_location_and_snippet() {
    let message = compile_error(UNDEFINED);
    assert!(message.contains("undefined symbol `gian`"), "{message}");
    assert!(
        message.contains(":2:15: error [FRS-EVAL-0002]"),
        "{message}"
    );
    assert!(message.contains("  2 | process = _ * gian;"), "{message}");
}

#[test]
fn a_failure_after_a_success_reports_itself() {
    // The complete text is not reset by a success and is per thread: the
    // second failure must not be answered with the first one's text.
    let first = compile_error(UNCLOSED);
    assert!(Factory::compile_from_string("ok", "process = _;", &[], true, 0).is_ok());
    let second = compile_error(UNDEFINED);
    assert!(first.contains("FRS-PARSE-0001"));
    assert!(
        second.contains("FRS-EVAL-0002") && !second.contains("FRS-PARSE-0001"),
        "{second}"
    );
}

#[test]
fn the_binary_prints_the_diagnostic_and_fails() {
    let path = std::env::temp_dir().join(format!(
        "faustprobe_compile_error_{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, UNCLOSED).expect("write dsp");
    let out = Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .args(["-n", "4"])
        .arg(&path)
        .output()
        .expect("run faustprobe");
    let _ = std::fs::remove_file(&path);

    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "nothing is rendered");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("faustprobe: parse failed for"),
        "{stderr}"
    );
    assert!(stderr.contains(":2:21: error [FRS-PARSE-0001]"), "{stderr}");
    assert!(stderr.contains("  2 | process = _ : *(0.5 ;"), "{stderr}");
    assert!(stderr.contains("^ unexpected token"), "{stderr}");
}
