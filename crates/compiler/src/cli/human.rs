//! Human-oriented diagnostic rendering.
//!
//! # Progressive disclosure
//!
//! The renderer shows the smallest complete explanation at
//! [`ErrorVerbosity::Standard`] and adds compiler-internal evidence above it.
//! Each level is a superset of the one below, so raising verbosity never hides
//! something the reader just saw:
//!
//! | Level | Adds |
//! | --- | --- |
//! | `Concise` | header, primary snippet, first help |
//! | `Standard` | every relevant label, rule/computed notes, traces, fixes |
//! | `Debug` | internal ids, box/signal previews, typed debug context |
//! | `Full` | untruncated traces and related diagnostics |
//!
//! # What this module must not do
//!
//! It reads typed fields only. Machine information travels in
//! [`diagnostics::Diagnostic::facts`], `traces`, `fixes`, and label roles;
//! notes and help are presentation text. The one exception is the paired
//! `A`/`B` composition block, which is a rendering convention over notes a
//! propagate diagnostic writes for that purpose — it classifies nothing.

use std::collections::BTreeSet;
use std::path::Path;

use diagnostics::{
    Applicability, DiagnosticBundle, DiagnosticTrace, Label, LabelStyle, Severity, SuggestedFix,
};
use unicode_width::UnicodeWidthStr;

use super::args::{DiagnosticPathStyle, ErrorVerbosity};

/// Tab width used when expanding source lines for display.
///
/// Matches the value `diagnostics::SourceMap` uses to derive human columns, so
/// a caret lands under the character the column names.
const TAB_WIDTH: usize = 4;

/// Maximum trace frames shown below [`ErrorVerbosity::Full`].
const MAX_TRACE_FRAMES: usize = 6;

/// Rendering policy for one human diagnostic run.
#[derive(Clone, Copy, Debug)]
pub struct HumanRenderOptions {
    /// How much internal evidence to include.
    pub verbosity: ErrorVerbosity,
    /// How source paths are spelled.
    pub path_style: DiagnosticPathStyle,
}

impl Default for HumanRenderOptions {
    fn default() -> Self {
        Self {
            verbosity: ErrorVerbosity::Standard,
            path_style: DiagnosticPathStyle::Absolute,
        }
    }
}

/// Formats one bundle for a terminal reader.
pub fn format_bundle(bundle: &DiagnosticBundle, options: HumanRenderOptions) -> String {
    let mut out = String::new();
    for diagnostic in bundle.as_slice() {
        render_header(&mut out, diagnostic, options);
        render_labels(&mut out, bundle, diagnostic, options);

        if options.verbosity == ErrorVerbosity::Concise {
            if let Some(help) = diagnostic.help.first() {
                out.push_str(&format!("  = help: {help}\n"));
            }
            continue;
        }

        render_paired_composition(&mut out, &diagnostic.notes);
        for note in visible_notes(&diagnostic.notes, options.verbosity) {
            out.push_str(&format!("  = note: {note}\n"));
        }
        for trace in &diagnostic.traces {
            render_trace(&mut out, trace, options.verbosity);
        }
        for fix in &diagnostic.fixes {
            render_fix(&mut out, fix);
        }
        for help in &diagnostic.help {
            out.push_str(&format!("  = help: {help}\n"));
        }
        if options.verbosity.shows_internals()
            && let Some(debug) = &diagnostic.debug
        {
            for (key, value) in &debug.fields {
                out.push_str(&format!("  = debug: {}={value:?}\n", key.as_str()));
            }
        }
        if options.verbosity.shows_everything() {
            for related in &diagnostic.related {
                out.push_str(&format!(
                    "  = related: [{}] {}\n",
                    related.code.0, related.message
                ));
            }
        }
    }
    out
}

/// Writes the `path:line:col: severity [CODE] message` header.
///
/// A diagnostic with no located label still gets a header, because a failure
/// the compiler cannot place is exactly the one a reader must not miss.
fn render_header(
    out: &mut String,
    diagnostic: &diagnostics::Diagnostic,
    options: HumanRenderOptions,
) {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Remark => "remark",
    };
    match primary_label(diagnostic) {
        Some(label) => out.push_str(&format!(
            "{}:{}:{}: {severity} [{}] {}\n",
            display_path(&label.span.file, options.path_style),
            label.span.line,
            label.span.col,
            diagnostic.code.0,
            diagnostic.message
        )),
        None => out.push_str(&format!(
            "{severity} [{}] {}\n",
            diagnostic.code.0, diagnostic.message
        )),
    }
}

/// Renders every label worth showing, deduplicated by span and message.
///
/// `Concise` keeps only the primary label. Above it, secondary labels are shown
/// too: a conflicting declaration or a call site is usually where the reader
/// has to look next, and dropping them was the main reason terminal output used
/// to carry less than the JSON channel already did.
///
/// A label in a different file than the previous one gets a `--> path` line, so
/// a diagnostic spanning an import chain stays readable.
fn render_labels(
    out: &mut String,
    bundle: &DiagnosticBundle,
    diagnostic: &diagnostics::Diagnostic,
    options: HumanRenderOptions,
) {
    let mut shown = BTreeSet::new();
    let mut current_file: Option<&Path> = primary_label(diagnostic).map(|l| l.span.file.as_path());
    let labels: Vec<&Label> = if options.verbosity == ErrorVerbosity::Concise {
        primary_label(diagnostic).into_iter().collect()
    } else {
        ordered_labels(diagnostic)
    };

    // Labels that land on one line share its snippet: echoing the same source
    // line once per label is noise, and the carets are what distinguish them.
    let mut current_line: Option<(&Path, u32)> = None;
    for label in labels {
        let key = (
            label.span.file.clone(),
            label.span.line,
            label.span.col,
            label.message.clone(),
        );
        if !shown.insert(key) {
            continue;
        }
        if current_file != Some(label.span.file.as_path()) {
            out.push_str(&format!(
                "  --> {}\n",
                display_path(&label.span.file, options.path_style)
            ));
            current_file = Some(label.span.file.as_path());
            current_line = None;
        }
        let single_line = label.span.end_line <= label.span.line;
        let repeats_line =
            single_line && current_line == Some((label.span.file.as_path(), label.span.line));
        render_snippet(out, bundle, label, repeats_line);
        current_line = single_line.then_some((label.span.file.as_path(), label.span.line));
    }
}

/// Orders labels primary-first, then by source position.
///
/// Producers append labels in whatever order suits them; a reader needs the
/// blamed location first and the rest in the order they appear in the file.
fn ordered_labels(diagnostic: &diagnostics::Diagnostic) -> Vec<&Label> {
    let mut labels: Vec<&Label> = diagnostic.labels.iter().collect();
    labels.sort_by_key(|label| {
        (
            u8::from(label.style == LabelStyle::Secondary),
            label.span.file.clone(),
            label.span.line,
            label.span.col,
        )
    });
    labels
}

/// Returns the label that names the blamed location.
fn primary_label(diagnostic: &diagnostics::Diagnostic) -> Option<&Label> {
    diagnostic
        .labels
        .iter()
        .find(|label| label.style == LabelStyle::Primary)
        .or_else(|| diagnostic.labels.first())
}

/// Writes the source line(s) covered by one label, with a caret run.
///
/// A multi-line span shows its first and last lines with an elision marker
/// between them: the two boundaries are what identify the construct, and
/// echoing a hundred lines in between would bury the diagnostic.
///
/// `line_already_shown` suppresses the source line when the previous label
/// printed it, leaving only this label's caret row.
fn render_snippet(
    out: &mut String,
    bundle: &DiagnosticBundle,
    label: &Label,
    line_already_shown: bool,
) {
    let Some(first) = source_line(bundle, label.span.file.as_path(), label.span.line) else {
        return;
    };
    let expanded = expand_tabs(&first);
    if !line_already_shown {
        out.push_str(&format!("  {} | {}\n", label.span.line, expanded));
    }

    if label.span.end_line > label.span.line {
        out.push_str("    | ...\n");
        if let Some(last) = source_line(bundle, label.span.file.as_path(), label.span.end_line) {
            out.push_str(&format!(
                "  {} | {}\n",
                label.span.end_line,
                expand_tabs(&last)
            ));
        }
        out.push_str(&format!("    | {}\n", label.message));
        return;
    }

    out.push_str(&format!(
        "    | {} {}\n",
        caret_run(&first, label.span.col, label.span.end_col),
        label.message
    ));
}

/// Builds the caret run for a single-line span.
///
/// Columns count Unicode scalars in the *raw* line, while the rendered line has
/// its tabs expanded — so the offset is the display width of the raw prefix
/// after the same expansion, not a scalar count. That keeps a caret under its
/// character on lines containing tabs or wide glyphs.
///
/// A zero-width span (an insertion point) still gets one caret: the reader
/// needs a position, not a range.
fn caret_run(raw_line: &str, col: u32, end_col: u32) -> String {
    let start = display_offset(raw_line, col);
    let end = display_offset(raw_line, end_col).max(start);
    format!("{}{}", " ".repeat(start), "^".repeat((end - start).max(1)))
}

/// Rendered width of the prefix of `raw_line` before 1-based scalar column `col`.
fn display_offset(raw_line: &str, col: u32) -> usize {
    let scalars = usize::try_from(col.saturating_sub(1)).unwrap_or(0);
    let prefix: String = raw_line.chars().take(scalars).collect();
    expand_tabs(&prefix).width()
}

/// Replaces tabs with spaces so column arithmetic matches what is printed.
fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        if ch == '\t' {
            let pad = TAB_WIDTH - (out.width() % TAB_WIDTH);
            out.push_str(&" ".repeat(pad));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Renders one typed trace as a single arrow-joined line.
fn render_trace(out: &mut String, trace: &DiagnosticTrace, verbosity: ErrorVerbosity) {
    if trace.frames.is_empty() {
        return;
    }
    let limit = if verbosity.shows_everything() {
        trace.frames.len()
    } else {
        MAX_TRACE_FRAMES
    };
    let shown = trace
        .frames
        .iter()
        .take(limit)
        .map(|frame| {
            frame
                .name
                .as_deref()
                .unwrap_or(frame.description.as_ref())
                .to_owned()
        })
        .collect::<Vec<_>>();
    let elision = if trace.frames.len() > limit {
        format!(" (+{} more)", trace.frames.len() - limit)
    } else {
        String::new()
    };
    out.push_str(&format!(
        "  = trace ({}): {}{elision}\n",
        trace_kind_label(trace.kind),
        shown.join(" -> ")
    ));
}

/// Renders one suggested fix with its applicability.
///
/// The applicability is printed because it is the difference between an edit a
/// reader can apply blindly and one they must think about.
fn render_fix(out: &mut String, fix: &SuggestedFix) {
    out.push_str(&format!(
        "  = fix ({}): {}\n",
        applicability_label(fix.applicability),
        fix.title
    ));
    if let Some(explanation) = &fix.explanation {
        out.push_str(&format!("    {explanation}\n"));
    }
}

/// Selects the notes shown at one verbosity level.
///
/// The hidden notes are internal previews that duplicate typed debug fields;
/// everything explaining the failure stays visible at `Standard`.
fn visible_notes(notes: &[Box<str>], verbosity: ErrorVerbosity) -> Vec<&str> {
    let paired = has_paired_composition(notes);
    notes
        .iter()
        .filter(|note| {
            // `node_id=` and `box_expr=` are internal IR that duplicates typed
            // debug fields. `expr=` stays: it is the readable Faust-like
            // rendering, which is context a terminal reader wants.
            if !verbosity.shows_internals()
                && (note.starts_with("node_id=") || note.starts_with("box_expr="))
            {
                return false;
            }
            // The A/B block below already prints these.
            !(paired && (note.starts_with("A ") || note.starts_with("B ")))
        })
        .map(AsRef::as_ref)
        .collect()
}

/// Whether a propagate diagnostic wrote the paired `A`/`B` composition notes.
fn has_paired_composition(notes: &[Box<str>]) -> bool {
    notes.iter().any(|note| note.starts_with("A "))
        && notes.iter().any(|note| note.starts_with("B "))
}

/// Renders the condensed `Here A ... / while B ...` composition block.
fn render_paired_composition(out: &mut String, notes: &[Box<str>]) {
    if !has_paired_composition(notes) {
        return;
    }
    let find = |prefix: &str| -> Option<String> {
        notes
            .iter()
            .find(|note| note.starts_with(prefix))
            .and_then(|note| note.split_once(" = "))
            .map(|(_, expr)| expr.to_owned())
    };
    let arity = |prefix: &str| -> Option<String> {
        notes
            .iter()
            .find_map(|note| note.strip_prefix(prefix).map(str::to_owned))
    };
    let (Some(a), Some(b)) = (find("A "), find("B ")) else {
        return;
    };
    out.push_str(&format!("  = note: Here  A = {a}\n"));
    if let Some(a_arity) = arity("A arity: ") {
        out.push_str(&format!("  = note: has {a_arity}\n"));
    }
    out.push_str(&format!("  = note: while B = {b}\n"));
    if let Some(b_arity) = arity("B arity: ") {
        out.push_str(&format!("  = note: has {b_arity}\n"));
    }
}

/// Returns one source line from the immutable compilation snapshot.
///
/// Falls back to the filesystem only for bundles built before a source map was
/// attached; that path can show a line the compiler never saw if the file
/// changed since, which is exactly why the snapshot is preferred.
fn source_line(bundle: &DiagnosticBundle, path: &Path, line_number: u32) -> Option<String> {
    if let Some(source) = bundle.source_map().find_by_name(path) {
        return source.line_text(line_number).map(str::to_owned);
    }
    let source = std::fs::read_to_string(path).ok()?;
    let index = usize::try_from(line_number.checked_sub(1)?).ok()?;
    source.lines().nth(index).map(str::to_owned)
}

/// Spells one source path according to the selected style.
fn display_path(path: &Path, style: DiagnosticPathStyle) -> String {
    match style {
        DiagnosticPathStyle::Absolute => path.display().to_string(),
        DiagnosticPathStyle::Basename => path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        DiagnosticPathStyle::Relative => std::env::current_dir()
            .ok()
            .and_then(|cwd| path.strip_prefix(&cwd).ok().map(Path::to_path_buf))
            .map_or_else(
                || path.display().to_string(),
                |rel| rel.display().to_string(),
            ),
    }
}

const fn trace_kind_label(kind: diagnostics::TraceKind) -> &'static str {
    match kind {
        diagnostics::TraceKind::Binding => "binding",
        diagnostics::TraceKind::Import => "import",
        diagnostics::TraceKind::Expansion => "expansion",
        diagnostics::TraceKind::Evaluation => "evaluation",
        diagnostics::TraceKind::Transformation => "transformation",
        diagnostics::TraceKind::Causal => "causal",
    }
}

const fn applicability_label(applicability: Applicability) -> &'static str {
    match applicability {
        Applicability::MachineApplicable => "machine-applicable",
        Applicability::MaybeIncorrect => "maybe-incorrect",
        Applicability::HasPlaceholders => "has-placeholders",
        Applicability::Manual => "manual",
    }
}
