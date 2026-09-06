//! Structural canonicalization rewrites applied during signal-forest preparation.
//!
//! This module contains the two pure tree-rewriting passes that normalize the
//! staged signal forest before type inference and FIR lowering:
//!
//! # `canonicalize_unary_rec_projections`
//!
//! Collapses any logical projection index onto slot `0` for single-slot symbolic
//! recursion groups. The classic C++ pipeline expresses the same intent through
//! `inlineDegenerateRecursions(...)`, but the fast-lane uses physical slot
//! vectors, so it normalizes earlier: any `proj(k, group)` where `group` has
//! exactly one body is rewritten to `proj(0, group)`.
//!
//! # Explicit Limitation
//! The unary-recursion canonicalization performed here is **not** a 1:1 port of
//! the C++ `inlineDegenerateRecursions(...)` pass.
//!
//! Concretely, the Rust fast-lane currently does **not**:
//! - build the recursive dependency graph,
//! - detect degenerate recursive projections through the C++ graph analysis,
//! - rewrite projections through `hasProjDefinition(...)` / `setProjDefinition(...)`,
//! - or inline recursive projection definitions under delays the way the C++
//!   rewrite rules do.
//!
//! Instead, this stage performs a smaller compatibility normalization tailored
//! to the FIR preparation contract: when a symbolic recursion group has one
//! physical slot, any logical projection index targeting that group is
//! canonicalized to slot `0`. This is sufficient for the current fast-lane
//! consumers, but it should not be mistaken for a full Rust port of the C++
//! degenerate-recursion elimination machinery.
//!
//! # `canonicalize_one_sample_delays`
//!
//! Rewrites every `Delay(x, 1)` node to the canonical `Delay1(x)` form so that
//! all downstream consumers (notably SIGGEN interpretation and recursion
//! lowering) see one stable unary-delay representation. `normalize` may legally
//! expose unary feedback as `Delay(x, 1)`; this pass collapses it before FIR
//! lowering.
//!
//! # `drop_clock_annotations`
//!
//! Rewrites every `Clocked(env, x)` to `x`. Used for table generators only:
//! a generator runs once at table-fill time, outside every clock domain, so
//! the domain annotations propagation put on its stateful primitives (when
//! the generator was written inside an `ondemand` body) carry nothing there,
//! and the generator sub-module has no domain table to decode them against.

use std::collections::{HashMap, HashSet};

use signals::{SigBuilder, SigId, SigMatch, match_sig};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref, sym_rec};

use super::SignalPrepareError;

/// Rewrites symbolic recursion projections so unary groups always use slot `0`.
///
/// C++ parity note: the classic pipeline can still carry logical projection
/// indices on degenerate recursive groups and resolves them through projection
/// identity later on. The fast-lane uses physical slot vectors, so it
/// canonicalizes `proj(k, group)` to `proj(0, group)` when `group` has one body.
///
/// This is intentionally a preparation-level normalization:
/// - downstream reduced typing only sees dense slot indices,
/// - FIR lowering can keep using `Vec<slot>` recursion carriers,
/// - the behavior stays stable even if different frontends expose the same
///   degenerate recursive projection through different logical indices.
///
/// Explicit limitation: the pass does not decide whether a projection is
/// degenerate from recursive dependency analysis. It only observes the already
/// materialized symbolic shape and canonicalizes projections targeting groups
/// whose body list has arity `1`.
pub(super) fn canonicalize_unary_rec_projections(
    arena: &mut TreeArena,
    root: SigId,
) -> Result<SigId, SignalPrepareError> {
    let mut unary_groups = HashMap::new();
    let mut visited = HashSet::new();
    collect_unary_sym_groups(arena, root, &mut unary_groups, &mut visited)?;
    let mut memo = HashMap::new();
    rewrite_unary_rec_projections(arena, root, &unary_groups, &mut memo)
}

/// Rebuilds the staged forest so literal one-sample delays use `Delay1`.
///
/// `normalize` may legally expose unary feedback as `Delay(x, 1)`. The
/// fast-lane keeps a narrower canonical form for one-sample delays so all
/// downstream consumers (notably SIGGEN interpretation and recursion lowering)
/// can reason on a single representation.
pub(super) fn canonicalize_one_sample_delays(
    arena: &mut TreeArena,
    outputs: &[SigId],
) -> Result<Vec<SigId>, SignalPrepareError> {
    let mut memo = HashMap::new();
    outputs
        .iter()
        .map(|&sig| rewrite_one_sample_delays(arena, sig, &mut memo))
        .collect()
}

/// Collects symbolic recursion variables whose body list has exactly one slot.
///
/// The collected set drives [`rewrite_unary_rec_projections`].  The traversal
/// uses a `visited` set to avoid re-visiting shared DAG nodes (signal forests
/// use structural sharing so the same sub-tree can appear under multiple
/// parents).  `cons`-encoded child lists are expanded explicitly rather than
/// treated as opaque signal nodes, because the arena represents list spines as
/// regular nodes and `match_sig` would not recurse into them otherwise.
fn collect_unary_sym_groups(
    arena: &TreeArena,
    sig: SigId,
    unary_groups: &mut HashMap<SigId, usize>,
    visited: &mut HashSet<SigId>,
) -> Result<(), SignalPrepareError> {
    if !visited.insert(sig) {
        return Ok(());
    }

    if let Some((var, body_list)) = match_sym_rec(arena, sig) {
        let bodies = list_to_vec(arena, body_list).ok_or_else(|| {
            SignalPrepareError::Typing("malformed symbolic recursion body list".to_owned())
        })?;
        if bodies.len() == 1 {
            unary_groups.insert(var, 1);
        }
        for body in bodies {
            collect_unary_sym_groups(arena, body, unary_groups, visited)?;
        }
        return Ok(());
    }

    if arena.is_nil(sig) {
        return Ok(());
    }

    let node = arena.node(sig).ok_or_else(|| {
        SignalPrepareError::Typing(format!(
            "missing node {} during unary recursion canonicalization",
            sig.as_u32()
        ))
    })?;
    for child in node.children.as_slice() {
        if arena.is_list(*child) {
            let items = list_to_vec(arena, *child).ok_or_else(|| {
                SignalPrepareError::Typing(
                    "malformed list during unary recursion canonicalization".to_owned(),
                )
            })?;
            for item in items {
                collect_unary_sym_groups(arena, item, unary_groups, visited)?;
            }
        } else {
            collect_unary_sym_groups(arena, *child, unary_groups, visited)?;
        }
    }
    Ok(())
}

/// Rebuilds one prepared signal/list tree with canonical unary recursion indices.
///
/// For every `proj(k, group)` where `group` resolves to a symbolic recursion
/// binder with one body, the rebuilt node becomes `proj(0, group)`. The pass is
/// memoized, so shared subtrees remain shared in the staging arena.
fn rewrite_unary_rec_projections(
    arena: &mut TreeArena,
    sig: SigId,
    unary_groups: &HashMap<SigId, usize>,
    memo: &mut HashMap<SigId, SigId>,
) -> Result<SigId, SignalPrepareError> {
    if let Some(mapped) = memo.get(&sig) {
        return Ok(*mapped);
    }

    let rewritten = if arena.is_nil(sig) {
        sig
    } else if arena.is_list(sig) {
        let head = arena.hd(sig).ok_or_else(|| {
            SignalPrepareError::Typing(
                "malformed list during unary recursion canonicalization".to_owned(),
            )
        })?;
        let tail = arena.tl(sig).ok_or_else(|| {
            SignalPrepareError::Typing(
                "malformed list during unary recursion canonicalization".to_owned(),
            )
        })?;
        let head = rewrite_unary_rec_projections(arena, head, unary_groups, memo)?;
        let tail = rewrite_unary_rec_projections(arena, tail, unary_groups, memo)?;
        arena.cons(head, tail)
    } else if let Some((var, body_list)) = match_sym_rec(arena, sig) {
        let body_list = rewrite_unary_rec_projections(arena, body_list, unary_groups, memo)?;
        sym_rec(arena, var, body_list)
    } else if let SigMatch::Proj(index, group) = match_sig(arena, sig) {
        let group = rewrite_unary_rec_projections(arena, group, unary_groups, memo)?;
        let canonical_index = if let Some(var) = match_sym_ref(arena, group) {
            if unary_groups.contains_key(&var) {
                0
            } else {
                index
            }
        } else if let Some((var, body_list)) = match_sym_rec(arena, group) {
            if unary_groups.contains_key(&var) {
                0
            } else {
                let bodies = list_to_vec(arena, body_list).ok_or_else(|| {
                    SignalPrepareError::Typing("malformed symbolic recursion body list".to_owned())
                })?;
                if bodies.len() == 1 { 0 } else { index }
            }
        } else {
            index
        };
        let mut b = SigBuilder::new(arena);
        b.proj(canonical_index, group)
    } else {
        let node = arena.node(sig).cloned().ok_or_else(|| {
            SignalPrepareError::Typing(format!(
                "missing node {} during unary recursion canonicalization",
                sig.as_u32()
            ))
        })?;
        let mut children = Vec::with_capacity(node.children.len());
        for child in node.children.as_slice() {
            children.push(rewrite_unary_rec_projections(
                arena,
                *child,
                unary_groups,
                memo,
            )?);
        }
        arena.intern(node.kind, &children)
    };

    memo.insert(sig, rewritten);
    Ok(rewritten)
}

/// Rebuilds the staged forest without its `Clocked(env, x)` annotations.
pub(super) fn drop_clock_annotations(
    arena: &mut TreeArena,
    outputs: &[SigId],
) -> Result<Vec<SigId>, SignalPrepareError> {
    let mut memo = HashMap::new();
    outputs
        .iter()
        .map(|&sig| rewrite_drop_clock_annotations(arena, sig, &mut memo))
        .collect()
}

fn rewrite_drop_clock_annotations(
    arena: &mut TreeArena,
    sig: SigId,
    memo: &mut HashMap<SigId, SigId>,
) -> Result<SigId, SignalPrepareError> {
    if let Some(mapped) = memo.get(&sig) {
        return Ok(*mapped);
    }
    let rewritten = if arena.is_nil(sig) {
        sig
    } else if arena.is_list(sig) {
        let head = arena.hd(sig).ok_or_else(|| {
            SignalPrepareError::Typing("malformed list while dropping clock annotations".to_owned())
        })?;
        let tail = arena.tl(sig).ok_or_else(|| {
            SignalPrepareError::Typing("malformed list while dropping clock annotations".to_owned())
        })?;
        let head = rewrite_drop_clock_annotations(arena, head, memo)?;
        let tail = rewrite_drop_clock_annotations(arena, tail, memo)?;
        arena.cons(head, tail)
    } else if let SigMatch::Clocked(_, inner) = match_sig(arena, sig) {
        rewrite_drop_clock_annotations(arena, inner, memo)?
    } else {
        let node = arena.node(sig).cloned().ok_or_else(|| {
            SignalPrepareError::Typing(format!(
                "missing node {} while dropping clock annotations",
                sig.as_u32()
            ))
        })?;
        let mut children = Vec::with_capacity(node.children.len());
        for child in node.children.as_slice() {
            children.push(rewrite_drop_clock_annotations(arena, *child, memo)?);
        }
        arena.intern(node.kind, &children)
    };
    memo.insert(sig, rewritten);
    Ok(rewritten)
}

fn rewrite_one_sample_delays(
    arena: &mut TreeArena,
    sig: SigId,
    memo: &mut HashMap<SigId, SigId>,
) -> Result<SigId, SignalPrepareError> {
    if let Some(mapped) = memo.get(&sig) {
        return Ok(*mapped);
    }

    let rewritten = if arena.is_nil(sig) {
        sig
    } else if arena.is_list(sig) {
        let head = arena.hd(sig).ok_or_else(|| {
            SignalPrepareError::Typing(
                "malformed list during one-sample delay canonicalization".to_owned(),
            )
        })?;
        let tail = arena.tl(sig).ok_or_else(|| {
            SignalPrepareError::Typing(
                "malformed list during one-sample delay canonicalization".to_owned(),
            )
        })?;
        let head = rewrite_one_sample_delays(arena, head, memo)?;
        let tail = rewrite_one_sample_delays(arena, tail, memo)?;
        arena.cons(head, tail)
    } else if let Some((var, body_list)) = match_sym_rec(arena, sig) {
        let body_list = rewrite_one_sample_delays(arena, body_list, memo)?;
        sym_rec(arena, var, body_list)
    } else if let SigMatch::Delay(value, amount) = match_sig(arena, sig) {
        let value = rewrite_one_sample_delays(arena, value, memo)?;
        let amount = rewrite_one_sample_delays(arena, amount, memo)?;
        if matches!(match_sig(arena, amount), SigMatch::Int(1)) {
            let mut b = SigBuilder::new(arena);
            b.delay1(value)
        } else {
            let mut b = SigBuilder::new(arena);
            b.delay(value, amount)
        }
    } else {
        let node = arena.node(sig).cloned().ok_or_else(|| {
            SignalPrepareError::Typing(format!(
                "missing node {} during one-sample delay canonicalization",
                sig.as_u32()
            ))
        })?;
        let mut children = Vec::with_capacity(node.children.len());
        for child in node.children.as_slice() {
            children.push(rewrite_one_sample_delays(arena, *child, memo)?);
        }
        arena.intern(node.kind, &children)
    };

    memo.insert(sig, rewritten);
    Ok(rewritten)
}
