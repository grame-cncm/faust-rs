#![forbid(unsafe_code)]

//! Structured diagnostics report model for `faust-rs`.
//!
//! # Source provenance (C++)
//! - `compiler/errors/*` (error classes and reporting helpers)
//! - parser/eval/propagate diagnostics conventions in pass-specific code
//!
//! # Role in pipeline
//! - Define a shared, typed diagnostic envelope used by all compiler stages.
//! - Keep stable diagnostic codes (`codes::*`) suitable for tests, CI gates and
//!   tooling integrations.
//! - Offer stage/severity/source-span metadata independent from output format.
//! - Leave operational errors and recovery decisions in their owning crates.
//!
//! # Design invariants
//! - Diagnostic codes are stable identifiers: textual wording can evolve without
//!   breaking CI/tool consumers.
//! - Stage attribution is explicit (`Stage` enum) so failures can be bucketed
//!   per pipeline step.
//! - Rendering policy is caller-owned: this crate models data, not UI.
//! - [`Diagnostic`] and [`DiagnosticBundle`] are report data, not
//!   [`std::error::Error`] implementations: they may contain warnings, remarks,
//!   or several messages.
//!
//! # API mapping status
//! - Public API is `adapted`: equivalent diagnostic intent to C++ with Rust
//!   ownership/typing and structured aggregation via [`DiagnosticBundle`].

use std::collections::BTreeMap;
use std::path::PathBuf;

pub mod codes;
mod model_v2;
mod source;

pub use codes::all_codes;
pub use model_v2::{
    Applicability, DebugContext, DetailCode, DiagnosticCategory, DiagnosticTrace, DiagnosticValue,
    FactKey, IrReference, LabelRole, RelatedDiagnostic, SuggestedFix, TextEdit, TraceFrame,
    TraceKind,
};
pub use source::{
    ContentHash, HumanPosition, LspPosition, SourceCoordinateError, SourceFile, SourceId,
    SourceKind, SourceMap, SourceMapBuilder, SourceRange,
};

/// Diagnostic severity level.
///
/// Severity is intentionally orthogonal to stage/code so callers can sort or
/// filter diagnostics independently from their origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    /// A blocking problem that prevents successful compilation.
    Error,
    /// A non-blocking problem that should be shown to the user.
    Warning,
    /// An informational remark attached to successful or recoverable flows.
    Remark,
}

/// Compiler stage producing one diagnostic.
///
/// This stage taxonomy is stable enough for CI reports and user-facing grouped
/// rendering, even if exact wording changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Source loading and import resolution.
    SourceReader,
    /// Lexical analysis.
    Lexer,
    /// Grammar parsing and parse recovery.
    Parser,
    /// Box-level semantic evaluation.
    Eval,
    /// Box-to-signal propagation and structural checks.
    Propagate,
    /// Signal normalization passes.
    Normalize,
    /// Signal type and interval inference.
    TypeInference,
    /// Mid-level transform passes.
    Transform,
    /// FIR lowering and FIR-level checks.
    Fir,
    /// Backend code generation.
    Codegen,
    /// Top-level compiler orchestration.
    Compiler,
}

/// Stable diagnostic code identifier used across crates and CI tooling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiagnosticCode(pub &'static str);

/// File-local compatibility source span.
///
/// Lines and columns are 1-based. Existing producers treat `end_col` as the
/// half-open caret boundary. New diagnostics should use canonical
/// [`SourceRange`] values and convert through [`SourceMap::to_source_span`]
/// only at legacy producer or renderer boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    /// File where this span originates.
    pub file: PathBuf,
    /// 1-based start line.
    pub line: u32,
    /// 1-based start column.
    pub col: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// 1-based end column.
    pub end_col: u32,
}

impl SourceSpan {
    /// Creates a source span.
    #[must_use]
    pub fn new(file: impl Into<PathBuf>, line: u32, col: u32, end_line: u32, end_col: u32) -> Self {
        Self {
            file: file.into(),
            line,
            col,
            end_line,
            end_col,
        }
    }
}

/// Source label style.
///
/// Labels distinguish the main blame location from related context locations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    /// Main location that should be highlighted first.
    Primary,
    /// Related location that provides extra context.
    Secondary,
}

/// One labeled source span attached to a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Label {
    /// Visual role of the label in rendered diagnostics.
    pub style: LabelStyle,
    /// Machine-readable semantic role, independent from [`Self::message`].
    pub role: LabelRole,
    /// Source location attached to this label.
    pub span: SourceSpan,
    /// User-facing label text.
    pub message: Box<str>,
}

impl Label {
    /// Creates a label.
    #[must_use]
    pub fn new(style: LabelStyle, span: SourceSpan, message: impl Into<Box<str>>) -> Self {
        let role = match style {
            LabelStyle::Primary => LabelRole::PrimaryCause,
            LabelStyle::Secondary => LabelRole::DerivedFrom,
        };
        Self {
            style,
            role,
            span,
            message: message.into(),
        }
    }

    /// Sets the typed semantic role and returns the updated label.
    #[must_use]
    pub fn with_role(mut self, role: LabelRole) -> Self {
        self.role = role;
        self
    }
}

/// Structured diagnostic payload shared across compiler stages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity level of the diagnostic.
    pub severity: Severity,
    /// Compiler stage that emitted this diagnostic.
    pub stage: Stage,
    /// Stable machine-readable diagnostic code.
    pub code: DiagnosticCode,
    /// Backend/pass-local detail code.
    pub detail_code: Option<DetailCode>,
    /// High-level ownership category.
    pub category: DiagnosticCategory,
    /// Main human-readable message.
    pub message: Box<str>,
    /// Source labels attached to this diagnostic.
    pub labels: Vec<Label>,
    /// Additional explanatory notes.
    pub notes: Vec<Box<str>>,
    /// Suggested actionable fixes.
    pub help: Vec<Box<str>>,
    /// Deterministically ordered machine-readable facts.
    pub facts: BTreeMap<FactKey, DiagnosticValue>,
    /// Typed causal traces.
    pub traces: Vec<DiagnosticTrace>,
    /// Applicability-graded fixes.
    pub fixes: Vec<SuggestedFix>,
    /// Related, non-recursive diagnostics.
    pub related: Vec<RelatedDiagnostic>,
    /// Opt-in internal evidence.
    pub debug: Option<DebugContext>,
}

impl Diagnostic {
    /// Creates a diagnostic with empty labels/notes/help.
    #[must_use]
    pub fn new(
        severity: Severity,
        stage: Stage,
        code: DiagnosticCode,
        message: impl Into<Box<str>>,
    ) -> Self {
        Self {
            severity,
            stage,
            code,
            detail_code: None,
            category: default_category(stage),
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
            facts: BTreeMap::new(),
            traces: Vec::new(),
            fixes: Vec::new(),
            related: Vec::new(),
            debug: None,
        }
    }

    /// Adds one source label and returns the updated diagnostic.
    #[must_use]
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Adds one note and returns the updated diagnostic.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<Box<str>>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Adds one help entry and returns the updated diagnostic.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<Box<str>>) -> Self {
        self.help.push(help.into());
        self
    }

    /// Sets the high-level diagnostic category.
    #[must_use]
    pub fn with_category(mut self, category: DiagnosticCategory) -> Self {
        self.category = category;
        self
    }

    /// Sets a backend/pass-local stable detail code.
    #[must_use]
    pub fn with_detail_code(mut self, detail_code: impl Into<Box<str>>) -> Self {
        self.detail_code = Some(DetailCode::new(detail_code));
        self
    }

    /// Adds or replaces one typed fact.
    #[must_use]
    pub fn with_fact(
        mut self,
        key: impl Into<Box<str>>,
        value: impl Into<DiagnosticValue>,
    ) -> Self {
        self.facts.insert(FactKey::new(key), value.into());
        self
    }

    /// Adds one typed causal trace.
    #[must_use]
    pub fn with_trace(mut self, trace: DiagnosticTrace) -> Self {
        self.traces.push(trace);
        self
    }

    /// Adds one structured suggested fix.
    #[must_use]
    pub fn with_fix(mut self, fix: SuggestedFix) -> Self {
        self.fixes.push(fix);
        self
    }

    /// Adds one related diagnostic summary.
    #[must_use]
    pub fn with_related(mut self, related: RelatedDiagnostic) -> Self {
        self.related.push(related);
        self
    }

    /// Sets opt-in internal/debug evidence.
    #[must_use]
    pub fn with_debug_context(mut self, debug: DebugContext) -> Self {
        self.debug = Some(debug);
        self
    }

    /// Adds or replaces one typed debug field.
    #[must_use]
    pub fn with_debug_fact(
        mut self,
        key: impl Into<Box<str>>,
        value: impl Into<DiagnosticValue>,
    ) -> Self {
        self.debug
            .get_or_insert_with(DebugContext::new)
            .insert(key, value);
        self
    }
}

const fn default_category(stage: Stage) -> DiagnosticCategory {
    match stage {
        Stage::SourceReader => DiagnosticCategory::Environment,
        Stage::Compiler => DiagnosticCategory::CompilerBug,
        Stage::Codegen | Stage::Fir | Stage::Transform => DiagnosticCategory::UnsupportedFeature,
        Stage::Lexer
        | Stage::Parser
        | Stage::Eval
        | Stage::Propagate
        | Stage::Normalize
        | Stage::TypeInference => DiagnosticCategory::UserCode,
    }
}

/// Aggregated diagnostics for one stage/session outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticBundle {
    diagnostics: Vec<Diagnostic>,
    source_map: SourceMap,
}

impl DiagnosticBundle {
    /// Creates an empty bundle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one diagnostic, normalizing its note order.
    pub fn push(&mut self, mut diagnostic: Diagnostic) {
        normalize_note_order(&mut diagnostic);
        self.diagnostics.push(diagnostic);
    }

    /// Associates the immutable source snapshots compiled in this session.
    ///
    /// Human renderers use it to avoid re-reading a file that changed after
    /// compilation, and the JSON renderer emits its stable source metadata.
    pub fn set_source_map(&mut self, source_map: SourceMap) {
        self.source_map = source_map;
    }

    /// Returns the immutable source snapshots for this compilation.
    #[must_use]
    pub fn source_map(&self) -> &SourceMap {
        &self.source_map
    }

    /// Extends this bundle with another sequence of diagnostics.
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        for diagnostic in diagnostics {
            self.push(diagnostic);
        }
    }

    /// Returns all diagnostics as a read-only slice.
    #[must_use]
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Number of diagnostics stored in this bundle.
    #[must_use]
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns `true` when no diagnostics are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Counts diagnostics with [`Severity::Error`].
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }
}

impl From<Vec<Diagnostic>> for DiagnosticBundle {
    fn from(diagnostics: Vec<Diagnostic>) -> Self {
        let mut bundle = Self::new();
        bundle.extend(diagnostics);
        bundle
    }
}

/// Canonical explanation order shared by every stage.
///
/// Producers add notes in whatever order is convenient while building a
/// diagnostic, which made two stages explaining the same kind of failure read
/// differently. Ordering once, at the point a diagnostic enters a bundle, gives
/// every consumer the same shape: what went wrong, which rule says so, what the
/// compiler computed, then supporting context.
///
/// The sort is stable, so notes that share a rank keep the order their producer
/// chose — the ranks impose a skeleton, not a total order.
fn normalize_note_order(diagnostic: &mut Diagnostic) {
    diagnostic
        .notes
        .sort_by_key(|note| note_rank(note.as_ref()));
}

/// Rank of one note in the canonical explanation order.
fn note_rank(note: &str) -> u8 {
    if note.starts_with("cause:") {
        0
    } else if note.starts_with("rule:") {
        1
    } else if note.starts_with("computed:") {
        2
    } else if note.starts_with("suggested target:") {
        3
    } else {
        // Context: scopes, binding traces, owning definitions, previews, and
        // anything a stage adds that the skeleton does not name.
        4
    }
}

/// Borrowing conversion contract for phase-local errors to diagnostics.
pub trait ToDiagnostic {
    /// Builds a structured [`Diagnostic`] without consuming the phase-local error.
    fn to_diagnostic(&self) -> Diagnostic;
}

#[cfg(test)]
mod tests {
    use super::{
        Diagnostic, DiagnosticBundle, DiagnosticCategory, DiagnosticCode, DiagnosticValue, Label,
        LabelRole, LabelStyle, Severity, SourceSpan, Stage,
    };

    #[test]
    fn diagnostic_builder_keeps_fields_and_payloads() {
        let span = SourceSpan::new("foo.dsp", 3, 5, 3, 9);
        let diag = Diagnostic::new(
            Severity::Error,
            Stage::Parser,
            DiagnosticCode("FRS-PARSE-0001"),
            "unexpected token",
        )
        .with_label(Label::new(LabelStyle::Primary, span.clone(), "here"))
        .with_note("while parsing process definition")
        .with_help("check missing ';'");

        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.stage, Stage::Parser);
        assert_eq!(diag.code, DiagnosticCode("FRS-PARSE-0001"));
        assert_eq!(diag.message.as_ref(), "unexpected token");
        assert_eq!(diag.labels.len(), 1);
        assert_eq!(diag.labels[0].span, span);
        assert_eq!(diag.labels[0].role, LabelRole::PrimaryCause);
        assert_eq!(diag.notes.len(), 1);
        assert_eq!(diag.help.len(), 1);
    }

    #[test]
    fn v2_builder_keeps_typed_category_detail_and_facts() {
        let diag = Diagnostic::new(
            Severity::Error,
            Stage::Codegen,
            DiagnosticCode("FRS-CODEGEN-0001"),
            "backend rejected an instruction",
        )
        .with_category(DiagnosticCategory::UnsupportedFeature)
        .with_detail_code("FRS-CGEN-WASM-0007")
        .with_fact("backend", "wasm")
        .with_fact("actual", 3_i64);

        assert_eq!(diag.category, DiagnosticCategory::UnsupportedFeature);
        assert_eq!(
            diag.detail_code.as_ref().map(|code| code.as_str()),
            Some("FRS-CGEN-WASM-0007")
        );
        assert_eq!(
            diag.facts.get(&super::FactKey::new("backend")),
            Some(&DiagnosticValue::String("wasm".into()))
        );
        assert_eq!(
            diag.facts.get(&super::FactKey::new("actual")),
            Some(&DiagnosticValue::Integer(3))
        );
    }

    #[test]
    fn bundle_counts_error_severity_only() {
        let mut bundle = DiagnosticBundle::new();
        // Use a real registered code: a made-up literal here would be picked
        // up by the textual extraction behind
        // `compiler::cli::tests::frozen_frs_code_table_matches_source` and
        // would look like a public diagnostic code that nothing ever emits.
        bundle.push(Diagnostic::new(
            Severity::Warning,
            Stage::Eval,
            crate::codes::EVAL_GENERIC_FAILURE,
            "non-fatal warning",
        ));
        bundle.push(Diagnostic::new(
            Severity::Error,
            Stage::Eval,
            DiagnosticCode("FRS-EVAL-0001"),
            "undefined symbol",
        ));

        assert_eq!(bundle.len(), 2);
        assert_eq!(bundle.error_count(), 1);
        assert!(!bundle.is_empty());
    }
}
