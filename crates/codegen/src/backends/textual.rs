//! Shared expression-layout primitives for textual backends.
//!
//! Textual targets commonly share the same question even when their concrete
//! syntax differs: does a nested infix expression need parentheses in one
//! operand position?  Keeping that decision here prevents each backend from
//! falling back to fully parenthesized output, which can exceed parser nesting
//! limits on large Faust graphs.
//!
//! # C++ source provenance
//!
//! [`infix_operand_needs_parentheses`] is the Rust adaptation of
//! `TextInstVisitor::{leftArgNeedsParentheses,rightArgNeedsParentheses}` in
//! `compiler/generator/text_instructions.hh` at Faust C++ commit `8eebea429`.
//! [`c_like_fir_operator`] carries the precedence and associativity data from
//! `compiler/signals/binop.cpp` at the same commit.
//!
//! # Reuse contract
//!
//! The core decision accepts language-independent [`InfixOperator`] values.
//! A future textual backend with different precedence rules can therefore use
//! the same algorithm with its own descriptors.  Cmajor and other C-like
//! targets can use [`c_like_fir_operator`] directly.

use fir::FirBinOp;

/// Describes one target-language infix operator for parenthesis decisions.
///
/// Larger precedence values bind more tightly. `associative` permits a nested
/// occurrence of the exact same operator on the right to be flattened. The
/// conservative flag matches operators for which the Faust C++ text emitter
/// always parenthesizes binary operands to avoid ambiguous or warning-prone
/// source forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InfixOperator {
    precedence: u8,
    associative: bool,
    conservative: bool,
}

impl InfixOperator {
    /// Creates a target-language infix descriptor.
    #[must_use]
    pub(crate) const fn new(precedence: u8, associative: bool, conservative: bool) -> Self {
        Self {
            precedence,
            associative,
            conservative,
        }
    }
}

/// Identifies the operand position occupied by a nested infix expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperandSide {
    /// The nested expression is the parent's left operand.
    Left,
    /// The nested expression is the parent's right operand.
    Right,
}

/// Returns whether a nested infix operand requires parentheses.
///
/// `same_operator` must describe operator identity, not merely equal token
/// spelling: arithmetic and logical right shifts may share a token in a target
/// language while retaining distinct semantics. Left-associated trees at the
/// same precedence need no parentheses. A same-operator right child may also
/// omit them only when the parent is explicitly associative.
#[must_use]
pub(crate) const fn infix_operand_needs_parentheses(
    parent: InfixOperator,
    child: InfixOperator,
    side: OperandSide,
    same_operator: bool,
) -> bool {
    if parent.conservative || child.conservative {
        return true;
    }
    match side {
        OperandSide::Left => parent.precedence > child.precedence,
        OperandSide::Right => {
            !(parent.precedence < child.precedence || (same_operator && parent.associative))
        }
    }
}

/// Returns the C-like precedence policy used by Faust's textual FIR emitters.
///
/// The numeric values intentionally match `gBinOpTable` in the pinned C++
/// compiler. Addition and multiplication are the only operators marked
/// associative there. Comparisons, shifts, bitwise AND, and bitwise OR retain
/// the C++ emitter's conservative-parenthesis behavior.
#[must_use]
pub(crate) const fn c_like_fir_operator(op: FirBinOp) -> InfixOperator {
    match op {
        FirBinOp::Mul => InfixOperator::new(8, true, false),
        FirBinOp::Div | FirBinOp::Rem => InfixOperator::new(8, false, false),
        FirBinOp::Add => InfixOperator::new(7, true, false),
        FirBinOp::Sub => InfixOperator::new(7, false, false),
        FirBinOp::Lsh | FirBinOp::ARsh | FirBinOp::LRsh => InfixOperator::new(6, false, true),
        FirBinOp::Lt | FirBinOp::Le | FirBinOp::Gt | FirBinOp::Ge => {
            InfixOperator::new(5, false, true)
        }
        FirBinOp::Eq | FirBinOp::Ne => InfixOperator::new(4, false, true),
        FirBinOp::And => InfixOperator::new(3, false, true),
        FirBinOp::Xor => InfixOperator::new(2, false, false),
        FirBinOp::Or => InfixOperator::new(1, false, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs(parent: FirBinOp, child: FirBinOp, side: OperandSide) -> bool {
        infix_operand_needs_parentheses(
            c_like_fir_operator(parent),
            c_like_fir_operator(child),
            side,
            parent == child,
        )
    }

    #[test]
    fn c_like_precedence_preserves_required_grouping() {
        assert!(needs(FirBinOp::Mul, FirBinOp::Add, OperandSide::Left));
        assert!(needs(FirBinOp::Mul, FirBinOp::Add, OperandSide::Right));
        assert!(!needs(FirBinOp::Add, FirBinOp::Mul, OperandSide::Right));
        assert!(needs(FirBinOp::Sub, FirBinOp::Sub, OperandSide::Right));
        assert!(!needs(FirBinOp::Sub, FirBinOp::Sub, OperandSide::Left));
    }

    #[test]
    fn c_like_associative_right_chains_flatten() {
        assert!(!needs(FirBinOp::Add, FirBinOp::Add, OperandSide::Right));
        assert!(!needs(FirBinOp::Mul, FirBinOp::Mul, OperandSide::Right));
        assert!(needs(FirBinOp::Xor, FirBinOp::Xor, OperandSide::Right));
    }

    #[test]
    fn c_like_conservative_operators_parenthesize_binary_operands() {
        assert!(needs(FirBinOp::Lt, FirBinOp::Add, OperandSide::Left));
        assert!(needs(FirBinOp::Add, FirBinOp::Lt, OperandSide::Right));
        assert!(needs(FirBinOp::And, FirBinOp::And, OperandSide::Left));
        assert!(needs(FirBinOp::Or, FirBinOp::Or, OperandSide::Right));
    }
}
