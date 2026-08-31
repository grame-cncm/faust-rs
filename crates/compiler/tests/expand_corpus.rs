//! Differential tests for `-e` expansion against the captured C++ reference.
//!
//! Scope:
//! - Every fixture in `tests/expand/dsp` that has a recorded oracle is expanded
//!   and compared line by line with `tests/expand/oracle`.
//! - Fixtures without an oracle (faust-rs extensions, and the one program the
//!   reference compiler cannot expand at all) are still required to expand.
//! - Expansion is idempotent and the document layout is stable.
//!
//! Host-dependent lines (compiler version, option spelling, library paths)
//! and the libraries' own editorial metadata (versions, license strings) are
//! normalized exactly as `xtask expand-oracle` normalizes them when recording,
//! so a difference here is a difference in what the two compilers actually
//! emit — not in which faustlibraries release happens to be on the machine.

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
/// version, option spelling, installation-dependent library paths, and the
/// editorial metadata the standard libraries declare about themselves.
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
        } else if let Some(key) = volatile_library_metadata_key(line) {
            out.push_str(&format!("declare {key} \"<lib>\";\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Returns the declare key when a line carries library self-description that
/// changes with every faustlibraries release (version bumps, license-string
/// normalizations, copyright edits). The key survives normalization so a
/// missing or reordered declaration still fails; only the upstream-editable
/// value is masked.
fn volatile_library_metadata_key(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("declare ")?;
    let (key, _) = rest.split_once(' ')?;
    const VOLATILE_SUFFIXES: [&str; 4] = [
        "_lib_version",
        "_lib_license",
        "_lib_copyright",
        "_lib_author",
    ];
    VOLATILE_SUFFIXES
        .iter()
        .any(|suffix| key.ends_with(suffix))
        .then_some(key)
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

// ── Round trip ────────────────────────────────────────────────────────────────

/// Compiles one source string to C++ with no import search path at all.
fn compile_expansion_to_cpp(source: &str) -> String {
    let source = source.to_owned();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .compile_source_to_cpp(
                    "expanded.dsp",
                    &source,
                    &codegen::backends::cpp::CppOptions::default(),
                )
                .expect("an expansion must compile on its own")
        })
        .expect("spawn")
        .join()
        .expect("no panic")
}

/// Compiles one fixture file to C++ with its normal search paths.
fn compile_fixture_to_cpp(path: &Path) -> String {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .compile_file_to_cpp(&path, &[], &codegen::backends::cpp::CppOptions::default())
                .unwrap_or_else(|error| panic!("{} must compile: {error}", path.display()))
        })
        .expect("spawn")
        .join()
        .expect("no panic")
}

/// Drops the lines that describe the compilation rather than the DSP.
///
/// `m->declare(...)` carries `filename`, `compile_options`, the per-library
/// keys, `library_path*` and `version` — all of which the expansion
/// legitimately changes. Everything else is the algorithm, compared verbatim:
/// generated variable names describe the program, so an expansion and its
/// original must produce the same ones.
fn algorithm_only(generated: &str) -> Vec<&str> {
    generated
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim_start().starts_with("m->declare("))
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn expansions_are_self_contained_and_preserve_generated_code() {
    // This is the property that makes expansion useful, and it is stronger
    // than matching the reference text: an expansion must compile with no
    // library search path, and produce the same DSP algorithm as compiling
    // the original.
    for fixture in fixtures() {
        let expanded = expand(&fixture);
        let from_expansion = compile_expansion_to_cpp(&expanded);
        let direct = compile_fixture_to_cpp(&fixture);
        assert_eq!(
            algorithm_only(&direct),
            algorithm_only(&from_expansion),
            "compiling the expansion of {} produced different code",
            fixture.display()
        );
    }
}

#[test]
fn generated_names_do_not_depend_on_what_was_evaluated_first() {
    // Recursion carriers used to be numbered by arena node id, so the same
    // DSP compiled directly and compiled from its own expansion produced
    // `fRec157` and `fRec161`. The names must describe the program, not the
    // session that compiled it.
    for fixture in fixtures() {
        let direct = compile_fixture_to_cpp(&fixture);
        let from_expansion = compile_expansion_to_cpp(&expand(&fixture));
        assert_eq!(
            generated_state_names(&direct),
            generated_state_names(&from_expansion),
            "{} names differ between a direct compilation and its expansion",
            fixture.display()
        );
    }
}

/// Collects the generated state-carrier names appearing in one C++ module.
fn generated_state_names(generated: &str) -> std::collections::BTreeSet<String> {
    const PREFIXES: [&str; 6] = ["fRec", "iRec", "fRecCur", "iRecCur", "fVec", "iVec"];
    let mut out = std::collections::BTreeSet::new();
    let bytes = generated.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let word = &generated[start..index];
            if PREFIXES.iter().any(|prefix| word.starts_with(prefix)) {
                out.insert(word.to_owned());
            }
        } else {
            index += 1;
        }
    }
    out
}

#[test]
fn every_corpus_program_expands_into_something_that_compiles() {
    // `tests/expand/` is organized by construct family, which makes it a good
    // differential against the reference but a narrow test of the printer: it
    // held one simple `fad` fixture and no `rad` at all, so an expansion that
    // dropped parentheses around AD call arguments — and therefore re-parsed
    // with the wrong arity — passed every check while thirteen `tests/corpus`
    // programs could not be expanded at all. Breadth belongs here.
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("the DSP corpus must exist")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("dsp"))
        .collect();
    programs.sort();

    let mut expanded_count = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for program in &programs {
        // The property is that expansion preserves compilability, so the
        // subject is the programs that compile. The corpus deliberately holds
        // some that do not (the `err_*` family, and shapes the lowering does
        // not support), and their expansions must fail too — as they do.
        if compile_fixture_fallible(program).is_err() {
            continue;
        }
        match expand_fallible(program) {
            Ok(expansion) => {
                if let Err(error) = compile_expansion_fallible(&expansion) {
                    failures.push(format!(
                        "{}: expansion does not compile: {error}",
                        program.display()
                    ));
                }
            }
            Err(error) => {
                failures.push(format!("{}: does not expand: {error}", program.display()));
            }
        }
        expanded_count += 1;
    }

    assert!(
        failures.is_empty(),
        "{} of {expanded_count} expansions do not compile on their own:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        expanded_count > 150,
        "only {expanded_count} corpus programs expanded; the corpus looks truncated"
    );
}

/// Compiles one fixture file, returning the error as text instead of panicking.
fn compile_fixture_fallible(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .compile_file_to_cpp(&path, &[], &codegen::backends::cpp::CppOptions::default())
                .map_err(|error| error.to_string())
        })
        .expect("spawn")
        .join()
        .expect("no panic")
}

/// Expands one program, returning the compiler's error instead of panicking.
fn expand_fallible(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .expand_file_to_dsp(&path, &[], &[])
                .map_err(|error| error.to_string())
        })
        .expect("spawn")
        .join()
        .expect("no panic")
}

/// Compiles one expansion with no search path, returning the error as text.
fn compile_expansion_fallible(source: &str) -> Result<String, String> {
    let source = source.to_owned();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .compile_source_to_cpp(
                    "expanded.dsp",
                    &source,
                    &codegen::backends::cpp::CppOptions::default(),
                )
                .map_err(|error| error.to_string())
        })
        .expect("spawn")
        .join()
        .expect("no panic")
}

#[test]
fn declare_values_containing_backslashes_survive_re_expansion() {
    // Regression: `declare` values were emitted with backslashes escaped while
    // the Faust lexer's string rule (`"[^"]*"`) performs no escape processing.
    // Reading an expansion back therefore saw the escape itself as data and
    // escaped it again, so the value doubled on every pass and the document
    // never converged.
    //
    // On Unix no path contains a backslash, so nothing exercised it and only
    // the Windows CI runner failed — on `library_path` entries holding paths
    // like `D:\a\faust-rs\faustlibraries\stdfaust.lib`. This drives the same
    // shape through ordinary metadata so any platform catches a relapse.
    // A raw string so the DSP text holds single backslashes, exactly as a
    // Windows path does.
    let source = r#"declare winpath "D:\a\faust-rs\libs\stdfaust.lib";
process = 0;"#;

    let once = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .expand_source_to_dsp("win.dsp", source, &[], &[])
                .expect("expansion must succeed")
        })
        .expect("spawn")
        .join()
        .expect("no panic");

    let declared = once
        .lines()
        .find(|line| line.starts_with("declare winpath "))
        .expect("the value must survive into the header");
    assert!(
        declared.contains(r"D:\a\faust-rs\libs\stdfaust.lib"),
        "the value was altered on the way out: {declared}"
    );

    let twice = re_expand(&once);
    let thrice = re_expand(&twice);
    assert_eq!(twice, thrice, "backslash values must reach a fixed point");
}
