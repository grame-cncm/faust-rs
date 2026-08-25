//! One-sample compatibility classifier (plan provenance: execution-options
//! port §3.5, decision D2).
//!
//! `-os` removes the block boundary: `frame` processes exactly one sample and
//! receives no `count`. Operations whose current semantics are defined
//! *relative to the block* therefore have no one-sample meaning today:
//!
//! - `BlockReverseAD` carriers use block-scoped tape/carry/reset state, and
//!   their public gradient projections run in a second, reverse-order sample
//!   loop over the block;
//! - `ReverseTimeRec` carriers likewise denote reverse-time traversal of the
//!   block.
//!
//! Per D2, a program containing either carrier family is rejected under
//! `-os` with a typed diagnostic instead of inventing a persistent
//! one-sample reverse-AD meaning (which would need its own design decision).
//!
//! This classifier is deliberately **total presence detection** over the
//! prepared forest: unlike `classify_reverse_time_outputs` (which stops at
//! `SYMREC` boundaries because it decides *where the reverse loop goes*), it
//! walks through every child edge — an internal BRA lowered into the forward
//! loop still uses block-scoped tape state, so it must be detected wherever
//! it appears.

use std::collections::HashSet;

use signals::{SigId, SigMatch, match_sig};
use tlib::TreeArena;

/// Returns `true` when any node reachable from `signals` is a
/// `BlockReverseAD` or `ReverseTimeRec` carrier.
#[must_use]
pub(crate) fn contains_block_sensitive_operation(arena: &TreeArena, signals: &[SigId]) -> bool {
    let mut visited: HashSet<SigId> = HashSet::new();
    let mut stack: Vec<SigId> = signals.to_vec();
    while let Some(sig) = stack.pop() {
        if !visited.insert(sig) {
            continue;
        }
        if matches!(
            match_sig(arena, sig),
            SigMatch::BlockReverseAD { .. } | SigMatch::ReverseTimeRec(_)
        ) {
            return true;
        }
        if let Some(node) = arena.node(sig) {
            stack.extend(node.children.as_slice().iter().copied());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use signals::{BinOp, BlockRevPolicy, SigBuilder};

    #[test]
    fn plain_programs_are_one_sample_compatible() {
        let mut arena = TreeArena::new();
        let sig = {
            let mut b = SigBuilder::new(&mut arena);
            let x = b.input(0);
            let g = b.real(0.5);
            b.binop(BinOp::Mul, x, g)
        };
        assert!(!contains_block_sensitive_operation(&arena, &[sig]));
    }

    #[test]
    fn block_reverse_ad_is_detected_even_when_internal() {
        let mut arena = TreeArena::new();
        let out = {
            let mut b = SigBuilder::new(&mut arena);
            let x = b.real(2.0);
            let two = b.real(2.0);
            let body = b.binop(BinOp::Mul, two, x);
            let cot = b.real(1.0);
            let carrier = b.block_reverse_ad(&[body], &[x], &[cot], BlockRevPolicy::TapeFull);
            // Only the primal projection is public: the carrier is internal.
            let primal = b.proj(0, carrier);
            let one = b.real(1.0);
            b.binop(BinOp::Add, primal, one)
        };
        assert!(contains_block_sensitive_operation(&arena, &[out]));
    }

    #[test]
    fn reverse_time_rec_is_detected() {
        let mut arena = TreeArena::new();
        let out = {
            let mut b = SigBuilder::new(&mut arena);
            let x = b.input(0);
            let rec = b.reverse_time_rec(x);
            b.proj(0, rec)
        };
        assert!(contains_block_sensitive_operation(&arena, &[out]));
    }
}
