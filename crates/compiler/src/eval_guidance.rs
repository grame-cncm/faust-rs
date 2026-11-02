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
//! - `add_redefinition_labels` — both conflicting declaration sites, and what
//!   to do when the first one comes from an imported file;
//! - `add_waveform_table_guidance` — a size given to a table beside a
//!   `waveform`, which carries its own;
//! - `add_nonconstant_case_argument` — the argument a recursion dispatched on
//!   without ever reaching a numeric rule, and the rules it was matched
//!   against;
//! - `add_cycle_labels` — every top-level definition of an evaluation cycle;
//! - `add_feedback_cycle_guidance` — a cycle that goes through a delay, which
//!   is a feedback loop written as a self-reference: the way to write it with
//!   `~`;
//! - `add_unary_minus_access` — an access into a unary minus (`-L.g`), which
//!   the grammar reads `(0 - L).g`.

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
    defs_root: BoxId,
    entrypoint_name: &str,
    source_map: &SourceMap,
) -> Diagnostic {
    diagnostic = add_symbol_rename_fix(diagnostic, error, source_map);
    diagnostic = add_pattern_attempt_trace(diagnostic, error, arena);
    diagnostic = add_nonconstant_case_argument(diagnostic, error, arena);
    diagnostic = add_cycle_labels(diagnostic, error, ctx, arena, defs_root);
    diagnostic = add_feedback_cycle_guidance(diagnostic, error, arena, defs_root);
    diagnostic = add_unary_minus_access(diagnostic, error, arena);
    diagnostic = add_waveform_table_guidance(diagnostic, error, arena);
    add_redefinition_labels(diagnostic, error, ctx, arena, defs_root, entrypoint_name)
}

/// Explains a table refused for its size: `waveform{...}` is two signals, its
/// size and its contents, so `rdtable(8, waveform{...}, i)` is one too many.
fn add_waveform_table_guidance(
    diagnostic: Diagnostic,
    error: &eval::EvalError,
    arena: &tlib::TreeArena,
) -> Diagnostic {
    let eval::EvalError::TooManyArguments {
        node,
        function: Some(function),
        ..
    } = error
    else {
        return diagnostic;
    };
    if !matches!(function.as_str(), "rdtable" | "rwtable") {
        return diagnostic;
    }
    let BoxMatch::Appl(_, args) = match_box(arena, *node) else {
        return diagnostic;
    };
    let has_waveform = tlib::list_to_vec(arena, args).is_some_and(|args| {
        args.iter()
            .any(|arg| matches!(match_box(arena, *arg), BoxMatch::Waveform(_)))
    });
    if !has_waveform {
        return diagnostic;
    }
    diagnostic
        .with_note("`waveform{...}` is two signals, its size and its contents, so it counts for two arguments")
        .with_help(format!(
            "leave the size out, as a waveform carries it: `{function}(waveform{{...}}, ...)`"
        ))
}

/// Explains an access into a unary minus: `-L.g(2, k)` reads
/// `((0 - L).g)(2, k)`, since the grammar's unary minus takes the name `L`
/// alone, so the access applies to a number, not to the environment `L`
/// (C++: `no environment to access`).
fn add_unary_minus_access(
    diagnostic: Diagnostic,
    error: &eval::EvalError,
    arena: &tlib::TreeArena,
) -> Diagnostic {
    let eval::EvalError::ExpectedClosureValue {
        node,
        context: "access",
    } = error
    else {
        return diagnostic;
    };
    // The parser's unary minus: `boxSeq(boxPar(0, x), Sub)`.
    let BoxMatch::Seq(pair, op) = match_box(arena, *node) else {
        return diagnostic;
    };
    let (BoxMatch::Par(zero, operand), BoxMatch::Sub) =
        (match_box(arena, pair), match_box(arena, op))
    else {
        return diagnostic;
    };
    if !matches!(match_box(arena, zero), BoxMatch::Int(0)) {
        return diagnostic;
    }
    let name = match match_box(arena, operand) {
        BoxMatch::Ident(name) => name.to_owned(),
        _ => "…".to_owned(),
    };
    diagnostic
        .with_note(format!(
            "rule: the unary `-` applies to `{name}` alone, so `-{name}.f` reads `(0 - {name}).f`, an access into a number"
        ))
        .with_help(format!(
            "subtract the whole access from zero: `0 - {name}.f(...)`, in parentheses where the context needs them"
        ))
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
        // An entry point other than `process` was named by the caller (`-pn`):
        // the misspelling is in the command line, and renaming the definition
        // it resembles would break the program for everyone else.
        eval::EvalError::MissingProcessDefinition { entrypoint, .. } if entrypoint != "process" => {
            return diagnostic;
        }
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
    let eval::EvalError::PatternMatchFailed {
        node, arguments, ..
    } = error
    else {
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

/// Names the argument of a recursion that never reached a numeric rule.
///
/// The evaluator gives the position, the argument as first dispatched on and
/// the numeric patterns it could not reach; rendering the argument and the
/// declared rules needs the arena. The argument is the one of the outermost
/// application, the value the caller passed, before the recursion rewrote it
/// (`n - 1 - 1 ...`).
fn add_nonconstant_case_argument(
    diagnostic: Diagnostic,
    error: &eval::EvalError,
    arena: &tlib::TreeArena,
) -> Diagnostic {
    let eval::EvalError::CaseArgumentNotConstant {
        node,
        position,
        argument,
        patterns,
        ..
    } = error
    else {
        return diagnostic;
    };
    let argument = describe_runtime_argument(arena, *argument);
    let rules = declared_case_rules(arena, *node)
        .iter()
        .take(MAX_TRACED_RULES)
        .map(|rule| format!("({})", rule.patterns.join(", ")))
        .collect::<Vec<_>>();
    diagnostic
        .with_note(format!(
            "computed: argument {position} is {argument}, matched against the rules {}",
            rules.join(", ")
        ))
        .with_fact(
            "case_argument_position",
            u64::try_from(*position).unwrap_or(u64::MAX),
        )
        .with_fact("case_argument", argument)
        .with_fact("case_numeric_patterns", patterns.clone())
        .with_fact("pattern_rules", rules)
}

/// What makes an argument known only at run time, in source terms.
///
/// The argument is an evaluated box: the functions it went through are
/// closures and the parameters of a lambda applied with `:` are slots, neither
/// of which has a source spelling. What the programmer can act on is the UI
/// control or the input it comes from, so those are named, up to three. An
/// argument with neither is rendered whole.
fn describe_runtime_argument(arena: &tlib::TreeArena, argument: BoxId) -> String {
    const MAX_NAMED: usize = 3;
    let mut controls = Vec::new();
    let mut input = false;
    let mut stack = vec![argument];
    let mut visited = 0usize;
    while let Some(cur) = stack.pop() {
        visited += 1;
        if visited > 4096 || controls.len() == MAX_NAMED {
            break;
        }
        match match_box(arena, cur) {
            BoxMatch::Button(_)
            | BoxMatch::Checkbox(_)
            | BoxMatch::HSlider(..)
            | BoxMatch::VSlider(..)
            | BoxMatch::NumEntry(..) => {
                let rendered = compact_human_box_preview(arena, cur);
                if !controls.contains(&rendered) {
                    controls.push(rendered);
                }
                continue;
            }
            BoxMatch::Slot(_) | BoxMatch::Wire => input = true,
            _ => {}
        }
        if let Some(children) = arena.children(cur) {
            stack.extend(children.iter().rev());
        }
    }
    match (controls.as_slice(), input) {
        ([], false) => format!("`{}`", compact_human_box_preview(arena, argument)),
        ([], true) => "an input signal".to_owned(),
        (controls, _) => format!(
            "a signal that depends on {}",
            controls
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Labels each top-level definition of an evaluation cycle.
///
/// The generic labels point at the use that closes the cycle and at the
/// entry point, which leaves the definitions themselves unlabelled
/// (`x = x;`). A definition local to a `with` or a library is not among the
/// top-level ones and gets no label; the message names it.
fn add_cycle_labels(
    mut diagnostic: Diagnostic,
    error: &eval::EvalError,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
) -> Diagnostic {
    let eval::EvalError::LoopDetected { cycle, .. } = error else {
        return diagnostic;
    };
    // the last name closes the cycle: it is the first one again
    let mut seen = std::collections::HashSet::new();
    for name in cycle.iter().take(cycle.len().saturating_sub(1)) {
        if !seen.insert(name.as_str()) {
            continue;
        }
        let Some(span) = source_span_for_definition_name(ctx, arena, defs_root, name) else {
            continue;
        };
        if diagnostic.labels.iter().any(|label| label.span == span) {
            continue;
        }
        diagnostic = diagnostic.with_label(
            Label::new(
                LabelStyle::Secondary,
                span,
                format!("`{name}`, in the cycle"),
            )
            .with_role(LabelRole::DefinitionSite),
        );
    }
    diagnostic
}

/// Explains a cycle that goes through a delay: `y = a * x + b * y';` is a
/// recurrence written as in another language, and Faust writes it with `~`.
///
/// The definitions of the cycle are found by name, at the top level and in
/// every `with` and `letrec` block, and each one's uses of the others are
/// sorted into delayed (under `'`, `@`, `mem`, `prefix` or the feedback side of
/// a `~`) and immediate. The note is given when a use in the cycle is delayed
/// and the immediate uses alone close no cycle: a value that needs itself at
/// the same sample (`v1 = x + a * (v1 - x);`, or `y = y + y';`) is no
/// recurrence, and `~` would change what it says. A delay hidden in a library
/// function is not seen, and the generic help stays.
fn add_feedback_cycle_guidance(
    mut diagnostic: Diagnostic,
    error: &eval::EvalError,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
) -> Diagnostic {
    let eval::EvalError::LoopDetected { cycle, .. } = error else {
        return diagnostic;
    };
    let names: Vec<&str> = cycle
        .iter()
        .take(cycle.len().saturating_sub(1))
        .map(String::as_str)
        .collect();
    if names.is_empty() {
        return diagnostic;
    }
    let uses = cycle_uses(arena, defs_root, &names);
    // the first delayed use along the cycle, a delay written as such before
    // the implicit one of a `~`
    let delayed = [false, true].into_iter().find_map(|feedback| {
        names.iter().enumerate().find_map(|(at, from)| {
            let to = names[(at + 1) % names.len()];
            uses.iter()
                .find(|u| {
                    u.from == *from && u.to == to && u.delay.is_some_and(|d| (d == "~") == feedback)
                })
                .and_then(|u| Some((*from, to, u.delay?)))
        })
    });
    let Some((from, to, delay)) = delayed else {
        return diagnostic;
    };
    if immediate_uses_close_a_cycle(&names, &uses) {
        return diagnostic;
    }
    let past = match delay {
        "'" => format!("`{to}'`"),
        "~" => format!("`{to}` on the feedback side of a `~`"),
        operator => format!("`{to}` through `{operator}`"),
    };
    let subject = if from == to {
        format!("`{from}` uses its own past, {past}")
    } else {
        format!("`{from}` uses the past of `{to}`, {past}, and `{to}` needs `{from}`")
    };
    diagnostic.help.clear();
    diagnostic
        .with_note(format!(
            "{subject}, but a definition cannot refer to itself, even through a delay: a signal computed from its own past is written with the recursive composition `~`, which feeds the output of its left side back through its right side one sample later"
        ))
        .with_help("write the loop with `~`: `y = a * x + b * y';` is `y = x : *(a) : + ~ *(b);`")
        .with_help("when the new value needs the previous one by name, take it as the first input of the loop: `y = x : (\\(prev, x).(a * x + b * prev)) ~ _;`")
}

/// One use of a definition of a cycle by another (or by itself).
struct CycleUse<'a> {
    from: &'a str,
    to: &'a str,
    /// The delay it is under, `None` for an immediate use.
    delay: Option<&'static str>,
}

/// Every use, among the definitions named `names`, of one by another.
fn cycle_uses<'a>(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    names: &[&'a str],
) -> Vec<CycleUse<'a>> {
    let mut uses = Vec::new();
    let mut lists = vec![defs_root];
    let mut seen_lists = std::collections::HashSet::new();
    let mut seen_bodies = std::collections::HashSet::new();
    while let Some(mut list) = lists.pop() {
        if seen_lists.len() > 1 << 12 || !seen_lists.insert(list) {
            continue;
        }
        while !arena.is_nil(list) {
            let Some(next) = arena.tl(list) else { break };
            let definition = arena.hd(list).and_then(|def| {
                let name = arena.hd(def)?;
                let expr = arena.tl(arena.tl(def)?)?;
                Some((name, expr))
            });
            list = next;
            let Some((name, expr)) = definition else {
                continue;
            };
            // the local blocks of this definition are definitions too
            collect_local_lists(arena, expr, &mut lists, &mut seen_bodies);
            let BoxMatch::Ident(name) = match_box(arena, name) else {
                continue;
            };
            let Some(from) = names.iter().find(|wanted| **wanted == name) else {
                continue;
            };
            uses.extend(
                find_uses(arena, expr, names)
                    .into_iter()
                    .map(|(to, delay)| CycleUse { from, to, delay }),
            );
        }
    }
    uses
}

/// The definition lists of the `with` and `letrec` blocks under `node`.
fn collect_local_lists(
    arena: &tlib::TreeArena,
    node: BoxId,
    lists: &mut Vec<BoxId>,
    seen: &mut std::collections::HashSet<BoxId>,
) {
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        if seen.len() > 1 << 16 || !seen.insert(cur) {
            continue;
        }
        match match_box(arena, cur) {
            BoxMatch::WithLocalDef(_, definitions) => lists.push(definitions),
            BoxMatch::WithRecDef(_, definitions, others) => lists.extend([definitions, others]),
            _ => {}
        }
        if let Some(children) = arena.children(cur) {
            stack.extend(children.iter().copied());
        }
    }
}

/// The uses of `names` in an expression, each with the delay it is under.
///
/// The definitions of a local block are not walked here: they are definitions
/// of their own, and `cycle_uses` reads them as such. The walk keeps its own
/// stack, as expressions can be deeper than the thread's.
fn find_uses<'a>(
    arena: &tlib::TreeArena,
    expr: BoxId,
    names: &[&'a str],
) -> Vec<(&'a str, Option<&'static str>)> {
    // `x'` and `@(x, n)` are `x : '` and `(x, n) : @`; `y : @(3)` and `mem(y)`
    // apply the delay to its other arguments
    let delay_of = |op: BoxId| match match_box(arena, op) {
        BoxMatch::Delay1 => Some("'"),
        BoxMatch::Delay => Some("@"),
        BoxMatch::Prefix => Some("prefix"),
        BoxMatch::Appl(fun, _) => match match_box(arena, fun) {
            BoxMatch::Delay1 => Some("mem"),
            BoxMatch::Delay => Some("@"),
            BoxMatch::Prefix => Some("prefix"),
            _ => None,
        },
        _ => None,
    };
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![(expr, None)];
    while let Some((node, delay)) = stack.pop() {
        if seen.len() > 1 << 16 || !seen.insert((node, delay)) {
            continue;
        }
        match match_box(arena, node) {
            BoxMatch::Ident(name) => {
                if let Some(to) = names.iter().find(|wanted| **wanted == name) {
                    found.push((*to, delay));
                }
            }
            BoxMatch::Seq(left, right) if delay_of(right).is_some() => {
                stack.push((left, delay.or(delay_of(right))));
                stack.push((right, delay));
            }
            BoxMatch::Appl(fun, args) if delay_of(fun).is_some() => {
                let under = match match_box(arena, fun) {
                    BoxMatch::Delay1 => Some("mem"),
                    _ => delay_of(fun),
                };
                stack.push((args, delay.or(under)));
            }
            BoxMatch::Rec(forward, feedback) => {
                stack.push((forward, delay));
                stack.push((feedback, delay.or(Some("~"))));
            }
            BoxMatch::WithLocalDef(body, _) | BoxMatch::WithRecDef(body, _, _) => {
                stack.push((body, delay));
            }
            _ => {
                if let Some(children) = arena.children(node) {
                    stack.extend(children.iter().map(|child| (*child, delay)));
                }
            }
        }
    }
    found
}

/// Whether the immediate uses alone, among the definitions of the cycle, close
/// a cycle: then some value needs itself at the same sample.
fn immediate_uses_close_a_cycle(names: &[&str], uses: &[CycleUse<'_>]) -> bool {
    let index = |name: &str| names.iter().position(|n| *n == name);
    let edges: Vec<(usize, usize)> = uses
        .iter()
        .filter(|u| u.delay.is_none())
        .filter_map(|u| Some((index(u.from)?, index(u.to)?)))
        .collect();
    // Kahn's algorithm: what is left once every source is peeled is a cycle
    let mut incoming = vec![0usize; names.len()];
    for (_, to) in &edges {
        incoming[*to] += 1;
    }
    let mut ready: Vec<usize> = (0..names.len()).filter(|n| incoming[*n] == 0).collect();
    let mut peeled = 0;
    while let Some(node) = ready.pop() {
        peeled += 1;
        for (from, to) in &edges {
            if *from == node {
                incoming[*to] -= 1;
                if incoming[*to] == 0 {
                    ready.push(*to);
                }
            }
        }
    }
    peeled < names.len()
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
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Diagnostic {
    let eval::EvalError::RedefinedSymbol {
        symbol,
        name_node,
        first_def,
        second_def,
    } = error
    else {
        return diagnostic;
    };

    // The value of a definition may be shared, or have no site: the sites of
    // the name are the definitions themselves, in the order they were read.
    let provenance = ctx.box_provenance();
    let sites: Vec<SourceSpan> = provenance
        .origins_for(*name_node)
        .iter()
        .filter_map(|id| provenance.get(*id))
        .filter(|origin| origin.role == parser::BoxOriginRole::Definition)
        .map(|origin| source_span_from_parser_location(&origin.location))
        .collect();
    let mut second =
        source_span_for_definition_node(ctx, *second_def).or_else(|| sites.last().cloned());
    let mut first =
        source_span_for_definition_node(ctx, *first_def).or_else(|| sites.first().cloned());

    // Imported definitions are bound after those of the file that imports
    // them, whatever the order of the source: the conflict the programmer
    // introduced is the one in the program.
    let program = source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name)
        .map(|span| span.file);
    if let (Some(a), Some(b), Some(program)) = (&first, &second, &program)
        && a.file != b.file
        && (a.file == *program || b.file == *program)
    {
        if a.file == *program {
            std::mem::swap(&mut first, &mut second);
        }
        let imported = first.as_ref().map_or_else(String::new, |span| {
            span.file.file_name().map_or_else(
                || span.file.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        });
        diagnostic.help.clear();
        diagnostic = diagnostic.with_help(format!(
            "`{symbol}` already belongs to the imported `{imported}`: rename yours, or define it in a `with {{}}` block"
        ));
    }
    if let Some(span) = second {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Primary, span, "conflicting definition")
                .with_role(LabelRole::PrimaryCause),
        );
    }
    if let Some(span) = first {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Secondary, span, "first definition")
                .with_role(LabelRole::ConflictsWith),
        );
    }
    diagnostic
}
