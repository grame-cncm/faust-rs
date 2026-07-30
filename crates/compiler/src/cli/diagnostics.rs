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
//! See `docs/diagnostics-codes-reference-en.md` for the frozen `FRS-*` code table.

use super::args::{DiagnosticPathStyle, ErrorFormat, ErrorVerbosity};
use super::human::{self, HumanRenderOptions};
use compiler::diagnostics_json::{
    DiagnosticFieldSet, DiagnosticsV2RenderOptions, SourceTextPolicy, render_diagnostics_v2_json,
};
use diagnostics::DiagnosticBundle;

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
    render_diagnostics_v2_json(
        bundle,
        &DiagnosticsV2RenderOptions {
            source_text: SourceTextPolicy::AllMemorySources,
            fields: if matches!(verbosity, ErrorVerbosity::Debug) {
                DiagnosticFieldSet::Complete
            } else {
                DiagnosticFieldSet::Standard
            },
            ..Default::default()
        },
    )
}
