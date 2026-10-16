//! Definition lookup, environment binding, and abstraction construction.
//!
//! Manages the name-resolution and closure layer of the Faust evaluator:
//! - `bind_definitions` — extends an environment with a set of recursive bindings;
//! - `rewrite_captured_env` / `copy_env_replace_defs` — closure capture helpers;
//! - `decode_definition` — unwraps a parser definition node into name + body;
//! - `top_level_definition_names` — enumerates names in a top-level definition block;
//! - `map_children` — structural fallback that evaluates children and rebuilds;
//! - `ident_name` — validates that a node is an `Ident` and extracts its name;
//! - `build_abstr_from_parser_args` — constructs a lambda chain from a reversed
//!   parser argument list.

use super::*;

/// Structural fallback: evaluate all children, then rebuild the node unchanged in kind.
/// Recursively evaluates every child of one box node and rebuilds the parent.
///
/// This is the structural fallback used for box families whose semantics in
/// `eval` are "evaluate children, keep outer constructor". It preserves the
/// original node when no child changes, matching the hash-consing-friendly
/// behavior of the C++ tree layer.
pub(crate) fn map_children(
    arena: &mut TreeArena,
    expr: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<TreeId, EvalError> {
    let Some(node) = arena.node(expr).cloned() else {
        return Ok(expr);
    };
    let mut children = Vec::with_capacity(node.children.len());
    for child in node.children.as_slice() {
        let value = eval_value(arena, *child, env, loop_detector)?;
        // Preserve residual closures/pattern matchers as box nodes here instead
        // of symbolically forcing them. Generic parent nodes such as `par`
        // must be able to carry higher-order children unchanged so later case
        // matching can still see tupled functions the same way C++ does.
        children.push(force_value_to_box(arena, value, loop_detector)?);
    }
    let built = arena.intern(node.kind, &children);
    if matches!(
        match_box(arena, expr),
        BoxMatch::Par(..) | BoxMatch::Split(..) | BoxMatch::Merge(..) | BoxMatch::Rec(..)
    ) {
        loop_detector.record_composition_origin(built, expr);
    }
    Ok(built)
}

/// Binds a parser definition list into an environment, enforcing the no-redefinition rule.
///
/// Each definition in `defs` is a `cons(name, cons(args, expr))` node.
///
/// Parser-originated definition lists are expected to be pre-normalized by
/// `parser::ParseState::format_definitions()` so that `args` is typically `nil`
/// and `expr` is already one of:
/// - plain body,
/// - nested `abstr`,
/// - `case`.
///
/// The `args != nil` fallback is retained for direct test construction and
/// compatibility with any remaining raw-definition call sites.
///
/// # Redefinition check — C++ `addLayerDef` parity
///
/// Before each `bind`, the current scope layer is checked for an existing binding of the same
/// name via [`Environment::lookup_local`]. This matches the C++ `addLayerDef` check:
///
/// ```cpp
/// // environment.cpp — addLayerDef (simplified)
/// Tree olddef = nullptr;
/// if (getProperty(lenv, id, olddef)) {
///     if (def == olddef) { /* identical — silently accept */ }
///     else {
///         gGlobal->gErrorCount++;
///         throw faustexception("redefinition of symbols are not allowed: " + boxpp(id));
///     }
/// }
/// setProperty(lenv, id, def);
/// ```
///
/// In Rust:
/// - If the same name is already bound in the **current scope** with the **same captured
///   closure value** (`expr` + captured `EnvId`), the new definition is silently skipped.
/// - If the same name is bound with a **different** captured value, `EvalError::RedefinedSymbol`
///   is returned using the underlying expression nodes for diagnostics.
/// - If the name is not yet in the current scope (including the case where it only exists
///   in a parent scope — shadowing), the binding proceeds normally.
///
/// # C++ correspondence
///
/// | C++ call site | Rust equivalent |
/// |---|---|
/// | `pushMultiClosureDefs(ldefs, visited, lenv)` | `bind_definitions(arena, defs, &mut scoped)` with explicit captured definition closures |
/// | `pushValueDef(id, def, lenv)` | `env.bind(name, value)` (single-binding fast path) |
/// Binds a top-level or local definition list into the current environment.
///
/// Source provenance (C++):
/// - `compiler/evaluate/environment.cpp`
/// - `pushMultiClosureDefs(...)`
/// - `addLayerDef(...)`
///
/// Each definition is captured as needed so later lookups evaluate under the
/// lexical environment visible at definition time. Duplicate names in the same
/// scope are rejected here to preserve the C++ no-redefinition rule.
pub(crate) fn bind_definitions(
    arena: &mut TreeArena,
    mut defs: TreeId,
    env: &mut Environment,
) -> Result<(), EvalError> {
    while !arena.is_nil(defs) {
        let def = arena
            .hd(defs)
            .ok_or(EvalError::MalformedDefinitionNode { node: defs })?;
        let (name, args, value) = decode_definition(arena, def)?;
        let bound = if arena.is_nil(args) {
            value
        } else {
            build_abstr_from_parser_args(arena, args, value)?
        };
        // Intern the name to get a SymId. This is the bind path — intern_symbol is correct.
        let sym = arena.intern_symbol(&name);
        let captured = EvalValue::Closure(ClosureValue {
            expr: bound,
            env: env.clone(),
        });
        // C++ parity: addLayerDef checks for conflicting redefinition within the current layer.
        // Identical bindings (same TreeId = same hash-consed expression) are silently accepted.
        // Conflicting bindings (different TreeId) are an error.
        // Parent-scope shadowing is allowed and is NOT checked here.
        if let Some(existing) = env.lookup_local_value(sym) {
            if existing != captured {
                return Err(EvalError::RedefinedSymbol {
                    symbol: name,
                    first_def: existing.display_tree(),
                    second_def: captured.display_tree(),
                });
            }
            // existing == bound: identical redefinition — silently skip (C++ parity)
        } else {
            env.bind_value(sym, captured);
        }
        defs = arena
            .tl(defs)
            .ok_or(EvalError::MalformedDefinitionNode { node: defs })?;
    }
    Ok(())
}

/// Rewrites every captured environment reachable from `value` from `source_env`
/// to `copy_env`.
///
/// This helper exists for `boxModifLocalDef` parity: copied environments cannot
/// just duplicate direct bindings, they must also retarget any nested closures
/// so future lookups see the rewritten layer chain instead of the original.
pub(crate) fn rewrite_captured_env(
    value: EvalValue,
    old_env: &Environment,
    new_env: &Environment,
) -> EvalValue {
    match value {
        EvalValue::Box(id) => EvalValue::Box(id),
        EvalValue::Closure(closure) => {
            if closure.env.same_identity(old_env) {
                EvalValue::Closure(ClosureValue {
                    expr: closure.expr,
                    env: new_env.clone(),
                })
            } else {
                EvalValue::Closure(closure)
            }
        }
        EvalValue::PatternMatcher(pm) => EvalValue::PatternMatcher(pm),
    }
}

/// Creates a modified copy of one captured environment layer and replaces selected definitions.
///
/// Source provenance (C++):
/// - `compiler/evaluate/environment.cpp`
/// - `copyEnvReplaceDefs`
/// - `updateClosures`
///
/// The copied layer reuses the same parent stack as `source_env`, rewires any enclosed closure
/// that previously captured `source_env` so it now captures the copied layer, then appends the
/// replacement definitions as closures captured in `current_env`.
/// Clones the visible environment chain and replaces selected definitions.
///
/// The copy preserves lexical parent ordering while rebasing closure captures
/// onto the duplicated chain. This is the Rust equivalent of the C++
/// `copyEnvReplaceDefs(...)` family used by modifier definitions.
pub(crate) fn copy_env_replace_defs(
    arena: &mut TreeArena,
    source_env: &Environment,
    mut defs: TreeId,
    current_env: &Environment,
) -> Result<Environment, EvalError> {
    let (parent, _barrier, bindings) = source_env.layer_snapshot();
    let mut copy_env = source_env.spawn_child_with_parent(parent, false);

    for (sym, value) in bindings {
        copy_env.bind_value(sym, rewrite_captured_env(value, source_env, &copy_env));
    }

    while !arena.is_nil(defs) {
        let def = arena
            .hd(defs)
            .ok_or(EvalError::MalformedDefinitionNode { node: defs })?;
        let (name, args, value) = decode_definition(arena, def)?;
        let bound = if arena.is_nil(args) {
            value
        } else {
            build_abstr_from_parser_args(arena, args, value)?
        };
        let sym = arena.intern_symbol(&name);
        copy_env.bind_value(
            sym,
            EvalValue::Closure(ClosureValue {
                expr: bound,
                env: current_env.clone(),
            }),
        );
        defs = arena
            .tl(defs)
            .ok_or(EvalError::MalformedDefinitionNode { node: defs })?;
    }

    Ok(copy_env)
}

/// Decodes one parser definition node into `(name, args, expr)`.
pub(crate) fn decode_definition(
    arena: &TreeArena,
    def: TreeId,
) -> Result<(String, TreeId, TreeId), EvalError> {
    let name_node = arena
        .hd(def)
        .ok_or(EvalError::MalformedDefinitionNode { node: def })?;
    let payload = arena
        .tl(def)
        .ok_or(EvalError::MalformedDefinitionNode { node: def })?;
    let args = arena
        .hd(payload)
        .ok_or(EvalError::MalformedDefinitionNode { node: def })?;
    let expr = arena
        .tl(payload)
        .ok_or(EvalError::MalformedDefinitionNode { node: def })?;

    let name = match match_box(arena, name_node) {
        BoxMatch::Ident(s) => s.to_owned(),
        _ => match arena.kind(name_node) {
            Some(NodeKind::Symbol(s)) => s.as_ref().to_owned(),
            _ => {
                return Err(EvalError::MalformedDefinitionNode { node: def });
            }
        },
    };

    Ok((name, args, expr))
}

/// Extracts top-level definition names in deterministic order for diagnostics.
///
/// Names are sorted and deduplicated so diagnostic snapshots remain stable.
pub(crate) fn top_level_definition_names(
    arena: &TreeArena,
    mut defs: TreeId,
) -> Result<Vec<String>, EvalError> {
    let mut names = Vec::new();
    while !arena.is_nil(defs) {
        let def = arena
            .hd(defs)
            .ok_or(EvalError::MalformedDefinitionNode { node: defs })?;
        let (name, _args, _expr) = decode_definition(arena, def)?;
        names.push(name);
        defs = arena
            .tl(defs)
            .ok_or(EvalError::MalformedDefinitionNode { node: defs })?;
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Returns identifier text for one `BOXIDENT` node.
pub(crate) fn ident_name(arena: &TreeArena, id: TreeId) -> Result<String, EvalError> {
    match match_box(arena, id) {
        BoxMatch::Ident(name) => Ok(name.to_owned()),
        _ => Err(EvalError::NonIdentifierParameter { node: id }),
    }
}

/// Builds a curried lambda chain from a reversed parser argument list.
///
/// The C++ `buildBoxAbstr` iterates over a reversed parameter list and wraps
/// each head around the current body.  The Rust port mirrors this exactly:
/// for `[p2, p1]` (reversed) and body `B`, the result is `λp1.(λp2.B)`.
pub(crate) fn build_abstr_from_parser_args(
    arena: &mut TreeArena,
    mut args: TreeId,
    body: TreeId,
) -> Result<TreeId, EvalError> {
    // C++ parity (`buildBoxAbstr`): parser param lists are reversed, and each
    // head wraps the current body before recursing on tail.
    let mut out = body;
    while !arena.is_nil(args) {
        let head = arena
            .hd(args)
            .ok_or(EvalError::MalformedListNode { node: args })?;
        out = {
            let mut b = BoxBuilder::new(arena);
            b.abstr(head, out)
        };
        args = arena
            .tl(args)
            .ok_or(EvalError::MalformedListNode { node: args })?;
    }
    Ok(out)
}
