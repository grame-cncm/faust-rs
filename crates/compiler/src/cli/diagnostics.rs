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
//! Under `--error-format json`, [`print_bundle`] is the sole
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
//! Every `compiler::CompilerError` variant carries a structured
//! [`DiagnosticBundle`].
//! The total `CompilerError::diagnostic_bundle` accessor therefore keeps both
//! human and JSON rendering on the stable diagnostic model without a
//! text-only fallback path.
//!
//! See `docs/faust-error-model-en.md` for the frozen `FRS-*` code table.

use super::args::{DiagnosticPathStyle, ErrorFormat, ErrorVerbosity};
use super::human::{self, HumanRenderOptions};
use diagnostics::{
    Applicability, DiagnosticBundle, DiagnosticCategory, DiagnosticValue, Label, LabelRole,
    LabelStyle, Severity, SourceKind, SourceRange, Stage, TraceKind,
};
use serde_json::json;

/// Prints one bundle with an explicit path style.
pub fn print_bundle(
    bundle: &DiagnosticBundle,
    format: ErrorFormat,
    verbosity: ErrorVerbosity,
    path_style: DiagnosticPathStyle,
) {
    match format {
        ErrorFormat::Human => eprint!(
            "{}",
            human::format_bundle(
                bundle,
                HumanRenderOptions {
                    verbosity,
                    path_style,
                },
            )
        ),
        ErrorFormat::Json => println!(
            "{}",
            format_diagnostics_json_with_verbosity(bundle, verbosity)
        ),
    }
}

/// Formats diagnostics in a human-oriented form at the default verbosity.
///
/// Test-facing convenience over [`human::format_bundle`]; production callers go
/// through [`print_bundle`], which also carries the path style.
#[cfg(test)]
pub fn format_diagnostics_human(bundle: &DiagnosticBundle) -> String {
    human::format_bundle(bundle, HumanRenderOptions::default())
}

/// Formats diagnostics in human mode at an explicit verbosity.
#[cfg(test)]
pub fn format_diagnostics_human_with_verbosity(
    bundle: &DiagnosticBundle,
    verbosity: ErrorVerbosity,
) -> String {
    human::format_bundle(
        bundle,
        HumanRenderOptions {
            verbosity,
            ..HumanRenderOptions::default()
        },
    )
}

/// Formats the typed machine envelope at the default verbosity.
#[cfg(test)]
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
        Stage::TypeInference => "type_inference",
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
