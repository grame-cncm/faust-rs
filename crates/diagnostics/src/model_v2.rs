//! Typed diagnostics-v2 payloads.
//!
//! These values carry machine information independently from human prose.
//! Renderers must serialize these fields directly and must never recover them
//! by parsing [`crate::Diagnostic::notes`] or [`crate::Diagnostic::help`].

use std::collections::BTreeMap;

use crate::{DiagnosticCode, Label, SourceRange};

/// Stable high-level ownership category for one diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticCategory {
    /// Invalid or inconsistent Faust source.
    UserCode,
    /// Valid Faust construct not implemented by the selected path/backend.
    UnsupportedFeature,
    /// Invalid compiler option or incompatible option combination.
    InvalidOptions,
    /// Missing file, import, tool, runtime symbol, or other environment input.
    Environment,
    /// Cooperative cancellation requested by the caller.
    Cancelled,
    /// Internal invariant failure that should be reported to compiler authors.
    CompilerBug,
}

/// Backend/pass-local stable subcode without changing the top-level `FRS-*`
/// registry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DetailCode(Box<str>);

impl DetailCode {
    /// Creates a stable detail code.
    #[must_use]
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    /// Returns the stable string spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable key for one machine-readable diagnostic fact.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FactKey(Box<str>);

impl FactKey {
    /// Creates a fact key.
    #[must_use]
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    /// Returns the key spelling used by JSON v2.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Typed value attached to a [`FactKey`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticValue {
    /// UTF-8 string.
    String(Box<str>),
    /// Signed integer.
    Integer(i64),
    /// Unsigned integer.
    Unsigned(u64),
    /// Finite/non-finite real rendered through a stable textual spelling.
    Real(Box<str>),
    /// Boolean.
    Boolean(bool),
    /// Ordered string list.
    StringList(Vec<Box<str>>),
    /// Inclusive integer interval.
    IntegerRange { min: i64, max: i64 },
    /// Nested deterministic object.
    Object(BTreeMap<FactKey, DiagnosticValue>),
}

impl From<&str> for DiagnosticValue {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<String> for DiagnosticValue {
    fn from(value: String) -> Self {
        Self::String(value.into_boxed_str())
    }
}

impl From<Box<str>> for DiagnosticValue {
    fn from(value: Box<str>) -> Self {
        Self::String(value)
    }
}

impl From<i64> for DiagnosticValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<u64> for DiagnosticValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for DiagnosticValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<Vec<String>> for DiagnosticValue {
    fn from(values: Vec<String>) -> Self {
        Self::StringList(values.into_iter().map(String::into_boxed_str).collect())
    }
}

impl From<Vec<Box<str>>> for DiagnosticValue {
    fn from(values: Vec<Box<str>>) -> Self {
        Self::StringList(values)
    }
}

/// Semantic role of a source label, independent from its display text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelRole {
    /// Location that directly caused the diagnostic.
    PrimaryCause,
    /// Symbol/expression use.
    UseSite,
    /// Symbol/rule definition.
    DefinitionSite,
    /// Function or abstraction call.
    CallSite,
    /// Composition or other operator.
    Operator,
    /// Location at which a construct/value was expected.
    ExpectedHere,
    /// Conflicting location.
    ConflictsWith,
    /// Import directive.
    ImportSite,
    /// Token before a parser recovery point.
    PreviousToken,
    /// Matching delimiter related to the primary delimiter.
    MatchingDelimiter,
    /// Location from which a derived IR value originated.
    DerivedFrom,
}

/// Kind of causal path represented by one trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceKind {
    /// Name binding/use resolution.
    Binding,
    /// Import chain.
    Import,
    /// Generated iteration/macro-style expansion.
    Expansion,
    /// Box evaluation/call chain.
    Evaluation,
    /// Cross-pass IR derivation.
    Transformation,
    /// General causal chain.
    Causal,
}

/// Stable reference to a compiler IR value in debug/tooling output.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IrReference {
    /// IR family such as `box`, `signal`, or `fir`.
    pub kind: Box<str>,
    /// Session-local numeric id.
    pub id: u64,
}

/// One frame in a typed diagnostic trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceFrame {
    /// Optional source-level or pass-level name.
    pub name: Option<Box<str>>,
    /// Optional canonical source range.
    pub span: Option<SourceRange>,
    /// Optional IR reference, normally emitted only in debug/full modes.
    pub ir: Option<IrReference>,
    /// Human-readable frame description.
    pub description: Box<str>,
}

/// Ordered causal trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticTrace {
    /// Trace family.
    pub kind: TraceKind,
    /// Root-to-leaf ordered frames.
    pub frames: Vec<TraceFrame>,
}

/// Confidence level for applying one suggested fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Applicability {
    /// Deterministic edit that preserves the compiler's intended repair.
    MachineApplicable,
    /// Concrete edit that may alter intended DSP semantics.
    MaybeIncorrect,
    /// Edit template containing user-selected placeholders.
    HasPlaceholders,
    /// Guidance that requires manual reasoning and has no exact edit.
    Manual,
}

/// One half-open source replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// Range to replace.
    pub range: SourceRange,
    /// Replacement UTF-8 text.
    pub replacement: Box<str>,
}

/// Structured source/configuration repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestedFix {
    /// Concise action title.
    pub title: Box<str>,
    /// Confidence/applicability classification.
    pub applicability: Applicability,
    /// Ordered, non-overlapping source edits.
    pub edits: Vec<TextEdit>,
    /// Optional explanation of semantic impact.
    pub explanation: Option<Box<str>>,
}

/// Non-recursive related diagnostic summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedDiagnostic {
    /// Stable top-level code.
    pub code: DiagnosticCode,
    /// Human-readable summary.
    pub message: Box<str>,
    /// Relevant labeled locations.
    pub labels: Vec<Label>,
}

/// Opt-in internal evidence excluded from standard output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DebugContext {
    /// Deterministically ordered internal fields.
    pub fields: BTreeMap<FactKey, DiagnosticValue>,
}

impl DebugContext {
    /// Creates an empty debug context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces one typed debug field.
    #[must_use]
    pub fn with_field(
        mut self,
        key: impl Into<Box<str>>,
        value: impl Into<DiagnosticValue>,
    ) -> Self {
        self.fields.insert(FactKey::new(key), value.into());
        self
    }

    /// Adds or replaces one typed debug field in place.
    pub fn insert(&mut self, key: impl Into<Box<str>>, value: impl Into<DiagnosticValue>) {
        self.fields.insert(FactKey::new(key), value.into());
    }
}
