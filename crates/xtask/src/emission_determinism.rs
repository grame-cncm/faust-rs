//! Run-to-run C++ emission determinism gate (plan D1).
//!
//! See `porting/scalar-emission-determinism-plan-2026-07-20-en.md`. This
//! module implements the mechanical gate that the plan lands *before* the
//! delay-subsystem ordering fix (D2): for every `tests/impulse-tests/dsp`
//! case, under the three configs exercised by the 396-case byte-identity
//! arbiter (`scalar`, `vec-lv0`, `vec-lv1`, all double precision), emit the
//! same DSP `--passes` times with a **fresh** [`compiler::Compiler`] each
//! time and byte-compare the results. `HashMap`-ordered delay-line
//! collections (`crates/transform/src/signal_fir/delay/`) make struct
//! `fVec*` field order and `lDelayN` numbering flip from run to run on
//! delay-heavy DSPs; this gate detects that class of defect directly,
//! independent of the external R0.5 arbiter worktree.
//!
//! An `--allowlist` freezes currently-known-unstable cases (case id
//! `<dsp-stem>/<config>`) so the gate can land green on day one and go red
//! only on a genuine regression; D2 is expected to shrink the allowlist to
//! empty (plan D2/D3). Findings are deterministic and sorted, repo-relative
//! paths only, matching the reporting discipline of `structure_check.rs`.

use super::*;
use compiler::{Compiler, ComputeMode, RealType};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Corpus directory, relative to the workspace root.
const CORPUS_ROOT: &str = "tests/impulse-tests/dsp";

/// Default number of passes when `--passes` is not given.
const DEFAULT_PASSES: usize = 2;

/// Worker stack size, matched to `vector_coverage.rs`'s compiler workers:
/// the transform pipeline recurses over signal graphs and can need more than
/// the default 2 MiB thread stack on deeply nested corpus cases.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// One emission configuration exercised per corpus case.
struct EmissionConfig {
    /// Config id fragment used in case ids (`<stem>/<name>`).
    name: &'static str,
    /// Compute mode passed to [`compiler::Compiler::with_compute_mode`].
    mode: ComputeMode,
}

/// The three configs the 396-case byte-identity arbiter exercises per DSP,
/// all at double precision (plan D1).
const CONFIGS: [EmissionConfig; 3] = [
    EmissionConfig {
        name: "scalar",
        mode: ComputeMode::Scalar,
    },
    EmissionConfig {
        name: "vec-lv0",
        mode: ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 0,
        },
    },
    EmissionConfig {
        name: "vec-lv1",
        mode: ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 1,
        },
    },
];

/// Outcome of one pass of one (case, config).
#[derive(Clone, PartialEq, Eq)]
enum PassOutcome {
    Emitted(String),
    Failed(String),
}

/// Per-(case, config) verdict after comparing all passes.
enum CaseVerdict {
    /// Every pass produced byte-identical output.
    Stable,
    /// Every pass failed to compile with the identical `Display` text.
    CompileError(String),
    /// At least one pass differs from the first (output or error text).
    Unstable,
}

/// Runs the `emission-determinism` gate.
pub(crate) fn emission_determinism(
    options: EmissionDeterminismArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let passes = options.passes.unwrap_or(DEFAULT_PASSES);

    let root = workspace_root();
    let corpus_dir = root.join(CORPUS_ROOT);
    let mut dsp_paths = collect_dsp_files(&corpus_dir)?;
    dsp_paths.sort();

    if !options.case.is_empty() {
        let wanted = options
            .case
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let mut found: BTreeSet<String> = BTreeSet::new();
        dsp_paths.retain(|path| {
            let Some(stem) = dsp_stem(path) else {
                return false;
            };
            let keep = wanted.contains(stem.as_str());
            if keep {
                found.insert(stem);
            }
            keep
        });
        for missing in &wanted {
            if !found.contains(*missing) {
                return Err(format!(
                    "emission-determinism: no corpus case matches --case {missing}"
                )
                .into());
            }
        }
    }

    let mut search_paths = vec![corpus_dir.clone()];
    for candidate in ["/opt/homebrew/share/faust", "/usr/local/share/faust"] {
        let candidate = PathBuf::from(candidate);
        if candidate.is_dir() {
            search_paths.push(candidate);
        }
    }

    let allowlist = match &options.allowlist {
        Some(path) => parse_allowlist(path)?,
        None => BTreeSet::new(),
    };

    // Work items: one per (dsp index, config index) pair, so the bounded
    // worker pool below can parallelize across the full case x config
    // matrix instead of only across DSPs.
    let work_items = dsp_paths.len() * CONFIGS.len();
    let worker_count = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(work_items.max(1));

    println!(
        "emission-determinism: checking {} case(s) x {} config(s), {} pass(es), {} worker(s)",
        dsp_paths.len(),
        CONFIGS.len(),
        passes,
        worker_count
    );

    let next_item = AtomicUsize::new(0);
    // Indexed slots (not print-as-you-go) so the result collection stays
    // deterministic regardless of which worker finishes a given item first.
    let results: Mutex<Vec<Option<(String, CaseVerdict)>>> =
        Mutex::new((0..work_items).map(|_| None).collect());

    std::thread::scope(|scope| -> Result<(), std::io::Error> {
        for worker_index in 0..worker_count {
            std::thread::Builder::new()
                .name(format!("emission-determinism-{worker_index}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    loop {
                        let item_index = next_item.fetch_add(1, Ordering::Relaxed);
                        if item_index >= work_items {
                            break;
                        }
                        let dsp_index = item_index / CONFIGS.len();
                        let config_index = item_index % CONFIGS.len();
                        let dsp_path = &dsp_paths[dsp_index];
                        let config = &CONFIGS[config_index];
                        let case_id =
                            format!("{}/{}", dsp_stem(dsp_path).unwrap_or_default(), config.name);
                        let verdict =
                            check_case_determinism(dsp_path, &search_paths, config, passes);
                        results
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)[item_index] =
                            Some((case_id, verdict));
                    }
                })?;
        }
        Ok(())
    })?;

    let results = results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let mut stable = 0usize;
    let mut compile_error = 0usize;
    let mut unstable_ids: BTreeSet<String> = BTreeSet::new();
    let mut compile_error_lines: Vec<String> = Vec::new();
    let mut unstable_lines: Vec<String> = Vec::new();
    let mut allowed_lines: Vec<String> = Vec::new();
    let mut unallowed_unstable: Vec<String> = Vec::new();

    for entry in results {
        let (case_id, verdict) = entry.expect("emission-determinism worker omitted a work item");
        match verdict {
            CaseVerdict::Stable => stable += 1,
            CaseVerdict::CompileError(message) => {
                compile_error += 1;
                let message = repo_relative_message(&message, &root);
                compile_error_lines.push(format!("skipped (compile error) {case_id}: {message}"));
            }
            CaseVerdict::Unstable => {
                unstable_ids.insert(case_id.clone());
                if allowlist.contains(&case_id) {
                    allowed_lines.push(format!("allowed (frozen) {case_id}"));
                } else {
                    unstable_lines.push(format!("unstable {case_id}"));
                    unallowed_unstable.push(case_id);
                }
            }
        }
    }

    compile_error_lines.sort();
    unstable_lines.sort();
    allowed_lines.sort();
    unallowed_unstable.sort();

    let mut stale_lines: Vec<String> = Vec::new();
    for entry in &allowlist {
        if !unstable_ids.contains(entry) {
            stale_lines.push(format!(
                "stale allowlist entry {entry} (case is not unstable in this run)"
            ));
        }
    }
    stale_lines.sort();

    for line in &compile_error_lines {
        println!("emission-determinism: {line}");
    }
    for line in &unstable_lines {
        eprintln!("emission-determinism: {line}");
    }
    for line in &allowed_lines {
        println!("emission-determinism: {line}");
    }
    for line in &stale_lines {
        println!("emission-determinism: {line}");
    }

    if let Some(path) = &options.write_unstable {
        let mut text = unstable_ids.iter().cloned().collect::<Vec<_>>().join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
    }

    let allowlisted = unstable_ids.len() - unallowed_unstable.len();
    println!(
        "emission-determinism: {} cases x {} configs, {} passes: {} stable, {} unstable ({} allowlisted), {} compile-error",
        dsp_paths.len(),
        CONFIGS.len(),
        passes,
        stable,
        unstable_ids.len(),
        allowlisted,
        compile_error
    );

    if unallowed_unstable.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "emission-determinism: {} unstable case(s) not in the allowlist",
            unallowed_unstable.len()
        )
        .into())
    }
}

/// Emits `dsp_path` under `config` `passes` times, each with a fresh
/// [`compiler::Compiler`], and classifies the result.
fn check_case_determinism(
    dsp_path: &Path,
    search_paths: &[PathBuf],
    config: &EmissionConfig,
    passes: usize,
) -> CaseVerdict {
    let mut outcomes: Vec<PassOutcome> = Vec::with_capacity(passes);
    for _ in 0..passes {
        let compiler = Compiler::new()
            .with_real_type(RealType::Float64)
            .with_compute_mode(config.mode);
        let outcome = match compiler.compile_file_to_cpp(
            dsp_path,
            search_paths,
            &codegen::backends::cpp::CppOptions::default(),
        ) {
            Ok(text) => PassOutcome::Emitted(text),
            Err(error) => PassOutcome::Failed(error.to_string()),
        };
        outcomes.push(outcome);
    }

    let first = &outcomes[0];
    let all_identical = outcomes.iter().all(|outcome| outcome == first);
    if !all_identical {
        return CaseVerdict::Unstable;
    }
    match first {
        PassOutcome::Emitted(_) => CaseVerdict::Stable,
        PassOutcome::Failed(message) => CaseVerdict::CompileError(message.clone()),
    }
}

/// Rewrites any absolute workspace-root prefix embedded in a compiler error
/// message to a repo-relative path, matching the repo-relative-only finding
/// discipline of `structure_check.rs`. The corpus and search paths passed to
/// [`Compiler::compile_file_to_cpp`] are absolute (derived from
/// [`workspace_root`]), so `CompilerError::Display` text can otherwise embed
/// the absolute path of the machine running the gate.
fn repo_relative_message(message: &str, root: &Path) -> String {
    let root_prefix = format!("{}/", root.display());
    if root_prefix.len() <= 1 {
        return message.to_owned();
    }
    message.replace(root_prefix.as_str(), "")
}

/// Collects every `*.dsp` file directly under `dir`.
fn collect_dsp_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "dsp") {
            files.push(path);
        }
    }
    Ok(files)
}

/// Returns the file stem (no extension) of a DSP path, if valid UTF-8.
fn dsp_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
}

/// Parses an allowlist file: one case id (`<dsp-stem>/<config>`) per line,
/// `#` comments and blank lines ignored.
fn parse_allowlist(path: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read allowlist {}: {error}", path.display()))?;
    let mut entries = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        entries.insert(line.to_owned());
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_ignores_comments_and_blanks() {
        let dir = std::env::temp_dir().join(format!(
            "faust-rs-emission-determinism-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        fs::write(&path, "# comment\n\nzita_rev1/scalar\nzita_rev1/vec-lv0\n").unwrap();
        let parsed = parse_allowlist(&path).unwrap();
        assert_eq!(
            parsed,
            BTreeSet::from([
                "zita_rev1/scalar".to_owned(),
                "zita_rev1/vec-lv0".to_owned()
            ])
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn identical_outcomes_are_stable() {
        let outcomes = [
            PassOutcome::Emitted("same".to_owned()),
            PassOutcome::Emitted("same".to_owned()),
        ];
        assert!(outcomes.iter().all(|outcome| outcome == &outcomes[0]));
    }

    #[test]
    fn differing_outcomes_are_unstable() {
        let outcomes = [
            PassOutcome::Emitted("a".to_owned()),
            PassOutcome::Emitted("b".to_owned()),
        ];
        assert!(!outcomes.iter().all(|outcome| outcome == &outcomes[0]));
    }
}
