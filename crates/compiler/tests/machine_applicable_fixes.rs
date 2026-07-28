//! Independent checker for the machine-applicable fix contract (G8).
//!
//! A fix marked `machine_applicable` claims that applying its edits verbatim
//! repairs the diagnostic. This checker holds it to that claim end to end: it
//! drives the real binary, applies the edits to the source bytes, recompiles,
//! and requires both that the targeted diagnostic is gone and that no new one
//! took its place.
//!
//! Anything weaker than `machine_applicable` is deliberately *not* applied
//! here. `maybe_incorrect` edits are allowed to change DSP semantics, so
//! "compiles afterwards" would not be evidence of anything.

use std::path::PathBuf;
use std::process::Command;

/// Absolute path to the built `faust-rs` binary under test.
fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_faust-rs"))
}

/// Writes `source` to a scratch file and returns the v2 diagnostics payload.
fn check_json(dir: &std::path::Path, name: &str, source: &str) -> serde_json::Value {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("scratch DSP should be writable");
    let output = Command::new(bin_path())
        .arg("--check")
        .arg(&path)
        .arg("--error-format")
        .arg("json")
        .output()
        .expect("faust-rs should run");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document for {name}: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// Applies one fix's edits to `source`.
///
/// Edits are applied back to front so that an earlier edit cannot shift the
/// offsets of a later one — the schema promises they are ordered and
/// non-overlapping, which is exactly what makes this safe.
fn apply_edits(source: &str, fix: &serde_json::Value) -> String {
    let mut edits = fix["edits"]
        .as_array()
        .expect("a fix must carry an edits array")
        .clone();
    edits.sort_by_key(|edit| {
        std::cmp::Reverse(edit["range"]["start"].as_u64().expect("start offset"))
    });

    let mut patched = source.to_owned();
    for edit in edits {
        let start =
            usize::try_from(edit["range"]["start"].as_u64().expect("start")).expect("start");
        let end = usize::try_from(edit["range"]["end"].as_u64().expect("end")).expect("end");
        let replacement = edit["replacement"].as_str().expect("replacement text");
        patched.replace_range(start..end, replacement);
    }
    patched
}

/// Returns every `machine_applicable` fix in the payload, with its diagnostic's
/// stable code.
fn machine_applicable_fixes(payload: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    payload["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .flat_map(|diagnostic| {
            let code = diagnostic["code"].as_str().unwrap_or_default().to_owned();
            diagnostic["fixes"]
                .as_array()
                .expect("fixes array")
                .iter()
                .filter(|fix| fix["applicability"] == "machine_applicable")
                .map(move |fix| (code.clone(), fix.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Sources whose repair the compiler claims to know exactly.
const REPAIRABLE: &[(&str, &str)] = &[
    ("missing_semicolon.dsp", "process = _\n"),
    ("missing_paren.dsp", "process = (_ , _;\n"),
];

#[test]
fn applying_a_machine_applicable_fix_removes_the_diagnostic_it_targets() {
    let dir = std::env::temp_dir().join("faust-rs-machine-fixes");
    std::fs::create_dir_all(&dir).expect("scratch directory should be creatable");

    let mut applied = 0usize;
    for (name, source) in REPAIRABLE {
        let payload = check_json(&dir, name, source);
        let fixes = machine_applicable_fixes(&payload);
        if fixes.is_empty() {
            // Not every recovery is unambiguous enough to earn an exact edit,
            // and refusing to guess is the correct outcome. The assertion
            // below still requires that at least one case does produce one.
            continue;
        }

        for (code, fix) in fixes {
            let patched = apply_edits(source, &fix);
            assert_ne!(
                &patched.as_str(),
                source,
                "{name}: a machine-applicable fix must actually change the source"
            );

            let after = check_json(&dir, name, &patched);
            let remaining = after["diagnostics"]
                .as_array()
                .expect("diagnostics array")
                .iter()
                .map(|diagnostic| diagnostic["code"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>();

            assert!(
                !remaining.contains(&code),
                "{name}: applying `{}` left {code} in place\npatched source:\n{patched}",
                fix["title"]
            );
            assert!(
                !remaining
                    .iter()
                    .any(|code| code.starts_with("FRS-PARSE-") || code.starts_with("FRS-LEX-")),
                "{name}: applying `{}` introduced a new parse error: {remaining:?}\npatched source:\n{patched}",
                fix["title"]
            );
            applied += 1;
        }
    }

    assert!(
        applied > 0,
        "no machine-applicable fix was exercised; the contract would be vacuous"
    );
}

#[test]
fn a_semantic_suggestion_is_never_marked_machine_applicable() {
    // Renaming to a visible symbol is reachable but changes which definition
    // runs, so it must stay reviewable.
    let dir = std::env::temp_dir().join("faust-rs-machine-fixes");
    std::fs::create_dir_all(&dir).expect("scratch directory should be creatable");
    let payload = check_json(
        &dir,
        "typo.dsp",
        "filter(x) = x * 0.5;\nprocess = filtre;\n",
    );

    assert!(
        machine_applicable_fixes(&payload).is_empty(),
        "a rename suggestion must not claim to be safe to apply blindly: {payload}"
    );
}
