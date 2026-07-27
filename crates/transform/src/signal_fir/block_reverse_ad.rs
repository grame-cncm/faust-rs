//! Signal-level helpers for `SigBlockReverseAD` FIR lowering (Phase B3/B4/B5/B6).
//!
//! This module provides pure signal-tree utilities used by `module/` when
//! lowering `SigBlockReverseAD` carriers into FIR reverse-mode adjoint code.
//! All functions here are `pub(super)` and are called exclusively from
//! `module/`; the FIR-emitting logic itself lives there as `&mut self`
//! methods on `SignalToFirLower`.
//!
//! # Scope
//!
//! **Phase B3** supports the TBPTT(BS, BS) backward sweep without a tape:
//! body signals must be *trivially reverse-evaluable* (i.e. their forward
//! value can be correctly reconstructed in the reverse sample loop without
//! storing a tape).  Non-trivially-evaluable signals (e.g. `Delay1(Delay1(x))`)
//! cause a `SignalFirError::UnsupportedSignalNode` at lowering time.
//!
//! **Phase B4** extends B3 with a per-sample forward tape.  When a body
//! contains a node that is **not** trivially reverse-evaluable but is needed
//! as a multiplier or divisor in a backward rule (e.g. `Delay1(x)` inside a
//! `Mul`), [`collect_tape_needed_values`] identifies which signal values must
//! be recorded during the forward loop.  `module/` then:
//! 1. Declares a `fBraTapeN: Array(real_ty, MAX_BRA_TAPE_BLOCK_SIZE)` struct
//!    field for each tape-needed signal.
//! 2. Stores each value into the tape in the `immediate` sample phase (before
//!    delay-register updates in `post_output`) so the pre-step delay state is
//!    captured correctly.
//! 3. In the reverse loop, loads from the tape via `load_bra_fwd_value`
//!    instead of calling `lower_signal` — which would read incorrect state.
//!
//! The word “reverse loop” in this helper module names the mathematical sweep
//! direction, not necessarily a distinct generated C++ loop.  The FIR lowerer
//! can place the sweep in two schedules:
//!
//! - **Split public-output schedule**: a public gradient projection
//!   `Proj(M+j, BlockReverseAD)` is classified as reverse-time, so `compute()`
//!   has a forward loop for primals and a separate loop from `count-1` down to
//!   `0` for gradients.
//! - **Inline adaptive schedule**: the gradient projection is consumed inside
//!   a forward-time expression, often a recursive parameter update.  The public
//!   output may then be only the outer primal recursion.  In that case
//!   `module/` emits the BRA adjoint statements into the current forward
//!   loop slice; the generated C++ may contain no separate backward loop even
//!   though these helpers still build the same local transpose rules.
//!   This is not the LTI/IIR `ReverseTimeRec` fast path: no linearity proof is
//!   being used here, and the derivative remains the general block-tape BRA
//!   program.
//!
//! In both schedules the generated statements are block-local.  BRA carry
//! fields are reset at `compute()` entry; tape arrays, when present, are
//! scratch buffers whose slots `0..count-1` are written before the matching
//! slots are read by the sweep.
//!
//! **Phase B5** extends B4 with full backward rule coverage:
//! - `Delay(c, x)`: adjoint propagated via a circular carry buffer
//!   `fBraDelayCarryN: Array(real_ty, c)` declared as a struct field.
//!   At reverse step `n`, slot `n % c` carries `adj[Delay(c,x)][n+c]` written
//!   by the step `c` iterations earlier in the reverse loop.
//! - `Prefix(init, x)`: scalar carry (same mechanism as `Delay1`) plus a
//!   boundary term at sample 0 that propagates the cotangent to `init`.
//! - Smooth unary ops: `Tan`, `Asin`, `Acos`, `Atan`, `Log10`, `Abs`.
//! - Binary ops: `Pow`, `Atan2`, `Min`/`Max` (subgradient via `Select2`).
//! - Piecewise-constant / discrete ops: `Floor`, `Ceil`, `Rint`, `Round`,
//!   comparison `BinOp`s, bitwise `BinOp`s — all contribute zero gradient.
//!
//! **Phase B6** extends B5 with recursive circuits (`+~` feedback loops):
//! - After `de_bruijn_to_sym`, a Faust recursive circuit produces
//!   `Proj(slot, SYMREC(var, body_list))` as the top-level output and
//!   `Proj(slot, SYMREF(var))` as the back-reference inside the body (always
//!   wrapped in a `Delay1`).
//! - [`collect_bra_postorder`] recurses into the SYMREC body for SYMREC nodes
//!   and stops at SYMREF nodes (cycle guard).
//! - `module/` implements the TBPTT adjoint for recursive outputs:
//!   `adj[y[slot][n]] = cotangent[slot][n] + carry_from_step_n+1`, where the
//!   carry is pre-loaded from the `Delay1(Proj(slot, SYMREF))` carry struct
//!   field **before** the reverse-postorder walk (step 3a of
//!   `ensure_bra_backward_sweep`).  The `Delay1` arm then only stores the new
//!   carry (no redundant load), and the SYMREF Proj arm does nothing.
//!
//! # Trivial reverse-evaluability
//!
//! A signal is *trivially reverse-evaluable* if it is either:
//! - a leaf (`Real`, `Int`, `Input`, `HSlider`, `VSlider`, `NumEntry`,
//!   `Button`, `Checkbox`), or
//! - a pure stateless combinator whose children are all trivially
//!   reverse-evaluable (`BinOp`, `IntCast`, `FloatCast`, unary math).
//!
//! `Delay1`, `Delay`, `Prefix`, and recursive carriers are **not** trivially
//! re-evaluable: their forward value at a given sample depends on prior state
//! that is not available in the reverse loop without a tape.

use std::collections::HashSet;

use signals::{BinOp, SigId, SigMatch, match_sig};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};

/// Returns `true` if `sig` can be correctly re-evaluated in the **reverse**
/// sample loop without accessing a recorded tape.
#[allow(dead_code)]
///
/// A trivially reverse-evaluable signal is either a stateless leaf (constant,
/// audio input, UI control) or a pure stateless combinator of such nodes.
///
/// `Delay1`, `Delay`, `Prefix`, and recursive carriers return `false`:
/// their forward value depends on prior state that is absent in the reverse
/// loop unless a tape is recorded (Phase B4).
pub(super) fn is_trivially_reverse_evaluable(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::Real(_) | SigMatch::Int(_) => true,
        SigMatch::Input(_) => true,
        SigMatch::HSlider(_) | SigMatch::VSlider(_) | SigMatch::NumEntry(_) => true,
        SigMatch::Button(_) | SigMatch::Checkbox(_) => true,
        // Foreign constants (e.g. `ma.SR = fconstant(int fSampleRate, <math.h>)`) and
        // foreign variables are compile-time or run-time external scalars that are
        // neither stateful nor differentiable w.r.t. any Faust seed.
        SigMatch::FConst(..) | SigMatch::FVar(..) => true,
        SigMatch::IntCast(x) | SigMatch::FloatCast(x) | SigMatch::BitCast(x) => {
            is_trivially_reverse_evaluable(arena, x)
        }
        SigMatch::BinOp(_, lhs, rhs) => {
            is_trivially_reverse_evaluable(arena, lhs) && is_trivially_reverse_evaluable(arena, rhs)
        }
        SigMatch::Sin(x)
        | SigMatch::Cos(x)
        | SigMatch::Tan(x)
        | SigMatch::Asin(x)
        | SigMatch::Acos(x)
        | SigMatch::Atan(x)
        | SigMatch::Exp(x)
        | SigMatch::Exp10(x)
        | SigMatch::Log(x)
        | SigMatch::Log10(x)
        | SigMatch::Sqrt(x)
        | SigMatch::Abs(x)
        | SigMatch::Floor(x)
        | SigMatch::Ceil(x)
        | SigMatch::Rint(x)
        | SigMatch::Round(x) => is_trivially_reverse_evaluable(arena, x),
        _ => false,
    }
}

/// Performs a DFS postorder traversal of the signal sub-tree rooted at `root`.
///
/// Each signal ID is visited at most once; `visited` guards against re-entry
/// in DAG-shared patterns (e.g. `x * x` where `x` is a shared leaf).
///
/// The traversal is intentionally shallow at `Delay1` children: the child
/// signal `x` in `Delay1(x)` **is** included in the postorder because the
/// backward adjoint for `x` at the previous reverse step must be tracked, but
/// the traversal stops at any node kind not covered by the match below (an
/// unsupported kind will be caught with a proper error in
/// [`crate::signal_fir::module::SignalToFirLower::propagate_bra_adj`]).
pub(super) fn collect_bra_postorder(
    arena: &TreeArena,
    root: SigId,
    visited: &mut HashSet<SigId>,
    order: &mut Vec<SigId>,
) {
    if !visited.insert(root) {
        return;
    }
    match match_sig(arena, root) {
        // Leaves — no children.
        SigMatch::Real(_)
        | SigMatch::Int(_)
        | SigMatch::Input(_)
        | SigMatch::HSlider(_)
        | SigMatch::VSlider(_)
        | SigMatch::NumEntry(_)
        | SigMatch::Button(_)
        | SigMatch::Checkbox(_)
        // Foreign constants/variables (e.g. `fSampleRate` via `ma.SR`): leaf nodes with
        // no differentiable children.  Their gradient contribution is always zero.
        | SigMatch::FConst(..)
        | SigMatch::FVar(..) => {}
        // Unary casts and unary math — one child.
        SigMatch::IntCast(x)
        | SigMatch::FloatCast(x)
        | SigMatch::BitCast(x)
        | SigMatch::Sin(x)
        | SigMatch::Cos(x)
        | SigMatch::Tan(x)
        | SigMatch::Asin(x)
        | SigMatch::Acos(x)
        | SigMatch::Atan(x)
        | SigMatch::Exp(x)
        | SigMatch::Exp10(x)
        | SigMatch::Log(x)
        | SigMatch::Log10(x)
        | SigMatch::Sqrt(x)
        | SigMatch::Abs(x)
        | SigMatch::Floor(x)
        | SigMatch::Ceil(x)
        | SigMatch::Rint(x)
        | SigMatch::Round(x) => {
            collect_bra_postorder(arena, x, visited, order);
        }
        // Binary operations — two children.
        SigMatch::BinOp(_, lhs, rhs)
        | SigMatch::Pow(lhs, rhs)
        | SigMatch::Atan2(lhs, rhs)
        | SigMatch::Min(lhs, rhs)
        | SigMatch::Max(lhs, rhs) => {
            collect_bra_postorder(arena, lhs, visited, order);
            collect_bra_postorder(arena, rhs, visited, order);
        }
        // Delay1: recurse into the value child so its adjoint can be tracked.
        SigMatch::Delay1(x) => {
            collect_bra_postorder(arena, x, visited, order);
        }
        // Delay(sig, amount): recurse into the delayed signal.
        // The amount is not differentiated (delay length is discrete).
        SigMatch::Delay(sig, _amount) => {
            collect_bra_postorder(arena, sig, visited, order);
        }
        // Prefix(init, sig): both init and sig contribute to the adjoint.
        SigMatch::Prefix(init, sig) => {
            collect_bra_postorder(arena, init, visited, order);
            collect_bra_postorder(arena, sig, visited, order);
        }
        // Attach(value, effect) is transparent to differentiation. The
        // attached branch is forced by the forward lowering, but it does not
        // contribute to the adjoint and must not be replayed in the backward
        // sweep.
        SigMatch::Attach(value, _effect) => {
            collect_bra_postorder(arena, value, visited, order);
        }
        // Proj(slot, Rec(bodies)): recurse into the corresponding body expression.
        //
        // The body expression contains `Delay1(Proj(slot, group))` which
        // back-references this same Proj node; the `visited` guard prevents
        // infinite recursion — when the recursion body tries to visit the Proj
        // again, `visited.insert` returns `false` and the traversal stops.
        //
        // After this arm, `order.push(root)` below places `Proj(slot, group)`
        // *after* all its body nodes, giving the correct postorder: body
        // sub-expressions are processed before the Proj itself in the backward
        // reverse-postorder walk.
        SigMatch::Proj(slot, group_sig) => {
            // After `de_bruijn_to_sym` two Proj forms appear:
            //
            // • `Proj(slot, SYMREC(var, body_list))` — the top-level output of a
            //   recursive group.  Recurse into `body_list[slot]` so that the body
            //   sub-expressions are postorder-visited before the Proj itself.
            //   The `visited` guard prevents infinite recursion when the body
            //   contains back-references through `Proj(slot, SYMREF(var))`.
            //
            // • `Proj(slot, SYMREF(var))` — a back-reference inside the recursive
            //   body (the "self" reference for the recurrence).  This is a cycle;
            //   we stop traversal here.  The carry mechanism in
            //   `ensure_bra_backward_sweep` (pre-scan step) loads the struct carry
            //   variable for the corresponding `Delay1(Proj(slot, SYMREF))` node
            //   and seeds it into `adj[body_sigs[slot]]` before the reverse-
            //   postorder walk, so no further traversal is needed.
            if let Some((_var, body_list)) = match_sym_rec(arena, group_sig) {
                // SYMREC top-level output: recurse into the body expression.
                if let Some(bodies) = list_to_vec(arena, body_list) {
                    let slot_usize = usize::try_from(slot).unwrap_or(usize::MAX);
                    if let Some(&body) = bodies.get(slot_usize) {
                        collect_bra_postorder(arena, body, visited, order);
                    }
                }
            } else if match_sym_ref(arena, group_sig).is_some() {
                // SYMREF back-reference: cycle — stop traversal.
                // The adjoint carry is handled via the pre-scan in the FIR lowering.
            }
            // For other Proj kinds (ReverseTimeRec, BlockReverseAD nested) the
            // backward pass will return an UnsupportedSignalNode error when it
            // encounters this node.
        }
        // Any other kind: include in postorder; the backward pass will
        // return an unsupported-node error when it encounters it.
        _ => {}
    }
    order.push(root);
}

/// Collects the set of signals whose **forward** value must be stored on a
/// tape during the forward sample loop so that the backward sweep can load
/// them instead of re-evaluating `lower_signal` in reverse order.
///
/// A signal `v` is *tape-needed* when the backward differentiation rule for
/// some node in `postorder` must multiply or divide by `v`'s forward value
/// *and* `v` is **not** trivially reverse-evaluable (i.e.
/// [`is_trivially_reverse_evaluable`] returns `false` for `v`).
///
/// Concretely:
///
/// | Node kind                         | Tape-needed values                         |
/// |-----------------------------------|--------------------------------------------|
/// | `Mul(lhs, rhs)` / `Div(lhs, rhs)`| `lhs` if not trivial; `rhs` if not trivial |
/// | `Min(lhs, rhs)` / `Max(lhs, rhs)`| `lhs` if not trivial; `rhs` if not trivial |
/// | `Pow(x, y)`                       | `x`, `y`, and `sig` (= `x^y`) if either not trivial |
/// | `Atan2(y, x)`                     | `lhs` if not trivial; `rhs` if not trivial |
/// | `Sin(x)` / `Cos(x)` / `Tan(x)`   | `x` if not trivial                         |
/// | `Asin(x)` / `Acos(x)` / `Atan(x)`| `x` if not trivial                         |
/// | `Log(x)` / `Log10(x)` / `Abs(x)` | `x` if not trivial                         |
/// | `Exp(x)`                          | `sig` (= `exp(x)`) if `x` not trivial      |
/// | `Sqrt(x)`                         | `sig` (= `sqrt(x)`) if `x` not trivial     |
///
/// For `Exp` and `Sqrt` the backward rule reuses the node value itself
/// (`y_bar * val[sig]` and `y_bar / (2 * val[sig])` respectively), so `sig`
/// is taped rather than `x` when `x` is not trivially re-evaluable.
///
/// The returned set may be empty when all backward operands are trivially
/// re-evaluable (the common case for purely feedforward bodies).
pub(super) fn collect_tape_needed_values(arena: &TreeArena, postorder: &[SigId]) -> HashSet<SigId> {
    let mut needed = HashSet::new();
    for &sig in postorder {
        match match_sig(arena, sig) {
            SigMatch::BinOp(BinOp::Mul | BinOp::Div, lhs, rhs) => {
                if !is_trivially_reverse_evaluable(arena, lhs) {
                    needed.insert(lhs);
                }
                if !is_trivially_reverse_evaluable(arena, rhs) {
                    needed.insert(rhs);
                }
            }
            // Min/Max subgradient needs operand values for the indicator cond.
            SigMatch::Min(lhs, rhs) | SigMatch::Max(lhs, rhs) => {
                if !is_trivially_reverse_evaluable(arena, lhs) {
                    needed.insert(lhs);
                }
                if !is_trivially_reverse_evaluable(arena, rhs) {
                    needed.insert(rhs);
                }
            }
            // Pow(x, y): backward rule needs x, y, and val[sig] = x^y.
            SigMatch::Pow(lhs, rhs) => {
                if !is_trivially_reverse_evaluable(arena, lhs) {
                    needed.insert(lhs);
                }
                if !is_trivially_reverse_evaluable(arena, rhs) {
                    needed.insert(rhs);
                }
                if !is_trivially_reverse_evaluable(arena, lhs)
                    || !is_trivially_reverse_evaluable(arena, rhs)
                {
                    needed.insert(sig); // val[sig] = x^y
                }
            }
            // Atan2(y, x): backward rule needs both operand values.
            SigMatch::Atan2(lhs, rhs) => {
                if !is_trivially_reverse_evaluable(arena, lhs) {
                    needed.insert(lhs);
                }
                if !is_trivially_reverse_evaluable(arena, rhs) {
                    needed.insert(rhs);
                }
            }
            SigMatch::Sin(x)
            | SigMatch::Cos(x)
            | SigMatch::Tan(x)
            | SigMatch::Log(x)
            | SigMatch::Log10(x)
            | SigMatch::Asin(x)
            | SigMatch::Acos(x)
            | SigMatch::Atan(x)
            | SigMatch::Abs(x) => {
                if !is_trivially_reverse_evaluable(arena, x) {
                    needed.insert(x);
                }
            }
            SigMatch::Exp(x) | SigMatch::Exp10(x) | SigMatch::Sqrt(x)
                if !is_trivially_reverse_evaluable(arena, x) =>
            {
                // Backward rule uses val[sig], not x directly.
                needed.insert(sig);
            }
            _ => {}
        }
    }
    needed
}
