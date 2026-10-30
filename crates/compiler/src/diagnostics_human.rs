//! Human-oriented diagnostic rendering, shared by the `faust-rs` binary and the
//! FFI layers ([`crate::CompilerError::rendered_diagnostics`]).
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
//!
//! # Color
//!
//! [`HumanRenderOptions::color`] adds ANSI styles and nothing else: every style
//! of a colorless [`Palette`] is empty, so both paths are the same code and
//! removing the escapes gives back the plain text byte for byte. The renderer
//! never looks at a terminal. The caller decides, which is why
//! [`crate::CompilerError::rendered_diagnostics`] stays plain for FFI hosts.

use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::Path;

use anstyle::{AnsiColor, Style};
use clap::ValueEnum;
use diagnostics::{
    Applicability, DiagnosticBundle, DiagnosticTrace, Label, LabelStyle, Severity, SuggestedFix,
};
use unicode_width::UnicodeWidthStr;

/// How source paths are spelled in rendered human diagnostics.
///
/// Only presentation: the JSON channel always reports the compiled source name
/// verbatim, because a tool resolving a range needs the path the compiler
/// actually used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum DiagnosticPathStyle {
    /// The path exactly as the compiler recorded it.
    #[default]
    Absolute,
    /// Relative to the working directory when that is shorter.
    ///
    /// Keeps CI logs and shared transcripts readable without hiding which file
    /// is meant.
    Relative,
    /// File name only.
    ///
    /// For sharing a diagnostic without disclosing directory structure.
    Basename,
}

/// Diagnostic verbosity level for CLI rendering.
///
/// The levels form a ladder of progressive disclosure: each one shows
/// everything the previous one did, plus more. `Standard` is the contract for
/// a terminal user — the complete actionable cause and nothing else — while
/// `Debug` and `Full` add compiler-internal evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ErrorVerbosity {
    /// Header, primary location, and the shortest safe fix.
    ///
    /// For callers that only route the reader to the failing line.
    Concise,
    /// Everything needed to act: all relevant labels, rule and computed facts,
    /// traces, and fixes.
    #[default]
    Standard,
    /// Standard plus internal ids and typed debug context.
    Debug,
    /// Debug plus untruncated traces and related diagnostics.
    Full,
}

impl ErrorVerbosity {
    /// Whether compiler-internal evidence is shown.
    #[must_use]
    pub fn shows_internals(self) -> bool {
        self >= Self::Debug
    }

    /// Whether traces and related diagnostics are shown untruncated.
    #[must_use]
    pub fn shows_everything(self) -> bool {
        self == Self::Full
    }
}

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
    /// Whether to emit ANSI styles. Resolved by the caller, which knows the
    /// stream the text goes to.
    pub color: bool,
}

impl Default for HumanRenderOptions {
    fn default() -> Self {
        Self {
            verbosity: ErrorVerbosity::Standard,
            path_style: DiagnosticPathStyle::Absolute,
            color: false,
        }
    }
}

/// The styles of one render, all empty when color is off.
///
/// Uses the 16 ANSI colors only, which each terminal theme remaps to something
/// readable on its own background. The gutter is the bright blue because plain
/// blue is close to invisible on many dark themes.
#[derive(Clone, Copy, Default)]
struct Palette {
    error: Style,
    warning: Style,
    remark: Style,
    strong: Style,
    gutter: Style,
    secondary: Style,
    note: Style,
    help: Style,
    fix: Style,
    trace: Style,
    internal: Style,
}

impl Palette {
    fn new(color: bool) -> Self {
        if !color {
            return Self::default();
        }
        let fg = |color: AnsiColor| Style::new().fg_color(Some(color.into()));
        Self {
            error: fg(AnsiColor::Red).bold(),
            warning: fg(AnsiColor::Yellow).bold(),
            remark: fg(AnsiColor::Cyan).bold(),
            strong: Style::new().bold(),
            gutter: fg(AnsiColor::BrightBlue),
            secondary: fg(AnsiColor::BrightBlue),
            note: fg(AnsiColor::Cyan),
            help: fg(AnsiColor::Green),
            fix: fg(AnsiColor::Green).bold(),
            trace: fg(AnsiColor::Magenta),
            internal: Style::new().dimmed(),
        }
    }

    /// The style of what is blamed, carets included.
    const fn severity(&self, severity: Severity) -> Style {
        match severity {
            Severity::Error => self.error,
            Severity::Warning => self.warning,
            Severity::Remark => self.remark,
        }
    }
}

/// Wraps `text` in `style`; an empty style writes no escape at all.
fn paint(style: Style, text: impl Display) -> String {
    format!("{style}{text}{style:#}")
}

/// The line-number column of one diagnostic's snippets.
///
/// Its width is that of the longest number the diagnostic shows, so that a
/// caret row lines up with its source row and rows of different labels line up
/// with each other. Numbers are right-aligned.
#[derive(Clone, Copy)]
struct Gutter {
    style: Style,
    width: usize,
}

impl Gutter {
    /// The column of a source row.
    fn numbered(self, line: u32) -> String {
        paint(self.style, format!("  {line:>0$} |", self.width))
    }

    /// The column of a caret or elision row.
    fn blank(self) -> String {
        paint(self.style, format!("  {:1$} |", "", self.width))
    }
}

/// Writes one `  = tag: body` line, with only the tag styled.
fn push_tagged(out: &mut String, style: Style, tag: &str, body: impl Display) {
    out.push_str(&format!("  {} {body}\n", paint(style, format!("= {tag}:"))));
}

/// Formats one bundle for a terminal reader.
pub fn format_bundle(bundle: &DiagnosticBundle, options: HumanRenderOptions) -> String {
    let palette = Palette::new(options.color);
    let mut out = String::new();
    for diagnostic in bundle.as_slice() {
        render_header(&mut out, diagnostic, options, &palette);
        render_labels(&mut out, bundle, diagnostic, options, &palette);

        if options.verbosity == ErrorVerbosity::Concise {
            if let Some(help) = diagnostic.help.first() {
                push_tagged(&mut out, palette.help, "help", help);
            }
            continue;
        }

        render_paired_composition(&mut out, &diagnostic.notes, &palette);
        for note in visible_notes(&diagnostic.notes, options.verbosity) {
            push_tagged(&mut out, palette.note, "note", note);
        }
        for trace in &diagnostic.traces {
            render_trace(&mut out, trace, options.verbosity, &palette);
        }
        for fix in &diagnostic.fixes {
            render_fix(&mut out, fix, &palette);
        }
        for help in &diagnostic.help {
            push_tagged(&mut out, palette.help, "help", help);
        }
        if options.verbosity.shows_internals()
            && let Some(debug) = &diagnostic.debug
        {
            for (key, value) in &debug.fields {
                let body = format!("{}={value:?}", key.as_str());
                push_tagged(&mut out, palette.internal, "debug", body);
            }
        }
        if options.verbosity.shows_everything() {
            for related in &diagnostic.related {
                let body = format!("[{}] {}", related.code.0, related.message);
                push_tagged(&mut out, palette.internal, "related", body);
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
    palette: &Palette,
) {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Remark => "remark",
    };
    let tag = paint(
        palette.severity(diagnostic.severity),
        format!("{severity} [{}]", diagnostic.code.0),
    );
    let message = paint(palette.strong, &diagnostic.message);
    match primary_label(diagnostic) {
        Some(label) => {
            let location = format!(
                "{}:{}:{}:",
                display_path(&label.span.file, options.path_style),
                label.span.line,
                label.span.col
            );
            let location = paint(palette.strong, location);
            out.push_str(&format!("{location} {tag} {message}\n"));
        }
        None => out.push_str(&format!("{tag} {message}\n")),
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
    palette: &Palette,
) {
    let mut shown = BTreeSet::new();
    let mut current_file: Option<&Path> = primary_label(diagnostic).map(|l| l.span.file.as_path());
    let labels: Vec<&Label> = if options.verbosity == ErrorVerbosity::Concise {
        primary_label(diagnostic).into_iter().collect()
    } else {
        ordered_labels(diagnostic)
    };

    let width = labels
        .iter()
        .map(|label| label.span.line.max(label.span.end_line))
        .max()
        .map_or(1, |line| line.to_string().len());
    let gutter = Gutter {
        style: palette.gutter,
        width,
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
                "  {} {}\n",
                paint(palette.gutter, "-->"),
                paint(
                    palette.strong,
                    display_path(&label.span.file, options.path_style)
                )
            ));
            current_file = Some(label.span.file.as_path());
            current_line = None;
        }
        let single_line = label.span.end_line <= label.span.line;
        let repeats_line =
            single_line && current_line == Some((label.span.file.as_path(), label.span.line));
        let label_style = match label.style {
            LabelStyle::Primary => palette.severity(diagnostic.severity),
            LabelStyle::Secondary => palette.secondary,
        };
        render_snippet(out, bundle, label, repeats_line, gutter, label_style);
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
    gutter: Gutter,
    label_style: Style,
) {
    let Some(first) = source_line(bundle, label.span.file.as_path(), label.span.line) else {
        return;
    };
    let expanded = expand_tabs(&first);
    if !line_already_shown {
        let number = gutter.numbered(label.span.line);
        out.push_str(&format!("{number} {expanded}\n"));
    }
    let blank = gutter.blank();

    if label.span.end_line > label.span.line {
        out.push_str(&format!("{blank} ...\n"));
        if let Some(last) = source_line(bundle, label.span.file.as_path(), label.span.end_line) {
            let number = gutter.numbered(label.span.end_line);
            out.push_str(&format!("{number} {}\n", expand_tabs(&last)));
        }
        out.push_str(&format!("{blank} {}\n", paint(label_style, &label.message)));
        return;
    }

    let carets = caret_run(&first, label.span.col, label.span.end_col);
    let annotation = paint(label_style, format!("{carets} {}", label.message));
    out.push_str(&format!("{blank} {annotation}\n"));
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
fn render_trace(
    out: &mut String,
    trace: &DiagnosticTrace,
    verbosity: ErrorVerbosity,
    palette: &Palette,
) {
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
    push_tagged(
        out,
        palette.trace,
        &format!("trace ({})", trace_kind_label(trace.kind)),
        format!("{}{elision}", shown.join(" -> ")),
    );
}

/// Renders one suggested fix with its applicability.
///
/// The applicability is printed because it is the difference between an edit a
/// reader can apply blindly and one they must think about.
fn render_fix(out: &mut String, fix: &SuggestedFix, palette: &Palette) {
    push_tagged(
        out,
        palette.fix,
        &format!("fix ({})", applicability_label(fix.applicability)),
        &fix.title,
    );
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
fn render_paired_composition(out: &mut String, notes: &[Box<str>], palette: &Palette) {
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
    push_tagged(out, palette.note, "note", format!("Here  A = {a}"));
    if let Some(a_arity) = arity("A arity: ") {
        push_tagged(out, palette.note, "note", format!("has {a_arity}"));
    }
    push_tagged(out, palette.note, "note", format!("while B = {b}"));
    if let Some(b_arity) = arity("B arity: ") {
        push_tagged(out, palette.note, "note", format!("has {b_arity}"));
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
