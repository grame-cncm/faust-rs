//! Lightweight structural checks for the `transform` crate (cleanup plan R9.3)
//! and the line threshold for `compiler` (cleanup plan 2026-07-26, phase P5).
//!
//! Deterministic, filesystem-only checks (findings sorted, repo-relative
//! paths only — never absolute paths):
//!
//! 1. no stale legacy internal `vector_*` import paths (R3 migrated the
//!    workspace to `signal_fir::vector::{...}`; the `pub use` facade
//!    re-exports in `signal_fir/mod.rs` are the only allowed mention);
//! 2. no production file above the review threshold in `crates/transform` or
//!    `crates/compiler` ([`MAX_PRODUCTION_LINES`] lines, `tests.rs` and
//!    `tests/` excluded);
//! 3. no checker file importing a producer entry point (the producer entry
//!    points listed in [`PRODUCER_ENTRY_POINTS`] must never be callable
//!    from a `check`/`verify` module), and [`PRODUCER_ENTRY_POINTS`] itself
//!    is cross-checked against the `pub fn`s actually present in the
//!    producer files so a renamed or newly added entry point cannot
//!    silently rot the list;
//! 4. no `check.rs` importing *anything* from a sibling producer file
//!    (`build`/`produce`/`materialize`/`session`): since the clock_ad
//!    checker-independence follow-up (2026-07-20), every vector checker
//!    re-derives with its own code, and the allowlist
//!    [`CHECKER_PRODUCER_IMPORT_ALLOWLIST`] is empty by design — a new
//!    entry is an architecture regression, not a freeze candidate;
//! 5. Rustdoc `missing_docs` cleanliness is enforced separately by
//!    `cargo rustdoc -p transform --lib -- -D missing-docs` (see plan R9.2);
//!    this check does not shell out to cargo so it stays fast and
//!    deterministic across platforms.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Review threshold for production files (plan R9.3). The largest file kept
/// intact by design is `vector/lower/signal.rs` (single lowerer `impl`).
const MAX_PRODUCTION_LINES: usize = 2_000;

/// Legacy internal alias paths that R3 retired for workspace-internal use.
const LEGACY_VECTOR_SEGMENTS: [&str; 4] = [
    "signal_fir::vector_analysis",
    "signal_fir::vector_plan",
    "signal_fir::vector_verify",
    "signal_fir::vector_state",
];

/// Producer entry points a checker module must never import or call.
///
/// Kept honest mechanically: `structure_check` scans every producer file
/// (`build.rs`/`produce.rs`/`materialize.rs` under `signal_fir/vector/`) for
/// `pub fn`s matching [`PRODUCER_ENTRY_PREFIXES`] and fails if this list
/// and the scan disagree in either direction.
const PRODUCER_ENTRY_POINTS: [&str; 11] = [
    "build_vector_plan(",
    "build_vector_plan_with_lockstep(",
    "build_vector_clock_ad_plan(",
    "build_vector_state_plan(",
    "build_vector_state_plan_with_clock(",
    "build_verified_vector_module(",
    "assemble_vector_fir(",
    "lower_vector_program(",
    "lower_pure_vector_program(",
    "build_event_order_certificate(",
    "build_state_event_order_certificate(",
];

/// Naming prefixes that identify a producer entry point among the `pub fn`s
/// of a producer file.
const PRODUCER_ENTRY_PREFIXES: [&str; 3] = ["build_", "assemble_", "lower_"];

/// `check.rs` files allowed to import from a sibling producer file.
///
/// Empty by design since the clock_ad checker-independence follow-up
/// (`porting/clock-ad-checker-independence-plan-2026-07-20-en.md`): a new
/// entry here is an architecture regression to fix, not to freeze.
const CHECKER_PRODUCER_IMPORT_ALLOWLIST: [&str; 0] = [];

/// Sibling module names that hold producer code inside a vector stage.
/// (`signal` is `lower/`'s producer file; `session` is `route/`'s.)
const PRODUCER_SIBLING_MODULES: [&str; 5] =
    ["build", "produce", "materialize", "session", "signal"];

/// File names whose `pub fn`s are scanned for the entry-point cross-check.
const PRODUCER_FILE_NAMES: [&str; 4] = ["build.rs", "produce.rs", "materialize.rs", "signal.rs"];

/// Runs every structural check and fails with a sorted finding list.
pub fn structure_check() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new("crates/transform/src");
    if !root.is_dir() {
        return Err("structure-check must run from the repository root".into());
    }
    let mut files = Vec::new();
    collect_rust_files(root, &mut files)?;
    // The line threshold also guards `crates/compiler`, whose facade and CLI
    // runner both crossed it before the 2026-07-26 cleanup. The checks that
    // follow (legacy vector paths, producer/checker separation) are
    // transform-specific and skip these files by construction, since none of
    // their patterns can match outside `signal_fir/vector/`.
    collect_rust_files(Path::new("crates/compiler/src"), &mut files)?;
    files.sort();

    let mut findings: Vec<String> = Vec::new();
    let mut scanned_entry_points: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let rel = path.to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(path)?;
        let is_test_file = rel.ends_with("/tests.rs") || rel.contains("/tests/");

        if !is_test_file {
            let lines = text.lines().count();
            if lines > MAX_PRODUCTION_LINES {
                findings.push(format!(
                    "{rel}: {lines} lines exceeds the {MAX_PRODUCTION_LINES}-line review threshold"
                ));
            }
        }

        if !rel.ends_with("signal_fir/mod.rs") {
            for segment in LEGACY_VECTOR_SEGMENTS {
                if text.contains(segment) {
                    findings.push(format!(
                        "{rel}: stale legacy internal import path `{segment}`"
                    ));
                }
            }
        }

        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_checker_file = file_name == "check.rs"
            || file_name == "checker_reachability.rs"
            || (rel.contains("/verify/") && !is_test_file);
        if is_checker_file {
            for entry in PRODUCER_ENTRY_POINTS {
                if text.contains(entry) {
                    findings.push(format!(
                        "{rel}: checker file references producer entry point `{}`",
                        entry.trim_end_matches('(')
                    ));
                }
            }
        }

        let in_vector_stage = rel.contains("signal_fir/vector/");
        if in_vector_stage && file_name == "check.rs" && !is_test_file {
            for sibling in PRODUCER_SIBLING_MODULES {
                let import = format!("use super::{sibling}::");
                if text.contains(&import)
                    && !CHECKER_PRODUCER_IMPORT_ALLOWLIST.contains(&rel.as_str())
                {
                    findings.push(format!(
                        "{rel}: check.rs imports from sibling producer module `{sibling}` \
                         (checker re-derivation must stay independent; the allowlist is \
                         empty by design)"
                    ));
                }
            }
        }
        if in_vector_stage && PRODUCER_FILE_NAMES.contains(&file_name.as_str()) && !is_test_file {
            collect_producer_entry_points(&text, &mut scanned_entry_points);
        }
    }

    // Cross-check the hardcoded entry-point list against the scan so a
    // renamed or newly added producer entry point cannot silently rot it.
    let listed: BTreeSet<String> = PRODUCER_ENTRY_POINTS
        .iter()
        .map(|entry| entry.trim_end_matches('(').to_owned())
        .collect();
    for missing in scanned_entry_points.difference(&listed) {
        findings.push(format!(
            "PRODUCER_ENTRY_POINTS is stale: producer file declares `{missing}` \
             but the list does not contain it"
        ));
    }
    for extra in listed.difference(&scanned_entry_points) {
        findings.push(format!(
            "PRODUCER_ENTRY_POINTS is stale: `{extra}` is listed but no producer \
             file declares it"
        ));
    }

    findings.sort();
    if findings.is_empty() {
        println!(
            "structure-check: OK ({} files, threshold {} lines)",
            files.len(),
            MAX_PRODUCTION_LINES
        );
        Ok(())
    } else {
        for finding in &findings {
            eprintln!("structure-check: {finding}");
        }
        Err(format!("structure-check: {} finding(s)", findings.len()).into())
    }
}

/// Scans one producer file's text for `pub fn` / `pub(crate) fn`
/// declarations whose name starts with one of
/// [`PRODUCER_ENTRY_PREFIXES`], collecting the names into `out`.
fn collect_producer_entry_points(text: &str, out: &mut BTreeSet<String>) {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let after_pub = if let Some(rest) = trimmed.strip_prefix("pub fn ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("pub(crate) fn ") {
            rest
        } else {
            continue;
        };
        let name: String = after_pub
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if PRODUCER_ENTRY_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            out.insert(name);
        }
    }
}

/// Collects every `.rs` file under `dir`, depth-first.
fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}
