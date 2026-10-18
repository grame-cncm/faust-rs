//! Optional integration scans for the external `faustlibraries/tests` corpus.
//!
//! These tests are ignored by default because they depend on a local checkout
//! outside the repository and are intended as a parity/work-in-progress probe.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use compiler::Compiler;

/// The faustlibraries checkout used when `FAUST_RS_FAUSTLIBRARIES_ROOT` is
/// unset: `faustlibraries` next to the directory that holds this repository.
fn default_faustlibraries_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .map_or_else(std::path::PathBuf::new, |dir| dir.join("faustlibraries"))
}

/// Its `tests` directory, used when `FAUST_RS_FAUSTLIBRARIES_TESTS_DIR` is unset.
fn default_faustlibraries_tests_dir() -> PathBuf {
    default_faustlibraries_root().join("tests")
}

fn faustlibraries_root() -> Option<PathBuf> {
    env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let path = default_faustlibraries_root();
            path.exists().then_some(path)
        })
}

fn faustlibraries_tests_dir() -> Option<PathBuf> {
    env::var_os("FAUST_RS_FAUSTLIBRARIES_TESTS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let path = default_faustlibraries_tests_dir();
            path.exists().then_some(path)
        })
}

fn faustlibraries_dsp_paths() -> Vec<PathBuf> {
    let Some(dir) = faustlibraries_tests_dir() else {
        eprintln!(
            "Skipping faustlibraries scan: set FAUST_RS_FAUSTLIBRARIES_TESTS_DIR or install {}",
            default_faustlibraries_tests_dir().display()
        );
        return Vec::new();
    };

    let mut files = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dsp"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn top_level_test_entrypoints(path: &Path) -> Vec<String> {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                return None;
            }

            let name_end = trimmed.find(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))?;
            let name = &trimmed[..name_end];
            if name.is_empty()
                || !name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
            {
                return None;
            }

            let mut rest = &trimmed[name_end..];
            rest = rest.trim_start();
            if rest.starts_with('(') {
                let close = rest.find(')')?;
                rest = &rest[(close + 1)..];
                rest = rest.trim_start();
            }

            if rest.starts_with('=') && name.ends_with("_test") {
                Some(name.to_owned())
            } else {
                None
            }
        })
        .collect()
}

#[test]
#[ignore = "local external corpus scan"]
fn faustlibraries_tests_parse_all_dsp_files() {
    let files = faustlibraries_dsp_paths();
    if files.is_empty() {
        return;
    }

    let compiler = Compiler::new();
    let Some(root) = faustlibraries_root() else {
        panic!(
            "faustlibraries root missing: set FAUST_RS_FAUSTLIBRARIES_ROOT or install {}",
            default_faustlibraries_root().display()
        );
    };
    let search_paths = vec![root];

    let failures = files
        .iter()
        .filter_map(|path| {
            compiler
                .compile_file(path, &search_paths)
                .err()
                .map(|err| format!("{}: {err}", path.display()))
        })
        .collect::<Vec<_>>();

    if !failures.is_empty() {
        let preview = failures.into_iter().take(20).collect::<Vec<_>>().join("\n");
        panic!("faustlibraries parse scan found failures:\n{preview}");
    }
}

#[test]
#[ignore = "local external corpus scan"]
fn faustlibraries_tests_eval_first_test_entrypoint_per_file() {
    let files = faustlibraries_dsp_paths();
    if files.is_empty() {
        return;
    }

    let Some(root) = faustlibraries_root() else {
        panic!(
            "faustlibraries root missing: set FAUST_RS_FAUSTLIBRARIES_ROOT or install {}",
            default_faustlibraries_root().display()
        );
    };
    let search_paths = vec![root];
    let mut failures = Vec::new();

    for path in &files {
        let entrypoint = top_level_test_entrypoints(path)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no *_test entrypoint found in {}", path.display()));
        eprintln!("eval scan: {} :: {}", path.display(), entrypoint);
        let compiler = Compiler::new().with_process_name(entrypoint.clone());
        if let Err(err) = compiler.compile_file_to_signals(path, &search_paths) {
            failures.push(format!("{} :: {} => {err}", path.display(), entrypoint));
        }
    }

    if !failures.is_empty() {
        let preview = failures.into_iter().take(20).collect::<Vec<_>>().join("\n");
        panic!("faustlibraries eval scan found failures:\n{preview}");
    }
}
