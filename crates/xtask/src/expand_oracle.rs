//! C++ reference capture for the `-e` / `--export-dsp` corpus.
//!
//! The `-e` option serializes an evaluated block diagram back to
//! self-contained Faust source. Its porting plan
//! (`porting/export-dsp-e-option-and-libfaust-api-port-plan-2026-08-12-en.md`)
//! takes the C++ compiler's own output as the reference, so this workflow
//! records that output once per fixture instead of transcribing it by hand.
//!
//! # Corpus layout
//! - `tests/expand/dsp/<name>.dsp` — one construct family per fixture;
//! - `tests/expand/oracle/<name>.dsp` — the captured `faust -e` expansion.
//!
//! # Why a capture step rather than inline expectations
//! The expansion text encodes several rules that are only discoverable by
//! observation: first-visit operator parenthesization, `ID_` numbering order,
//! `%g`-shaped real literals, and the metadata key mangling. Recording the
//! compiler's answer keeps those rules auditable and lets the checker fail on a
//! drift rather than on a transcription mistake.
//!
//! # Extension fixtures
//! Fixtures exercising faust-rs extensions the reference binary does not know
//! (`fad`, `rad`, `ondemand`, `upsampling`, `downsampling`) legitimately have
//! no oracle. They are reported as `skipped (unsupported by reference)` and are
//! never treated as failures; the Rust-side checks still cover them.

use super::*;

/// Corpus directory holding the `-e` fixtures, relative to the workspace root.
const EXPAND_DSP_REL_DIR: &str = "tests/expand/dsp";

/// Directory holding captured C++ expansions, relative to the workspace root.
const EXPAND_ORACLE_REL_DIR: &str = "tests/expand/oracle";

/// Outcome of one fixture capture.
enum CaptureOutcome {
    /// The reference compiler produced an expansion, and it re-compiled.
    Captured,
    /// The reference compiler rejected the source (an extension fixture).
    Unsupported(String),
}

/// Captures or verifies the C++ `-e` expansion for every corpus fixture.
///
/// Without `--check` the oracle files are (re)written. With `--check` they are
/// compared against a fresh capture and any difference fails, which is what
/// makes the recorded reference auditable in CI.
///
/// Every captured expansion is additionally fed back through the reference
/// compiler: an expansion that does not re-compile is not a usable reference,
/// and catching that here keeps the property out of the Rust-side checkers,
/// where a bad fixture would look like a port defect.
pub(crate) fn expand_oracle(args: ExpandOracleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = workspace_root();
    let dsp_dir = workspace.join(EXPAND_DSP_REL_DIR);
    let oracle_dir = workspace.join(EXPAND_ORACLE_REL_DIR);
    fs::create_dir_all(&oracle_dir)?;

    let (cpp_bin, from_path) = resolve_cpp_faust_bin();
    let fixtures = expand_fixtures(&dsp_dir)?;
    if fixtures.is_empty() {
        return Err(format!("no `-e` fixtures found in {EXPAND_DSP_REL_DIR}").into());
    }

    let mut captured = 0usize;
    let mut unsupported = Vec::new();
    let mut drifted = Vec::new();

    for fixture in &fixtures {
        let stem = fixture
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("fixture name is not valid UTF-8")?;
        let oracle_path = oracle_dir.join(format!("{stem}.dsp"));

        match capture_expansion(&cpp_bin, fixture)? {
            CaptureOutcome::Unsupported(reason) => {
                unsupported.push(format!("{stem}: {reason}"));
                // A rejected run can still have created the output file before
                // failing; leaving it behind would pollute the corpus with a
                // truncated capture.
                let scratch = scratch_expansion_path(&oracle_dir, stem);
                if scratch.exists() {
                    fs::remove_file(&scratch)?;
                }
                // A stale oracle for a now-unsupported fixture would silently
                // keep asserting an expansion the reference can no longer
                // produce, so it is removed rather than left behind.
                if oracle_path.exists() {
                    fs::remove_file(&oracle_path)?;
                }
            }
            CaptureOutcome::Captured => {
                let produced = fs::read_to_string(scratch_expansion_path(&oracle_dir, stem))?;
                let normalized = normalize_expansion(&produced, stem);
                if args.check {
                    let recorded = fs::read_to_string(&oracle_path).map_err(|error| {
                        format!(
                            "missing oracle {}: {error}; regenerate with `cargo run -p xtask -- expand-oracle`",
                            workspace_relative_path(&oracle_path)
                        )
                    })?;
                    if recorded != normalized {
                        drifted.push(stem.to_owned());
                    }
                } else {
                    fs::write(&oracle_path, &normalized)?;
                }
                fs::remove_file(scratch_expansion_path(&oracle_dir, stem))?;
                captured += 1;
            }
        }
    }

    if !drifted.is_empty() {
        return Err(format!(
            "captured `-e` expansions differ from {EXPAND_ORACLE_REL_DIR}: {}\nrefresh intentionally with `cargo run -p xtask -- expand-oracle`",
            drifted.join(", ")
        )
        .into());
    }

    println!(
        "expand oracle: {} fixtures, {captured} captured, {} unsupported by the reference{}",
        fixtures.len(),
        unsupported.len(),
        if args.check { " (checked)" } else { "" }
    );
    if from_path {
        println!("reference binary: `faust` from PATH (set FAUST_CPP_BIN to pin one)");
    } else {
        println!("reference binary: {}", cpp_bin.display());
    }
    for entry in &unsupported {
        println!("  skipped {entry}");
    }
    Ok(())
}

/// Returns the corpus fixtures in deterministic name order.
fn expand_fixtures(dsp_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut fixtures = fs::read_dir(dsp_dir)
        .map_err(|error| format!("cannot read {}: {error}", dsp_dir.display()))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("dsp"))
        .collect::<Vec<_>>();
    fixtures.sort();
    Ok(fixtures)
}

/// Temporary output path used for one capture.
///
/// The reference compiler writes `-e` output to the `-o` file, so a capture
/// needs a real path even when the result is only compared.
fn scratch_expansion_path(oracle_dir: &Path, stem: &str) -> PathBuf {
    oracle_dir.join(format!(".{stem}.capture"))
}

/// Runs `faust -e` on one fixture and verifies the result re-compiles.
fn capture_expansion(
    cpp_bin: &Path,
    fixture: &Path,
) -> Result<CaptureOutcome, Box<dyn std::error::Error>> {
    let stem = fixture
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("fixture name is not valid UTF-8")?;
    let oracle_dir = fixture
        .parent()
        .ok_or("fixture has no parent directory")?
        .parent()
        .ok_or("fixture corpus has no parent directory")?
        .join("oracle");
    let output_path = scratch_expansion_path(&oracle_dir, stem);

    let output = Command::new(cpp_bin)
        .arg("-e")
        .arg(fixture)
        .arg("-o")
        .arg(&output_path)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", cpp_bin.display()))?;
    if !output.status.success() {
        let reason = first_non_empty_line(&String::from_utf8_lossy(&output.stderr))
            .unwrap_or_else(|| "rejected by the reference compiler".to_owned());
        return Ok(CaptureOutcome::Unsupported(reason));
    }

    // An expansion that does not re-compile is not a reference. Verify it here
    // so a broken fixture fails at capture time with the compiler's own error.
    let recompile = Command::new(cpp_bin)
        .arg(&output_path)
        .arg("-o")
        .arg(output_path.with_extension("recompile.cpp"))
        .output()?;
    let recompiled = output_path.with_extension("recompile.cpp");
    if recompiled.exists() {
        fs::remove_file(&recompiled)?;
    }
    if !recompile.status.success() {
        return Err(format!(
            "captured expansion of {} does not re-compile: {}",
            stem,
            first_non_empty_line(&String::from_utf8_lossy(&recompile.stderr))
                .unwrap_or_else(|| "no error output".to_owned())
        )
        .into());
    }

    Ok(CaptureOutcome::Captured)
}

/// Removes the host- and invocation-dependent parts of one captured expansion.
///
/// Three lines cannot be recorded verbatim without making the corpus depend on
/// where it was captured: the reference compiler's version, the `compile_options`
/// string (which embeds the absolute fixture and output paths passed on the
/// command line), and the `library_path*` entries (absolute installation
/// paths). They are replaced with stable placeholders so the recorded file
/// stays comparable across machines while still showing that the line exists
/// and in which position.
fn normalize_expansion(expansion: &str, stem: &str) -> String {
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
        } else if line.starts_with("declare filename ") {
            out.push_str(&format!("declare filename \"{stem}.dsp\";\n"));
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
/// normalizations, copyright edits). Kept in lockstep with the same helper in
/// `crates/compiler/tests/expand_corpus.rs`: recording and comparison must
/// mask the same lines, or a libraries upgrade turns the corpus red with no
/// compiler change anywhere — which is exactly what the 2026-08 maths.lib
/// license-string update did to CI (its pinned checkout, a developer's
/// installed /usr/local/share/faust, and faustlibraries master all carried
/// different strings).
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
