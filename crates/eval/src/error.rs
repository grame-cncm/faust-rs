//! Evaluator error types and diagnostic conversions.

use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use diagnostics::codes;
use diagnostics::{Diagnostic, DiagnosticBundle, Severity, Stage, ToDiagnostic};
use tlib::TreeId;

use crate::suggestions::{SymbolSuggestion, rank_similar_names};

/// Performance statistics collected during evaluation.
///
/// Returned by [`eval_process_with_stats`](crate::eval_process_with_stats) alongside the evaluated box tree.
/// Provides the same information as the C++ `gGlobal->gStats` fields used for profiling
/// the evaluator, but without global mutable state — stats are accumulated locally and
/// returned by value.
///
/// # C++ correspondence
///
/// | Rust field | C++ equivalent | C++ location |
/// |---|---|---|
/// | `env_layers_pushed` | `gStats.fEnvLayersPushed` | `environment.cpp` — `pushNewLayer` |
/// | `env_lookups` | `gStats.fEnvLookups` | `environment.cpp` — `searchIdDef` |
/// | `env_lookup_total_depth` | `gStats.fEnvLookupTotalDepth` | `environment.cpp` — `searchIdDef` loop |
/// | `loop_detector_max_depth` | (no direct equivalent — C++ uses `gGlobal->gRecursionLimit`) | |
/// | `nodes_evaluated` | (not tracked in C++) | |
///
/// # Interpretation
///
/// These ratios describe the intended interpretation once all counters are wired:
///
/// - **`env_lookups / nodes_evaluated`**: average lookups per evaluated node. High values (> 3)
///   indicate deeply bound symbols that might benefit from flattening or interning.
/// - **`env_lookup_total_depth / env_lookups`**: average scope depth traversed per lookup.
///   Values > 3 indicate deep scope chains where caching may help.
/// - **`env_layers_pushed / nodes_evaluated`**: scope-push frequency. High values for iterative
///   forms (`ipar`/`iseq`) are expected.
///
/// As of the current port, instrumentation is still incremental: the field meanings are stable,
/// but not every evaluator path updates every counter yet. Consumers should therefore treat these
/// values as progressively improving telemetry, not as a fully complete profiling contract.
/// A non-fatal finding of the evaluator.
///
/// The compiler reports these under its semantic-warnings option, the class
/// the reference compiler prints under `-wall`; they never change the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalWarning {
    /// A widget modulation whose target matches no widget of its body. The
    /// body is left as it is, with a dangling extra input when the modulator
    /// has two inputs, as the reference compiler does.
    ///
    /// C++ equivalent: `WARNING : no modulation of: '...' took place in: ...`
    /// pushed on `gWarningMessages` by the modulation branch of `eval.cpp`.
    ModulationNoMatch {
        /// The modulation node.
        node: TreeId,
        /// The evaluated target, as the program wrote it (interpolated).
        target: String,
    },
}

#[derive(Clone, Debug, Default)]
/// Lightweight evaluator statistics returned by opt-in entry points.
pub struct EvalStats {
    /// Non-fatal findings collected during the evaluation, in order.
    pub warnings: Vec<EvalWarning>,
    /// Number of child scopes created via `push_scope()`.
    /// C++ equivalent: `gStats.fEnvLayersPushed`.
    pub env_layers_pushed: u64,
    /// Number of symbol lookups performed across all scopes.
    /// C++ equivalent: `gStats.fEnvLookups`.
    pub env_lookups: u64,
    /// Total scope depth traversed across all lookups (sum of per-lookup depths).
    /// Dividing by `env_lookups` gives the average lookup depth.
    /// C++ equivalent: `gStats.fEnvLookupTotalDepth`.
    pub env_lookup_total_depth: u64,
    /// Maximum loop-detector stack depth reached during evaluation.
    pub loop_detector_max_depth: usize,
    /// Total number of box nodes visited by `eval_box`.
    pub nodes_evaluated: u64,
    /// Definition-name map: evaluated `BoxId` → source definition name.
    ///
    /// Populated during eval whenever a named closure is forced to a concrete box.
    /// Used by the SVG draw module to label / fold named sub-diagrams.
    ///
    /// C++ equivalent: the property set by `setDefNameProperty(result, id)` in
    /// `compiler/evaluate/eval.cpp`.
    pub def_names: std::collections::HashMap<tlib::TreeId, String>,
}

/// Evaluator error.
///
/// Each variant corresponds to a distinct failure mode of the evaluation phase. All variants
/// carry enough context to produce rich diagnostics via [`ToDiagnostic`].
///
/// # C++ correspondence
///
/// C++ errors are thrown as `faustexception` with a formatted string message and global
/// `gGlobal->gErrorCount` increment. The Rust model uses typed `Result<_, EvalError>` returns
/// with structured context, enabling richer diagnostics without global state.
///
/// | Rust variant | C++ trigger |
/// |---|---|
/// | `MissingProcessDefinition` | `evalerror("... process is not defined")` in `eval.cpp` |
/// | `UndefinedSymbol` | `evalerror("... unknown id")` in `eval.cpp` |
/// | `RedefinedSymbol` | `throw faustexception("redefinition of symbols …")` in `environment.cpp` |
/// | `LoopDetected` | `faustassert` in C++ loop detector (aborts rather than throws) |
/// | `RecursionDepthExceeded` | `stackOverflowDetector::detect()` in C++ loop detector |
#[derive(Debug, Clone, PartialEq, Eq)]
/// Typed evaluator failure surface.
pub enum EvalError {
    MissingProcessDefinition {
        /// Requested top-level DSP entry-point name.
        entrypoint: String,
        /// Parser root definitions list used for fallback source-label resolution.
        definitions: TreeId,
        /// Deterministic list of top-level definition names available in this program.
        available_defs: Vec<String>,
    },
    UndefinedSymbol {
        symbol: String,
        /// Identifier node where resolution failed.
        node: TreeId,
        /// Names bound in the immediate lexical scope.
        local_scope: Vec<String>,
        /// Names visible across lexical parents.
        visible_scope: Vec<String>,
        /// Names bound at top-level.
        top_level_scope: Vec<String>,
    },
    MalformedDefinitionNode {
        node: TreeId,
    },
    MalformedListNode {
        node: TreeId,
    },
    MalformedCaseNode {
        node: TreeId,
    },
    EmptyArgumentList {
        /// Argument-list node that was expected to contain at least one item.
        node: TreeId,
    },
    NonIdentifierParameter {
        node: TreeId,
    },
    NonIdentifierIterationVariable {
        node: TreeId,
    },
    IterationCountNotInt {
        node: TreeId,
    },
    IterationCountTooLarge {
        value: i64,
    },
    NegativeIterationCount {
        value: i64,
    },
    PatternArityMismatch {
        /// Case-rules root node used to evaluate matching.
        node: TreeId,
        expected: usize,
        got: usize,
    },
    PatternMatchFailed {
        /// Case-rules root node where no rule matched provided arguments.
        node: TreeId,
        /// Arguments consumed by the matcher, in application order.
        ///
        /// These are the already-simplified argument boxes the matcher actually
        /// dispatched on. They let the compiler facade render a rule/attempt
        /// trace without exposing evaluator environments or automaton state.
        arguments: Vec<TreeId>,
    },
    /// Non-closure application received more arguments than the function input arity.
    TooManyArguments {
        /// Function-like node receiving too many arguments.
        node: TreeId,
        expected: usize,
        got: usize,
    },
    InvalidModulationLabel {
        node: TreeId,
    },
    InvalidLabelInterpolation {
        node: TreeId,
        ident: String,
        reason: &'static str,
    },
    InvalidModulationCircuit {
        node: TreeId,
        reason: &'static str,
    },
    InvalidSourceReference {
        node: TreeId,
        construct: &'static str,
    },
    /// `cinputs`, `cinput`, `coutputs` or `coutput` applied to an expression
    /// that does not evaluate to a closed block diagram. faust-rs extension.
    InvalidControlListOperand {
        node: TreeId,
        primitive: &'static str,
    },
    /// `cinput(i, e)` or `coutput(i, e)` with `i` at or past the count.
    /// faust-rs extension.
    ControlIndexOutOfRange {
        node: TreeId,
        primitive: &'static str,
        index: usize,
        count: usize,
    },
    /// A wildcard modulation target (`"*"`, `"group/*"`) that matches no
    /// control input of its body. faust-rs extension: a literal target that
    /// matches nothing is only the warning [`EvalWarning::ModulationNoMatch`].
    ModulationWildcardNoMatch {
        node: TreeId,
        target: String,
    },
    SourceFileNotFound {
        node: TreeId,
        construct: &'static str,
        target: String,
        current_file: Option<PathBuf>,
        search_paths: Vec<PathBuf>,
    },
    SourceReaderFailure {
        node: TreeId,
        construct: &'static str,
        target: String,
        message: String,
    },
    SourceParseFailure {
        node: TreeId,
        construct: &'static str,
        path: PathBuf,
        /// Original parser diagnostics, including imported source snapshots.
        diagnostics: DiagnosticBundle,
    },
    ExpectedClosureValue {
        node: TreeId,
        context: &'static str,
    },
    /// A symbol is redefined with a **different** value within the same lexical scope layer.
    ///
    /// Identical redefinitions (same `first_def == second_def` by `TreeId` identity) are
    /// silently ignored, matching C++ `addLayerDef` behavior:
    /// ```cpp
    /// if (def == olddef) { /* silent — hash-consed equality */ }
    /// else { throw faustexception("redefinition of symbols are not allowed: …"); }
    /// ```
    ///
    /// This check is performed only within the **current scope layer** (`lookup_local`), so
    /// shadowing a name from an outer scope is allowed and does not trigger this error.
    RedefinedSymbol {
        /// The symbol name that was defined more than once.
        symbol: String,
        /// The `TreeId` of the first (original) definition.
        first_def: TreeId,
        /// The `TreeId` of the conflicting second definition.
        second_def: TreeId,
    },
    LoopDetected {
        node: TreeId,
    },
    RecursionDepthExceeded {
        max_depth: usize,
    },
    /// A box expression was expected to evaluate to a compile-time numeric
    /// constant (type 0→1 with a numeric value), but did not.
    ///
    /// Occurs in slider parameter evaluation, table-size expressions, and
    /// similar contexts where the C++ compiler calls `eval2int` / `eval2double`.
    ///
    /// C++ equivalent: `evalerror("not a constant expression of type: (0->1)", …)`
    /// thrown by `eval2double` / `eval2int` in `eval.cpp`.
    NotAConstantExpression {
        node: TreeId,
    },
    /// Slider or numentry init value is outside the declared [min, max] range.
    ///
    /// C++ equivalent: the `checkRange` diagnostic in `eval.cpp`:
    /// `"init = ... outside of [min max] range in '...'"`.
    ///
    /// Values are stored as `u64` bit-patterns (via `f64::to_bits`) to keep the
    /// enum `Eq`-derivable; use `f64::from_bits` to recover them.
    SliderInitOutOfRange {
        /// Widget kind: `"hslider"`, `"vslider"`, or `"nentry"`.
        kind: &'static str,
        /// Evaluated label string (e.g. `"gain"`).
        label: String,
        init_bits: u64,
        min_bits: u64,
        max_bits: u64,
    },
    /// A slider, nentry or bargraph parameter that does not evaluate to a
    /// number. `arity` is the arity of the evaluated parameter box: `(0, 1)`
    /// for a signal known only at run time (C++ `tree2double`: "the parameter
    /// must be a real constant numerical expression"), anything else for a box
    /// of the wrong type (C++ `eval2double`: "not a constant expression of
    /// type : (0->1)"), `None` when the arity is unknown.
    WidgetParameterNotConstant {
        node: TreeId,
        widget: &'static str,
        label: String,
        parameter: &'static str,
        expression: String,
        arity: Option<(usize, usize)>,
    },
    /// A constant expression divides by a constant zero: `2.0 / 0`,
    /// `1 / (2 - 2)`, `par(i, 2, 1.0 / i)` at `i = 0`.
    ///
    /// Raised where the evaluator folds a numeric sequence, the point at which
    /// C++ `eval.cpp` lets the `faustexception` of `mterm::operator/=` through
    /// (`ERROR : division by 0 in 2 / 0`). It is an error whatever the width:
    /// the reference does not fold `2.0 / 0` to an infinity.
    DivisionByZero {
        /// The sequence being folded, which carries the source location.
        node: TreeId,
        /// The division as the normalizer saw it, e.g. `2 / 0`.
        detail: String,
    },
    /// Internal evaluator error — indicates a bug in the evaluator, not a user error.
    InternalError {
        message: String,
    },
    /// Cooperative cancellation: the external cancel flag was set (e.g., timeout).
    Cancelled,
}

impl EvalError {
    /// Ranked near-name candidates for an unresolved identifier.
    ///
    /// Candidates are drawn only from the scopes this error already recorded,
    /// so a suggestion can never name a symbol the programmer cannot reach from
    /// the failing site. Returns an empty vector for every other variant.
    ///
    /// The compiler facade uses this to decide whether a rename edit is safe to
    /// propose; see [`crate::suggestions::unambiguous_suggestion`].
    #[must_use]
    pub fn symbol_suggestions(&self) -> Vec<SymbolSuggestion> {
        match self {
            Self::UndefinedSymbol {
                symbol,
                local_scope,
                visible_scope,
                top_level_scope,
                ..
            } => rank_similar_names(
                symbol,
                local_scope
                    .iter()
                    .chain(visible_scope)
                    .chain(top_level_scope)
                    .map(String::as_str),
            ),
            Self::MissingProcessDefinition {
                entrypoint,
                available_defs,
                ..
            } => rank_similar_names(entrypoint, available_defs.iter().map(String::as_str)),
            _ => Vec::new(),
        }
    }
}

/// Attaches ranked near-name guidance without ever widening the visible scope.
///
/// Suggestions become a typed `suggested_symbols` fact plus one note. No fix is
/// created here: an exact rename edit needs the use-site source range, which
/// only the compiler facade owns.
fn with_symbol_suggestions(diagnostic: Diagnostic, suggestions: &[SymbolSuggestion]) -> Diagnostic {
    if suggestions.is_empty() {
        return diagnostic;
    }
    let names = suggestions
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    diagnostic
        .with_note(format!("did you mean: {}?", names.join(", ")))
        .with_fact("suggested_symbols", names)
        .with_fact(
            "suggestion_distance",
            u64::try_from(suggestions[0].distance).unwrap_or(u64::MAX),
        )
}

impl Display for EvalError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingProcessDefinition { entrypoint, .. } => {
                write!(f, "missing `{entrypoint}` definition")
            }
            Self::UndefinedSymbol { symbol, .. } => write!(f, "undefined symbol `{symbol}`"),
            Self::MalformedDefinitionNode { node } => {
                write!(f, "malformed definition node {}", node.as_u32())
            }
            Self::MalformedListNode { node } => {
                write!(f, "malformed list node {}", node.as_u32())
            }
            Self::MalformedCaseNode { node } => {
                write!(f, "malformed case node {}", node.as_u32())
            }
            Self::EmptyArgumentList { .. } => write!(f, "empty argument list"),
            Self::NonIdentifierParameter { node } => {
                write!(
                    f,
                    "abstraction parameter is not an identifier: {}",
                    node.as_u32()
                )
            }
            Self::NonIdentifierIterationVariable { node } => {
                write!(
                    f,
                    "iteration variable is not an identifier: {}",
                    node.as_u32()
                )
            }
            Self::IterationCountNotInt { node } => {
                write!(f, "iteration count is not an int node: {}", node.as_u32())
            }
            Self::IterationCountTooLarge { value } => {
                write!(f, "iteration count too large for this target: {value}")
            }
            Self::NegativeIterationCount { value } => {
                write!(f, "iteration count is negative: {value}")
            }
            Self::PatternArityMismatch { expected, got, .. } => {
                write!(f, "pattern arity mismatch: expected {expected}, got {got}")
            }
            Self::PatternMatchFailed { .. } => write!(f, "no case rule matches arguments"),
            Self::TooManyArguments { expected, got, .. } => {
                write!(
                    f,
                    "too many arguments: expected at most {expected}, got {got}"
                )
            }
            Self::InvalidModulationLabel { node } => {
                write!(f, "invalid modulation label at node {}", node.as_u32())
            }
            Self::InvalidLabelInterpolation { ident, reason, .. } => {
                write!(
                    f,
                    "cannot interpolate label placeholder `%{ident}`: {reason}"
                )
            }
            Self::InvalidModulationCircuit { reason, .. } => {
                write!(f, "invalid modulation circuit: {reason}")
            }
            Self::InvalidControlListOperand { primitive, .. } => write!(
                f,
                "`{primitive}` expects a closed block diagram as its expression"
            ),
            Self::ControlIndexOutOfRange {
                primitive,
                index,
                count,
                ..
            } => {
                let what = if *primitive == "cinput" {
                    "control inputs"
                } else {
                    "bargraphs"
                };
                write!(
                    f,
                    "`{primitive}` index {index} is out of range: the expression has {count} {what}"
                )
            }
            Self::ModulationWildcardNoMatch { target, .. } => write!(
                f,
                "the modulation target `{target}` matches no control input of the expression"
            ),
            Self::InvalidSourceReference { construct, .. } => {
                write!(
                    f,
                    "{construct} requires a string-like source filename literal"
                )
            }
            Self::SourceFileNotFound {
                construct, target, ..
            } => {
                write!(f, "{construct} could not resolve source file `{target}`")
            }
            Self::SourceReaderFailure {
                construct, target, ..
            } => {
                write!(f, "{construct} failed while reading source file `{target}`")
            }
            Self::SourceParseFailure {
                construct, path, ..
            } => {
                write!(
                    f,
                    "{construct} loaded `{}` but parsing failed",
                    path.display()
                )
            }
            Self::ExpectedClosureValue { context, .. } => {
                write!(f, "{context} requires a captured closure value")
            }
            Self::RedefinedSymbol { symbol, .. } => {
                write!(
                    f,
                    "symbol `{symbol}` redefined with a different value in the same scope"
                )
            }
            Self::LoopDetected { node } => {
                write!(f, "recursive evaluation loop on node {}", node.as_u32())
            }
            Self::RecursionDepthExceeded { max_depth } => {
                write!(f, "stack overflow in eval (depth budget {max_depth})")
            }
            Self::NotAConstantExpression { node } => {
                write!(
                    f,
                    "expression is not a compile-time numeric constant (type 0→1): node {}",
                    node.as_u32()
                )
            }
            Self::WidgetParameterNotConstant {
                widget,
                label,
                parameter,
                expression,
                arity,
                ..
            } => match arity {
                Some((ins, outs)) if (*ins, *outs) != (0, 1) => write!(
                    f,
                    "the {parameter} of {widget}(\"{label}\") is not a constant expression of type (0->1): `{expression}` has type ({ins}->{outs})"
                ),
                _ => write!(
                    f,
                    "the {parameter} of {widget}(\"{label}\") must be a real constant numerical expression, and `{expression}` is not"
                ),
            },
            // the reference's words: `ERROR : division by 0 in 2 / 0`
            Self::DivisionByZero { detail, .. } => write!(f, "division by 0 in {detail}"),
            Self::SliderInitOutOfRange {
                kind,
                label,
                init_bits,
                min_bits,
                max_bits,
            } => {
                let (init, min, max) = (
                    f64::from_bits(*init_bits),
                    f64::from_bits(*min_bits),
                    f64::from_bits(*max_bits),
                );
                write!(
                    f,
                    "init = {init} outside of [{min} {max}] range in '{kind}(\"{label}\",{init},{min},{max},...)'"
                )
            }
            Self::InternalError { message } => {
                write!(f, "internal evaluator error: {message}")
            }
            Self::Cancelled => write!(f, "evaluation cancelled (timeout or abort)"),
        }
    }
}

impl std::error::Error for EvalError {}

/// Converts one evaluator error into the workspace diagnostics model.
///
/// This keeps `EvalError` as the local phase error type while exposing
/// stable stage/code metadata for compiler-level aggregation and CLI rendering.
impl ToDiagnostic for EvalError {
    fn to_diagnostic(&self) -> Diagnostic {
        let message = self.to_string();
        match self {
            Self::MissingProcessDefinition {
                entrypoint,
                available_defs,
                ..
            } => with_symbol_suggestions(
                Diagnostic::new(
                    Severity::Error,
                    Stage::Eval,
                    codes::EVAL_MISSING_PROCESS,
                    message,
                ),
                &self.symbol_suggestions(),
            )
            .with_note(format!(
                "cause: required top-level `{entrypoint}` definition is missing"
            ))
            .with_note(format!(
                "entrypoint contract: one top-level `{entrypoint} = ...;` definition is required"
            ))
            .with_note(format!(
                "available top-level definitions: {}",
                if available_defs.is_empty() {
                    "<none>".to_owned()
                } else {
                    available_defs.join(", ")
                }
            ))
            .with_detail_code("missing-entrypoint")
            .with_fact("entrypoint", entrypoint.clone())
            .with_fact("available_definitions", available_defs.clone())
            .with_help(format!(
                "define `{entrypoint} = ...;` in the top-level definitions"
            ))
            .with_help(format!("template: {entrypoint} = _;")),
            Self::UndefinedSymbol {
                symbol,
                local_scope,
                visible_scope,
                top_level_scope,
                ..
            } => with_symbol_suggestions(
                Diagnostic::new(
                    Severity::Error,
                    Stage::Eval,
                    codes::EVAL_UNDEFINED_SYMBOL,
                    message,
                ),
                &self.symbol_suggestions(),
            )
            .with_note("cause: unresolved identifier in current lexical scope")
            .with_note("rule: referenced identifier must be present in visible lexical scope")
            .with_note(format!(
                "computed: `{symbol}` is not present in current visible scope"
            ))
            .with_note(format!(
                "scope.local={}",
                if local_scope.is_empty() {
                    "<none>".to_owned()
                } else {
                    local_scope.join(", ")
                }
            ))
            .with_note(format!(
                "scope.visible={}",
                if visible_scope.is_empty() {
                    "<none>".to_owned()
                } else {
                    visible_scope.join(", ")
                }
            ))
            .with_note(format!(
                "scope.top_level={}",
                if top_level_scope.is_empty() {
                    "<none>".to_owned()
                } else {
                    top_level_scope.join(", ")
                }
            ))
            .with_detail_code("undefined-binding")
            .with_fact("symbol", symbol.clone())
            .with_fact("scope_local", local_scope.clone())
            .with_fact("scope_visible", visible_scope.clone())
            .with_fact("scope_top_level", top_level_scope.clone())
            .with_help("define the symbol in scope or fix the identifier name")
            .with_help(format!("template: {symbol} = ...; // define before use"))
            .with_help("for top-level aliases: define target before first use"),
            Self::PatternArityMismatch { expected, got, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_ARITY_MISMATCH,
                message,
            )
            .with_note("cause: case pattern arity does not match provided argument tuple")
            .with_note("rule: case rule arity must match provided argument tuple arity")
            .with_note(format!(
                "computed: expected={expected}, provided={got}, delta={}",
                *got as i128 - *expected as i128
            ))
            .with_note(format!(
                "suggested target: call case function with exactly {expected} argument(s)"
            ))
            .with_help("adapt the case pattern arity or provide the expected number of arguments")
            .with_help("template: case { (x, y) => ...; }; // 2-argument rule"),
            Self::TooManyArguments { expected, got, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_ARITY_MISMATCH,
                message,
            )
            .with_note("cause: function application provides more arguments than accepted")
            .with_note(
                "rule: non-closure application requires provided arguments <= function input arity",
            )
            .with_note(format!(
                "computed: provided={got}, expected_max={expected}, overflow={}",
                got.saturating_sub(*expected)
            ))
            .with_note(format!(
                "suggested target: remove {} extra argument(s)",
                got.saturating_sub(*expected)
            ))
            .with_help("remove extra arguments or expand the function input arity")
            .with_help("template: f(a, b); // keep provided args <= function input arity"),
            Self::InvalidModulationLabel { .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: modulation target did not resolve to a valid label string")
            .with_note("rule: modulation target must be a string-like Faust label")
            .with_help("use a literal label such as [\"gain\" : _ -> expr]"),
            Self::InvalidLabelInterpolation { ident, reason, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note(format!(
                "cause: label placeholder `{ident}` did not resolve to an integer constant"
            ))
            .with_note(format!("computed: {reason}"))
            .with_help(
                "bind the placeholder name to an integer constant expression before using it in a label",
            ),
            Self::InvalidModulationCircuit { reason, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: modulation circuit violates Faust box-arity constraints")
            .with_note(format!("computed: {reason}"))
            .with_help("use a modulation circuit with at most 2 inputs and exactly 1 output"),
            Self::InvalidControlListOperand { primitive, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note(format!(
                "cause: the expression of `{primitive}` still holds unapplied functions or free variables once evaluated"
            ))
            .with_help(format!(
                "pass a block diagram, e.g. `{primitive}(component(\"x.dsp\"))` or `{primitive}(hslider(\"g\", 0, 0, 1, 0.01) : *(2))`"
            )),
            Self::ControlIndexOutOfRange {
                primitive, count, ..
            } => {
                let list = if *primitive == "cinput" {
                    "cinputs"
                } else {
                    "coutputs"
                };
                Diagnostic::new(
                    Severity::Error,
                    Stage::Eval,
                    codes::EVAL_CONTROL_INDEX_OUT_OF_RANGE,
                    message,
                )
                .with_note("rule: indices are 0-based, in the order of the program's interface")
                .with_help(if *count == 0 {
                    format!("the expression has none: `outputs({list}(e))` is 0")
                } else {
                    format!(
                        "use an index in [0, {}], or iterate with `par(i, outputs({list}(e)), ...)`",
                        count - 1
                    )
                })
            }
            Self::ModulationWildcardNoMatch { target, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_MODULATION_WILDCARD_NO_MATCH,
                message,
            )
            .with_note(
                "rule: a wildcard target must rebind at least one slider, numentry, button or checkbox; bargraphs are never matched",
            )
            .with_note(format!(
                "computed: `{target}` matched nothing; group prefixes are matched on the group labels in order, without a `h:`/`v:`/`t:` type prefix"
            ))
            .with_help("check the group names with `faust-rs -json`, or use `\"*\"` alone to match every control input"),
            Self::InvalidSourceReference { construct, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note(format!(
                "cause: `{construct}` expects a literal source filename carried directly by the box tree"
            ))
            .with_help("template: component(\"file.dsp\") or library(\"file.dsp\")"),
            Self::SourceFileNotFound {
                target,
                current_file,
                search_paths,
                ..
            } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note(format!(
                "current file: {}",
                current_file
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<memory>".to_owned())
            ))
            .with_note(format!(
                "search paths: {}",
                if search_paths.is_empty() {
                    "<none>".to_owned()
                } else {
                    search_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ))
            .with_help(format!("check that `{target}` exists in the active import path")),
            Self::SourceReaderFailure {
                construct,
                message: detail,
                ..
            } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note(format!("source reader failure in `{construct}`: {detail}")),
            Self::SourceParseFailure { diagnostics, .. } => {
                Diagnostic::new(
                    Severity::Error,
                    Stage::Eval,
                    codes::EVAL_GENERIC_FAILURE,
                    message,
                )
                .with_detail_code("nested-source-parse-failure")
                .with_fact(
                    "nested_error_count",
                    u64::try_from(diagnostics.error_count()).unwrap_or(u64::MAX),
                )
                .with_note("the original parser diagnostics follow unchanged")
            }
            Self::ExpectedClosureValue { context, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note(
                "cause: evaluator expected a captured lexical environment but received a plain box value",
            )
            .with_note(format!(
                "rule: `{context}` only applies to values that carry a captured environment"
            ))
            .with_help("apply the operator to an environment or abstraction value instead"),
            Self::RedefinedSymbol {
                symbol,
                first_def,
                second_def,
            } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_REDEFINED_SYMBOL,
                message,
            )
            .with_note(
                "cause: the same symbol is bound twice with conflicting values in the same scope",
            )
            .with_note(
                "rule: each symbol may appear at most once per `with {}` block or definition list",
            )
            .with_note(format!(
                "computed: `{symbol}` first bound to node {}, then to node {} (different values)",
                first_def.as_u32(),
                second_def.as_u32()
            ))
            .with_note(
                "note: identical redefinitions (same expression) are silently accepted — \
                 only conflicting redefinitions are errors",
            )
            .with_help(format!("remove the duplicate `{symbol} = ...;` definition"))
            .with_help(
                "if shadowing was intended, move the inner definition to a nested `with {}` block"
                    .to_string(),
            ),
            Self::PatternMatchFailed { .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: no case rule matched the provided argument tuple")
            .with_note("rule: at least one case pattern must match the provided argument tuple")
            .with_note("computed: provided tuple did not match any declared case pattern")
            .with_help("add a matching case rule or add a catch-all pattern"),
            Self::IterationCountNotInt { .. }
            | Self::IterationCountTooLarge { .. }
            | Self::NegativeIterationCount { .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_ITERATION_INVALID,
                message,
            )
            .with_note("cause: iterative combinator count is not a valid non-negative integer")
            .with_note(
                "rule: iterator count must be integer, non-negative, and within supported range",
            )
            .with_help("iteration count must be a non-negative integer in target range"),
            Self::RecursionDepthExceeded { max_depth } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: evaluator recursion exhausted the guarded stack budget")
            .with_note(format!(
                "computed: the evaluator crossed its recursion budget before finishing ({max_depth} frames)"
            ))
            .with_help("check recursive definitions for a missing base case or non-decreasing recursive call"),
            Self::DivisionByZero { detail, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_DIVISION_BY_ZERO,
                message,
            )
            .with_note("cause: a constant expression divides by a constant zero")
            .with_note("rule: the divisor of a constant division must not be zero, in integers or in reals")
            .with_note(format!("computed: `{detail}`, after the operands were folded to constants"))
            .with_help(
                "check the value the divisor takes here: an iteration index starts at 0, \
                 and a function argument may be 0 at this call",
            ),
            Self::SliderInitOutOfRange {
                kind,
                label,
                init_bits,
                min_bits,
                max_bits,
            } => {
                let (init, min, max) = (
                    f64::from_bits(*init_bits),
                    f64::from_bits(*min_bits),
                    f64::from_bits(*max_bits),
                );
                Diagnostic::new(
                    Severity::Error,
                    Stage::Eval,
                    codes::EVAL_SLIDER_INIT_OUT_OF_RANGE,
                    message,
                )
                .with_note(format!(
                    "cause: widget `{kind}(\"{label}\",...)` has init={init} outside [{min}, {max}]"
                ))
                .with_note("rule: init must satisfy min <= init <= max")
                .with_help(format!("set init to a value in [{min}, {max}], e.g. {min}"))
            }
            Self::WidgetParameterNotConstant { widget, arity, .. } => {
                let parameters = if widget.ends_with("bargraph") {
                    "its min and max"
                } else {
                    "its init, min, max and step"
                };
                let diagnostic = Diagnostic::new(
                    Severity::Error,
                    Stage::Eval,
                    codes::EVAL_WIDGET_PARAMETER_NOT_CONSTANT,
                    message,
                )
                .with_note(format!(
                    "rule: the parameters of a `{widget}`, {parameters}, are evaluated at compile time and must fold to numbers"
                ));
                if *arity == Some((0, 1)) {
                    diagnostic
                        .with_note("computed: the parameter is a signal known only at run time: an input, a UI control, or the argument of a function applied with `:`")
                        .with_help("pass a constant; a function whose argument sets a widget parameter must be called with it, `f(0.5)`, not composed with it, `0.5 : f`")
                } else {
                    diagnostic.with_help("pass a single constant number")
                }
            }
            _ => Diagnostic::new(
                Severity::Error,
                Stage::Eval,
                codes::EVAL_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: evaluator reached an unsupported or malformed intermediate form"),
        }
    }
}
