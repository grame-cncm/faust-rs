//! Diagnostic enrichment — source spans, definition ownership, and paired context.
//!
//! Provides the helpers that attach detailed context to compiler diagnostics:
//! - `add_paired_propagate_context` — adds A/B expression notes to composed-box
//!   arity-mismatch errors;
//! - `maybe_add_source_label` / `maybe_add_eval_source_labels` — attach
//!   source-file span notes derived from tree node metadata;
//! - `source_span_*` — extract span start/end/file from a node or definition;
//! - `find_definition_*` / `owner_definition_name_for_node` — locate the
//!   enclosing definition for a given node in the box tree;
//! - `definition_reference_edges` / `collect_definition_refs` — build the
//!   definition cross-reference graph used in alias binding traces;
//! - `alias_binding_trace_for_node` — produces an ordered chain of definition
//!   names showing how a node was reached through symbol aliases.

use super::*;

// ─── Propagate diagnostic enrichment ─────────────────────────────────────────

/// Enriches arity-mismatch diagnostics with explicit paired A/B expression context.
///
/// The paired notes make propagate failures easier to read when the offending
/// node is a composed expression (`A:B`, `A<:B`, `A:>B`, `A~B`) rather than a
/// leaf. They are intentionally additive: if arity inference for either side
/// fails, the original diagnostic is kept and only the successfully computed
/// side notes are attached.
pub(crate) fn add_paired_propagate_context(
    mut diagnostic: Diagnostic,
    error: &PropagateError,
    arena: &tlib::TreeArena,
) -> Diagnostic {
    let (node, op_name) = match error {
        PropagateError::SeqArityMismatch { node, .. } => (*node, "seq"),
        PropagateError::SplitArityMismatch { node, .. } => (*node, "split"),
        PropagateError::MergeArityMismatch { node, .. } => (*node, "merge"),
        PropagateError::RecArityMismatch { node, .. } => (*node, "rec"),
        _ => return diagnostic,
    };

    let (left, right) = match match_box(arena, node) {
        BoxMatch::Seq(left, right)
        | BoxMatch::Split(left, right)
        | BoxMatch::Merge(left, right)
        | BoxMatch::Rec(left, right) => (left, right),
        _ => return diagnostic,
    };

    let left_expr = compact_human_box_preview(arena, left);
    let right_expr = compact_human_box_preview(arena, right);
    diagnostic = diagnostic.with_note(format!("A ({op_name} left) = {left_expr}"));
    diagnostic = diagnostic.with_note(format!("B ({op_name} right) = {right_expr}"));

    let mut arity_cache = ArityCache::new();
    if let Ok(left_flat) = propagate::try_build_flat_box(arena, left)
        && let Ok(a) = propagate::box_arity_typed(arena, left_flat, &mut arity_cache)
    {
        diagnostic = diagnostic.with_note(format!(
            "A arity: inputs={} outputs={}",
            a.inputs, a.outputs
        ));
    }
    if let Ok(right_flat) = propagate::try_build_flat_box(arena, right)
        && let Ok(b) = propagate::box_arity_typed(arena, right_flat, &mut arity_cache)
    {
        diagnostic = diagnostic.with_note(format!(
            "B arity: inputs={} outputs={}",
            b.inputs, b.outputs
        ));
    }

    diagnostic
}

// ─── Source label helpers ─────────────────────────────────────────────────────

/// How deep the search for a section of `-` goes into a composition.
const SECTION_DEPTH: usize = 32;

/// Says what a composition mismatch may have meant.
///
/// A section of `-` in an argument comes first: `*(-(x))` or `max(-(x))` reads
/// as a negation, keeps an input, and writing the negation would connect the
/// widths. Otherwise, for `:`, a list written before it whose last item fits
/// is often `,` read as binding less tightly than `:`, and the widths name the
/// operator that would connect them.
pub(crate) fn add_composition_guidance(
    diagnostic: Diagnostic,
    error: &PropagateError,
    arena: &tlib::TreeArena,
    composition_origins: &std::collections::HashMap<BoxId, Option<BoxId>>,
) -> Diagnostic {
    let (node, left_outputs, right_inputs) = match error {
        PropagateError::SeqArityMismatch {
            node,
            left_outputs,
            right_inputs,
        }
        | PropagateError::SplitArityMismatch {
            node,
            left_outputs,
            right_inputs,
        }
        | PropagateError::MergeArityMismatch {
            node,
            left_outputs,
            right_inputs,
        } => (*node, *left_outputs, *right_inputs),
        _ => return diagnostic,
    };
    let sections = match match_box(arena, node) {
        BoxMatch::Seq(_, right) | BoxMatch::Split(_, right) | BoxMatch::Merge(_, right) => {
            minus_sections_at_inputs(arena, right, 0)
        }
        _ => 0,
    };
    let connects = |inputs: usize| match error {
        PropagateError::SeqArityMismatch { .. } => left_outputs == inputs,
        PropagateError::SplitArityMismatch { .. } => inputs.is_multiple_of(left_outputs),
        _ => left_outputs.is_multiple_of(inputs),
    };
    if (1..=sections.min(right_inputs)).any(|negated| connects(right_inputs - negated)) {
        return diagnostic
            .with_note("`-(x)` as an argument is the section `_ - x`, not a negation: it keeps an input of its own")
            .with_help("write the negation as `0 - x`");
    }
    if !matches!(error, PropagateError::SeqArityMismatch { .. }) {
        return diagnostic;
    }
    let BoxMatch::Seq(left, right) = match_box(arena, node) else {
        return diagnostic;
    };
    let mut diagnostic = diagnostic;
    // a composition evaluation left as written has no entry: it is its source
    let source = composition_origins
        .get(&node)
        .copied()
        .unwrap_or(Some(node))
        .and_then(|source| match match_box(arena, source) {
            BoxMatch::Seq(source_left, source_right) => Some((source_left, source_right)),
            _ => None,
        });
    if let Some(last) =
        source.and_then(|(source_left, _)| last_listed_item(arena, source_left, left))
        && box_arity(arena, last).is_some_and(|(_, outputs)| outputs == right_inputs)
    {
        diagnostic = diagnostic.with_help(format!(
            "if only the last item of A goes into the {right_inputs} input(s) of B, write `a, (b : c)`: `,` binds more tightly than `:`, so `a, b : c` reads `(a, b) : c`"
        ));
    }
    if right_inputs == 0 {
        // A call that takes no input is often one given an argument too many,
        // which fills its input: no factor there.
        let factor = source.is_some_and(|(_, source_right)| {
            !matches!(match_box(arena, source_right), BoxMatch::Appl(..))
        });
        if factor
            && left_outputs == 1
            && box_arity(arena, right).is_some_and(|(_, outputs)| outputs == 1)
        {
            diagnostic = diagnostic.with_help(
                "B takes no input, so `:` has nothing to feed: if B is a factor of A, write `A * B` or `A : *(B)`",
            );
        }
        return diagnostic;
    }
    match composition_operator_hint(left_outputs, right_inputs) {
        Some(hint) => diagnostic.with_help(hint),
        None => diagnostic,
    }
}

/// The operator that would connect these widths, if `:` was not meant.
///
/// The widths decide which family fits: a multiple is split with `<:` or
/// mixed with `:>` (or given one copy of B per group, side by side), and any
/// other difference leaves inputs of B open with `si.bus` or drops or passes
/// the outputs of A that B has no room for. The extra outputs or inputs are
/// taken to be the last ones, as `,` would place them.
fn composition_operator_hint(left_outputs: usize, right_inputs: usize) -> Option<String> {
    if left_outputs == 0 || right_inputs == 0 || left_outputs == right_inputs {
        return None;
    }
    let bus = |width: usize| {
        if width == 1 {
            "_".to_owned()
        } else {
            format!("si.bus({width})")
        }
    };
    if right_inputs > left_outputs {
        if right_inputs.is_multiple_of(left_outputs) {
            return Some(format!(
                "`A <: B` would split the {left_outputs} output(s) of A over the {right_inputs} inputs of B"
            ));
        }
        let open = right_inputs - left_outputs;
        let left_open = if open == 1 {
            "its last input open, as an input".to_owned()
        } else {
            format!("its last {open} inputs open, as inputs")
        };
        return Some(format!(
            "`(A, {}) : B` would feed the first {left_outputs} inputs of B from A and leave {left_open} of the whole",
            bus(open)
        ));
    }
    if left_outputs.is_multiple_of(right_inputs) {
        let copies = left_outputs / right_inputs;
        let side_by_side = if copies <= 3 {
            format!("({})", vec!["B"; copies].join(", "))
        } else {
            format!("par(i, {copies}, B)")
        };
        let each = if right_inputs == 1 {
            "each output".to_owned()
        } else {
            format!("each group of {right_inputs}")
        };
        return Some(format!(
            "`A :> B` would merge the {left_outputs} outputs of A into the {right_inputs} input(s) of B, and `A : {side_by_side}` would give {each} its own B"
        ));
    }
    let extra = left_outputs - right_inputs;
    let (block, dropped) = if extra == 1 {
        (
            "!".to_owned(),
            "the last one, and `A : (B, _)` would pass it on".to_owned(),
        )
    } else {
        (
            format!("si.block({extra})"),
            format!("the last {extra}, and `A : (B, si.bus({extra}))` would pass them on"),
        )
    };
    Some(format!(
        "`A : (B, {block})` would feed the first {right_inputs} outputs of A to B and drop {dropped}"
    ))
}

/// The evaluated last item of a list written before `:`, as in `a, b : c`.
///
/// `source` is the left operand as written, `evaluated` the same operand after
/// evaluation, which keeps the `,` of the source. A left operand that is a name
/// or a call is no list written there, and gives nothing.
fn last_listed_item(arena: &tlib::TreeArena, source: BoxId, evaluated: BoxId) -> Option<BoxId> {
    let BoxMatch::Par(..) = match_box(arena, source) else {
        return None;
    };
    let (mut source, mut evaluated) = (source, evaluated);
    for _ in 0..SECTION_DEPTH {
        match (match_box(arena, source), match_box(arena, evaluated)) {
            (BoxMatch::Par(_, source_right), BoxMatch::Par(_, evaluated_right)) => {
                source = source_right;
                evaluated = evaluated_right;
            }
            (BoxMatch::Par(..), _) => return None,
            _ => return Some(evaluated),
        }
    }
    None
}

/// The `(inputs, outputs)` of an evaluated box, when they can be computed.
fn box_arity(arena: &tlib::TreeArena, node: BoxId) -> Option<(usize, usize)> {
    let flat = propagate::try_build_flat_box(arena, node).ok()?;
    let arity = propagate::box_arity_typed(arena, flat, &mut ArityCache::new()).ok()?;
    Some((arity.inputs, arity.outputs))
}

/// How many inputs of this expression come from a section of `-` given to a
/// primitive: `*(-(x))` is evaluated to `_, (_, x : -) : *`, `max(-(x))` to
/// `_, (_, x : -) : max`, and `select2(-(x))` to `(_, x : -), (_, _) : select2`.
///
/// Only what reaches the inputs counts: a section further down a composition is
/// fed by what precedes it, and is often a `_ - x` written on purpose.
fn minus_sections_at_inputs(arena: &tlib::TreeArena, node: BoxId, depth: usize) -> usize {
    if depth == SECTION_DEPTH {
        return 0;
    }
    match match_box(arena, node) {
        BoxMatch::Seq(left, right) => {
            if is_primitive(arena, right) && matches!(match_box(arena, left), BoxMatch::Par(..)) {
                minus_sections_in_arguments(arena, left, depth + 1)
            } else {
                minus_sections_at_inputs(arena, left, depth + 1)
            }
        }
        BoxMatch::Par(left, right) => {
            minus_sections_at_inputs(arena, left, depth + 1)
                + minus_sections_at_inputs(arena, right, depth + 1)
        }
        BoxMatch::Split(left, _) | BoxMatch::Merge(left, _) | BoxMatch::Rec(left, _) => {
            minus_sections_at_inputs(arena, left, depth + 1)
        }
        _ => 0,
    }
}

/// The same count over the arguments of a primitive, a tree of `Par`.
fn minus_sections_in_arguments(arena: &tlib::TreeArena, args: BoxId, depth: usize) -> usize {
    if depth == SECTION_DEPTH {
        return 0;
    }
    if let BoxMatch::Par(left, right) = match_box(arena, args) {
        return minus_sections_in_arguments(arena, left, depth + 1)
            + minus_sections_in_arguments(arena, right, depth + 1);
    }
    if is_minus_section(arena, args) {
        1
    } else {
        minus_sections_at_inputs(arena, args, depth + 1)
    }
}

/// Whether this box is `_, x : -`, as `-(x)` is evaluated.
fn is_minus_section(arena: &tlib::TreeArena, node: BoxId) -> bool {
    let BoxMatch::Seq(pair, op) = match_box(arena, node) else {
        return false;
    };
    let BoxMatch::Par(first, _) = match_box(arena, pair) else {
        return false;
    };
    matches!(match_box(arena, op), BoxMatch::Sub)
        && matches!(match_box(arena, first), BoxMatch::Wire)
}

/// Whether this box is a primitive, under its infix symbol or its name.
fn is_primitive(arena: &tlib::TreeArena, node: BoxId) -> bool {
    prim_infix_symbol(arena, node).is_some() || prim_readable_name(arena, node).is_some()
}

/// Labels the definitions of the operands of a failing composition.
///
/// A mismatch shows where two widths meet, and the width that is wrong is
/// usually written in one of those definitions (`capture : *(level)` of a
/// `capture` that gained an output). Only definitions the file of the composition
/// has once are labelled, and at most four, the left operand first.
fn add_operand_definition_labels(
    mut diagnostic: Diagnostic,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    source: BoxId,
    composition: &SourceSpan,
    parameters: &std::collections::HashSet<BoxId>,
) -> Diagnostic {
    const SHOWN: usize = 4;
    let mut names = Vec::new();
    collect_operand_names(arena, source, 0, &mut names);
    names.retain(|name| !parameters.contains(name));
    let mut shown = 0;
    for name in names {
        let provenance = ctx.box_provenance();
        let mut sites = provenance
            .origins_for(name)
            .iter()
            .filter_map(|id| provenance.get(*id))
            .filter(|origin| origin.role == parser::BoxOriginRole::Definition)
            .map(|origin| source_span_from_parser_location(&origin.location))
            .filter(|span| span.file == composition.file);
        let (Some(site), None) = (sites.next(), sites.next()) else {
            continue;
        };
        let BoxMatch::Ident(text) = match_box(arena, name) else {
            continue;
        };
        let message = format!("`{text}` is defined here");
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Secondary, site, message).with_role(LabelRole::DefinitionSite),
        );
        shown += 1;
        if shown == SHOWN {
            break;
        }
    }
    diagnostic
}

/// The names an operand is made of: those it composes and the functions it
/// calls, not their arguments, whose width is not the operand's.
fn collect_operand_names(
    arena: &tlib::TreeArena,
    node: BoxId,
    depth: usize,
    names: &mut Vec<BoxId>,
) {
    const DEPTH: usize = 32;
    if depth == DEPTH {
        return;
    }
    match match_box(arena, node) {
        BoxMatch::Ident(_) => {
            if !names.contains(&node) {
                names.push(node);
            }
        }
        BoxMatch::Par(left, right)
        | BoxMatch::Seq(left, right)
        | BoxMatch::Split(left, right)
        | BoxMatch::Merge(left, right)
        | BoxMatch::Rec(left, right) => {
            collect_operand_names(arena, left, depth + 1, names);
            collect_operand_names(arena, right, depth + 1, names);
        }
        BoxMatch::Appl(fun, _) => collect_operand_names(arena, fun, depth + 1, names),
        BoxMatch::VGroup(_, body)
        | BoxMatch::HGroup(_, body)
        | BoxMatch::TGroup(_, body)
        | BoxMatch::WithLocalDef(body, _)
        | BoxMatch::WithRecDef(body, _, _) => {
            collect_operand_names(arena, body, depth + 1, names);
        }
        _ => {}
    }
}

/// The names bound as parameters in the top-level definition that holds
/// `node`, its local functions included. An operand of one of these names is no
/// definition, whatever else the file defines under it. Walking the whole
/// definition rather than the path to `node` may leave out a label, never add a
/// false one.
fn parameters_around(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
) -> std::collections::HashSet<BoxId> {
    let mut parameters = std::collections::HashSet::new();
    let found = owner_definition_name_for_node(arena, defs_root, node)
        .and_then(|owner| find_definition_name_and_expr(arena, defs_root, &owner));
    let Some((_, expr)) = found else {
        return parameters;
    };
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![expr];
    while let Some(cur) = stack.pop() {
        if seen.len() > 1 << 16 || !seen.insert(cur) {
            continue;
        }
        if let BoxMatch::Abstr(parameter, _) = match_box(arena, cur) {
            parameters.insert(parameter);
        }
        if let Some(children) = arena.children(cur) {
            stack.extend(children.iter().copied());
        }
    }
    parameters
}

/// Attaches source labels for propagate/arity diagnostics.
///
/// When the owning definition is known, this prefers that origin as primary and
/// keeps process call-site as secondary to improve alias-chain readability.
/// This policy reflects the common Faust failure mode where the concrete bad
/// composition is inside a helper definition but only becomes observable once
/// referenced by `process`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maybe_add_source_label(
    mut diagnostic: Diagnostic,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
    owner_definition: Option<&str>,
    entrypoint_name: &str,
    composition_origins: Option<&std::collections::HashMap<BoxId, Option<BoxId>>>,
) -> Diagnostic {
    // A composition built by evaluation has no position of its own; the one
    // it was evaluated from does, when the sources write it at one place.
    // Its descendants would mislead: the first located one is usually in
    // the body of a function the composition called (`(_, _)` of `m` for a
    // faulty `m(1) : …`). A box built from two compositions, or from one the
    // sources write at several places, gets no composition label.
    if let Some(origin) = composition_origins.and_then(|origins| origins.get(&node)) {
        if let Some((source, span)) =
            origin.and_then(|source| Some((source, unique_source_span(ctx, source)?)))
        {
            let parameters = parameters_around(arena, defs_root, source);
            diagnostic = diagnostic.with_label(
                Label::new(LabelStyle::Primary, span.clone(), "failing composition")
                    .with_role(LabelRole::Operator),
            );
            return add_operand_definition_labels(
                diagnostic,
                ctx,
                arena,
                source,
                &span,
                &parameters,
            );
        }
        let fallback =
            source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name);
        if let Some(span) = fallback {
            diagnostic = diagnostic.with_label(
                Label::new(LabelStyle::Primary, span, "the call that leads here")
                    .with_role(LabelRole::CallSite),
            );
        }
        return diagnostic;
    }
    if let Some(owner) = owner_definition {
        let exact_span = source_span_for_node_in_definition(ctx, arena, defs_root, node, owner);
        let owner_span = source_span_for_definition_name(ctx, arena, defs_root, owner);
        let call_span =
            source_span_for_entrypoint_binding_target(ctx, arena, defs_root, entrypoint_name)
                .or_else(|| {
                    source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name)
                });
        if let Some(primary_span) = exact_span.clone().or_else(|| owner_span.clone()) {
            diagnostic = diagnostic.with_label(
                Label::new(
                    LabelStyle::Primary,
                    primary_span.clone(),
                    if exact_span.is_some() {
                        "failing composition"
                    } else {
                        "related source"
                    },
                )
                .with_role(if exact_span.is_some() {
                    LabelRole::Operator
                } else {
                    LabelRole::DefinitionSite
                }),
            );
            if let Some(definition_span) = owner_span
                && definition_span != primary_span
            {
                diagnostic = diagnostic.with_label(
                    Label::new(
                        LabelStyle::Secondary,
                        definition_span,
                        "enclosing definition",
                    )
                    .with_role(LabelRole::DefinitionSite),
                );
            }
            if let Some(secondary_span) = call_span
                && secondary_span != primary_span
            {
                diagnostic = diagnostic.with_label(
                    Label::new(LabelStyle::Secondary, secondary_span, "related call site")
                        .with_role(LabelRole::CallSite),
                );
            }
            return diagnostic;
        }
        diagnostic = diagnostic
            .with_note("origin span unavailable; pointing to nearest call/owner site".to_owned());
    }

    let span = source_span_from_node_or_descendant(ctx, arena, node)
        .or_else(|| source_span_for_definition_of_expr(ctx, arena, defs_root, node))
        .or_else(|| {
            source_span_for_entrypoint_binding_target(ctx, arena, defs_root, entrypoint_name)
        })
        .or_else(|| source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name));
    if let Some(span) = span {
        diagnostic = diagnostic.with_label(Label::new(LabelStyle::Primary, span, "related source"));
    }
    diagnostic
}

/// Labels a missing entry point: the definition whose name looks like a
/// misspelling of it, when exactly one does, and nothing otherwise.
///
/// Nothing in the source asks for the entry point — its name is the program's
/// contract (`process`) or comes from `-pn` — so there is no use site to point
/// at, and the C++ compiler reports this error without a location. The generic
/// node labelling, given the definition list, underlined its first located
/// identifier as the "call site": an unrelated line. The rename fix of
/// `eval_guidance` edits the span labelled here.
pub(crate) fn maybe_add_missing_entrypoint_label(
    diagnostic: Diagnostic,
    error: &eval::EvalError,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
) -> Diagnostic {
    let suggestions = error.symbol_suggestions();
    let Some(best) = eval::unambiguous_suggestion(&suggestions) else {
        return diagnostic;
    };
    let Some(span) = source_span_for_definition_name(ctx, arena, defs_root, &best.name) else {
        return diagnostic;
    };
    diagnostic.with_label(
        Label::new(LabelStyle::Primary, span, "similar definition")
            .with_role(LabelRole::DefinitionSite),
    )
}

/// Attaches eval-oriented primary/secondary labels when available.
///
/// Label policy:
/// - alias-chain mode (`owner_definition` known): primary origin definition,
///   secondary process call-site.
/// - fallback mode: primary nearest call/use site, secondary owning definition.
///
/// This differs slightly from propagate labeling because eval failures often
/// arise during symbol resolution or application, where the use-site can be
/// more actionable than the eventual enclosing composition site.
pub(crate) fn maybe_add_eval_source_labels(
    mut diagnostic: Diagnostic,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
    owner_definition: Option<&str>,
    entrypoint_name: &str,
) -> Diagnostic {
    if let Some(owner) = owner_definition {
        // An application has no place of its own: the name it calls does
        let used = match match_box(arena, node) {
            BoxMatch::Appl(fun, _) => fun,
            _ => node,
        };
        let use_span = source_span_for_node_in_definition(ctx, arena, defs_root, used, owner);
        let definition_primary = if used == node {
            "definition site"
        } else {
            "failing call"
        };
        let origin_span = source_span_for_definition_name(ctx, arena, defs_root, owner);
        let call_span =
            source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name);
        if let Some(primary_span) = use_span.clone().or_else(|| origin_span.clone()) {
            let primary_role = if use_span.is_some() {
                LabelRole::UseSite
            } else if used == node {
                LabelRole::DefinitionSite
            } else {
                LabelRole::CallSite
            };
            diagnostic = diagnostic.with_label(
                Label::new(
                    LabelStyle::Primary,
                    primary_span.clone(),
                    if use_span.is_some() {
                        "failing use"
                    } else {
                        definition_primary
                    },
                )
                .with_role(primary_role),
            );
            if let Some(definition_span) = origin_span
                && definition_span != primary_span
            {
                diagnostic = diagnostic.with_label(
                    Label::new(
                        LabelStyle::Secondary,
                        definition_span,
                        "enclosing definition",
                    )
                    .with_role(LabelRole::DefinitionSite),
                );
            }
            if let Some(secondary_span) = call_span
                && secondary_span != primary_span
            {
                diagnostic = diagnostic.with_label(
                    Label::new(LabelStyle::Secondary, secondary_span, "call site")
                        .with_role(LabelRole::CallSite),
                );
            }
            return diagnostic;
        }
        diagnostic = diagnostic
            .with_note("origin span unavailable; pointing to nearest call/owner site".to_owned());
    }

    // No owning definition: the node is in a library, in a `with`, or was
    // built by evaluation. Only a span the sources give at one place is used
    // (`unique_source_span`): a hash-consed identifier or literal found as a
    // descendant can be any of its occurrences, in any file (#21 underlined
    // `ma = library("maths.lib")` in stdfaust.lib). The program's own call is
    // labelled when the failure is elsewhere or cannot be located.
    let call = source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name);
    let failing = source_span_from_node_or_unique_descendant(ctx, arena, node);
    let Some(failing_span) = failing else {
        if let Some(call_span) = call {
            diagnostic = diagnostic.with_label(
                Label::new(LabelStyle::Primary, call_span, "the call that leads here")
                    .with_role(LabelRole::CallSite),
            );
        }
        return diagnostic;
    };
    diagnostic = diagnostic.with_label(
        Label::new(
            LabelStyle::Primary,
            failing_span.clone(),
            "failing expression",
        )
        .with_role(LabelRole::PrimaryCause),
    );
    if let Some(definition_span) = source_span_for_definition_of_expr(ctx, arena, defs_root, node)
        && definition_span != failing_span
    {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Secondary, definition_span, "definition site")
                .with_role(LabelRole::DefinitionSite),
        );
    }
    if let Some(call_span) = call
        && call_span.file != failing_span.file
    {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Secondary, call_span, "the call that leads here")
                .with_role(LabelRole::CallSite),
        );
    }
    diagnostic
}

/// Labels the rules of a `case` that refused a call, which stays the failure.
pub(crate) fn add_case_rules_label(
    diagnostic: Diagnostic,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    rules: BoxId,
) -> Diagnostic {
    let primary = primary_span_of(&diagnostic);
    match source_span_for_definition_of_expr(ctx, arena, defs_root, rules) {
        Some(span) if Some(&span) != primary.as_ref() => diagnostic.with_label(
            Label::new(LabelStyle::Secondary, span, "the rules that do not match")
                .with_role(LabelRole::DefinitionSite),
        ),
        _ => diagnostic,
    }
}

fn primary_span_of(diagnostic: &Diagnostic) -> Option<SourceSpan> {
    diagnostic
        .labels
        .iter()
        .find(|label| label.style == LabelStyle::Primary)
        .map(|label| label.span.clone())
}

/// The span of `node` when the sources mention it at a single place.
///
/// Hash-consing gives every occurrence of a structurally equal box one node,
/// so a node met in several places, in one file or across files, has no
/// single location to point at.
pub(crate) fn unique_source_span(ctx: &parser::ParserCtx, node: BoxId) -> Option<SourceSpan> {
    let provenance = ctx.box_provenance();
    let mut locations = provenance
        .origins_for(node)
        .iter()
        .filter_map(|id| provenance.get(*id))
        .map(|origin| &origin.location);
    let first = locations.next()?;
    locations
        .all(|location| location == first)
        .then(|| source_span_from_parser_location(first))
}

/// [`unique_source_span`] of `node`, or of its first composite descendant
/// that has one, in depth-first order.
///
/// Leaves are skipped: an identifier, a literal or a primitive is the same
/// node wherever it is written, and the occurrences recorded for it do not
/// cover every file it appears in, so a single recorded one proves nothing.
pub(crate) fn source_span_from_node_or_unique_descendant(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    node: BoxId,
) -> Option<SourceSpan> {
    if let Some(span) = unique_source_span(ctx, node) {
        return Some(span);
    }
    let mut stack: Vec<BoxId> = arena.children(node).map(|c| c.to_vec()).unwrap_or_default();
    stack.reverse();
    let mut visited = 0usize;
    while let Some(cur) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        if matches!(
            match_box(arena, cur),
            BoxMatch::Ident(_)
                | BoxMatch::PatternVar(_)
                | BoxMatch::Int(_)
                | BoxMatch::Real(_)
                | BoxMatch::Wire
                | BoxMatch::Cut
        ) {
            continue;
        }
        let Some(children) = arena.children(cur).filter(|c| !c.is_empty()) else {
            continue;
        };
        if let Some(span) = unique_source_span(ctx, cur) {
            return Some(span);
        }
        stack.extend(children.iter().rev());
    }
    None
}

/// Labels a recursion on a non-constant argument whose function is not one of
/// the program's top-level definitions: a function of a library, or one local
/// to a `with`.
///
/// The rules the evaluator reports carry no source location of their own,
/// and their located descendants are identifiers shared by every file
/// (`ma`, `s`), which the generic fallback of [`maybe_add_eval_source_labels`]
/// would underline in an unrelated library. The entry point's definition is
/// the program's own call that leads to the recursion, so it alone is
/// labelled.
pub(crate) fn add_library_recursion_call_label(
    diagnostic: Diagnostic,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Diagnostic {
    let diagnostic = diagnostic.with_note(
        "the recursing function is not a top-level definition of this program: it is defined in a library or inside a `with`",
    );
    match source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name) {
        Some(span) => diagnostic.with_label(
            Label::new(
                LabelStyle::Primary,
                span,
                "the call that leads to the recursion",
            )
            .with_role(LabelRole::CallSite),
        ),
        None => diagnostic,
    }
}

/// Selects the exact parser occurrence of `node` inside one top-level
/// definition when hash-consing gave the semantic node multiple candidates.
///
/// Definition identifier locations delimit the lexical region. The function
/// deliberately returns `None` when zero or multiple candidates remain so a
/// caller can use a conservative definition-level fallback.
pub(crate) fn source_span_for_node_in_definition(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
    owner: &str,
) -> Option<SourceSpan> {
    let (owner_name, owner_expr) = find_definition_name_and_expr(arena, defs_root, owner)?;
    let local_names = local_definition_names(arena, owner_expr);
    let owner_start = ctx
        .box_provenance()
        .origins_for(owner_name)
        .iter()
        .filter_map(|id| ctx.box_provenance().get(*id))
        .find(|origin| origin.role == parser::BoxOriginRole::Definition)?
        .location
        .clone();

    let mut next_start = None;
    let mut defs = defs_root;
    while !arena.is_nil(defs) {
        let def = arena.hd(defs)?;
        let name = arena.hd(def)?;
        defs = arena.tl(defs)?;
        // A `with` of the owner may define a name that is also a top-level one,
        // often in a library: its site is inside the owner and ends nothing.
        if local_names.contains(&name) {
            continue;
        }
        for id in ctx.box_provenance().origins_for(name) {
            let Some(origin) = ctx.box_provenance().get(*id) else {
                continue;
            };
            if origin.role == parser::BoxOriginRole::Definition
                && origin.location.file() == owner_start.file()
                && (origin.location.line(), origin.location.col())
                    > (owner_start.line(), owner_start.col())
                && next_start
                    .as_ref()
                    .is_none_or(|next: &parser::SourceLocation| {
                        (origin.location.line(), origin.location.col()) < (next.line(), next.col())
                    })
            {
                next_start = Some(origin.location.clone());
            }
        }
    }

    let candidates = ctx
        .box_provenance()
        .origins_for(node)
        .iter()
        .filter_map(|id| ctx.box_provenance().get(*id))
        .filter(|origin| origin.role == parser::BoxOriginRole::Use)
        .filter(|origin| origin.location.file() == owner_start.file())
        .filter(|origin| {
            (origin.location.line(), origin.location.col())
                >= (owner_start.line(), owner_start.col())
        })
        .filter(|origin| {
            next_start.as_ref().is_none_or(|next| {
                (origin.location.line(), origin.location.col()) < (next.line(), next.col())
            })
        })
        .collect::<Vec<_>>();
    let [origin] = candidates.as_slice() else {
        return None;
    };
    Some(SourceSpan::new(
        origin.location.file(),
        origin.location.line(),
        origin.location.col(),
        origin.location.end_line(),
        origin.location.end_col(),
    ))
}

// ─── Source span resolution ───────────────────────────────────────────────────

/// Resolves one source span from the node itself, then falls back to labeled descendants.
pub(crate) fn source_span_from_node_or_descendant(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    node: BoxId,
) -> Option<SourceSpan> {
    if let Some(span) = source_span_for_node(ctx, node) {
        return Some(span);
    }

    let mut stack = vec![node];
    let mut visited = 0usize;
    while let Some(cur) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }

        if let Some(span) = source_span_for_node(ctx, cur) {
            return Some(span);
        }

        if let Some(children) = arena.children(cur) {
            for child in children.iter().rev() {
                stack.push(*child);
            }
        }
    }
    None
}

/// Resolves one source span for a node from parser `use_prop` / `def_prop`.
pub(crate) fn source_span_for_node(ctx: &parser::ParserCtx, node: BoxId) -> Option<SourceSpan> {
    if let Some(origin) = ctx
        .box_provenance()
        .origins_for(node)
        .iter()
        .rev()
        .filter_map(|id| ctx.box_provenance().get(*id))
        .find(|origin| origin.role == parser::BoxOriginRole::Use)
    {
        return Some(source_span_from_parser_location(&origin.location));
    }
    let loc = ctx.use_prop(node).or_else(|| ctx.def_prop(node))?;
    Some(source_span_from_parser_location(loc))
}

/// Resolves one source span for a definition node, preferring `def_prop`.
///
/// This is used for alias fallback (`process = foo;`) where we want the location
/// of the defining equation, not the use-site of `foo`.
pub(crate) fn source_span_for_definition_node(
    ctx: &parser::ParserCtx,
    node: BoxId,
) -> Option<SourceSpan> {
    if let Some(origin) = ctx
        .box_provenance()
        .origins_for(node)
        .iter()
        .filter_map(|id| ctx.box_provenance().get(*id))
        .find(|origin| origin.role == parser::BoxOriginRole::Definition)
    {
        return Some(source_span_from_parser_location(&origin.location));
    }
    let loc = ctx.def_prop(node).or_else(|| ctx.use_prop(node))?;
    Some(source_span_from_parser_location(loc))
}

pub(crate) fn source_span_from_parser_location(loc: &parser::SourceLocation) -> SourceSpan {
    SourceSpan::new(
        loc.file(),
        loc.line(),
        loc.col(),
        loc.end_line(),
        loc.end_col(),
    )
}

/// Fallback source span for the configured entry-point definition identifier.
///
/// Used when the offending propagated/evaluated node cannot be mapped to a more
/// specific source location.
pub(crate) fn source_span_for_entrypoint_definition(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Option<SourceSpan> {
    let mut defs = defs_root;
    let mut visited = 0usize;
    while !arena.is_nil(defs) {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        let def = arena.hd(defs)?;
        let name = arena.hd(def)?;
        if let BoxMatch::Ident(name_str) = match_box(arena, name)
            && name_str == entrypoint_name
        {
            return source_span_for_node(ctx, name);
        }
        defs = arena.tl(defs)?;
    }
    None
}

/// Fallback source span for direct entry-point aliases (`entry = <ident>;`).
///
/// When the configured entry-point is a direct identifier alias, this resolves
/// the target definition location (for example `foo = ...; synth = foo;` ->
/// label on `foo = ...`).
pub(crate) fn source_span_for_entrypoint_binding_target(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Option<SourceSpan> {
    let (_entry_name, entry_expr) =
        find_definition_name_and_expr(arena, defs_root, entrypoint_name)?;
    let BoxMatch::Ident(target_name) = match_box(arena, entry_expr) else {
        return None;
    };
    let (target_def_name, _target_expr) =
        find_definition_name_and_expr(arena, defs_root, target_name)?;
    source_span_for_definition_node(ctx, target_def_name)
}

/// Finds one `(definition_name, definition_expr)` pair by identifier name
/// in the parser root definitions list.
pub(crate) fn find_definition_name_and_expr(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    wanted: &str,
) -> Option<(BoxId, BoxId)> {
    let mut defs = defs_root;
    let mut visited = 0usize;
    while !arena.is_nil(defs) {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        let def = arena.hd(defs)?;
        let name = arena.hd(def)?;
        let args_expr = arena.tl(def)?;
        let expr = arena.tl(args_expr)?;
        if let BoxMatch::Ident(name_str) = match_box(arena, name)
            && name_str == wanted
        {
            return Some((name, expr));
        }
        defs = arena.tl(defs)?;
    }
    None
}

/// Fallback source span from a definition whose expression matches (or contains) `node`.
///
/// This covers alias chains such as:
/// `foo = <bad>; bar = foo; process = bar,bar;`
/// where the failing node belongs to `foo` but `process` is not a direct identifier alias.
pub(crate) fn source_span_for_definition_of_expr(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
) -> Option<SourceSpan> {
    let mut defs = defs_root;
    let mut visited = 0usize;
    while !arena.is_nil(defs) {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        let def = arena.hd(defs)?;
        let name = arena.hd(def)?;
        let args_expr = arena.tl(def)?;
        let expr = arena.tl(args_expr)?;
        if expr == node || subtree_contains_node(arena, expr, node) {
            return source_span_for_definition_node(ctx, name);
        }
        defs = arena.tl(defs)?;
    }
    None
}

/// Resolves a source span for one top-level definition name.
///
/// Resolution prefers the definition identifier span, then falls back to the
/// definition expression subtree when identifier metadata is unavailable.
pub(crate) fn source_span_for_definition_name(
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    wanted: &str,
) -> Option<SourceSpan> {
    let (name, expr) = find_definition_name_and_expr(arena, defs_root, wanted)?;
    source_span_for_definition_node(ctx, name)
        .or_else(|| source_span_from_node_or_descendant(ctx, arena, expr))
}

/// Returns `true` when the subtree rooted at `root` contains `needle`.
///
/// Uses iterative depth-first traversal bounded at 4096 visited nodes to avoid
/// infinite loops on DAG-shared or aliased subtrees.  The conservative bound
/// means very large subtrees may produce a false negative; callers that use
/// this for ownership detection already tolerate that with a `None` fallback.
/// The names defined by the `with` and `letrec` blocks under `root`, at any
/// depth. Bounded like the other walks of this file: it serves diagnostics only.
fn local_definition_names(
    arena: &tlib::TreeArena,
    root: BoxId,
) -> std::collections::HashSet<BoxId> {
    let mut names = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![root];
    while let Some(cur) = stack.pop() {
        if seen.len() > 1 << 16 || !seen.insert(cur) {
            continue;
        }
        let blocks = match match_box(arena, cur) {
            BoxMatch::WithLocalDef(_, definitions) => [Some(definitions), None],
            BoxMatch::WithRecDef(_, definitions, others) => [Some(definitions), Some(others)],
            _ => [None, None],
        };
        for mut list in blocks.into_iter().flatten() {
            while !arena.is_nil(list) {
                let Some(next) = arena.tl(list) else { break };
                if let Some(name) = arena.hd(list).and_then(|def| arena.hd(def)) {
                    names.insert(name);
                }
                list = next;
            }
        }
        if let Some(children) = arena.children(cur) {
            stack.extend(children.iter().copied());
        }
    }
    names
}

pub(crate) fn subtree_contains_node(arena: &tlib::TreeArena, root: BoxId, needle: BoxId) -> bool {
    if root == needle {
        return true;
    }
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(cur) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        if cur == needle {
            return true;
        }
        if let Some(children) = arena.children(cur) {
            for child in children.iter().rev() {
                stack.push(*child);
            }
        }
    }
    false
}

// ─── Definition graph helpers ─────────────────────────────────────────────────

/// Returns the owning definition name for one offending expression node.
///
/// The search is structural and bounded. It is used only for diagnostics, so a
/// conservative `None` fallback is preferable to panicking on malformed or
/// unusually deep definition lists.
pub(crate) fn owner_definition_name_for_node(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
) -> Option<Box<str>> {
    let mut defs = defs_root;
    let mut visited = 0usize;
    while !arena.is_nil(defs) {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        let def = arena.hd(defs)?;
        let name = arena.hd(def)?;
        let args_expr = arena.tl(def)?;
        let expr = arena.tl(args_expr)?;
        if (expr == node || subtree_contains_node(arena, expr, node))
            && let BoxMatch::Ident(name_str) = match_box(arena, name)
        {
            return Some(name_str.into());
        }
        defs = arena.tl(defs)?;
    }
    None
}

/// Selects the owning definition that is actually reachable from the requested
/// entry point when one hash-consed node occurs in several definitions.
///
/// A breadth-first traversal follows the deterministic definition-reference
/// graph. This prevents the old structural "first definition containing this
/// `TreeId`" fallback from blaming an unreachable identical occurrence.
pub(crate) fn reachable_owner_definition_name_for_node(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
    entrypoint_name: &str,
) -> Option<Box<str>> {
    let mut owners = HashSet::new();
    let mut defs = defs_root;
    let mut visited = 0usize;
    while !arena.is_nil(defs) {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        let def = arena.hd(defs)?;
        let name = arena.hd(def)?;
        let args_expr = arena.tl(def)?;
        let expr = arena.tl(args_expr)?;
        if (expr == node || subtree_contains_node(arena, expr, node))
            && let BoxMatch::Ident(name_str) = match_box(arena, name)
        {
            owners.insert(Box::<str>::from(name_str));
        }
        defs = arena.tl(defs)?;
    }

    let edges = definition_reference_edges(arena, defs_root);
    let mut queue = VecDeque::from([Box::<str>::from(entrypoint_name)]);
    let mut seen = HashSet::new();
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if owners.contains(&name) {
            return Some(name);
        }
        if let Some(next) = edges.get(&name) {
            queue.extend(next.iter().cloned());
        }
    }
    owner_definition_name_for_node(arena, defs_root, node)
}

/// Builds one deterministic reference graph between top-level definition names.
///
/// Each edge `A -> B` means definition `A` references identifier `B` somewhere in its expression.
pub(crate) fn definition_reference_edges(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
) -> HashMap<Box<str>, Vec<Box<str>>> {
    let mut defs = defs_root;
    let mut visited = 0usize;
    let mut rows: Vec<(Box<str>, BoxId)> = Vec::new();
    while !arena.is_nil(defs) {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        let Some(def) = arena.hd(defs) else {
            break;
        };
        let Some(name) = arena.hd(def) else {
            break;
        };
        let Some(args_expr) = arena.tl(def) else {
            break;
        };
        let Some(expr) = arena.tl(args_expr) else {
            break;
        };
        if let BoxMatch::Ident(name_str) = match_box(arena, name) {
            rows.push((name_str.into(), expr));
        }
        defs = match arena.tl(defs) {
            Some(next) => next,
            None => break,
        };
    }

    let known = rows
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<HashSet<_>>();

    let mut out: HashMap<Box<str>, Vec<Box<str>>> = HashMap::new();
    for (name, expr) in rows {
        let mut refs = collect_definition_refs(arena, expr, &known);
        refs.sort_unstable();
        refs.dedup();
        out.insert(name, refs);
    }
    out
}

/// Collects all definition-name identifiers referenced in one expression subtree.
pub(crate) fn collect_definition_refs(
    arena: &tlib::TreeArena,
    root: BoxId,
    known: &HashSet<Box<str>>,
) -> Vec<Box<str>> {
    let mut refs = Vec::new();
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(cur) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > 4096 {
            break;
        }
        if let BoxMatch::Ident(name) = match_box(arena, cur)
            && known.contains(name)
        {
            refs.push(name.into());
        }
        if let Some(children) = arena.children(cur) {
            for child in children.iter().rev() {
                stack.push(*child);
            }
        }
    }
    refs
}

/// Finds one alias/binding trace from the configured entry-point to the owner of `node`.
///
/// The trace is expression-reference based (not only direct aliases), allowing contextual chains
/// such as `process = bar,bar; bar = foo; foo = ...` -> `process -> bar -> foo`.
pub(crate) fn alias_binding_trace_for_node(
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
    entrypoint_name: &str,
) -> Option<String> {
    let owner = reachable_owner_definition_name_for_node(arena, defs_root, node, entrypoint_name)?;
    if owner.as_ref() == entrypoint_name {
        return Some(entrypoint_name.to_owned());
    }

    let edges = definition_reference_edges(arena, defs_root);
    if !edges.contains_key(entrypoint_name) {
        return None;
    }

    let mut queue: VecDeque<Vec<Box<str>>> = VecDeque::new();
    let mut seen: HashSet<Box<str>> = HashSet::new();
    queue.push_back(vec![entrypoint_name.into()]);
    seen.insert(entrypoint_name.into());

    while let Some(path) = queue.pop_front() {
        let Some(last) = path.last() else {
            continue;
        };
        if last.as_ref() == owner.as_ref() {
            return Some(path.join(" -> "));
        }
        let Some(nexts) = edges.get(last) else {
            continue;
        };
        for next in nexts {
            if seen.insert(next.clone()) {
                let mut extended = path.clone();
                extended.push(next.clone());
                queue.push_back(extended);
            }
        }
    }

    None
}
