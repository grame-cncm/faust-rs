//! Boxes in normal form: trees the evaluator has already produced and whose
//! evaluation is the identity in every environment, so that the evaluator can
//! return them without walking them.
//!
//! # Why
//!
//! The evaluator memoizes `eval(expr, env)` per environment layer, as the C++
//! compiler does with `getEvalProperty`, but its layers are never shared: every
//! pattern-matcher application or iteration step opens a fresh layer. A variable
//! bound to an already evaluated tree (the tail `xs` of a list in
//! `take(n, (x, xs)) = take(n - 1, xs)`) is re-evaluated at every recursion
//! level, in a layer the cache has never seen, and the walk costs the size of
//! the tree: `ba.take` over a list of `n` elements was cubic (0.7 s at 120
//! elements, 4 s at 240, hours at 1090, where the C++ compiler, whose layers
//! are hash-consed, takes 7 s). A tree in normal form needs no walk at all.
//!
//! # What is in normal form
//!
//! A tree built only of numbers, wires, cuts, primitives, waveforms, user
//! interface elements whose label carries no `%` variable and whose numeric
//! parameters are in normal form, and the compositions `,` `:` `~` `<:` `:>`
//! of such trees. Everything that evaluation can rewrite is excluded:
//! identifiers, applications, abstractions, closures, `case`, `with`, `letrec`,
//! iterations, accesses, components and libraries, metadata, routes, foreign
//! declarations, `ondemand` and its kin, the AD primitives, and anything the
//! matcher does not know. The verdict is memoized per tree, so a hash-consed
//! list of `n` elements is classified once, in `O(n)`, and every later look-up
//! of it or of its tails is `O(1)`.
//!
//! The fast path applies to trees the evaluator has produced (`eval_value`
//! records every box it returns in `LoopDetector::evaluated_boxes`), never to a
//! source tree of the same shape: `(1, 2) : +` written by the user still has to
//! fold to `3`, and a deep source expression still has to meet the nesting
//! budget. Idempotence of evaluation on produced trees is what the fast path
//! relies on:
//! numbers and primitives evaluate to themselves; the compositions rebuild the
//! same node from the same children, except a `:` whose left side is a
//! numerical tuple, which folds to a number when evaluated (an application of
//! a primitive to constants produces it unfolded, and counts on the next
//! evaluation to fold it), and which is therefore excluded; a widget
//! with an evaluated label and numeric parameters is rebuilt identically. A
//! label with an unresolved `%name` is the one widget case that could change
//! with the environment, and is excluded by the `%` test.

use boxes::{BoxMatch, match_box};
use tlib::{TreeArena, TreeId};

use crate::label::label_node_text;
use crate::simplify::is_numerical_tuple_box;

/// What one node contributes to the verdict, before its children are known.
enum Shape {
    /// The verdict is known from the node alone.
    Leaf(bool),
    /// The node is in normal form if and only if these children are.
    All(Vec<TreeId>),
}

fn shape(arena: &TreeArena, id: TreeId) -> Shape {
    let label_ok = |label: TreeId| label_node_text(arena, label).is_some_and(|s| !s.contains('%'));
    match match_box(arena, id) {
        BoxMatch::Int(_) | BoxMatch::Real(_) | BoxMatch::Wire | BoxMatch::Cut => Shape::Leaf(true),
        BoxMatch::Add
        | BoxMatch::Sub
        | BoxMatch::Mul
        | BoxMatch::Div
        | BoxMatch::Rem
        | BoxMatch::And
        | BoxMatch::Or
        | BoxMatch::Xor
        | BoxMatch::Lsh
        | BoxMatch::LRsh
        | BoxMatch::Rsh
        | BoxMatch::Lt
        | BoxMatch::Le
        | BoxMatch::Gt
        | BoxMatch::Ge
        | BoxMatch::Eq
        | BoxMatch::Ne
        | BoxMatch::Pow
        | BoxMatch::Acos
        | BoxMatch::Asin
        | BoxMatch::Atan
        | BoxMatch::Atan2
        | BoxMatch::Cos
        | BoxMatch::Sin
        | BoxMatch::Tan
        | BoxMatch::Exp
        | BoxMatch::Exp10
        | BoxMatch::Log
        | BoxMatch::Log10
        | BoxMatch::Sqrt
        | BoxMatch::Abs
        | BoxMatch::Fmod
        | BoxMatch::Remainder
        | BoxMatch::Floor
        | BoxMatch::Ceil
        | BoxMatch::Rint
        | BoxMatch::Round
        | BoxMatch::Delay
        | BoxMatch::Delay1
        | BoxMatch::Min
        | BoxMatch::Max
        | BoxMatch::Prefix
        | BoxMatch::IntCast
        | BoxMatch::FloatCast
        | BoxMatch::ReadOnlyTable
        | BoxMatch::WriteReadTable
        | BoxMatch::Select2
        | BoxMatch::Select3
        | BoxMatch::AssertBounds
        | BoxMatch::Lowest
        | BoxMatch::Highest
        | BoxMatch::Attach
        | BoxMatch::Enable
        | BoxMatch::Control => Shape::Leaf(true),
        BoxMatch::Waveform(_) => Shape::Leaf(true),
        // A `:` whose left side is a numerical tuple may fold to a number
        // (`(20 : log)`, as an application of a primitive to constants
        // produces it, folds to `2.9957` when it is evaluated again): not a
        // fixed point of evaluation.
        BoxMatch::Seq(a, _) if is_numerical_tuple_box(arena, a) => Shape::Leaf(false),
        BoxMatch::Seq(a, b)
        | BoxMatch::Par(a, b)
        | BoxMatch::Rec(a, b)
        | BoxMatch::Split(a, b)
        | BoxMatch::Merge(a, b) => Shape::All(vec![a, b]),
        BoxMatch::Button(label) | BoxMatch::Checkbox(label) => Shape::Leaf(label_ok(label)),
        BoxMatch::VSlider(label, cur, min, max, step)
        | BoxMatch::HSlider(label, cur, min, max, step)
        | BoxMatch::NumEntry(label, cur, min, max, step) => {
            if label_ok(label) {
                Shape::All(vec![cur, min, max, step])
            } else {
                Shape::Leaf(false)
            }
        }
        BoxMatch::VBargraph(label, min, max) | BoxMatch::HBargraph(label, min, max) => {
            if label_ok(label) {
                Shape::All(vec![min, max])
            } else {
                Shape::Leaf(false)
            }
        }
        BoxMatch::VGroup(label, body)
        | BoxMatch::HGroup(label, body)
        | BoxMatch::TGroup(label, body) => {
            if label_ok(label) {
                Shape::All(vec![body])
            } else {
                Shape::Leaf(false)
            }
        }
        _ => Shape::Leaf(false),
    }
}

/// Whether `root` is in normal form (see the module documentation). The verdict
/// of every visited node is memoized in `cache`; the walk is iterative, a list
/// of a thousand elements being a tree a thousand deep.
pub(crate) fn is_normal_form(
    arena: &TreeArena,
    root: TreeId,
    cache: &mut ahash::HashMap<TreeId, bool>,
) -> bool {
    if let Some(&verdict) = cache.get(&root) {
        return verdict;
    }
    // (node, children already pushed)
    let mut stack: Vec<(TreeId, bool)> = vec![(root, false)];
    while let Some((id, expanded)) = stack.pop() {
        if cache.contains_key(&id) {
            continue;
        }
        match shape(arena, id) {
            Shape::Leaf(verdict) => {
                cache.insert(id, verdict);
            }
            Shape::All(children) => {
                if expanded {
                    let verdict = children
                        .iter()
                        .all(|child| cache.get(child).copied().unwrap_or(false));
                    cache.insert(id, verdict);
                } else {
                    stack.push((id, true));
                    for child in children {
                        if !cache.contains_key(&child) {
                            stack.push((child, false));
                        }
                    }
                }
            }
        }
    }
    cache.get(&root).copied().unwrap_or(false)
}
