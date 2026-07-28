//! Human and JSON diagnostic rendering for the CLI.
//!
//! The compiler library exposes structured diagnostic bundles.  This module
//! converts those bundles into the two command-line contracts supported by the
//! binary: concise human diagnostics and machine-readable JSON diagnostics.
//! It also contains the CLI-only helpers for source snippets, caret spans,
//! note filtering, paired composition context, and debug-only diagnostic
//! fields.
//!
//! ## The machine channel contract (D1)
//!
//! Under `--error-format json`, [`print_structured_diagnostics`] is the sole
//! writer of stdout content: it prints exactly one well-formed JSON document,
//! with no leading or trailing non-JSON bytes, and nothing else on stdout
//! precedes or follows it for that invocation. Human-readable prefix lines
//! (e.g. `"C++ pipeline failed: ..."`) belong to `--error-format human` only
//! and are the caller's responsibility (see
//! `runner::report_pipeline_failure`), never printed here in JSON mode.
//! Diagnostics always go to stdout in JSON mode and to stderr in human mode
//! -- this asymmetry is intentional: JSON mode targets automated consumers
//! (CI, IDE tooling, a future MCP server) that read one stream, while human
//! mode targets a terminal where stdout is reserved for a dump mode's
//! generated output (`--dump-cpp`, `--dump-sig`, ...).
//!
//! Every [`CompilerError`] variant carries a structured [`DiagnosticBundle`].
//! The total `CompilerError::diagnostic_bundle` accessor therefore keeps both
//! human and JSON rendering on the stable diagnostic model without a
//! text-only fallback path.
//!
//! See `docs/diagnostics-codes-en.md` for the frozen `FRS-*` code table.

use super::args::{ErrorFormat, ErrorVerbosity};
use compiler::CompilerError;
use diagnostics::{
    Applicability, DiagnosticBundle, DiagnosticCategory, DiagnosticValue, Label, LabelRole,
    LabelStyle, Severity, SourceKind, SourceRange, Stage, TraceKind,
};
use serde_json::json;
use std::path::Path;

/// Prints structured diagnostics according to the selected CLI format.
///
/// See the module-level docs for the D1 stdout/stderr contract this
/// function upholds under `--error-format json`.
pub fn print_structured_diagnostics(
    err: &CompilerError,
    format: ErrorFormat,
    verbosity: ErrorVerbosity,
) {
    let bundle = err.diagnostic_bundle();
    match format {
        ErrorFormat::Human => match verbosity {
            ErrorVerbosity::Standard => eprint!("{}", format_diagnostics_human(bundle)),
            ErrorVerbosity::Debug => eprint!(
                "{}",
                format_diagnostics_human_with_verbosity(bundle, verbosity)
            ),
        },
        ErrorFormat::Json => match verbosity {
            ErrorVerbosity::Standard => println!("{}", format_diagnostics_json(bundle)),
            ErrorVerbosity::Debug => println!(
                "{}",
                format_diagnostics_json_with_verbosity(bundle, verbosity)
            ),
        },
    }
}

/// Formats diagnostics in a human-oriented form.
///
/// When a primary label is available and its source file can be read, this renderer
/// includes a source snippet line and a caret span.
pub fn format_diagnostics_human(bundle: &DiagnosticBundle) -> String {
    format_diagnostics_human_with_verbosity(bundle, ErrorVerbosity::Standard)
}

/// Formats diagnostics in human mode with an explicit verbosity contract.
///
/// `Standard` hides low-level internal notes while `Debug` keeps the full note
/// stream for troubleshooting/benchmark parity workflows.
pub fn format_diagnostics_human_with_verbosity(
    bundle: &DiagnosticBundle,
    verbosity: ErrorVerbosity,
) -> String {
    let mut out = String::new();
    for diag in bundle.as_slice() {
        let severity = match diag.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Remark => "remark",
        };
        if let Some(label) = diag.labels.first() {
            out.push_str(&format!(
                "{}:{}:{}: {} [{}] {}\n",
                label.span.file.display(),
                label.span.line,
                label.span.col,
                severity,
                diag.code.0,
                diag.message
            ));
            if let Some(line) = source_line(bundle, label.span.file.as_path(), label.span.line) {
                out.push_str(&format!("  {} | {}\n", label.span.line, line));
                out.push_str(&format!(
                    "    | {} {}\n",
                    caret_span(label.span.col, label.span.end_col),
                    label.message
                ));
            }
        } else {
            out.push_str(&format!("{severity} [{}] {}\n", diag.code.0, diag.message));
        }

        let paired = paired_context_from_notes(&diag.notes);
        if let Some(ctx) = &paired {
            out.push_str(&format!("  = note: Here  A = {}\n", ctx.a_expr));
            if let Some(arity) = &ctx.a_arity {
                out.push_str(&format!("  = note: has {arity}\n"));
            }
            out.push_str(&format!("  = note: while B = {}\n", ctx.b_expr));
            if let Some(arity) = &ctx.b_arity {
                out.push_str(&format!("  = note: has {arity}\n"));
            }
        }

        for note in filtered_notes_for_human(&diag.notes, paired.is_some(), verbosity) {
            out.push_str(&format!("  = note: {note}\n"));
        }
        for help in &diag.help {
            out.push_str(&format!("  = help: {help}\n"));
        }
    }
    out
}

/// Rendered A/B sub-expressions extracted from a binary composition diagnostic.
///
/// Faust composition errors often involve two mismatched signal processes (e.g.
/// `A : B` where A's output count ≠ B's input count).  `PairedContext` holds
/// the human-readable rendering of both sides so the CLI can emit a C++-style
/// "Here A ... / while B ..." message without baking that format into the
/// structured diagnostic schema.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PairedContext {
    /// Human-readable rendering of the left-hand (A) sub-expression.
    a_expr: String,
    /// Human-readable rendering of the right-hand (B) sub-expression.
    b_expr: String,
    /// Signal arity of A (e.g. `"2→1"`), if available.
    a_arity: Option<String>,
    /// Signal arity of B (e.g. `"1→2"`), if available.
    b_arity: Option<String>,
}

/// Extracts paired composition context (`A`/`B`) from diagnostic notes.
///
/// This enables C++-style human rendering (`Here A ... / while B ...`) without
/// changing the structured diagnostic schema.
fn paired_context_from_notes(notes: &[Box<str>]) -> Option<PairedContext> {
    let mut a_expr = None::<String>;
    let mut b_expr = None::<String>;
    let mut a_arity = None::<String>;
    let mut b_arity = None::<String>;

    for note in notes {
        if let Some(rest) = note.strip_prefix("A arity: ") {
            a_arity = Some(rest.to_owned());
            continue;
        }
        if let Some(rest) = note.strip_prefix("B arity: ") {
            b_arity = Some(rest.to_owned());
            continue;
        }
        if let Some(rest) = note.strip_prefix("A ") {
            if let Some((_, expr)) = rest.split_once(" = ") {
                a_expr = Some(expr.to_owned());
            }
            continue;
        }
        if let Some(rest) = note.strip_prefix("B ") {
            if let Some((_, expr)) = rest.split_once(" = ") {
                b_expr = Some(expr.to_owned());
            }
            continue;
        }
    }

    Some(PairedContext {
        a_expr: a_expr?,
        b_expr: b_expr?,
        a_arity,
        b_arity,
    })
}

/// Filters note lines for human rendering.
///
/// When paired context exists, low-level `A ...` / `B ...` notes are hidden from
/// direct printing because they are rendered as condensed C++-style blocks.
///
/// Internal machine-oriented notes (`node_id`, `box_expr`) are also hidden in
/// standard human mode to keep output focused on actionable diagnostics.
fn filtered_notes_for_human(
    notes: &[Box<str>],
    has_paired_context: bool,
    verbosity: ErrorVerbosity,
) -> Vec<&str> {
    let mut out = Vec::new();
    for note in notes {
        if matches!(verbosity, ErrorVerbosity::Standard)
            && (note.starts_with("node_id=") || note.starts_with("box_expr="))
        {
            continue;
        }
        if has_paired_context
            && (note.starts_with("A ")
                || note.starts_with("B ")
                || note.starts_with("A arity: ")
                || note.starts_with("B arity: "))
        {
            continue;
        }
        out.push(note.as_ref());
    }
    out
}

/// Returns one source line from the immutable compilation snapshot, falling
/// back to the filesystem for legacy bundles without a [`diagnostics::SourceMap`].
fn source_line(bundle: &DiagnosticBundle, path: &Path, line_number: u32) -> Option<String> {
    if let Some(source) = bundle.source_map().find_by_name(path) {
        return source.line_text(line_number).map(str::to_owned);
    }
    let source = std::fs::read_to_string(path).ok()?;
    let idx = usize::try_from(line_number.checked_sub(1)?).ok()?;
    source.lines().nth(idx).map(str::to_owned)
}

/// Builds a caret marker string from 1-based `(col, end_col)` bounds.
fn caret_span(col: u32, end_col: u32) -> String {
    let start = usize::try_from(col.saturating_sub(1)).unwrap_or(0);
    let end = usize::try_from(end_col.saturating_sub(1)).unwrap_or(start);
    let width = end.saturating_sub(start).max(1);
    format!("{}{}", " ".repeat(start), "^".repeat(width))
}

/// Formats the typed, versioned machine diagnostics envelope.
///
/// This function serializes only typed machine fields. It never classifies
/// label/note/help prose or extracts facts from string prefixes.
pub fn format_diagnostics_json(bundle: &DiagnosticBundle) -> String {
    format_diagnostics_json_with_verbosity(bundle, ErrorVerbosity::Standard)
}

/// Formats diagnostics JSON with optional typed debug evidence.
pub fn format_diagnostics_json_with_verbosity(
    bundle: &DiagnosticBundle,
    verbosity: ErrorVerbosity,
) -> String {
    let sources = bundle
        .source_map()
        .iter()
        .map(|source| {
            let text = match source.kind() {
                SourceKind::Memory | SourceKind::VirtualLibrary => Some(source.text()),
                SourceKind::File | SourceKind::ImportedFile => None,
            };
            json!({
                "id": source.id().as_u32(),
                "name": source.name().display().to_string(),
                "kind": source_kind_name(source.kind()),
                "content_hash": source.content_hash().to_hex(),
                "text": text,
            })
        })
        .collect::<Vec<_>>();
    let diagnostics = bundle
        .as_slice()
        .iter()
        .map(|diag| {
            let labels = diag
                .labels
                .iter()
                .map(|label| label_v2_json(bundle, label))
                .collect::<Vec<_>>();
            let facts = diag
                .facts
                .iter()
                .map(|(key, value)| (key.as_str().to_owned(), diagnostic_value_json(value)))
                .collect::<serde_json::Map<_, _>>();
            let traces = diag
                .traces
                .iter()
                .map(|trace| {
                    json!({
                        "kind": trace_kind_name(trace.kind),
                        "frames": trace.frames.iter().map(|frame| {
                            json!({
                                "name": frame.name,
                                "range": frame.span.map(source_range_json),
                                "ir": frame.ir.as_ref().map(|ir| json!({
                                    "kind": ir.kind,
                                    "id": ir.id,
                                })),
                                "description": frame.description,
                            })
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            let fixes = diag
                .fixes
                .iter()
                .map(|fix| {
                    json!({
                        "title": fix.title,
                        "applicability": applicability_name(fix.applicability),
                        "edits": fix.edits.iter().map(|edit| json!({
                            "range": source_range_json(edit.range),
                            "replacement": edit.replacement,
                        })).collect::<Vec<_>>(),
                        "explanation": fix.explanation,
                    })
                })
                .collect::<Vec<_>>();
            let related = diag
                .related
                .iter()
                .map(|related| {
                    json!({
                        "code": related.code.0,
                        "message": related.message,
                        "labels": related.labels.iter()
                            .map(|label| label_v2_json(bundle, label))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            let debug = if matches!(verbosity, ErrorVerbosity::Debug) {
                diag.debug.as_ref().map(|debug| {
                    debug
                        .fields
                        .iter()
                        .map(|(key, value)| (key.as_str().to_owned(), diagnostic_value_json(value)))
                        .collect::<serde_json::Map<_, _>>()
                })
            } else {
                None
            };
            json!({
                "severity": severity_name(diag.severity),
                "stage": stage_name(diag.stage),
                "code": diag.code.0,
                "detail_code": diag.detail_code.as_ref().map(|code| code.as_str()),
                "category": category_name(diag.category),
                "message": diag.message,
                "labels": labels,
                "facts": facts,
                "traces": traces,
                "fixes": fixes,
                "related": related,
                "notes": diag.notes,
                "help": diag.help,
                "debug": debug,
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "schema_version": 2,
        "compiler": {
            "name": "faust-rs",
            "version": env!("CARGO_PKG_VERSION"),
            "target": format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        },
        "request": {
            "mode": serde_json::Value::Null,
            "backend": serde_json::Value::Null,
            "normalized_options": [],
        },
        "status": if bundle.error_count() == 0 { "success" } else { "failed" },
        "sources": sources,
        "diagnostics": diagnostics,
    }))
    .expect("diagnostics v2 JSON formatting should not fail")
}

fn label_v2_json(bundle: &DiagnosticBundle, label: &Label) -> serde_json::Value {
    let range = bundle
        .source_map()
        .from_source_span(&label.span)
        .ok()
        .map(source_range_json);
    json!({
        "style": match label.style {
            LabelStyle::Primary => "primary",
            LabelStyle::Secondary => "secondary",
        },
        "role": label_role_name(label.role),
        "range": range,
        "compatibility_span": {
            "file": label.span.file.display().to_string(),
            "line": label.span.line,
            "col": label.span.col,
            "end_line": label.span.end_line,
            "end_col": label.span.end_col,
        },
        "message": label.message,
    })
}

fn source_range_json(range: SourceRange) -> serde_json::Value {
    json!({
        "source_id": range.source.as_u32(),
        "start": range.start,
        "end": range.end,
    })
}

fn diagnostic_value_json(value: &DiagnosticValue) -> serde_json::Value {
    match value {
        DiagnosticValue::String(value) => json!({"type": "string", "value": value}),
        DiagnosticValue::Integer(value) => json!({"type": "integer", "value": value}),
        DiagnosticValue::Unsigned(value) => json!({"type": "unsigned", "value": value}),
        DiagnosticValue::Real(value) => json!({"type": "real", "value": value}),
        DiagnosticValue::Boolean(value) => json!({"type": "boolean", "value": value}),
        DiagnosticValue::StringList(values) => {
            json!({"type": "string_list", "value": values})
        }
        DiagnosticValue::IntegerRange { min, max } => {
            json!({"type": "integer_range", "min": min, "max": max})
        }
        DiagnosticValue::Object(fields) => {
            let value = fields
                .iter()
                .map(|(key, value)| (key.as_str().to_owned(), diagnostic_value_json(value)))
                .collect::<serde_json::Map<_, _>>();
            json!({"type": "object", "value": value})
        }
    }
}

const fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Remark => "remark",
    }
}

const fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::SourceReader => "source_reader",
        Stage::Lexer => "lexer",
        Stage::Parser => "parser",
        Stage::Eval => "eval",
        Stage::Propagate => "propagate",
        Stage::Normalize => "normalize",
        Stage::Transform => "transform",
        Stage::Fir => "fir",
        Stage::Codegen => "codegen",
        Stage::Compiler => "compiler",
    }
}

const fn category_name(category: DiagnosticCategory) -> &'static str {
    match category {
        DiagnosticCategory::UserCode => "user_code",
        DiagnosticCategory::UnsupportedFeature => "unsupported_feature",
        DiagnosticCategory::InvalidOptions => "invalid_options",
        DiagnosticCategory::Environment => "environment",
        DiagnosticCategory::Cancelled => "cancelled",
        DiagnosticCategory::CompilerBug => "compiler_bug",
    }
}

const fn source_kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::File => "file",
        SourceKind::Memory => "memory",
        SourceKind::ImportedFile => "imported_file",
        SourceKind::VirtualLibrary => "virtual_library",
    }
}

const fn label_role_name(role: LabelRole) -> &'static str {
    match role {
        LabelRole::PrimaryCause => "primary_cause",
        LabelRole::UseSite => "use_site",
        LabelRole::DefinitionSite => "definition_site",
        LabelRole::CallSite => "call_site",
        LabelRole::Operator => "operator",
        LabelRole::ExpectedHere => "expected_here",
        LabelRole::ConflictsWith => "conflicts_with",
        LabelRole::ImportSite => "import_site",
        LabelRole::PreviousToken => "previous_token",
        LabelRole::MatchingDelimiter => "matching_delimiter",
        LabelRole::DerivedFrom => "derived_from",
    }
}

const fn trace_kind_name(kind: TraceKind) -> &'static str {
    match kind {
        TraceKind::Binding => "binding",
        TraceKind::Import => "import",
        TraceKind::Expansion => "expansion",
        TraceKind::Evaluation => "evaluation",
        TraceKind::Transformation => "transformation",
        TraceKind::Causal => "causal",
    }
}

const fn applicability_name(applicability: Applicability) -> &'static str {
    match applicability {
        Applicability::MachineApplicable => "machine_applicable",
        Applicability::MaybeIncorrect => "maybe_incorrect",
        Applicability::HasPlaceholders => "has_placeholders",
        Applicability::Manual => "manual",
    }
}
