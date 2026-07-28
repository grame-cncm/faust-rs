//! FIR dump scanning workflow.
//!
//! The `fir-dump-scan` command compiles corpus cases to FIR and checks that
//! structural dumps expose expanded loop bodies. It is a regression guard for
//! traversal and rendering behavior rather than a semantic equivalence test.

use super::*;

// ---------------------------------------------------------------------------
// `fir-dump-scan`
// ---------------------------------------------------------------------------

/// Parsed options for `fir-dump-scan`.
#[derive(Debug)]
pub(crate) struct FirDumpScanOptions {
    /// Explicit compile corpus cases selected with repeated `--case`.
    pub(crate) cases: Vec<PathBuf>,
    /// Signal-to-FIR lane used before rendering `dump_fir`.
    pub(crate) lane: TraceLane,
}

impl Default for FirDumpScanOptions {
    /// Returns the default `fir-dump-scan` settings.
    ///
    /// The fast lane is the default because it is the active parity target for
    /// the structural loop-expansion checks in this workflow.
    fn default() -> Self {
        Self {
            cases: Vec::new(),
            lane: TraceLane::Fast,
        }
    }
}

impl From<FirDumpScanArgs> for FirDumpScanOptions {
    fn from(args: FirDumpScanArgs) -> Self {
        Self {
            cases: args.case,
            lane: args.lane.into(),
        }
    }
}

/// Scans `dump_fir` output for unexpanded loop-body placeholders.
pub(crate) fn fir_dump_scan(
    options: impl Into<FirDumpScanOptions>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = options.into();
    let cases = if options.cases.is_empty() {
        corpus_files()?
    } else {
        options.cases
    };
    let compiler = compiler::Compiler::new();

    let mut compiled_cases = 0usize;
    let mut skipped_compile = 0usize;
    let mut loop_nodes_seen = 0usize;
    let mut issues: Vec<String> = Vec::new();

    for case in cases {
        let lowered = match compiler
            .compile_file_default_to_fir_with_lane(&case, options.lane.to_signal_fir_lane())
        {
            Ok(out) => out,
            Err(e) => {
                skipped_compile += 1;
                println!("skip {} (FIR compile failed: {e})", case.display());
                continue;
            }
        };
        let rendered = dump_fir(&lowered.store, lowered.module);
        compiled_cases += 1;
        loop_nodes_seen += count_loop_nodes_in_dump(&rendered);

        let missing = find_unexpanded_loop_bodies(&rendered);
        if missing.is_empty() {
            println!("ok {} [lane={}]", case.display(), options.lane.as_str());
            continue;
        }

        for (loop_kind, loop_id, body_id) in missing {
            issues.push(format!(
                "{} [lane={}] {loop_kind} node #{loop_id} body #{body_id} not expanded in dump_fir output",
                case.display(),
                options.lane.as_str()
            ));
        }
    }

    if !issues.is_empty() {
        for issue in &issues {
            println!("[FAIL] {issue}");
        }
        return Err(format!(
            "fir-dump-scan failed: {} issue(s) across {} compiled case(s) (skipped_compile={})",
            issues.len(),
            compiled_cases,
            skipped_compile
        )
        .into());
    }

    println!(
        "fir-dump-scan: OK (lane={}, compiled_cases={}, skipped_compile={}, loop_nodes_seen={})",
        options.lane.as_str(),
        compiled_cases,
        skipped_compile,
        loop_nodes_seen
    );
    Ok(())
}

/// Counts loop statements rendered by `dump_fir`.
///
/// The scanner uses string matching instead of reparsing FIR because the goal
/// is specifically to validate the textual dumper's body expansion behavior.
pub(crate) fn count_loop_nodes_in_dump(rendered: &str) -> usize {
    rendered.matches("SimpleForLoop {").count()
        + rendered.matches("ForLoop {").count()
        + rendered.matches("IteratorForLoop {").count()
}

/// Finds loop entries whose referenced body ids never appear as expanded nodes.
pub(crate) fn find_unexpanded_loop_bodies(rendered: &str) -> Vec<(&'static str, u32, u32)> {
    let mut issues = Vec::new();
    for line in rendered.lines() {
        let Some((loop_kind, loop_id, body_id)) = parse_loop_line_body_ids(line) else {
            continue;
        };
        let body_marker = format!("#{body_id} ");
        if !rendered.contains(&body_marker) {
            issues.push((loop_kind, loop_id, body_id));
        }
    }
    issues
}

/// Parses one `dump_fir` line to extract `(loop_kind, loop_id, body_id)`.
///
/// Returns `None` for non-loop lines or if the textual shape does not match the
/// current dumper contract.
pub(crate) fn parse_loop_line_body_ids(line: &str) -> Option<(&'static str, u32, u32)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('#')?;
    let loop_id_end = rest.find(' ')?;
    let loop_id = rest[..loop_id_end].parse().ok()?;
    let rest = &rest[loop_id_end + 1..];

    let loop_kind = if rest.starts_with("SimpleForLoop {") {
        "SimpleForLoop"
    } else if rest.starts_with("ForLoop {") {
        "ForLoop"
    } else if rest.starts_with("IteratorForLoop {") {
        "IteratorForLoop"
    } else {
        return None;
    };

    let body_key = "body: TreeId(";
    let body_pos = rest.find(body_key)?;
    let body_tail = &rest[body_pos + body_key.len()..];
    let body_end = body_tail.find(')')?;
    let body_id = body_tail[..body_end].parse().ok()?;
    Some((loop_kind, loop_id, body_id))
}
