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
use diagnostics::{DiagnosticBundle, LabelStyle, Severity, Stage};
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

/// Formats diagnostics in a machine-oriented JSON payload.
pub fn format_diagnostics_json(bundle: &DiagnosticBundle) -> String {
    format_diagnostics_json_with_verbosity(bundle, ErrorVerbosity::Standard)
}

/// Formats diagnostics in JSON with optional debug-oriented enrichment.
///
/// `Standard` keeps the stable CI/IDE contract.
/// `Debug` adds extracted low-level fields under `diagnostics[*].debug`.
pub fn format_diagnostics_json_with_verbosity(
    bundle: &DiagnosticBundle,
    verbosity: ErrorVerbosity,
) -> String {
    let diagnostics = bundle
        .as_slice()
        .iter()
        .map(|diag| {
            let labels = diag
                .labels
                .iter()
                .map(|label| {
                    let role = label_role(label.message.as_ref());
                    json!({
                        "style": match label.style {
                            LabelStyle::Primary => "primary",
                            LabelStyle::Secondary => "secondary",
                        },
                        "role": role,
                        "file": label.span.file.display().to_string(),
                        "line": label.span.line,
                        "col": label.span.col,
                        "end_line": label.span.end_line,
                        "end_col": label.span.end_col,
                        "message": label.message,
                    })
                })
                .collect::<Vec<_>>();
            let mut payload = json!({
                "severity": match diag.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Remark => "remark",
                },
                "stage": match diag.stage {
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
                },
                "code": diag.code.0,
                "message": diag.message,
                "labels": labels,
                "notes": diag.notes,
                "help": diag.help,
                "context": diagnostic_context_from_notes(diag.notes.as_slice()),
            });
            if matches!(verbosity, ErrorVerbosity::Debug)
                && let Some(obj) = payload.as_object_mut()
            {
                obj.insert(
                    "debug".to_owned(),
                    diagnostic_debug_from_notes(diag.notes.as_slice()),
                );
            }
            payload
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({ "diagnostics": diagnostics }))
        .expect("diagnostics JSON formatting should not fail")
}

/// Maps human label messages to stable JSON role identifiers.
///
/// This keeps machine-readable output decoupled from prose used in human mode.
fn label_role(message: &str) -> Option<&'static str> {
    match message {
        "call site" => Some("call_site"),
        "definition site" => Some("definition_site"),
        _ => None,
    }
}

/// Extracts structured context fields from diagnostic notes when present.
///
/// The extraction is best-effort and intentionally tolerant: unknown notes are
/// ignored so textual diagnostics can evolve without breaking JSON consumers.
fn diagnostic_context_from_notes(notes: &[Box<str>]) -> serde_json::Value {
    let mut owner_definition = None::<String>;
    let mut binding_trace = None::<Vec<String>>;
    let mut scope_local = None::<String>;
    let mut scope_visible = None::<String>;
    let mut scope_top_level = None::<String>;

    for note in notes {
        if let Some(owner) = note.strip_prefix("error originates from definition '") {
            owner_definition = Some(owner.trim_end_matches('\'').to_owned());
            continue;
        }
        if let Some(trace) = note.strip_prefix("binding_trace=") {
            let path = trace.split(" -> ").map(str::to_owned).collect::<Vec<_>>();
            if !path.is_empty() {
                binding_trace = Some(path);
            }
            continue;
        }
        if let Some(v) = note.strip_prefix("scope.local=") {
            scope_local = Some(v.to_owned());
            continue;
        }
        if let Some(v) = note.strip_prefix("scope.visible=") {
            scope_visible = Some(v.to_owned());
            continue;
        }
        if let Some(v) = note.strip_prefix("scope.top_level=") {
            scope_top_level = Some(v.to_owned());
            continue;
        }
    }

    json!({
        "owner_definition": owner_definition,
        "binding_trace_path": binding_trace,
        "scope": {
            "local": scope_local,
            "visible": scope_visible,
            "top_level": scope_top_level,
        }
    })
}

/// Extracts debug-only fields from diagnostic notes.
///
/// This keeps internal details (`node_id`, `box_expr`) out of the default JSON
/// surface while still allowing explicit debug workflows.
fn diagnostic_debug_from_notes(notes: &[Box<str>]) -> serde_json::Value {
    let mut node_id = None::<u32>;
    let mut box_expr = None::<String>;
    for note in notes {
        if let Some(v) = note.strip_prefix("node_id=") {
            node_id = v.parse::<u32>().ok();
            continue;
        }
        if let Some(v) = note.strip_prefix("box_expr=") {
            box_expr = Some(v.to_owned());
            continue;
        }
    }
    json!({
        "node_id": node_id,
        "box_expr": box_expr
    })
}
