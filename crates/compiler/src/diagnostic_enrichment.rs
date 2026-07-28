//! Diagnostic enrichment — source spans, definition ownership, and paired context.
//!
//! Provides the helpers that attach detailed context to compiler diagnostics:
//! - `add_paired_propagate_context` — adds A/B expression notes to composed-box
//!   arity-mismatch errors;
//! - `maybe_add_source_label` / `eval_source_labels` — attaches source-file span
//!   notes derived from tree node metadata;
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

/// Attaches source labels for propagate/arity diagnostics.
///
/// When the owning definition is known, this prefers that origin as primary and
/// keeps process call-site as secondary to improve alias-chain readability.
/// This policy reflects the common Faust failure mode where the concrete bad
/// composition is inside a helper definition but only becomes observable once
/// referenced by `process`.
pub(crate) fn maybe_add_source_label(
    mut diagnostic: Diagnostic,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    node: BoxId,
    owner_definition: Option<&str>,
    entrypoint_name: &str,
) -> Diagnostic {
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
        let use_span = source_span_for_node_in_definition(ctx, arena, defs_root, node, owner);
        let origin_span = source_span_for_definition_name(ctx, arena, defs_root, owner);
        let call_span =
            source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name);
        if let Some(primary_span) = use_span.clone().or_else(|| origin_span.clone()) {
            let primary_role = if use_span.is_some() {
                LabelRole::UseSite
            } else {
                LabelRole::DefinitionSite
            };
            diagnostic = diagnostic.with_label(
                Label::new(
                    LabelStyle::Primary,
                    primary_span.clone(),
                    if use_span.is_some() {
                        "failing use"
                    } else {
                        "definition site"
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

    let primary = source_span_from_node_or_descendant(ctx, arena, node)
        .or_else(|| source_span_for_definition_of_expr(ctx, arena, defs_root, node))
        .or_else(|| {
            source_span_for_entrypoint_binding_target(ctx, arena, defs_root, entrypoint_name)
        })
        .or_else(|| source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name));
    let Some(primary_span) = primary else {
        return diagnostic;
    };
    diagnostic = diagnostic.with_label(
        Label::new(LabelStyle::Primary, primary_span.clone(), "call site")
            .with_role(LabelRole::CallSite),
    );
    let secondary = source_span_for_definition_of_expr(ctx, arena, defs_root, node)
        .or_else(|| source_span_for_entrypoint_definition(ctx, arena, defs_root, entrypoint_name));
    if let Some(secondary_span) = secondary
        && secondary_span != primary_span
    {
        diagnostic = diagnostic.with_label(
            Label::new(LabelStyle::Secondary, secondary_span, "definition site")
                .with_role(LabelRole::DefinitionSite),
        );
    }
    diagnostic
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
    let (owner_name, _) = find_definition_name_and_expr(arena, defs_root, owner)?;
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
        defs = arena.tl(defs)?;
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

fn source_span_from_parser_location(loc: &parser::SourceLocation) -> SourceSpan {
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
