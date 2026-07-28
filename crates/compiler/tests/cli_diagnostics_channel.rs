//! Checker for the "clean machine channel" contract (P0 of
//! `porting/mcp-server-analysis-and-plan-2026-07-21-en.md`).
//!
//! This is the independent checker for D1 (JSON diagnostics alone on
//! stdout) and D2 (`--check` always emits a payload, success and failure
//! sharing one schema). It spawns the actual built `faust-rs` binary
//! (`CARGO_BIN_EXE_faust-rs`) rather than calling library internals, because
//! the property under test -- "what bytes land on which stream" -- is a
//! property of process-level `println!`/`eprintln!` orchestration in
//! `crates/compiler/src/cli/runner.rs` and `.../diagnostics.rs`, not of the
//! `compiler` library API.
//!
//! Coverage is one success case and one failure case per stage-family
//! namespace (LEX/PARSE/SRC/EVAL/PROP/COMP/FIR/SFIR), all driven through
//! `--check --error-format json` so success and failure go through the same
//! mode. Two namespaces have no natural corpus fixture:
//!
//! - **SRC**: `FRS-SRC-*` codes are unused dead code (see
//!   `docs/diagnostics-codes-en.md`); the real-world failure tied to that
//!   pipeline stage (an unresolved `import`) surfaces through the D1
//!   no-bundle fallback (`code: null`) instead -- see
//!   `fixtures/cli_diagnostics/src_missing_import.dsp`.
//! - **LEX**: `FRS-LEX-0001`'s call site is live but unreachable from any
//!   DSP text (the lexer's catch-all rule matches every byte). There is no
//!   fixture that can drive it through the CLI; instead
//!   `crates/compiler/src/cli/tests.rs` has a synthetic-diagnostic unit test
//!   for the LEX code shape (documented there and in this module's report).
//!
//! `FRS-FIR-*` errors require a compiler bug and are unreachable, but the
//! *warning* code `FRS-FIR-0002` is reachable from any DSP under
//! `--fir-verify-strict`, which promotes warnings to fatal: an ordinary
//! passthrough already raises `FIR-M07` ("expected DSP API function
//! 'getSampleRate' is not declared"). The FIR case therefore uses a plain
//! corpus DSP.
//!
//! (An earlier revision of this module used a constant-zero-division fixture
//! here, on the belief that a dedicated fixture was needed to reach the FIR
//! verifier at all. That was wrong on two counts: strict mode fires on every
//! DSP, and constant division by zero is now rejected earlier, in signal
//! preparation, matching C++ `mterm::operator/=`. That fixture now backs
//! `division_by_zero_is_a_clean_diagnostic_not_a_panic` below.)

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Absolute path to the built `faust-rs` binary under test.
fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_faust-rs"))
}

/// Resolves a path under the shared `tests/corpus/` fixture directory
/// (workspace-relative, shared with every other crate's differential tests).
fn corpus_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(name)
}

/// Resolves a path under this crate's own `tests/fixtures/cli_diagnostics/`
/// directory, used for the two namespaces with no natural corpus case.
fn local_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cli_diagnostics")
        .join(name)
}

/// Runs `faust-rs <path> --check --error-format json [extra_args...]` and
/// returns the raw process output.
fn run_check_json(path: &Path, extra_args: &[&str]) -> Output {
    Command::new(bin_path())
        .arg(path)
        .arg("--check")
        .arg("--error-format")
        .arg("json")
        .args(extra_args)
        .output()
        .expect("failed to spawn faust-rs")
}

/// A parsed `--check --error-format json` result: exit status plus stdout
/// decoded as UTF-8 and parsed as JSON.
struct CheckResult {
    success: bool,
    stdout_raw: Vec<u8>,
    stdout: serde_json::Value,
    stderr: String,
}

/// Runs the check and asserts the D1 "no leading/trailing non-JSON bytes"
/// contract: stdout must decode as UTF-8 and parse as JSON with nothing
/// else around it (`serde_json::from_slice` on the *entire* byte buffer,
/// not a trimmed/scraped substring).
fn run_and_assert_clean_json(path: &Path, extra_args: &[&str]) -> CheckResult {
    let output = run_check_json(path, extra_args);
    let stdout_raw = output.stdout.clone();
    let stdout_text = String::from_utf8(output.stdout)
        .unwrap_or_else(|e| panic!("stdout for {} was not valid UTF-8: {e}", path.display()));
    let stdout: serde_json::Value = serde_json::from_str(&stdout_text).unwrap_or_else(|e| {
        panic!(
            "stdout for {} did not parse as a single JSON document \
             (leading/trailing non-JSON bytes present): {e}\n--- stdout ---\n{stdout_text}",
            path.display()
        )
    });
    assert!(
        stdout["diagnostics"].is_array(),
        "expected a top-level \"diagnostics\" array for {}, got: {stdout}",
        path.display()
    );
    CheckResult {
        success: output.status.success(),
        stdout_raw,
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// First diagnostic's `code` field, or panics if the array is empty.
fn first_code(result: &CheckResult) -> &str {
    result.stdout["diagnostics"][0]["code"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "expected diagnostics[0].code to be a string, got: {}",
                result.stdout
            )
        })
}

/// First diagnostic's `message` field, or panics if the array is empty.
fn first_message(result: &CheckResult) -> &str {
    result.stdout["diagnostics"][0]["message"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "expected diagnostics[0].message to be a string, got: {}",
                result.stdout
            )
        })
}

// ─── D2: success shares the same schema as failure ────────────────────────

#[test]
fn check_json_success_has_empty_diagnostics_no_leading_bytes_exit_0() {
    let result = run_and_assert_clean_json(&corpus_path("rep_01_passthrough.dsp"), &[]);
    assert!(
        result.success,
        "expected exit 0 on a valid DSP, stderr: {}",
        result.stderr
    );
    assert_eq!(
        result.stdout["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "success must report an empty diagnostics array so success and \
         failure share one schema, got: {}",
        result.stdout
    );
    assert!(
        result.stdout_raw.starts_with(b"{"),
        "stdout must start with '{{' with no leading bytes, got: {:?}",
        String::from_utf8_lossy(&result.stdout_raw)
    );
}

// ─── D1 x D2: one failure case per stage-family namespace ──────────────────

#[test]
fn check_json_parse_family_failure_is_clean() {
    let result = run_and_assert_clean_json(&corpus_path("err_01_parse_missing_rhs.dsp"), &[]);
    assert!(!result.success, "expected exit 1 on a parse failure");
    assert!(
        first_code(&result).starts_with("FRS-PARSE-"),
        "expected a FRS-PARSE-* code, got {}",
        first_code(&result)
    );
}

#[test]
fn check_json_eval_family_failure_is_clean() {
    let result = run_and_assert_clean_json(&corpus_path("err_02_eval_missing_process.dsp"), &[]);
    assert!(!result.success, "expected exit 1 on an eval failure");
    assert!(
        first_code(&result).starts_with("FRS-EVAL-"),
        "expected a FRS-EVAL-* code, got {}",
        first_code(&result)
    );
}

#[test]
fn check_json_propagate_family_failure_is_clean() {
    let result =
        run_and_assert_clean_json(&corpus_path("err_03_propagate_split_mismatch.dsp"), &[]);
    assert!(!result.success, "expected exit 1 on a propagate failure");
    assert!(
        first_code(&result).starts_with("FRS-PROP-"),
        "expected a FRS-PROP-* code, got {}",
        first_code(&result)
    );
}

#[test]
fn check_json_compiler_type_family_failure_is_clean() {
    // FRS-COMP-0004 (signal type / sigtype validation); the only reachable
    // FRS-COMP-* code today (see docs/diagnostics-codes-en.md).
    let result = run_and_assert_clean_json(&corpus_path("rep_74_soundfile_basic.dsp"), &[]);
    assert!(
        !result.success,
        "expected exit 1 on a type-validation failure"
    );
    assert_eq!(first_code(&result), "FRS-COMP-0004");
}

#[test]
fn check_json_sfir_family_failure_is_clean() {
    let result = run_and_assert_clean_json(&corpus_path("err_fad_rad_temporal.dsp"), &[]);
    assert!(
        !result.success,
        "expected exit 1 on a signal->FIR lowering failure"
    );
    assert!(
        first_code(&result).starts_with("FRS-SFIR-"),
        "expected a FRS-SFIR-* code, got {}",
        first_code(&result)
    );

    // The diagnostic already carries the code, so the message must not repeat
    // it. Using the error's Display here used to render
    // "error [FRS-SFIR-0004] [FRS-SFIR-0004] signal preparation failed: ...".
    let message = first_message(&result);
    assert!(
        !message.contains(first_code(&result)),
        "the message must not repeat the code the diagnostic already carries, got: {message}"
    );
}

#[test]
fn check_json_fir_family_failure_is_clean() {
    // `--fir-verify-strict` promotes FIR verifier warnings to fatal, and
    // FIR-M07 ("expected DSP API function 'getSampleRate' is not declared")
    // is raised for an ordinary DSP -- no special fixture needed.
    let result = run_and_assert_clean_json(
        &corpus_path("rep_01_passthrough.dsp"),
        &["--fir-verify-strict"],
    );
    assert!(!result.success, "expected exit 1 under --fir-verify-strict");
    assert!(
        first_code(&result).starts_with("FRS-FIR-"),
        "expected a FRS-FIR-* code, got {}",
        first_code(&result)
    );
}

/// A backend emission failure must carry `FRS-CODEGEN-0001` plus the backend
/// and its own `FRS-CGEN-*` code as notes.
///
/// Until 2026-07-21 all five codegen variants returned `None` from
/// `CompilerError::diagnostic_bundle()`, so a backend failure reached the user
/// through the `code: null` envelope. One `FRS-CODEGEN-*` code covers every
/// backend on purpose: the backends already own a 27-code
/// `FRS-CGEN-<LANG>-NNNN` taxonomy, carried here as a note exactly as
/// `FRS-FIR-0002` carries `fir_code=...`.
///
/// # Choice of fixture
///
/// This test needs a program the WASM backend rejects *at emission*, and the
/// choice matters more than it looks. It used to use
/// `err_rad_delay_temporal_unsupported.dsp`, which the backend rejected only
/// because its RAD reverse-time loop was outside the WASM subset — a gap we
/// always intended to close, and closing it (2026-07-26) silently removed this
/// test's premise: the program compiled, exit status became 0, and the test
/// failed in CI for a reason unrelated to what it checks.
///
/// `rep_77_foreign_variable.dsp` is anchored differently. A foreign variable is
/// a symbol resolved by the host C toolchain; a WASM module has no such thing,
/// so the rejection is structural rather than a to-do. Prefer that kind of
/// anchor when a test needs a backend to fail: an unimplemented feature is a
/// moving target, an impossible one is not.
#[test]
fn codegen_backend_failure_is_structured() {
    let output = Command::new(bin_path())
        .arg(corpus_path("rep_77_foreign_variable.dsp"))
        .args(["-lang", "wasm", "--error-format", "json"])
        .output()
        .expect("failed to spawn faust-rs");
    assert!(
        !output.status.success(),
        "expected exit 1 — if this program now compiles to WASM, the test needs \
         a different fixture, not a relaxed assertion (see the doc comment)"
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be one clean JSON document: {e}\n{stdout}"));
    let diag = &parsed["diagnostics"][0];

    assert_eq!(diag["code"], "FRS-CODEGEN-0001");
    assert_eq!(diag["stage"], "codegen");
    let notes: Vec<&str> = diag["notes"]
        .as_array()
        .expect("notes must be an array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("backend: wasm")),
        "the failing backend must be named, got {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("codegen_code=FRS-CGEN-")),
        "the backend's own stable code must travel as a note, got {notes:?}"
    );
}

/// Division by a constant zero must be a structured diagnostic, not a panic.
///
/// Parity: the reference C++ compiler rejects `process = _ / (0 : *(0));`
/// with `ERROR : division by 0 in IN[0] / 0.0f` and exit status 1 (verified
/// directly, and unaffected by `-wall`). Before this test existed, faust-rs
/// printed raw panic text to stderr, swallowed the unwind in
/// `simplify_signals_fastlane`, and then reported `Check OK: 0 diagnostics`
/// with exit 0 -- a false green on a program the reference compiler rejects.
#[test]
fn division_by_zero_is_a_clean_diagnostic_not_a_panic() {
    let result = run_and_assert_clean_json(&local_fixture("fir_constant_zero_division.dsp"), &[]);
    assert!(
        !result.success,
        "expected exit 1 on a constant division by zero, matching C++"
    );
    assert_eq!(first_code(&result), "FRS-SFIR-0004");
    assert!(
        first_message(&result).contains("division by 0"),
        "expected the message to name the division by zero, got {}",
        first_message(&result)
    );
    assert!(
        !result.stderr.contains("panicked"),
        "the expected rejection must not print panic text to stderr, got: {}",
        result.stderr
    );
}

/// An unresolved import must carry `FRS-SRC-0002`, a span at the directive,
/// and the list of searched directories.
///
/// Until 2026-07-21 the whole `FRS-SRC-*` family was declared but never
/// constructed, so this failure reached the user through the `code: null`
/// no-bundle fallback with no span and no notes — for what is the single most
/// common newcomer error. The reference C++ compiler reports only
/// `ERROR : unable to open file <name>`, so the span and the searched-path
/// list are deliberately more informative than parity.
#[test]
fn check_json_src_family_unresolved_import_is_structured() {
    let result = run_and_assert_clean_json(&local_fixture("src_missing_import.dsp"), &[]);
    assert!(!result.success, "expected exit 1 on an unresolved import");
    assert_eq!(first_code(&result), "FRS-SRC-0002");
    assert!(
        first_message(&result).contains("cannot resolve import"),
        "unexpected message: {}",
        first_message(&result)
    );

    let labels = result.stdout["diagnostics"][0]["labels"]
        .as_array()
        .expect("labels must be an array");
    assert_eq!(
        labels.len(),
        1,
        "expected one label pointing at the import directive, got: {}",
        result.stdout["diagnostics"][0]
    );
    let line = labels[0]["line"].as_u64().expect("label must carry a line");
    assert!(line >= 1, "label line must be 1-based, got {line}");

    let notes: Vec<&str> = result.stdout["diagnostics"][0]["notes"]
        .as_array()
        .expect("notes must be an array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("searched")),
        "the diagnostic must list the searched directories, got: {notes:?}"
    );
}

// ─── Human format stays byte-for-byte unchanged (regression guard) ────────

#[test]
fn error_format_human_still_writes_diagnostics_to_stderr_not_stdout() {
    // D1 explicitly requires --error-format human behavior to be preserved
    // byte for byte. This is a coarse regression guard, not a full snapshot:
    // stdout must be empty and stderr must carry the pipeline-failed prefix
    // plus the FRS code, on a plain (not --check) failing compile.
    let output = Command::new(bin_path())
        .arg(corpus_path("err_09_eval_undefined_symbol.dsp"))
        .arg("--error-format")
        .arg("human")
        .output()
        .expect("failed to spawn faust-rs");
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "human mode must not write anything to stdout on failure, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pipeline failed:"),
        "expected the human-mode prefix line on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("FRS-EVAL-"),
        "expected an FRS-EVAL-* code in the human rendering, got: {stderr}"
    );
}

// ─── Dump modes (not --check): success keeps generated output, no JSON mixed in ──

#[test]
fn error_format_json_success_with_dump_mode_is_generated_code_not_diagnostics() {
    // D1's documented rule: on a *successful* compile in a dump mode
    // (--dump-cpp here), stdout carries the generated output as before, and
    // no diagnostics payload is added -- there is nothing to interleave with
    // because the CLI never emits a diagnostics envelope on a dump-mode
    // success today (only --check does, by design, since --check emits no
    // other stdout content).
    let output = Command::new(bin_path())
        .arg(corpus_path("rep_01_passthrough.dsp"))
        .arg("--dump-cpp")
        .arg("--error-format")
        .arg("json")
        .output()
        .expect("failed to spawn faust-rs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim_start().starts_with("/*"),
        "expected generated C++ text on stdout, got: {}",
        &stdout[..stdout.len().min(200)]
    );
    assert!(
        !stdout.contains("\"diagnostics\""),
        "a successful dump-mode compile must not mix a diagnostics payload \
         into the generated-code stdout"
    );
}

#[test]
fn error_format_json_failure_with_dump_mode_is_clean_json_on_stdout() {
    let output = Command::new(bin_path())
        .arg(corpus_path("err_09_eval_undefined_symbol.dsp"))
        .arg("--dump-cpp")
        .arg("--error-format")
        .arg("json")
        .output()
        .expect("failed to spawn faust-rs");
    assert!(!output.status.success());
    let stdout_text = String::from_utf8(output.stdout).expect("stdout must be valid UTF-8");
    let value: serde_json::Value = serde_json::from_str(&stdout_text).unwrap_or_else(|e| {
        panic!("stdout did not parse as a single JSON document: {e}\n{stdout_text}")
    });
    assert!(value["diagnostics"].is_array());
    assert!(
        output.stderr.is_empty(),
        "the human-readable prefix line must be suppressed under \
         --error-format json, got stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}
