//! Lightweight structural checks for the crates this readability campaign
//! restructured (plan `porting/faust-rs-structure-readability-plan-2026-08-18-{en,fr}.md`,
//! phases P1–P5), plus the vector producer/checker rules from cleanup plan R9.3.
//!
//! Deterministic, filesystem-only checks (findings sorted, repo-relative
//! paths only — never absolute paths):
//!
//! 1. no stale legacy internal `vector_*` import paths (R3 migrated the
//!    workspace to `signal_fir::vector::{...}`; the `pub use` facade
//!    re-exports in `signal_fir/mod.rs` are the only allowed mention);
//! 2. no production file above the review threshold in `crates/transform`,
//!    `crates/compiler`, `crates/fir`, or `crates/codegen`
//!    ([`MAX_PRODUCTION_LINES`] lines, `tests.rs` and `tests/` excluded)
//!    unless the file is named in [`KNOWN_OVERSIZED_FILES`] with a reason.
//!    That list is cross-checked both ways: every entry must name a file that
//!    actually exists and is actually over the threshold, so an exception a
//!    later split has resolved gets flagged for removal instead of lingering;
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
//! 5. every crate in [`DOCUMENTED_CRATES`] declares `#![deny(missing_docs)]`
//!    verbatim in its `lib.rs`. `deny` is required, not `warn`: an inner
//!    `#![warn(missing_docs)]` attribute overrides the command-line
//!    `-D warnings` that clippy and CI already pass, so a `warn` there is
//!    invisible to every existing gate — exactly the shape of a documented
//!    guarantee (`transform`'s plan R9.2, `compiler`'s own doc comment) that
//!    was never actually enforced until this was caught with a rejecting
//!    mutation on 2026-08-18. `deny` needs no separate command: it fails
//!    `cargo build`/`check`/`clippy`/`test` directly for that crate;
//! 6. no bare porting-plan codename (`P4.3b`, `R1`, `V5`, `S7`, `§4.8`, …)
//!    in a `crates/transform` comment outside a provenance context: a
//!    comment block (contiguous `//`/`///`/`//!` lines) may cite codenames
//!    only if it also names a `porting/` document, a `*-en.md`/`*-fr.md`
//!    file, or contains `provenance`/`history`. Codenames must not carry
//!    the explanation — present-tense semantic names do (legibility
//!    campaign E1, `porting/transform-legibility-analysis-2026-08-25-en.md`);
//! 7. no file-level `#![allow(dead_code)]` in `crates/transform`: a blanket
//!    allowance hides real dead clusters (the `loop_graph.rs` case E1
//!    removed masked 11 dead items); scope allowances to items, with a
//!    reason;
//! 8. no production function in `crates/transform` above
//!    [`MAX_PRODUCTION_FN_LINES`] lines unless it is named in
//!    [`OVERSIZED_FUNCTIONS`] with a ceiling — the legibility campaign E2
//!    ratchet (`porting/transform-legibility-analysis-2026-08-25-en.md`).
//!    The list is cross-checked both ways: every entry must name a function
//!    that exists, still exceeds the threshold (a decomposed function must
//!    be removed from the list), and has not grown past its recorded
//!    ceiling.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Review threshold for production files not named in [`KNOWN_OVERSIZED_FILES`].
///
/// Lowered from 2400 to 2000 on 2026-08-18 (readability campaign P6), after
/// P3 and P5 split every file that had been driving the old threshold up:
/// `fir/checker.rs` (3336 lines), `codegen/backends/interp/compiler.rs`
/// (2225), and `codegen/backends/interp/fbc_to_cpp.rs` (2255) are all under
/// 1000 lines now. 2000 leaves real margin above the largest *unexcepted*
/// file in the scanned crates — it is a
/// deliberate value, not one backed into by whatever remains largest, which
/// is the trap this threshold fell into twice before P6 (see git history for
/// the 2100 → 2200 → 2400 progression this const used to describe).
///
/// Every file still over this line count is named in
/// [`KNOWN_OVERSIZED_FILES`] with a reason instead of raising the number
/// again: raising the general threshold to fit one file hides every other
/// file's growth behind it, which is exactly what happened to the old 2400.
const MAX_PRODUCTION_LINES: usize = 2_000;

/// Production files that stay over [`MAX_PRODUCTION_LINES`] on purpose,
/// each with the reason splitting it is not this campaign's job.
///
/// Checked both ways by [`structure_check`]: every path here must resolve to
/// a file that (a) exists among the scanned crates and (b) is still actually
/// over the threshold. A file that shrinks below it no longer needs the
/// exception — leaving it here would let the list silently accumulate dead
/// entries, so a shrunk file is flagged as a stale exception to remove
/// rather than passing silently.
const KNOWN_OVERSIZED_FILES: [(&str, &str); 12] = [
    (
        "crates/transform/src/signal_fir/vector/lower/signal.rs",
        "numerically sensitive lowering path, kept structurally intact by          design (readability plan §7); this is the file that drove the old          threshold up twice before P6 named it as an exception instead",
    ),
    (
        "crates/compiler/src/lib.rs",
        "facade file dense with types and free functions, not one          collapsible block like `checker.rs`/`compiler.rs` were; not          targeted by P1–P5, splitting it is separate future work",
    ),
    (
        "crates/fir/src/inliner.rs",
        "not touched by this campaign (P5 covered `checker.rs` only);          backlog for a future FIR-crate phase",
    ),
    (
        "crates/codegen/src/backends/rust/mod.rs",
        "textual backend emitter; readability plan §2.9 measured the seven          textual backends at 21-57% pairwise similarity (not near-duplicates)          with 7-34% inline test code each — legitimate size, not addressed          by P3/P4",
    ),
    (
        "crates/codegen/src/backends/cpp/mod.rs",
        "textual backend emitter, see the `rust/mod.rs` entry above",
    ),
    (
        "crates/codegen/src/backends/wasm/mod.rs",
        "textual backend emitter, see the `rust/mod.rs` entry above",
    ),
    (
        "crates/codegen/src/backends/c/mod.rs",
        "textual backend emitter, see the `rust/mod.rs` entry above",
    ),
    (
        "crates/codegen/src/backends/julia/mod.rs",
        "textual backend emitter, see the `rust/mod.rs` entry above",
    ),
    (
        "crates/codegen/src/backends/asc/mod.rs",
        "textual backend emitter, see the `rust/mod.rs` entry above",
    ),
    (
        "crates/codegen/src/backends/cmajor/mod.rs",
        "textual backend emitter, see the `rust/mod.rs` entry above",
    ),
    (
        "crates/codegen/src/backends/interp/executor.rs",
        "interpreter dispatch loop, deliberately not split in P3: splitting          it by opcode family the way `fbc_to_cpp.rs` was split would add a          second match per executed instruction in the hottest loop of the          product, and no gate in this repository measures interpreter          throughput to prove that cost absent (see the P3 journal entry,          commit 11fd5c40)",
    ),
    (
        "crates/codegen/src/backends/cranelift/lowering.rs",
        "not analyzed by this campaign; backlog for a future codegen phase",
    ),
];

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

/// Crates required to declare `#![deny(missing_docs)]` verbatim in their
/// `lib.rs`. Every entry here measured at zero `missing_docs` errors under
/// `cargo rustdoc -p <crate> --lib -- -D missing-docs` on 2026-08-18.
///
/// This is deliberately short: it lists the crates this readability campaign
/// structurally restructured and confirmed clean (P1's `transform`, and
/// `compiler` via its call-site migrations), not every crate that should
/// eventually have one. `fir`, `codegen`, `parser`, `eval`, and `propagate`
/// carry real, pre-existing documentation debt — into the hundreds of items
/// for `codegen` and `fir` — that this phase did not write. Adding a crate
/// here is a commitment to it *already* being clean; writing the missing
/// docs for the rest is separate work for whoever takes it on.
const DOCUMENTED_CRATES: [&str; 2] = ["transform", "compiler"];

/// Runs every structural check and fails with a sorted finding list.

/// Review threshold for one production function body in `crates/transform`
/// (E2 of the legibility campaign). Functions above it must be named in
/// [`OVERSIZED_FUNCTIONS`]; the list shrinks as recipe decompositions land
/// and must end empty.
const MAX_PRODUCTION_FN_LINES: usize = 200;

/// Functions still over [`MAX_PRODUCTION_FN_LINES`], each with the ceiling
/// it measured when listed. Cross-checked both ways: an entry whose function
/// no longer exists, no longer exceeds the threshold, or exceeds its ceiling
/// is a finding. Decomposing a function removes its entry; the ceiling only
/// ever goes down.
const OVERSIZED_FUNCTIONS: [(&str, &str, usize); 19] = [
    ("crates/transform/src/signal_fir/module/build.rs", "build_module", 757),
    ("crates/transform/src/signal_fir/vector/plan/build.rs", "build_vector_plan", 585),
    ("crates/transform/src/signal_prepare/verify.rs", "verify_prepared_signal", 558),
    ("crates/transform/src/signal_fir/vector/plan/fusion.rs", "build_fused_serial_groups", 536),
    ("crates/transform/src/signal_fir/vector/verify/fused_groups.rs", "verify_fused_serial_groups_after_plan", 447),
    ("crates/transform/src/signal_fir/vector/lower/signal.rs", "lower_raw", 345),
    ("crates/transform/src/signal_fir/vector/lower/signal.rs", "lower_vector_program_impl", 306),
    ("crates/transform/src/signal_fir/module/bra.rs", "propagate_bra_adj", 300),
    ("crates/transform/src/signal_fir/mod.rs", "compile_fastlane_inner", 282),
    ("crates/transform/src/signal_fir/module/clocked.rs", "ensure_guarded_block", 277),
    ("crates/transform/src/signal_fir/module/core_lowering.rs", "lower_signal", 274),
    ("crates/transform/src/clk_env/mod.rs", "infer_uncached", 261),
    ("crates/transform/src/signal_fir/vector/assemble/materialize.rs", "materialize_action", 260),
    ("crates/transform/src/signal_fir/module/arithmetic.rs", "lower_proj", 258),
    ("crates/transform/src/signal_fir/vector/assemble/check.rs", "verify_assembled_fused_serial_groups", 252),
    ("crates/transform/src/signal_fir/vector/analysis/dependencies.rs", "signal_dependencies", 249),
    ("crates/transform/src/signal_fir/vector/verify/error.rs", "fmt", 238),
    ("crates/transform/src/signal_fir/vector/module/build.rs", "build_verified_vector_module_with_evidence", 230),
    ("crates/transform/src/signal_fir/vector/module/lifecycle.rs", "assemble_module", 209),
];

/// Scans one production file's text for function spans `(name, line_count)`.
///
/// Brace-balance measurement from the `fn` line to its closing brace; the
/// inline `#[cfg(test)] mod …` region (by convention the tail of a file) is
/// excluded. Brace counting is textual — the same deliberate approximation
/// the review threshold uses for files.
fn production_fn_spans(text: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut test_start = lines.len();
    for i in 0..lines.len().saturating_sub(1) {
        if lines[i].trim() == "#[cfg(test)]" && lines[i + 1].trim_start().starts_with("mod ") {
            test_start = i;
            break;
        }
    }
    let mut spans = Vec::new();
    let mut i = 0;
    while i < test_start {
        if let Some(name) = fn_decl_name(lines[i]) {
            let mut depth = 0i64;
            let mut opened = false;
            let mut j = i;
            while j < lines.len() {
                depth += lines[j].matches('{').count() as i64;
                depth -= lines[j].matches('}').count() as i64;
                if lines[j].contains('{') {
                    opened = true;
                }
                if opened && depth <= 0 {
                    break;
                }
                j += 1;
            }
            spans.push((name, j - i + 1));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    spans
}

/// The declared name when a line starts a `fn` item (any visibility).
fn fn_decl_name(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("pub") {
        rest = after.trim_start();
        if rest.starts_with('(') {
            rest = &rest[rest.find(')')? + 1..];
            rest = rest.trim_start();
        }
    }
    let after_fn = rest.strip_prefix("fn ")?;
    let name: String = after_fn
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Allowance markers that make a comment block a provenance context: a block
/// containing any of these may cite plan codenames (check 6).
const PROVENANCE_MARKERS: [&str; 5] = ["porting/", "-en.md", "-fr.md", "provenance", "history"];

/// Returns the first bare plan codename in a comment line, if any.
///
/// A codename is `P`/`R`/`V`/`S` followed by digits, an optional `.digits`,
/// and an optional trailing lowercase letter (`P4.3b`, `R1`, `V5b`, `S7`),
/// delimited by non-alphanumeric characters — or `§` followed by a digit.
fn find_plan_codename(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '§' {
            if chars.get(i + 1).is_some_and(char::is_ascii_digit) {
                return Some("§...".to_owned());
            }
            i += 1;
            continue;
        }
        if matches!(c, 'P' | 'R' | 'V' | 'S')
            && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            && (i == 0 || !chars[i - 1].is_ascii_alphanumeric())
        {
            let mut j = i + 2;
            while chars.get(j).is_some_and(char::is_ascii_digit) {
                j += 1;
            }
            if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
                j += 2;
                while chars.get(j).is_some_and(char::is_ascii_digit) {
                    j += 1;
                }
            }
            if chars.get(j).is_some_and(|c| c.is_ascii_lowercase()) {
                j += 1;
            }
            if !chars.get(j).is_some_and(|c| c.is_ascii_alphanumeric()) {
                return Some(chars[i..j].iter().collect());
            }
        }
        i += 1;
    }
    None
}

/// Check 6: bare plan codenames in `crates/transform` comments outside a
/// provenance context. Blocks are contiguous comment lines; a block naming a
/// `porting/` document or containing a provenance/history marker may cite
/// codenames freely.
fn codename_findings(rel: &str, text: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let lines: Vec<(usize, &str)> = text.lines().enumerate().collect();
    let mut block: Vec<(usize, &str)> = Vec::new();
    let flush = |block: &mut Vec<(usize, &str)>, findings: &mut Vec<String>| {
        let allowed = block.iter().any(|(_, l)| {
            let lower = l.to_ascii_lowercase();
            PROVENANCE_MARKERS.iter().any(|m| lower.contains(m))
        });
        if !allowed {
            for (n, l) in block.iter() {
                if let Some(token) = find_plan_codename(l) {
                    findings.push(format!(
                        "{rel}:{}: bare plan codename `{token}` in a comment without                          provenance context (name the behavior; demote the codename to a                          `Plan provenance:` mention)",
                        n + 1
                    ));
                }
            }
        }
        block.clear();
    };
    for (n, raw) in lines {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("//") {
            block.push((n, trimmed));
        } else {
            flush(&mut block, &mut findings);
        }
    }
    flush(&mut block, &mut findings);
    findings
}

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
    collect_rust_files(Path::new("crates/fir/src"), &mut files)?;
    collect_rust_files(Path::new("crates/codegen/src"), &mut files)?;
    files.sort();

    let known_oversized: std::collections::BTreeMap<&str, &str> =
        KNOWN_OVERSIZED_FILES.iter().copied().collect();
    let mut oversized_seen: BTreeSet<&str> = BTreeSet::new();
    let mut oversized_fn_seen: BTreeSet<(String, String)> = BTreeSet::new();

    let mut findings: Vec<String> = Vec::new();
    let mut scanned_entry_points: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let rel = path.to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(path)?;
        let is_test_file = rel.ends_with("/tests.rs") || rel.contains("/tests/");

        if !is_test_file {
            let lines = text.lines().count();
            if lines > MAX_PRODUCTION_LINES {
                if let Some(&exception_path) = known_oversized.keys().find(|k| **k == rel) {
                    oversized_seen.insert(exception_path);
                } else {
                    findings.push(format!(
                        "{rel}: {lines} lines exceeds the {MAX_PRODUCTION_LINES}-line review                          threshold and is not in KNOWN_OVERSIZED_FILES"
                    ));
                }
            }
        }

        if rel.starts_with("crates/transform/") {
            findings.extend(codename_findings(&rel, &text));
            if !is_test_file {
                // A file may declare several functions with one name (trait
                // impls — `fmt`); the exemption governs the longest of them.
                let mut longest: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for (name, len) in production_fn_spans(&text) {
                    let slot = longest.entry(name).or_insert(0);
                    *slot = (*slot).max(len);
                }
                for (name, len) in &longest {
                    let entry = OVERSIZED_FUNCTIONS
                        .iter()
                        .find(|(f, n, _)| *f == rel && n == name);
                    if entry.is_some() {
                        oversized_fn_seen.insert((rel.clone(), name.clone()));
                    }
                    match entry {
                        None if *len > MAX_PRODUCTION_FN_LINES => findings.push(format!(
                            "{rel}: fn `{name}` is {len} lines, over the \
                             {MAX_PRODUCTION_FN_LINES}-line function threshold and not in \
                             OVERSIZED_FUNCTIONS (decompose it, or list it with a ceiling)"
                        )),
                        Some((_, _, ceiling)) if len > ceiling => findings.push(format!(
                            "{rel}: fn `{name}` grew to {len} lines, past its recorded \
                             {ceiling}-line ceiling in OVERSIZED_FUNCTIONS"
                        )),
                        Some((_, _, _)) if *len <= MAX_PRODUCTION_FN_LINES => findings.push(format!(
                            "OVERSIZED_FUNCTIONS is stale: `{name}` in {rel} is {len} lines, \
                             no longer over the threshold — remove its entry"
                        )),
                        _ => {}
                    }
                }
            }
            if text.contains("#![allow(dead_code)]") {
                findings.push(format!(
                    "{rel}: file-level `#![allow(dead_code)]` (blanket allowances hide \
                     real dead clusters; scope the allowance to items, with a reason)"
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

    // Cross-check OVERSIZED_FUNCTIONS existence: an entry whose file or
    // function the scan never saw is a typo or a stale rename.
    for (file, name, _) in &OVERSIZED_FUNCTIONS {
        if !oversized_fn_seen.contains(&((*file).to_owned(), (*name).to_owned())) {
            findings.push(format!(
                "OVERSIZED_FUNCTIONS is stale: no fn `{name}` found in {file}"
            ));
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

    // Cross-check KNOWN_OVERSIZED_FILES against the scan both ways: a path
    // that does not exist among the scanned crates is a typo or a stale
    // rename, and a path that exists but is no longer over the threshold is
    // a resolved exception that should be removed instead of lingering.
    let scanned_paths: BTreeSet<String> = files
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    findings.extend(stale_oversized_exceptions(
        &KNOWN_OVERSIZED_FILES,
        &scanned_paths,
        &oversized_seen,
        MAX_PRODUCTION_LINES,
    ));

    // Every crate in DOCUMENTED_CRATES must declare `#![deny(missing_docs)]`
    // verbatim. `warn` here would compile clean and silently stop enforcing
    // anything, since an inner attribute overrides the command-line
    // `-D warnings` clippy and CI already pass — the exact regression a
    // rejecting mutation caught on 2026-08-18.
    for crate_name in DOCUMENTED_CRATES {
        let lib_path = Path::new("crates").join(crate_name).join("src/lib.rs");
        let contents = fs::read_to_string(&lib_path).ok();
        if let Some(finding) =
            missing_deny_attribute(crate_name, &lib_path.to_string_lossy(), contents.as_deref())
        {
            findings.push(finding);
        }
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

/// Findings for [`KNOWN_OVERSIZED_FILES`] entries that no longer match reality.
///
/// An entry is stale in one of two ways: it names a path absent from the
/// scanned files (typo, rename, or the file was deleted), or it names a path
/// that exists but measured at or under `max_lines` this run (the split that
/// resolved it landed, and the exception is dead weight). `seen` is the set of
/// exception paths whose file was actually found over the threshold during the
/// scan; a path not in `seen` but present in `scanned_paths` falls into the
/// second case.
pub(crate) fn stale_oversized_exceptions(
    known: &[(&str, &str)],
    scanned_paths: &BTreeSet<String>,
    seen: &BTreeSet<&str>,
    max_lines: usize,
) -> Vec<String> {
    let mut findings = Vec::new();
    for (exception_path, _reason) in known {
        if !scanned_paths.contains(*exception_path) {
            findings.push(format!(
                "KNOWN_OVERSIZED_FILES is stale: `{exception_path}` does not exist \
                 among the scanned crates"
            ));
        } else if !seen.contains(exception_path) {
            findings.push(format!(
                "KNOWN_OVERSIZED_FILES is stale: `{exception_path}` is no longer over \
                 the {max_lines}-line threshold; remove its exception"
            ));
        }
    }
    findings
}

/// One finding if `crate_name`'s `lib.rs` does not declare
/// `#![deny(missing_docs)]` verbatim, or `None` if it does.
///
/// `lib_rs_contents` is `None` when the file could not be read (a
/// [`DOCUMENTED_CRATES`] entry naming a crate whose `lib.rs` moved or was
/// deleted), which is itself a finding rather than a silent pass.
pub(crate) fn missing_deny_attribute(
    crate_name: &str,
    lib_rs_path: &str,
    lib_rs_contents: Option<&str>,
) -> Option<String> {
    match lib_rs_contents {
        Some(text) if text.contains("#![deny(missing_docs)]") => None,
        Some(_) => Some(format!(
            "{lib_rs_path}: DOCUMENTED_CRATES requires `#![deny(missing_docs)]` \
             verbatim, and it is missing (a `#![warn(missing_docs)]` attribute \
             compiles clean but enforces nothing)"
        )),
        None => Some(format!(
            "DOCUMENTED_CRATES lists `{crate_name}`, but {lib_rs_path} does not exist"
        )),
    }
}
