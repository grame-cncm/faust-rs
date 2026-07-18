//! Golden snapshot generation and validation workflows.
//!
//! This module owns Rust and C++ golden snapshot enumeration, generation, and
//! checking. Snapshot paths stay repository-relative so generated artifacts are
//! portable across local checkouts and CI hosts.

use super::*;

// ---------------------------------------------------------------------------
// Golden snapshot workflows
// ---------------------------------------------------------------------------

/// Returns `false` for corpus fixtures that cannot be golden-checked because
/// they import the repo-root `interleave.lib` (the spectral FFT-on-`ondemand`
/// examples). The golden import search path is `tests/corpus` +
/// `/usr/local/share/faust` (see `default_import_search_paths`), which does not
/// include the repository root, so `library("interleave.lib")` never resolves.
/// These examples are exercised by the runtime tests
/// (`crates/compiler/tests/interleave_fft.rs`, the impulse-runner effect
/// checks) instead of by golden snapshots.
pub(crate) fn is_rust_golden_eligible(source_path: &Path) -> bool {
    match fs::read_to_string(source_path) {
        Ok(text) => !text.contains("interleave.lib"),
        Err(_) => true,
    }
}

/// Enumerates the corpus/golden pairs checked by `golden-check`.
///
/// Rust references enumerate directly from `tests/corpus`, while C++ references
/// enumerate the snapshot directories so missing snapshots are reported as
/// absent corpus sources instead of silently skipped.
pub(crate) fn golden_cases_for_check(
    golden_ref: GoldenRef,
) -> Result<Vec<(String, PathBuf)>, io::Error> {
    let root = workspace_root();
    match golden_ref {
        GoldenRef::Rust => {
            let mut cases = Vec::new();
            for file in corpus_files()? {
                if !is_rust_golden_eligible(&file) {
                    continue;
                }
                cases.push((case_name(&file)?, file));
            }
            Ok(cases)
        }
        GoldenRef::Cpp => {
            let golden_root = root.join("tests/golden").join(golden_ref.as_dir_name());
            let mut cases = Vec::new();
            for entry in fs::read_dir(golden_root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let case = entry
                    .file_name()
                    .to_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid golden case directory name",
                        )
                    })?;
                let expected = entry.path().join("compiler_stdout.txt");
                if expected.is_file() {
                    let source = root.join("tests/corpus").join(format!("{case}.dsp"));
                    cases.push((case, source));
                }
            }
            cases.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(cases)
        }
    }
}

/// Golden reference family used by snapshot workflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoldenRef {
    /// Rust-generated reference snapshots under `tests/golden/rust`.
    Rust,
    /// C++ reference snapshots under `tests/golden/cpp`.
    Cpp,
}

impl GoldenRef {
    /// Returns the snapshot subdirectory name for this reference family.
    pub(crate) fn as_dir_name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Cpp => "cpp",
        }
    }
}

/// Returns the on-disk golden snapshot path for one case/reference family.
pub(crate) fn golden_file_for_ref(case: &str, golden_ref: GoldenRef) -> PathBuf {
    workspace_root()
        .join("tests/golden")
        .join(golden_ref.as_dir_name())
        .join(case)
        .join("compiler_stdout.txt")
}

/// Normalizes generated text before snapshot comparison.
pub(crate) fn normalize(text: &str) -> String {
    let mut normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = normalized
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();

    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    normalized = lines.join("\n");
    normalized.push('\n');
    normalized
}
