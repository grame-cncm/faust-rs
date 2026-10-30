//! What `faustprobe` says when the program does not compile.
//!
//! The probe compiles through the C API, whose 4096-byte `error_msg` carries
//! the one-line summary: "parse failed for x.dsp: errors=1, recoveries=0,
//! diagnostics=1" tells that there is an error and not where. The probe reads
//! `getCCompleteCraneliftDSPFactoryError` instead, and prints what `faust-rs`
//! prints: the summary, then location, source snippet, notes and fixes.

use cranelift_ffi::factory::set_complete_error_color;
use cranelift_ffi::probe::engine::Factory;
use std::process::{Command, Output};

mod common;
use common::probe_source;

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

// ---------------------------------------------------------------- --color

/// A failing run of the binary on `source`, whose outcome the test environment
/// cannot influence: the variables `--color auto` reads start out unset.
fn failing_run(tag: &str, source: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
    let path =
        std::env::temp_dir().join(format!("faustprobe_color_{tag}_{}.dsp", std::process::id()));
    std::fs::write(&path, source).expect("write dsp");
    let out = Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .args(["-n", "4"])
        .args(args)
        .arg(&path)
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .envs(env.iter().copied())
        .output()
        .expect("run faustprobe");
    let _ = std::fs::remove_file(&path);
    assert!(!out.status.success());
    out
}

fn has_escape(bytes: &[u8]) -> bool {
    bytes.contains(&0x1b)
}

#[test]
fn color_auto_stays_plain_on_a_pipe_and_follows_the_environment() {
    let run = |tag, env| failing_run(tag, UNCLOSED, &[], env).stderr;
    assert!(!has_escape(&run("auto", &[])));
    assert!(!has_escape(&run("no", &[("NO_COLOR", "1")])));
    assert!(has_escape(&run("force", &[("CLICOLOR_FORCE", "1")])));
}

#[test]
fn an_explicit_color_choice_outranks_the_environment() {
    let always = failing_run(
        "always",
        UNCLOSED,
        &["--color", "always"],
        &[("NO_COLOR", "1")],
    );
    assert!(has_escape(&always.stderr));
    let never = failing_run(
        "never",
        UNCLOSED,
        &["--color", "never"],
        &[("CLICOLOR_FORCE", "1")],
    );
    assert!(!has_escape(&never.stderr));
}

#[test]
fn color_always_changes_nothing_but_the_escapes() {
    let plain = failing_run("escapes", UNDEFINED, &["--color", "never"], &[]);
    let colored = failing_run("escapes", UNDEFINED, &["--color", "always"], &[]);
    assert!(colored.stdout.is_empty());
    assert_eq!(
        anstream::adapter::strip_bytes(&colored.stderr).into_vec(),
        plain.stderr
    );
}

#[test]
fn color_survives_the_rewriting_of_an_eval_error() {
    // Not located in the expression, then located in it: `--eval` searches the
    // text of both, for the symbol and for the location.
    for (tag, expr, expected) in [
        ("eval_symbol", "nope + 1", "`nope` is used by <eval 0>"),
        ("eval_location", "1 + (", "<eval 0>:1:"),
    ] {
        let source = "process = _;\n";
        let plain = failing_run(tag, source, &["--color", "never", "--eval", expr], &[]);
        let colored = failing_run(tag, source, &["--color", "always", "--eval", expr], &[]);
        assert!(has_escape(&colored.stderr));
        let stripped = anstream::adapter::strip_bytes(&colored.stderr).into_vec();
        assert_eq!(stripped, plain.stderr);
        let stripped = String::from_utf8_lossy(&stripped);
        assert!(stripped.contains(expected), "{stripped}");
    }
}

#[test]
fn color_always_never_reaches_the_json_channel() {
    let out = failing_run(
        "json",
        UNCLOSED,
        &["--color", "always", "--error-format", "json"],
        &[],
    );
    assert!(!has_escape(&out.stdout));
    assert!(!has_escape(&out.stderr));
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("stdout must stay one JSON document");
}

#[test]
fn complete_error_color_is_opt_in_and_per_thread() {
    let color_on_this_thread = std::thread::spawn(|| {
        set_complete_error_color(true);
        let colored = compile_error(UNDEFINED);
        set_complete_error_color(false);
        (colored, compile_error(UNDEFINED))
    });
    let (colored, restored) = color_on_this_thread.join().expect("join");
    assert!(has_escape(colored.as_bytes()), "{colored}");
    assert!(!has_escape(restored.as_bytes()), "{restored}");
    let plain = compile_error(UNDEFINED);
    assert!(!has_escape(plain.as_bytes()), "{plain}");
    assert_eq!(anstream::adapter::strip_str(&colored).to_string(), plain);
}

// ------------------------------------------------- --error-format json (F4)
//
// The compiler has a typed channel next to its text: diagnostics-v2 JSON, with
// the code, byte ranges and machine-applicable fixes. An agent applies a fix
// from it without reading prose. The probe reaches it at the Rust level, next
// to the per-thread complete text, and under the same rule: a report belongs
// to one failure and must not outlive it.

use cranelift_ffi::probe::engine::last_compile_failure;

fn report_of_last_failure() -> Option<serde_json::Value> {
    last_compile_failure()
        .and_then(|failure| failure.diagnostics_json)
        .map(|text| serde_json::from_str(&text).expect("the report is JSON"))
}

#[test]
fn the_typed_report_of_a_failure_is_kept_with_its_text() {
    let message = compile_error(UNCLOSED);
    let failure = last_compile_failure().expect("a failure was recorded");
    assert_eq!(failure.text, message);
    let report = report_of_last_failure().expect("a syntax error is typed");
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["request"]["backend"], "cranelift");
    let diagnostic = &report["diagnostics"][0];
    assert_eq!(diagnostic["code"], "FRS-PARSE-0001");
    // line 2, column 21: 18 bytes of comment line, then 20
    let fix = &diagnostic["fixes"][0];
    assert_eq!(fix["applicability"], "machine_applicable");
    assert_eq!(fix["edits"][0]["range"]["start"], 38);
    assert_eq!(fix["edits"][0]["replacement"], ")");
    // applying it is all it takes
    let mut fixed = UNCLOSED.to_owned();
    fixed.insert(38, ')');
    assert!(Factory::compile_from_string("fixed", &fixed, &[], true, 0).is_ok());
}

#[test]
fn a_report_describes_the_last_failure_only() {
    compile_error(UNCLOSED);
    compile_error(UNDEFINED);
    let report = report_of_last_failure().expect("typed");
    assert_eq!(report["diagnostics"][0]["code"], "FRS-EVAL-0002");
    // a failure that has no typed diagnostics has no report, and must not be
    // answered with the previous one's: here the arguments are refused before
    // any compilation
    let refused = Factory::compile_from_string_with_args(
        "args",
        "process = _;",
        &[],
        &["-I".to_owned()],
        true,
        0,
    );
    let text = refused.err().expect("a dangling -I is refused");
    let failure = last_compile_failure().expect("recorded");
    assert_eq!(failure.text, text);
    assert_eq!(failure.diagnostics_json, None, "{text}");
}

#[test]
fn error_format_json_prints_the_report_on_stdout_and_the_summary_on_stderr() {
    let (ok, stdout, stderr) = probe_source("json", UNCLOSED, &["--error-format", "json"]);
    assert!(!ok);
    // stdout is one JSON document and nothing else
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
    assert_eq!(report["diagnostics"][0]["code"], "FRS-PARSE-0001");
    assert_eq!(
        report["diagnostics"][0]["fixes"][0]["edits"][0]["range"]["start"],
        38
    );
    // stderr keeps the summary; the rendered text is what the report replaces
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with("faustprobe: parse failed for"),
        "{stderr}"
    );
}

#[test]
fn error_format_json_leaves_any_other_failure_as_text() {
    // a value out of range is not a compile failure: no document
    let gain = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01);\n";
    let (ok, stdout, stderr) = probe_source(
        "range",
        gain,
        &["--error-format", "json", "--set", "gain=7"],
    );
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("outside the range"), "{stderr}");
    // and a run that succeeds prints what it always printed
    let (ok, stdout, _) = probe_source(
        "fine",
        gain,
        &["--error-format", "json", "-n", "2", "--in", "dc"],
    );
    assert!(ok);
    assert_eq!(stdout, "frame,out0\n0,0.5\n1,0.5\n");
}

/// The polyphonic wrapper looks for an `effect` in the file by compiling a
/// wrapped copy, and carries on without one when that fails. The run then
/// ends on something else: the report of the failure it recovered from is not
/// the report of that.
#[test]
fn a_compile_failure_that_was_recovered_from_is_not_reported() {
    let voice = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01) * button(\"gate\");\n";
    let (ok, stdout, stderr) = probe_source(
        "recovered",
        voice,
        &[
            "--error-format",
            "json",
            "--nvoices",
            "2",
            "--set",
            "nope=1",
            "-n",
            "64",
        ],
    );
    assert!(!ok);
    assert!(stderr.contains("no control matching `nope`"), "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
}

#[test]
fn error_format_json_is_refused_with_eval() {
    let (ok, stdout, stderr) = probe_source(
        "eval",
        "g = 0.5;\n",
        &["--error-format", "json", "--eval", "g"],
    );
    assert!(!ok);
    assert!(stdout.is_empty());
    assert!(stderr.contains("--eval"), "{stderr}");
}
