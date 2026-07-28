//! Semantic guidance attached to evaluator diagnostics.
//!
//! The evaluator owns the *facts* of a failure (which symbol, which scopes,
//! which case rules). This module owns the *guidance* built from those facts,
//! because guidance needs three things `eval` deliberately does not have: the
//! box arena for rendering, the parser context for source locations, and the
//! session source map for canonical byte ranges.
//!
//! - `add_symbol_rename_fix` — an exact rename edit, only when one visible
//!   candidate wins unambiguously;
//! - `add_pattern_attempt_trace` — the declared `case` rules and the arguments
//!   actually dispatched on, without evaluator environments or automaton state;
//! - `add_redefinition_labels` — both conflicting declaration sites.

use super::*;

use eval::unambiguous_suggestion;

/// Bound on rendered `case` rules kept in one trace.
///
/// Generated or macro-expanded rule sets can be large; a diagnostic must stay
/// readable and deterministic rather than complete.
const MAX_TRACED_RULES: usize = 8;

/// Applies every evaluator-specific guidance step in a fixed order.
pub(crate) fn add_eval_guidance(
    mut diagnostic: Diagnostic,
    error: &eval::EvalError,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    source_map: &SourceMap,
) -> Diagnostic {
    diagnostic = add_symbol_rename_fix(diagnostic, error, source_map);
    diagnostic = add_pattern_attempt_trace(diagnostic, error, arena);
    add_redefinition_labels(diagnostic, error, ctx)
}

/// Proposes an exact rename when exactly one visible symbol is the clear match.
///
/// The two failure shapes rename in opposite directions, and getting that
/// backwards produces an edit that changes nothing:
///
/// - an **undefined symbol** is a misspelled *use*, so the use site becomes the
///   symbol that actually exists;
/// - a **missing entry point** is a misspelled *definition*, so the near-miss
///   definition name becomes the entry point the compiler is looking for.
///
/// Applicability stays [`Applicability::MaybeIncorrect`] even for a
/// distance-one match: the compiler knows the name is reachable, not that
/// renaming preserves the programmer's intent.
///
/// Nothing is emitted when the candidate set is ambiguous, when there is no
/// primary label, when the label span cannot be mapped to a canonical range, or
/// when the edit would replace the text with itself.
fn add_symbol_rename_fix(
    diagnostic: Diagnostic,
    error: &eval::EvalError,
    source_map: &SourceMap,
) -> Diagnostic {
    let suggestions = error.symbol_suggestions();
    let Some(best) = unambiguous_suggestion(&suggestions) else {
        return diagnostic;
    };
    let (replacement, explanation) = match error {
        eval::EvalError::MissingProcessDefinition { entrypoint, .. } => (
            entrypoint.clone(),
            format!(
                "`{}` looks like a misspelling of the required `{entrypoint}` entry point",
                best.name
            ),
        ),
        _ => (
            best.name.clone(),
            format!(
                "`{}` is visible from this site, but renaming changes which definition runs",
                best.name
            ),
        ),
    };
    let Some(label) = diagnostic
        .labels
        .iter()
        .find(|label| label.style == LabelStyle::Primary)
    else {
        return diagnostic;
    };
    let Ok(range) = source_map.from_source_span(&label.span) else {
        return diagnostic;
    };
    if source_map
        .slice(range)
        .is_ok_and(|text| text == replacement)
    {
        return diagnostic;
    }

    diagnostic.with_fix(SuggestedFix {
        title: format!("rename to `{replacement}`").into(),
        applicability: Applicability::MaybeIncorrect,
        edits: vec![TextEdit {
            range,
            replacement: replacement.into_boxed_str(),
        }],
        explanation: Some(explanation.into()),
    })
}

/// Records which `case` rules were declared and what the matcher dispatched on.
///
/// The trace is built from the rules tree the error already points at, so it
/// describes the program rather than the evaluator: one frame per declared rule
/// with its pattern arity and rendered pattern list, preceded by one frame for
/// the provided arguments.
fn add_pattern_attempt_trace(
    diagnostic: Diagnostic,
    error: &eval::EvalError,
    arena: &tlib::TreeArena,
) -> Diagnostic {
    let eval::EvalError::PatternMatchFailed { node, arguments } = error else {
        return diagnostic;
    };

    let rendered_arguments = arguments
        .iter()
        .map(|arg| compact_human_box_preview(arena, *arg))
        .collect::<Vec<_>>();
    let rules = declared_case_rules(arena, *node);

    let mut frames = vec![TraceFrame {
        name: Some("arguments".into()),
        span: None,
        ir: None,
        description: if rendered_arguments.is_empty() {
            "no argument was dispatched on".into()
        } else {
            format!("provided ({})", rendered_arguments.join(", ")).into()
        },
    }];
    for rule in rules.iter().take(MAX_TRACED_RULES) {
        frames.push(TraceFrame {
            name: Some(format!("rule {}", rule.index).into()),
            span: None,
            ir: None,
            description: format!("pattern ({}) did not match", rule.patterns.join(", ")).into(),
        });
    }
    if rules.len() > MAX_TRACED_RULES {
        frames.push(TraceFrame {
            name: None,
            span: None,
            ir: None,
            description: format!(
                "{} further rule(s) not shown",
                rules.len() - MAX_TRACED_RULES
            )
            .into(),
        });
    }

    diagnostic
        .with_note(format!(
            "computed: no rule survived after {} of {} argument(s)",
            arguments.len(),
            rules
                .first()
                .map_or(arguments.len(), |rule| rule.patterns.len())
        ))
        .with_fact(
            "pattern_rule_count",
            u64::try_from(rules.len()).unwrap_or(u64::MAX),
        )
        .with_fact(
            "pattern_argument_count",
            u64::try_from(arguments.len()).unwrap_or(u64::MAX),
        )
        .with_fact("pattern_arguments", rendered_arguments.clone())
        .with_fact(
            "pattern_rules",
            rules
                .iter()
                .map(|rule| format!("({})", rule.patterns.join(", ")))
                .collect::<Vec<_>>(),
        )
        .with_trace(DiagnosticTrace {
            kind: TraceKind::Evaluation,
            frames,
        })
}

/// One declared `case` rule, rendered for diagnostics.
struct DeclaredRule {
    /// 1-based source order of the rule.
    index: usize,
    /// Rendered left-hand pattern list.
    patterns: Vec<String>,
}

/// Reads the declared rules out of a case-rules tree in source order.
///
/// The evaluator stores the rule list reversed, and each rule as
/// `(pattern_list, rhs)`. Only the left-hand side is rendered: the right-hand
/// side is the action, not the reason the match failed. Traversal is bounded so
/// a malformed or cyclic list degrades to a shorter trace instead of hanging.
fn declared_case_rules(arena: &tlib::TreeArena, rules_root: BoxId) -> Vec<DeclaredRule> {
    let mut reversed = Vec::new();
    let mut cursor = rules_root;
    while !arena.is_nil(cursor) && reversed.len() < 4096 {
        let Some(rule) = arena.hd(cursor) else { break };
        let Some(patterns) = arena.hd(rule) else {
            break;
        };
        reversed.push(rendered_pattern_list(arena, patterns));
        let Some(next) = arena.tl(cursor) else { break };
        cursor = next;
    }
    reversed.reverse();
    reversed
        .into_iter()
        .enumerate()
        .map(|(offset, patterns)| DeclaredRule {
            index: offset + 1,
            patterns,
        })
        .collect()
}

/// Renders one rule's left-hand pattern list in written argument order.
///
/// The evaluator stores pattern lists reversed, like the rule list itself.
fn rendered_pattern_list(arena: &tlib::TreeArena, patterns: BoxId) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = patterns;
    while !arena.is_nil(cursor) && out.len() < 64 {
        let Some(pattern) = arena.hd(cursor) else {
            break;
        };
        out.push(rendered_pattern(arena, pattern));
        let Some(next) = arena.tl(cursor) else { break };
        cursor = next;
    }
    out.reverse();
    out
}

/// Renders one pattern the way the programmer wrote it.
///
/// A pattern variable is an internal wrapper around its identifier; showing the
/// wrapper would leak evaluator representation into user-facing guidance, so it
/// renders as the bare name. Everything else is an ordinary box expression.
fn rendered_pattern(arena: &tlib::TreeArena, pattern: BoxId) -> String {
    if let BoxMatch::PatternVar(ident) = match_box(arena, pattern)
        && let BoxMatch::Ident(name) = match_box(arena, ident)
    {
        return name.to_owned();
    }
    compact_human_box_preview(arena, pattern)
}

/// Labels both declarations involved in a same-scope redefinition.
///
/// The first declaration is the context and the second one is the conflict the
/// programmer introduced, so the second is primary. Either label is emitted
/// independently: a redefinition inside a generated `with {}` block may have a
/// locatable conflict and an unlocatable original.
fn add_redefinition_labels(
    mut diagnostic: Diagnostic,
    error: &eval::EvalError,
    ctx: &parser::ParserCtx,
) -> Diagnostic {
    let eval::EvalError::RedefinedSymbol {
        first_def,
        second_def,
        ..
    } = error
    else {
        return diagnostic;
    };

    if let Some(span) = source_span_for_definition_node(ctx, *second_def) {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Primary, span, "conflicting definition")
                .with_role(LabelRole::PrimaryCause),
        );
    }
    if let Some(span) = source_span_for_definition_node(ctx, *first_def) {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Secondary, span, "first definition")
                .with_role(LabelRole::ConflictsWith),
        );
    }
    diagnostic
}
