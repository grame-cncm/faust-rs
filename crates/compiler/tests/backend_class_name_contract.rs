//! Contract checker: textual backends name their DSP `mydsp` by default and
//! honor `-cn` / `--class-name`.
//!
//! # Why this exists
//!
//! The rule was convention rather than contract, and two backends had drifted
//! from it independently: `-lang asc` derived the name from the source file
//! stem, and `-lang interp` built its options with a hardcoded default so the
//! flag was silently ignored. Both were found by hand. This checker turns the
//! convention into something a new backend cannot quietly break.
//!
//! # Why it covers backends that do not exist yet
//!
//! The backend list is not written down here. It is read from the CLI's own
//! `--lang` possible-values list, so a backend added to `CliLang` is picked up
//! with no edit to this file.
//!
//! Applicability is self-selecting rather than an exclusion list to maintain:
//! a backend is subject to the contract when *either* half of it is
//! observable: the default output says `mydsp`, or the requested name shows up
//! under `--class-name`. Both halves are needed, and each was validated by a
//! mutation. Keying on the default alone would let the pre-fix `asc` backend —
//! which named the DSP after the source file — skip the check; keying on the
//! echo alone lets a backend that *ignores* the flag skip it, which is exactly
//! how the `interp` defect stayed invisible on the first draft of this file.
//! "The flag changes the output at all" was also tried and rejected: it
//! false-positives on the Cranelift report, whose JIT entry address differs
//! between runs. Formats carrying no DSP name
//! — the FIR dump, WAT text, the Cranelift report, the WASM binary — never
//! echo the requested name and opt out by construction, as a future one
//! would.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_faust-rs"))
}

/// Reads the `-lang` values straight from the CLI help text.
///
/// Clap has two renderings for a value enum and switches between them on its
/// own: a one-line `[possible values: a, b, c]` when no variant carries help,
/// and a `Possible values:` block of `- name` / `- name: description` lines as
/// soon as one does. Both are parsed here, anchored on the `--lang` section —
/// an earlier version searched the whole help for the first
/// `possible values: ` and silently picked up `--error-format`'s `human, json`
/// the day a `-lang` variant gained a doc comment.
fn lang_values() -> Vec<String> {
    let help = Command::new(bin())
        .arg("--help")
        .output()
        .expect("faust-rs --help must run");
    let text = String::from_utf8_lossy(&help.stdout);
    let section: String = text
        .lines()
        .skip_while(|line| !line.contains("--lang <LANG>"))
        .skip(1)
        // Stop at the next option, so a later option's list cannot be read.
        .take_while(|line| {
            !line.trim_start().starts_with('-') || line.trim_start().starts_with("- ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !section.is_empty(),
        "`--help` must document `--lang <LANG>`:\n{text}"
    );

    let inline = section.find("possible values: ").map(|start| {
        let start = start + "possible values: ".len();
        let end = start
            + section[start..]
                .find(']')
                .unwrap_or_else(|| panic!("unterminated possible-values list:\n{section}"));
        section[start..end]
            .split(',')
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .collect::<Vec<String>>()
    });
    let values = inline.unwrap_or_else(|| {
        section
            .lines()
            .skip_while(|line| !line.trim().eq_ignore_ascii_case("possible values:"))
            .skip(1)
            .map_while(|line| {
                let token = line.trim().strip_prefix("- ")?;
                // A variant carrying help renders as `name: description`.
                Some(
                    token
                        .split_once(':')
                        .map_or(token, |(name, _)| name)
                        .trim()
                        .to_owned(),
                )
            })
            .collect()
    });
    assert!(
        values.len() >= 5,
        "expected the real `-lang` list, parsed: {values:?}\nfrom:\n{section}"
    );
    values
}

fn emit(dsp: &PathBuf, lang: &str, extra: &[&str]) -> String {
    let out = Command::new(bin())
        .arg(dsp)
        .args(["-lang", lang])
        .args(extra)
        .output()
        .unwrap_or_else(|e| panic!("failed to run faust-rs -lang {lang}: {e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fixture() -> PathBuf {
    let dir = std::env::temp_dir().join("frs_backend_class_name_contract");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // A file stem deliberately unlike `mydsp`: a backend that derives the name
    // from the source would produce `named_after_file` and be caught.
    let dsp = dir.join("named_after_file.dsp");
    std::fs::write(&dsp, "process = _ * 0.5;\n").expect("write dsp");
    dsp
}

#[test]
fn textual_backends_default_to_mydsp_and_honor_class_name() {
    let dsp = fixture();
    let mut checked = Vec::new();

    for lang in lang_values() {
        let default_out = emit(&dsp, &lang, &[]);
        let custom_out = emit(&dsp, &lang, &["--class-name", "custom_name"]);
        let defaults_to_mydsp = default_out.contains("mydsp");
        let echoes_request = custom_out.contains("custom_name");
        if !defaults_to_mydsp && !echoes_request {
            // Neither half of the contract is observable: not a DSP-naming
            // format. See the module docs for why the selector is this pair.
            continue;
        }
        assert!(
            defaults_to_mydsp,
            "`-lang {lang}` honors `--class-name` but does not default to \
             `mydsp` — a backend that names the DSP must use the same default \
             as every other one (a source-file-derived name is the usual cause)"
        );
        assert!(
            echoes_request,
            "`-lang {lang}` names the DSP `mydsp` by default but ignored \
             `--class-name`; the flag must override the default name"
        );
        assert!(
            !custom_out.contains("mydsp"),
            "`-lang {lang}` kept `mydsp` alongside the requested `--class-name`"
        );
        checked.push(lang);
    }

    assert!(
        checked.len() >= 5,
        "expected several textual backends to be covered, got {checked:?} — \
         if backends stopped emitting `mydsp`, this checker is no longer \
         guarding anything"
    );
}
