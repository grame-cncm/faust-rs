//! Reusable diagnostics-v2 JSON rendering.
//!
//! This module is the library-owned machine channel shared by the CLI and FFI
//! adapters. It projects a [`DiagnosticBundle`] directly into the schema in
//! `docs/diagnostics-v2.schema.json`; callers must never reconstruct typed
//! fields by parsing human messages, notes, or help text.
//!
//! The WASM FFI uses [`render_complete_diagnostics_v2_json`] so its
//! parameter-free diagnostics query always returns every retained field.
//! [`DiagnosticFieldSet::Standard`] exists only to preserve the established
//! CLI compatibility view.
//!
//! # Source provenance
//!
//! Extracted from the former CLI-only renderer in
//! `crates/compiler/src/cli/diagnostics.rs`. The data model originates in the
//! Rust port's structured replacement for the C++ compiler's text-only error
//! reporting.

use diagnostics::{
    Applicability, DiagnosticBundle, DiagnosticCategory, DiagnosticValue, Label, LabelRole,
    LabelStyle, Severity, SourceId, SourceKind, SourceRange, Stage, TraceKind,
};
use serde_json::json;

/// Compiler identity written into a diagnostics-v2 envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticsCompilerMetadata {
    /// Compiler implementation name.
    pub name: String,
    /// Compiler package or build version.
    pub version: String,
    /// Target architecture/operating-system description.
    pub target: String,
}

impl Default for DiagnosticsCompilerMetadata {
    fn default() -> Self {
        Self {
            name: "faust-rs".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        }
    }
}

/// Request identity written into a diagnostics-v2 envelope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticsRequestMetadata {
    /// Host operation or compilation mode, when known.
    pub mode: Option<String>,
    /// Selected backend name, when known.
    pub backend: Option<String>,
    /// Normalized compiler options in deterministic order.
    pub normalized_options: Vec<String>,
}

/// Controls which immutable source snapshots are embedded in the JSON report.
///
/// Source ids, names, kinds, hashes, and diagnostic ranges are emitted under
/// every policy. This setting controls only the optional `sources[].text`
/// field, so privacy and payload size are independent from diagnostic detail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceTextPolicy {
    /// Never embed source text.
    None,
    /// Embed only the first in-memory primary source.
    PrimaryMemorySource,
    /// Embed every in-memory or virtual-library source.
    #[default]
    AllMemorySources,
}

/// Selects the machine fields included by the reusable renderer.
///
/// FFI consumers should use [`DiagnosticFieldSet::Complete`] through
/// [`render_complete_diagnostics_v2_json`]. The standard field set is retained
/// solely for the existing CLI JSON compatibility contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiagnosticFieldSet {
    /// Exclude compiler-internal `debug` values.
    #[default]
    Standard,
    /// Include every retained typed field, including `debug` values.
    Complete,
}

/// Options for rendering one diagnostics-v2 JSON envelope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticsV2RenderOptions {
    /// Compiler identity attached to the envelope.
    pub compiler: DiagnosticsCompilerMetadata,
    /// Request identity attached to the envelope.
    pub request: DiagnosticsRequestMetadata,
    /// Source snapshot embedding policy.
    pub source_text: SourceTextPolicy,
    /// Machine field projection.
    pub fields: DiagnosticFieldSet,
}

/// Renders one diagnostics-v2 JSON envelope.
///
/// Output is deterministic for the same bundle and options. The function
/// cannot fail because it builds a `serde_json::Value` containing only owned
/// strings, numbers, booleans, arrays, objects, and nulls.
#[must_use]
pub fn render_diagnostics_v2_json(
    bundle: &DiagnosticBundle,
    options: &DiagnosticsV2RenderOptions,
) -> String {
    let primary_memory_source = bundle
        .source_map()
        .iter()
        .find(|source| source.kind() == SourceKind::Memory)
        .map(|source| source.id());
    let sources = bundle
        .source_map()
        .iter()
        .map(|source| {
            let text = source_text(source.kind(), source.id(), primary_memory_source, options)
                .then(|| source.text());
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
        .map(|diagnostic| {
            let labels = diagnostic
                .labels
                .iter()
                .map(|label| label_v2_json(bundle, label))
                .collect::<Vec<_>>();
            let facts = diagnostic
                .facts
                .iter()
                .map(|(key, value)| (key.as_str().to_owned(), diagnostic_value_json(value)))
                .collect::<serde_json::Map<_, _>>();
            let traces = diagnostic
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
            let fixes = diagnostic
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
            let related = diagnostic
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
            let debug = match options.fields {
                DiagnosticFieldSet::Standard => None,
                DiagnosticFieldSet::Complete => diagnostic.debug.as_ref().map(|debug| {
                    debug
                        .fields
                        .iter()
                        .map(|(key, value)| (key.as_str().to_owned(), diagnostic_value_json(value)))
                        .collect::<serde_json::Map<_, _>>()
                }),
            };
            json!({
                "severity": severity_name(diagnostic.severity),
                "stage": stage_name(diagnostic.stage),
                "code": diagnostic.code.0,
                "detail_code": diagnostic.detail_code.as_ref().map(|code| code.as_str()),
                "category": category_name(diagnostic.category),
                "message": diagnostic.message,
                "labels": labels,
                "facts": facts,
                "traces": traces,
                "fixes": fixes,
                "related": related,
                "notes": diagnostic.notes,
                "help": diagnostic.help,
                "debug": debug,
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "schema_version": 2,
        "compiler": {
            "name": options.compiler.name,
            "version": options.compiler.version,
            "target": options.compiler.target,
        },
        "request": {
            "mode": options.request.mode,
            "backend": options.request.backend,
            "normalized_options": options.request.normalized_options,
        },
        "status": if bundle.error_count() == 0 { "success" } else { "failed" },
        "sources": sources,
        "diagnostics": diagnostics,
    }))
    .expect("diagnostics v2 JSON formatting should not fail")
}

/// Renders the complete diagnostics-v2 report expected by FFI consumers.
///
/// This parameter-free detail contract includes every retained label, fact,
/// trace frame, fix, related diagnostic, note, help entry, and typed debug
/// value. Consumers can derive smaller views locally without asking the
/// compiler to discard information.
#[must_use]
pub fn render_complete_diagnostics_v2_json(
    bundle: &DiagnosticBundle,
    compiler: DiagnosticsCompilerMetadata,
    request: DiagnosticsRequestMetadata,
    source_text: SourceTextPolicy,
) -> String {
    render_diagnostics_v2_json(
        bundle,
        &DiagnosticsV2RenderOptions {
            compiler,
            request,
            source_text,
            fields: DiagnosticFieldSet::Complete,
        },
    )
}

fn source_text(
    kind: SourceKind,
    id: SourceId,
    primary_memory_source: Option<SourceId>,
    options: &DiagnosticsV2RenderOptions,
) -> bool {
    match options.source_text {
        SourceTextPolicy::None => false,
        SourceTextPolicy::PrimaryMemorySource => {
            kind == SourceKind::Memory && Some(id) == primary_memory_source
        }
        SourceTextPolicy::AllMemorySources => {
            matches!(kind, SourceKind::Memory | SourceKind::VirtualLibrary)
        }
    }
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

#[cfg(test)]
mod tests {
    use diagnostics::{
        DebugContext, Diagnostic, DiagnosticBundle, DiagnosticCode, Severity, SourceKind,
        SourceMapBuilder, Stage,
    };

    use super::{
        DiagnosticFieldSet, DiagnosticsRequestMetadata, DiagnosticsV2RenderOptions,
        SourceTextPolicy, render_complete_diagnostics_v2_json, render_diagnostics_v2_json,
    };

    fn bundle_with_debug_and_sources() -> DiagnosticBundle {
        let mut sources = SourceMapBuilder::new();
        sources.add("main.dsp", SourceKind::Memory, "process = missing;\n");
        sources.add("library.lib", SourceKind::VirtualLibrary, "value = 1;\n");
        let mut bundle = DiagnosticBundle::new();
        bundle.set_source_map(sources.finish());
        bundle.push(
            Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                DiagnosticCode("FRS-EVAL-0002"),
                "undefined symbol",
            )
            .with_fact("symbol", "missing")
            .with_debug_context(DebugContext::new().with_field("box_id", 42_u64)),
        );
        bundle
    }

    #[test]
    fn complete_renderer_keeps_debug_and_request_metadata() {
        let bundle = bundle_with_debug_and_sources();
        let rendered = render_complete_diagnostics_v2_json(
            &bundle,
            Default::default(),
            DiagnosticsRequestMetadata {
                mode: Some("compile_dsp".to_owned()),
                backend: Some("wasm".to_owned()),
                normalized_options: vec!["-lang".to_owned(), "wasm".to_owned()],
            },
            SourceTextPolicy::None,
        );
        let value: serde_json::Value =
            serde_json::from_str(&rendered).expect("complete report must be JSON");

        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["request"]["mode"], "compile_dsp");
        assert_eq!(value["request"]["backend"], "wasm");
        assert_eq!(value["diagnostics"][0]["debug"]["box_id"]["value"], 42);
        assert!(value["sources"][0]["text"].is_null());
        assert!(value["sources"][1]["text"].is_null());
    }

    #[test]
    fn source_text_policies_are_independent_from_field_completeness() {
        let bundle = bundle_with_debug_and_sources();
        let options = DiagnosticsV2RenderOptions {
            source_text: SourceTextPolicy::PrimaryMemorySource,
            fields: DiagnosticFieldSet::Complete,
            ..Default::default()
        };
        let value: serde_json::Value =
            serde_json::from_str(&render_diagnostics_v2_json(&bundle, &options))
                .expect("report must be JSON");

        assert_eq!(value["sources"][0]["text"], "process = missing;\n");
        assert!(value["sources"][1]["text"].is_null());
        assert_eq!(value["diagnostics"][0]["debug"]["box_id"]["value"], 42);
    }

    #[test]
    fn standard_field_set_preserves_cli_debug_omission() {
        let bundle = bundle_with_debug_and_sources();
        let rendered = render_diagnostics_v2_json(&bundle, &DiagnosticsV2RenderOptions::default());
        let value: serde_json::Value =
            serde_json::from_str(&rendered).expect("standard report must be JSON");

        assert!(value["diagnostics"][0]["debug"].is_null());
        assert_eq!(value["sources"][1]["text"], "value = 1;\n");
    }
}
