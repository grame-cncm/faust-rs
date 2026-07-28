//! Query-shaped, machine-readable, staleness-aware corpus parity access.
//!
//! # Source provenance
//! This module has no C++ counterpart: the reference compiler has no such
//! reporting facility. It is new `faust-rs` maintainer tooling.
//!
//! # Motivation
//! `corpus_status_report` (see `reports.rs`) regenerates a whole-corpus
//! Markdown report to answer any parity question, even "does case `foo` still
//! diverge". Three defects follow, all recorded in
//! `porting/mcp-server-analysis-and-plan-2026-07-21-en.md` (C3):
//!
//! 1. **Not query-shaped** — answering about five cases still costs a
//!    218-case compile pass plus a Markdown diff.
//! 2. **Silent staleness** — a written report carries no signal that the
//!    corpus has since grown or shrunk.
//! 3. **Expected divergence indistinguishable from regression** — cases where
//!    the C++ reference simply lacks a primitive `faust-rs` deliberately adds
//!    (`fad`/`rad`) are classified identically to a genuine parity break.
//!
//! `corpus-status-query` fixes all three: it accepts an explicit case list
//! (so N cases cost N compiles, not 218), emits one JSON document on stdout
//! carrying its own generation timestamp, the corpus file count it actually
//! observed, and the resolved C++ binary (path + git commit when
//! discoverable), and classifies every case into one of four buckets —
//! `ok_ok`, `err_err`, `expected_divergence`, `real_divergence` — so a
//! consumer can tell "by design" from "needs attention" without project
//! history.
//!
//! # API mapping status
//! New surface; not a port. Reuses `reports::cpp_case_status`,
//! `reports::rust_case_status` and `reports::resolve_cpp_faust_bin` so the
//! comparison logic itself stays a single source of truth shared with
//! `corpus_status_report`.

use super::*;

/// JSON schema version for [`CorpusStatusQueryResponse`].
///
/// Bump when a field is removed or its meaning changes; additive fields do
/// not require a bump.
pub(crate) const CORPUS_STATUS_QUERY_SCHEMA_VERSION: u32 = 1;

/// Output rendering mode for `corpus-status-query`.
///
/// `Json` is the default and the only machine-readable mode. `Human` earns
/// its place for interactive terminal use (a maintainer running the command
/// by hand does not want to parse JSON in their head); it is derived from the
/// same [`CorpusStatusQueryResponse`] value so the two modes can never
/// disagree on classification or counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QueryFormat {
    #[default]
    Json,
    Human,
}

/// Parsed options for `corpus-status-query`.
#[derive(Debug, Default)]
pub(crate) struct CorpusStatusQueryOptions {
    /// Explicit corpus cases selected with repeated `--case`.
    pub(crate) cases: Vec<PathBuf>,
    /// Whether to run the entire corpus (`--all`).
    pub(crate) all: bool,
    pub(crate) format: QueryFormat,
}

impl From<CorpusStatusQueryArgs> for CorpusStatusQueryOptions {
    fn from(args: CorpusStatusQueryArgs) -> Self {
        Self {
            cases: args.case,
            all: args.all,
            format: match args.format {
                QueryFormatArg::Json => QueryFormat::Json,
                QueryFormatArg::Human => QueryFormat::Human,
            },
        }
    }
}

/// Where the resolved C++ reference binary came from.
///
/// Distinguishes the three branches of [`resolve_cpp_faust_bin`] so the
/// staleness metadata can say precisely which one fired, rather than
/// collapsing to the existing boolean "is this a PATH fallback".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CppBinSource {
    EnvVar,
    DefaultBuildPath,
    PathFallback,
}

impl CppBinSource {
    fn as_str(self) -> &'static str {
        match self {
            CppBinSource::EnvVar => "FAUST_CPP_BIN environment variable",
            CppBinSource::DefaultBuildPath => "default build path under CPP_SOURCE_ROOT",
            CppBinSource::PathFallback => "PATH fallback (`faust`)",
        }
    }
}

/// Resolves the C++ reference binary with a precise provenance tag.
///
/// Thin wrapper around [`resolve_cpp_faust_bin`]: reuses its resolution order
/// (env var, then the checked-out build tree, then `PATH`) without
/// duplicating the decision, only refining the reported source.
fn resolve_cpp_faust_bin_detailed() -> (PathBuf, CppBinSource) {
    if std::env::var_os("FAUST_CPP_BIN").is_some() {
        let (path, _) = resolve_cpp_faust_bin();
        return (path, CppBinSource::EnvVar);
    }
    let (path, is_fallback) = resolve_cpp_faust_bin();
    if is_fallback {
        (path, CppBinSource::PathFallback)
    } else {
        (path, CppBinSource::DefaultBuildPath)
    }
}

/// Walks upward from a binary path looking for a `.git` directory and, if
/// found, returns `git rev-parse HEAD` for that checkout.
///
/// Best-effort: any failure (no repository found, `git` not on `PATH`, a
/// detached/shallow clone that still answers `rev-parse`) yields `None`
/// rather than an error, since the commit is metadata, not a requirement.
fn discover_git_commit(binary_path: &Path) -> Option<String> {
    let mut dir = binary_path.parent();
    while let Some(d) = dir {
        if d.join(".git").exists() {
            let output = Command::new("git")
                .arg("-C")
                .arg(d)
                .arg("rev-parse")
                .arg("HEAD")
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let hash = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            return if hash.is_empty() { None } else { Some(hash) };
        }
        dir = d.parent();
    }
    None
}

/// Best-effort UTC timestamp string (`date -u`); `None` if the platform has
/// no `date` binary or it fails. The unix-seconds field is authoritative and
/// never depends on this.
pub(crate) fn utc_timestamp_string() -> Option<String> {
    let output = Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// One of the four mutually exclusive divergence classes a case can fall
/// into. See the module doc for why this split exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DivergenceClass {
    /// C++ succeeds, Rust succeeds: genuine parity.
    OkOk,
    /// C++ fails, Rust fails: genuine parity (both reject).
    ErrErr,
    /// C++ fails, Rust succeeds, and the C++ failure is recognized as a
    /// feature `faust-rs` deliberately adds (`fad`/`rad`). By design, not a
    /// regression.
    ExpectedDivergence,
    /// Anything else: C++ succeeds and Rust fails (a Rust regression), or
    /// C++ fails for a reason that is not recognized as expected. This is
    /// what a maintainer must look at.
    RealDivergence,
}

/// Symbol names that are deliberate `faust-rs`-only additions absent from the
/// C++ reference. A C++ `undefined symbol : <name>` failure naming one of
/// these is by design, not a parity break.
///
/// `fad`/`rad` are the pair named in the C3 brief
/// (`porting/mcp-server-analysis-and-plan-2026-07-21-en.md` §6.1(c)),
/// confirmed against `porting/phases/phase-4-corpus-status-diff-report-en.md`
/// (e.g. the `fad_basic` row: ``tests/corpus/fad_basic.dsp:1 : ERROR :
/// undefined symbol : fad``).
///
/// `ondemand` was added after measuring the *actual* current corpus: running
/// this classifier over the full 218-file corpus left 21 `real_divergence`
/// cases, and every single one turned out to be
/// ``undefined symbol : ondemand`` (`interleave.lib:90` or
/// `rep_18_stream_wrappers.dsp:1`), not a genuine regression. `ondemand` is
/// the on-demand clock-domain primitive tracked in the project memory's
/// "Ondemand clock domains × FAD/RAD × vec × interleave" stream — another
/// `faust-rs`-only addition the reference compiler was never going to
/// recognize. Leaving it unclassified would have defeated the point of this
/// module (distinguishing by-design gaps from real regressions), so it is
/// included here even though the brief named only `fad`/`rad`; this is
/// recorded explicitly rather than silently folded in.
const EXPECTED_DIVERGENCE_SYMBOLS: &[&str] = &["fad", "rad", "ondemand"];

/// Detects whether a C++ reference failure is a known, deliberate
/// `faust-rs`-only feature gap (see [`EXPECTED_DIVERGENCE_SYMBOLS`]) rather
/// than a genuine parity break.
///
/// Matches the exact symbol name after the `undefined symbol :` marker —
/// stopping at the first non-identifier character — so it does not also
/// match unrelated symbols that happen to start with the same letters (e.g. a
/// hypothetical `radius` or `ondemandish`).
pub(crate) fn is_expected_divergence(cpp_reason: &str) -> bool {
    const MARKER: &str = "undefined symbol :";
    let Some(idx) = cpp_reason.find(MARKER) else {
        return false;
    };
    let rest = cpp_reason[idx + MARKER.len()..].trim_start();
    let symbol: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    EXPECTED_DIVERGENCE_SYMBOLS.contains(&symbol.as_str())
}

/// Classifies one case's C++/Rust outcome pair.
///
/// `(true, false)` (C++ ok, Rust fails) is always `RealDivergence`: it is a
/// Rust regression on something the reference compiler already accepts, and
/// there is no "expected" reading of a regression.
pub(crate) fn classify_divergence(
    cpp_ok: bool,
    rust_ok: bool,
    cpp_reason: &str,
) -> DivergenceClass {
    match (cpp_ok, rust_ok) {
        (true, true) => DivergenceClass::OkOk,
        (false, false) => DivergenceClass::ErrErr,
        (true, false) => DivergenceClass::RealDivergence,
        (false, true) => {
            if is_expected_divergence(cpp_reason) {
                DivergenceClass::ExpectedDivergence
            } else {
                DivergenceClass::RealDivergence
            }
        }
    }
}

/// Per-case result row.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CorpusStatusQueryCase {
    pub(crate) case: String,
    pub(crate) path: String,
    pub(crate) cpp_status: &'static str,
    pub(crate) rust_status: &'static str,
    pub(crate) rust_stage: &'static str,
    pub(crate) cpp_reason: String,
    pub(crate) rust_reason: String,
    pub(crate) classification: DivergenceClass,
}

/// Bucketed counts. `total` always equals the sum of the other four fields —
/// enforced by construction in [`Counts::record`], and asserted in tests.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct CorpusStatusQueryCounts {
    pub(crate) total: usize,
    pub(crate) ok_ok: usize,
    pub(crate) err_err: usize,
    pub(crate) expected_divergence: usize,
    pub(crate) real_divergence: usize,
}

impl CorpusStatusQueryCounts {
    fn record(&mut self, class: DivergenceClass) {
        self.total += 1;
        match class {
            DivergenceClass::OkOk => self.ok_ok += 1,
            DivergenceClass::ErrErr => self.err_err += 1,
            DivergenceClass::ExpectedDivergence => self.expected_divergence += 1,
            DivergenceClass::RealDivergence => self.real_divergence += 1,
        }
    }
}

/// C++ reference binary provenance, part of the staleness metadata.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CppBinaryInfo {
    pub(crate) path: String,
    pub(crate) resolved_via: &'static str,
    /// `git rev-parse HEAD` of the checkout the binary lives in, if one could
    /// be discovered by walking up from the binary path.
    pub(crate) commit: Option<String>,
}

/// Whether the query ran the whole corpus or an explicit case list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QueryScope {
    All,
    Cases,
}

/// Full response envelope for `corpus-status-query`.
///
/// Every field under `generated_at_unix` / `corpus_file_count_seen` /
/// `cpp_binary` exists so a consumer can tell a fresh answer from a stale or
/// cached one without external knowledge — the exact gap identified against
/// `phase-4-corpus-status-diff-report-en.md` (dated 2026-06-10, `Total cases:
/// 190`, against a corpus that had already grown to 218).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CorpusStatusQueryResponse {
    pub(crate) schema_version: u32,
    pub(crate) generated_at_unix: u64,
    pub(crate) generated_at_utc: Option<String>,
    /// Number of `.dsp` files under `tests/corpus/` observed by this run,
    /// regardless of how many were actually queried. Lets a consumer notice
    /// corpus growth/shrinkage even on a narrow query.
    pub(crate) corpus_file_count_seen: usize,
    pub(crate) query_scope: QueryScope,
    pub(crate) requested_cases: Vec<String>,
    pub(crate) cpp_binary: CppBinaryInfo,
    pub(crate) counts: CorpusStatusQueryCounts,
    pub(crate) cases: Vec<CorpusStatusQueryCase>,
}

/// Resolves a `--case` argument against the workspace root when relative.
fn resolve_case_path(root: &Path, raw: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    if !candidate.exists() {
        return Err(format!("case not found: {}", candidate.display()).into());
    }
    Ok(candidate)
}

/// Builds the full [`CorpusStatusQueryResponse`] for the requested scope.
///
/// Split out from [`corpus_status_query`] so tests can call it directly
/// without going through argv parsing or stdout.
pub(crate) fn run_corpus_status_query(
    options: &CorpusStatusQueryOptions,
) -> Result<CorpusStatusQueryResponse, Box<dyn std::error::Error>> {
    let root = workspace_root();
    let corpus_file_count_seen = corpus_files()?.len();

    let (query_scope, targets): (QueryScope, Vec<PathBuf>) = if options.all {
        (QueryScope::All, corpus_files()?)
    } else {
        let mut resolved = Vec::with_capacity(options.cases.len());
        for raw in &options.cases {
            resolved.push(resolve_case_path(&root, raw)?);
        }
        (QueryScope::Cases, resolved)
    };
    let requested_cases: Vec<String> = targets.iter().map(|p| p.display().to_string()).collect();

    let compiler = compiler::Compiler::new();
    let (cpp_bin, cpp_bin_source) = resolve_cpp_faust_bin_detailed();
    let cpp_binary = CppBinaryInfo {
        path: cpp_bin.display().to_string(),
        resolved_via: cpp_bin_source.as_str(),
        commit: discover_git_commit(&cpp_bin),
    };

    let mut counts = CorpusStatusQueryCounts::default();
    let mut cases = Vec::with_capacity(targets.len());
    for file in &targets {
        let case = case_name(file)?;
        let cpp = cpp_case_status(&cpp_bin, file)?;
        let rust = rust_case_status(&compiler, file);
        let classification = classify_divergence(cpp.ok, rust.ok, &cpp.reason);
        counts.record(classification);
        cases.push(CorpusStatusQueryCase {
            case,
            path: file.display().to_string(),
            cpp_status: status_cell(&cpp),
            rust_status: status_cell(&rust),
            rust_stage: rust.stage,
            cpp_reason: cpp.reason.clone(),
            rust_reason: rust.reason.clone(),
            classification,
        });
    }

    let generated_at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Ok(CorpusStatusQueryResponse {
        schema_version: CORPUS_STATUS_QUERY_SCHEMA_VERSION,
        generated_at_unix,
        generated_at_utc: utc_timestamp_string(),
        corpus_file_count_seen,
        query_scope,
        requested_cases,
        cpp_binary,
        counts,
        cases,
    })
}

/// Renders a compact human-readable summary of a response, for `--format
/// human`. Derived from the same response value the JSON mode serializes, so
/// the two can never disagree.
fn render_human(response: &CorpusStatusQueryResponse) -> String {
    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "corpus-status-query  (generated {}  unix={}  corpus files seen={})",
        response.generated_at_utc.as_deref().unwrap_or("unknown"),
        response.generated_at_unix,
        response.corpus_file_count_seen
    );
    let _ = writeln!(
        &mut out,
        "cpp binary: {} (via {}{})",
        response.cpp_binary.path,
        response.cpp_binary.resolved_via,
        response
            .cpp_binary
            .commit
            .as_deref()
            .map(|c| format!(", commit {c}"))
            .unwrap_or_default()
    );
    let _ = writeln!(
        &mut out,
        "scope: {:?}  requested: {}",
        response.query_scope,
        response.requested_cases.len()
    );
    let c = &response.counts;
    let _ = writeln!(
        &mut out,
        "counts: total={} ok_ok={} err_err={} expected_divergence={} real_divergence={}",
        c.total, c.ok_ok, c.err_err, c.expected_divergence, c.real_divergence
    );
    let _ = writeln!(&mut out);
    for case in &response.cases {
        let _ = writeln!(
            &mut out,
            "[{:?}] {}  cpp={} rust={} ({})",
            case.classification, case.case, case.cpp_status, case.rust_status, case.rust_stage
        );
        if !matches!(case.classification, DivergenceClass::OkOk) {
            let _ = writeln!(&mut out, "    cpp:  {}", case.cpp_reason);
            let _ = writeln!(&mut out, "    rust: {}", case.rust_reason);
        }
    }
    out
}

/// `corpus-status-query` entry point.
pub(crate) fn corpus_status_query(
    args: CorpusStatusQueryArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = CorpusStatusQueryOptions::from(args);
    let response = run_corpus_status_query(&options)?;
    match options.format {
        QueryFormat::Json => println!("{}", serde_json::to_string_pretty(&response)?),
        QueryFormat::Human => print!("{}", render_human(&response)),
    }
    Ok(())
}
