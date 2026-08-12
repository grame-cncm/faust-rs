//! Differential tests for `-e` expansion against the captured C++ reference.
//!
//! Scope:
//! - Every fixture in `tests/expand/dsp` that has a recorded oracle is expanded
//!   and compared line by line with `tests/expand/oracle`.
//! - Fixtures without an oracle (faust-rs extensions, and the one program the
//!   reference compiler cannot expand at all) are still required to expand.
//! - Expansion is idempotent and the document layout is stable.
//!
//! The three host-dependent lines are normalized exactly as
//! `xtask expand-oracle` normalizes them when recording, so a difference here
//! is a difference in what the two compilers actually emit.

use std::path::{Path, PathBuf};

use compiler::Compiler;

/// Fixtures with no recorded C++ expansion, and why.
///
/// `031_fad` uses a faust-rs primitive the reference binary does not know.
/// `034_downsampling` is a program the reference *cannot* expand: its
/// `boxppShared::print` tests `isBoxUpsampling` twice
/// (`compiler/boxes/ppbox.cpp:615-617`) and throws on `BoxDownsampling`.
const FIXTURES_WITHOUT_ORACLE: &[&str] = &["031_fad", "034_downsampling"];

fn expand_dir(kind: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("expand")
        .join(kind)
}

/// Returns the corpus fixtures in deterministic name order.
fn fixtures() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(expand_dir("dsp"))
        .expect("the expansion corpus must exist")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("dsp"))
        .collect();
    out.sort();
    out
}

/// Replaces the values that legitimately differ from a C++ expansion.
///
/// Same substitutions `xtask expand-oracle` applies when recording: compiler
/// version, option spelling, and installation-dependent library paths.
fn normalize(expansion: &str) -> String {
    let mut out = String::with_capacity(expansion.len());
    for line in expansion.lines() {
        if line.starts_with("declare version ") {
            out.push_str("declare version \"<version>\";\n");
        } else if line.starts_with("declare compile_options ") {
            out.push_str("declare compile_options \"<options>\";\n");
        } else if let Some(rest) = line.strip_prefix("declare library_path")
            && let Some((index, _)) = rest.split_once(' ')
        {
            out.push_str(&format!("declare library_path{index} \"<path>\";\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Expands one fixture on a thread with room for deep evaluated diagrams.
fn expand(path: &Path) -> String {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .expand_file_to_dsp(&path, &[], &[])
                .unwrap_or_else(|error| panic!("{} must expand: {error}", path.display()))
        })
        .expect("spawn expansion thread")
        .join()
        .expect("expansion thread must not panic")
}

#[test]
fn expansions_match_the_cpp_reference() {
    let oracle_dir = expand_dir("oracle");
    let mut compared = 0usize;

    for fixture in fixtures() {
        let stem = fixture
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("fixture name")
            .to_owned();
        let oracle_path = oracle_dir.join(format!("{stem}.dsp"));

        let Ok(oracle) = std::fs::read_to_string(&oracle_path) else {
            assert!(
                FIXTURES_WITHOUT_ORACLE.contains(&stem.as_str()),
                "{stem} has no recorded C++ expansion; record one with \
                 `cargo run -p xtask -- expand-oracle` or list it in \
                 FIXTURES_WITHOUT_ORACLE with the reason"
            );
            // No reference to compare against, but expansion must still work.
            expand(&fixture);
            continue;
        };

        assert_eq!(
            normalize(&expand(&fixture)),
            oracle,
            "expansion of {stem} differs from the recorded C++ reference"
        );
        compared += 1;
    }

    assert!(
        compared >= 30,
        "the differential covered only {compared} fixtures; the corpus looks truncated"
    );
}

/// Re-expands an already-expanded document.
fn re_expand(expansion: &str) -> String {
    let expansion = expansion.to_owned();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .expand_source_to_dsp("expanded.dsp", &expansion, &[], &[])
                .expect("an expansion must itself expand")
        })
        .expect("spawn")
        .join()
        .expect("no panic")
}

#[test]
fn expansion_settles_after_the_second_pass() {
    // Expansion is not a fixed point after a single pass, and the reference
    // compiler is not either — verified against it on this same corpus. Two
    // things change on the second pass:
    //
    // - the header grows: an expansion declares its own `version` and
    //   `compile_options`, which the next pass reads as ordinary metadata and
    //   re-emits;
    // - the body can shrink: re-evaluating `(65536 : int)` folds it to the
    //   literal `65536`, so a second expansion is a simplification of the
    //   first, not a different program.
    //
    // What must hold — and what makes expansions usable as inputs — is that
    // both effects stop. A document that changed on every pass would mean
    // expansion does not converge.
    for fixture in fixtures() {
        let twice = re_expand(&expand(&fixture));
        let thrice = re_expand(&twice);
        assert_eq!(
            twice,
            thrice,
            "expanding {} does not converge by the third pass",
            fixture.display()
        );
    }
}

#[test]
fn the_header_layout_is_stable() {
    // The first two lines carry the compiler identity and the normalized
    // options, in that order, for every program. Tooling that reads an
    // expansion relies on this, and the C++ short-circuit reads line 1.
    for fixture in fixtures() {
        let expansion = expand(&fixture);
        let lines: Vec<&str> = expansion.lines().collect();
        assert!(
            lines[0].starts_with("declare version \""),
            "{}: first line is {:?}",
            fixture.display(),
            lines[0]
        );
        assert!(
            lines[1].starts_with("declare compile_options \""),
            "{}: second line is {:?}",
            fixture.display(),
            lines[1]
        );
        assert!(
            expansion.ends_with(";\n"),
            "{}: the document must end with the entry-point binding",
            fixture.display()
        );
    }
}
