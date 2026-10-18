//! Backend-neutral local AD rules for Signal nodes: the reverse-AD
//! contribution formulas, and the folding of literal zeros in derivative
//! expressions ([`DerivativeZeroFolder`]).
//!
//! `signals` normally owns the Signal IR representation and builder/matcher
//! surface. This module is the narrow exception where the crate also hosts
//! local, backend-neutral reverse-AD algebra shared by higher-level crates.
//! Keeping these rules here avoids drift between `propagate`'s symbolic RAD
//! pass and `transform`'s FIR `BlockReverseAD` lowering, both of which already
//! depend on `signals`.
//!
//! The boundary is intentionally small: this module classifies local math
//! rules and emits pure local contribution formulas through [`RadFormulaBuilder`].
//! It does not choose FIR storage, allocate block tapes, schedule reverse
//! loops, or inspect temporal/recursive structure. Those remain owned by the
//! caller.

use std::collections::HashMap;

use tlib::TreeArena;

use crate::{BinOp, SigBuilder, SigId, SigMatch, match_sig};

/// Unary math rules with a non-zero local reverse-AD contribution.
///
/// These variants describe the local transpose rule only. They do not imply
/// that a surrounding signal graph can be handled symbolically: temporal and
/// recursive structure is still classified by `propagate::reverse_ad` and may
/// require `BlockReverseAD`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RadUnaryMathRule {
    Sin,
    Cos,
    Tan,
    Exp,
    Exp10,
    Log,
    Log10,
    Sqrt,
    Abs,
    Acos,
    Asin,
    Atan,
}

/// Binary math node rules with local reverse-AD contributions.
///
/// The operand order follows the corresponding [`SigMatch`] variants. For
/// `Atan2(lhs, rhs)`, `lhs` is the numerator/y argument and `rhs` is the
/// denominator/x argument, matching Faust's `atan2(y, x)` convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RadBinaryMathRule {
    Pow,
    Min,
    Max,
    Atan2,
    Fmod,
    Remainder,
}

/// Reverse-AD classification for primitive `SIGBINOP` operators.
///
/// `Rem` is kept as a real-valued arithmetic rule here because the Signal RAD
/// model mirrors the FAD derivative convention for `rem/fmod/remainder`.
/// FIR `BlockReverseAD` may still refuse `Rem` when the prepared signal type
/// proves the operation is an integer/discrete recurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RadBinOpRule {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Discrete comparisons, bitwise operations, and shifts have zero
    /// contribution in the current RAD model.
    Zero,
}

/// Minimal expression builder needed by shared local RAD formulas.
///
/// Implementations decide how values are represented (`SigId`, `FirId`, ...),
/// how helper calls are registered, and how `select2` maps to the underlying IR.
/// The trait intentionally has no access to Signal nodes, FIR stores, tape
/// arrays, or error handling. Callers must load or reconstruct all primal
/// values before invoking the formula helpers.
pub trait RadFormulaBuilder {
    type Value: Copy;

    fn zero(&mut self) -> Self::Value;
    fn one(&mut self) -> Self::Value;
    fn ln_10(&mut self) -> Self::Value;
    fn add(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn sub(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn mul(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn div(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn pow(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn log(&mut self, x: Self::Value) -> Self::Value;
    fn cos(&mut self, x: Self::Value) -> Self::Value;
    fn sin(&mut self, x: Self::Value) -> Self::Value;
    fn sqrt(&mut self, x: Self::Value) -> Self::Value;
    fn abs(&mut self, x: Self::Value) -> Self::Value;
    fn floor(&mut self, x: Self::Value) -> Self::Value;
    fn round(&mut self, x: Self::Value) -> Self::Value;
    fn lt(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn le(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn gt(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    fn ge(&mut self, x: Self::Value, y: Self::Value) -> Self::Value;
    /// Selects `when_true` when `cond != 0`, otherwise `when_false`.
    ///
    /// This semantic wrapper hides the historical argument-order difference
    /// between Signal `select2` and FIR `select2`, so shared formulas can read
    /// as ordinary conditional expressions.
    fn select_nonzero(
        &mut self,
        cond: Self::Value,
        when_true: Self::Value,
        when_false: Self::Value,
    ) -> Self::Value;
}

impl RadFormulaBuilder for SigBuilder<'_> {
    type Value = SigId;

    fn zero(&mut self) -> Self::Value {
        SigBuilder::real(self, 0.0)
    }

    fn one(&mut self) -> Self::Value {
        SigBuilder::real(self, 1.0)
    }

    fn ln_10(&mut self) -> Self::Value {
        let ten = SigBuilder::real(self, 10.0);
        SigBuilder::log(self, ten)
    }

    fn add(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::add(self, x, y)
    }

    fn sub(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::sub(self, x, y)
    }

    fn mul(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::mul(self, x, y)
    }

    fn div(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::div(self, x, y)
    }

    fn pow(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::pow(self, x, y)
    }

    fn log(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::log(self, x)
    }

    fn cos(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::cos(self, x)
    }

    fn sin(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::sin(self, x)
    }

    fn sqrt(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::sqrt(self, x)
    }

    fn abs(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::abs(self, x)
    }

    fn floor(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::floor(self, x)
    }

    fn round(&mut self, x: Self::Value) -> Self::Value {
        SigBuilder::round(self, x)
    }

    fn lt(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::lt(self, x, y)
    }

    fn le(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::le(self, x, y)
    }

    fn gt(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::gt(self, x, y)
    }

    fn ge(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        SigBuilder::ge(self, x, y)
    }

    fn select_nonzero(
        &mut self,
        cond: Self::Value,
        when_true: Self::Value,
        when_false: Self::Value,
    ) -> Self::Value {
        SigBuilder::select2(self, cond, when_false, when_true)
    }
}

/// Returns the local contribution for one unary math rule.
///
/// `primal` is the forward value of the matched node. Builders that lower
/// block-reverse AD can pass a tape load for `exp`, `exp10`, `sqrt` and
/// `tan`, while symbolic builders can pass the reconstructed primal
/// expression (hash-consed onto the forward node).
///
/// Formulas:
///
/// - `sin(x)`: `y_bar * cos(x)`
/// - `cos(x)`: `-y_bar * sin(x)`
/// - `tan(x)`: `y_bar * (1 + tan(x)^2)` using `primal` (no `cos` call)
/// - `exp(x)`: `y_bar * exp(x)` using `primal`
/// - `log(x)`: `y_bar / x`
/// - `log10(x)`: `y_bar / (x * ln(10))`
/// - `sqrt(x)`: `y_bar / (2 * sqrt(x))` using `primal`
/// - `abs(x)`: `y_bar * x / abs(x)` using `primal`
/// - `acos/asin/atan`: standard inverse-trig local transpose rules.
#[must_use]
pub fn rad_unary_contribution<B: RadFormulaBuilder>(
    b: &mut B,
    rule: RadUnaryMathRule,
    x: B::Value,
    primal: B::Value,
    y_bar: B::Value,
) -> B::Value {
    match rule {
        RadUnaryMathRule::Sin => {
            let cos_x = b.cos(x);
            b.mul(y_bar, cos_x)
        }
        RadUnaryMathRule::Cos => {
            let sin_x = b.sin(x);
            let zero = b.zero();
            let neg_sin = b.sub(zero, sin_x);
            b.mul(y_bar, neg_sin)
        }
        RadUnaryMathRule::Tan => {
            let one = b.one();
            let tan_sq = b.mul(primal, primal);
            let sec_sq = b.add(one, tan_sq);
            b.mul(y_bar, sec_sq)
        }
        RadUnaryMathRule::Exp => b.mul(y_bar, primal),
        RadUnaryMathRule::Exp10 => {
            let ln_10 = b.ln_10();
            let scaled = b.mul(primal, ln_10);
            b.mul(y_bar, scaled)
        }
        RadUnaryMathRule::Log => b.div(y_bar, x),
        RadUnaryMathRule::Log10 => {
            let ln_10 = b.ln_10();
            let denom = b.mul(x, ln_10);
            b.div(y_bar, denom)
        }
        RadUnaryMathRule::Sqrt => {
            let one = b.one();
            let two = b.add(one, one);
            let denom = b.mul(two, primal);
            b.div(y_bar, denom)
        }
        RadUnaryMathRule::Abs => {
            let num = b.mul(y_bar, x);
            b.div(num, primal)
        }
        RadUnaryMathRule::Acos => {
            let one = b.one();
            let x_sq = b.mul(x, x);
            let inside = b.sub(one, x_sq);
            let root = b.sqrt(inside);
            let zero = b.zero();
            let numerator = b.sub(zero, y_bar);
            b.div(numerator, root)
        }
        RadUnaryMathRule::Asin => {
            let one = b.one();
            let x_sq = b.mul(x, x);
            let inside = b.sub(one, x_sq);
            let root = b.sqrt(inside);
            b.div(y_bar, root)
        }
        RadUnaryMathRule::Atan => {
            let one = b.one();
            let x_sq = b.mul(x, x);
            let denom = b.add(one, x_sq);
            b.div(y_bar, denom)
        }
    }
}

/// Returns local contributions for a binary math rule as `(lhs_bar, rhs_bar)`.
///
/// `primal` is the forward value of the matched node and is used by `pow`.
///
/// `Pow` deliberately uses `y_bar * rhs * lhs^(rhs - 1)` for the base
/// derivative rather than the algebraically equivalent `y_bar * lhs^rhs * rhs
/// / lhs`. The latter introduces `0/0` at common points such as `pow(0, 2)`.
#[must_use]
pub fn rad_binary_contributions<B: RadFormulaBuilder>(
    b: &mut B,
    rule: RadBinaryMathRule,
    lhs: B::Value,
    rhs: B::Value,
    primal: B::Value,
    y_bar: B::Value,
) -> (B::Value, B::Value) {
    match rule {
        RadBinaryMathRule::Pow => {
            let one = b.one();
            let rhs_minus_one = b.sub(rhs, one);
            let pow_x_ym1 = b.pow(lhs, rhs_minus_one);
            let scaled = b.mul(y_bar, rhs);
            let lhs_bar = b.mul(scaled, pow_x_ym1);
            let log_lhs = b.log(lhs);
            let rhs_inner = b.mul(y_bar, primal);
            let rhs_bar = b.mul(rhs_inner, log_lhs);
            (lhs_bar, rhs_bar)
        }
        RadBinaryMathRule::Min => {
            let cond = b.le(lhs, rhs);
            let zero = b.zero();
            (
                b.select_nonzero(cond, y_bar, zero),
                b.select_nonzero(cond, zero, y_bar),
            )
        }
        RadBinaryMathRule::Max => {
            let cond = b.ge(lhs, rhs);
            let zero = b.zero();
            (
                b.select_nonzero(cond, y_bar, zero),
                b.select_nonzero(cond, zero, y_bar),
            )
        }
        RadBinaryMathRule::Atan2 => {
            let lhs_sq = b.mul(lhs, lhs);
            let rhs_sq = b.mul(rhs, rhs);
            let denom = b.add(lhs_sq, rhs_sq);
            let lhs_factor = b.div(rhs, denom);
            let zero = b.zero();
            let neg_lhs = b.sub(zero, lhs);
            let rhs_factor = b.div(neg_lhs, denom);
            (b.mul(y_bar, lhs_factor), b.mul(y_bar, rhs_factor))
        }
        RadBinaryMathRule::Fmod => {
            let q = b.div(lhs, rhs);
            let floor_q = b.floor(q);
            let zero = b.zero();
            let neg_floor = b.sub(zero, floor_q);
            (y_bar, b.mul(y_bar, neg_floor))
        }
        RadBinaryMathRule::Remainder => {
            let q = b.div(lhs, rhs);
            let round_q = b.round(q);
            let zero = b.zero();
            let neg_round = b.sub(zero, round_q);
            (y_bar, b.mul(y_bar, neg_round))
        }
    }
}

/// Returns local contributions for arithmetic `SIGBINOP` rules.
///
/// `None` means the local rule has zero contribution for both operands. Callers
/// should not treat it as an unsupported node; support/error policy is decided
/// before this function by the owning AD pass.
#[must_use]
pub fn rad_binop_contributions<B: RadFormulaBuilder>(
    b: &mut B,
    rule: RadBinOpRule,
    lhs: B::Value,
    rhs: B::Value,
    y_bar: B::Value,
) -> Option<(B::Value, B::Value)> {
    match rule {
        RadBinOpRule::Add => Some((y_bar, y_bar)),
        RadBinOpRule::Sub => {
            let zero = b.zero();
            Some((y_bar, b.sub(zero, y_bar)))
        }
        RadBinOpRule::Mul => Some((b.mul(y_bar, rhs), b.mul(y_bar, lhs))),
        RadBinOpRule::Div => {
            let lhs_bar = b.div(y_bar, rhs);
            let rhs_sq = b.mul(rhs, rhs);
            let zero = b.zero();
            let neg_lhs = b.sub(zero, lhs);
            let scaled = b.div(neg_lhs, rhs_sq);
            Some((lhs_bar, b.mul(y_bar, scaled)))
        }
        RadBinOpRule::Rem => {
            let q = b.div(lhs, rhs);
            let floor_q = b.floor(q);
            let zero = b.zero();
            let neg_floor = b.sub(zero, floor_q);
            Some((y_bar, b.mul(y_bar, neg_floor)))
        }
        RadBinOpRule::Zero => None,
    }
}

/// Returns the local unary math rule and operand for a Signal node, if any.
#[must_use]
pub fn rad_unary_math_rule(sig: &SigMatch<'_>) -> Option<(RadUnaryMathRule, SigId)> {
    match *sig {
        SigMatch::Sin(x) => Some((RadUnaryMathRule::Sin, x)),
        SigMatch::Cos(x) => Some((RadUnaryMathRule::Cos, x)),
        SigMatch::Tan(x) => Some((RadUnaryMathRule::Tan, x)),
        SigMatch::Exp(x) => Some((RadUnaryMathRule::Exp, x)),
        SigMatch::Exp10(x) => Some((RadUnaryMathRule::Exp10, x)),
        SigMatch::Log(x) => Some((RadUnaryMathRule::Log, x)),
        SigMatch::Log10(x) => Some((RadUnaryMathRule::Log10, x)),
        SigMatch::Sqrt(x) => Some((RadUnaryMathRule::Sqrt, x)),
        SigMatch::Abs(x) => Some((RadUnaryMathRule::Abs, x)),
        SigMatch::Acos(x) => Some((RadUnaryMathRule::Acos, x)),
        SigMatch::Asin(x) => Some((RadUnaryMathRule::Asin, x)),
        SigMatch::Atan(x) => Some((RadUnaryMathRule::Atan, x)),
        _ => None,
    }
}

/// Returns the local binary math rule and operands for a Signal node, if any.
#[must_use]
pub fn rad_binary_math_rule(sig: &SigMatch<'_>) -> Option<(RadBinaryMathRule, SigId, SigId)> {
    match *sig {
        SigMatch::Pow(x, y) => Some((RadBinaryMathRule::Pow, x, y)),
        SigMatch::Min(x, y) => Some((RadBinaryMathRule::Min, x, y)),
        SigMatch::Max(x, y) => Some((RadBinaryMathRule::Max, x, y)),
        SigMatch::Atan2(x, y) => Some((RadBinaryMathRule::Atan2, x, y)),
        SigMatch::Fmod(x, y) => Some((RadBinaryMathRule::Fmod, x, y)),
        SigMatch::Remainder(x, y) => Some((RadBinaryMathRule::Remainder, x, y)),
        _ => None,
    }
}

/// Classifies a primitive binary operator for local reverse-AD dispatch.
#[must_use]
pub fn rad_binop_rule(op: BinOp) -> RadBinOpRule {
    match op {
        BinOp::Add => RadBinOpRule::Add,
        BinOp::Sub => RadBinOpRule::Sub,
        BinOp::Mul => RadBinOpRule::Mul,
        BinOp::Div => RadBinOpRule::Div,
        BinOp::Rem => RadBinOpRule::Rem,
        BinOp::Lsh
        | BinOp::ARsh
        | BinOp::LRsh
        | BinOp::Gt
        | BinOp::Lt
        | BinOp::Ge
        | BinOp::Le
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::And
        | BinOp::Or
        | BinOp::Xor => RadBinOpRule::Zero,
    }
}

/// Whether `sig` is the literal zero a derivative is spelled as
/// (`real(0.0)` or `int(0)`).
pub fn is_literal_zero(arena: &TreeArena, sig: SigId) -> bool {
    matches!(match_sig(arena, sig), SigMatch::Real(v) if v == 0.0)
        || matches!(match_sig(arena, sig), SigMatch::Int(0))
}

/// Folds the literal zeros of derivative expressions as they are built.
///
/// | node | fold |
/// |---|---|
/// | `0 · x`, `x · 0`, `0 / x` | `0` |
/// | `x + 0`, `0 + x`, `x − 0` | `x` |
/// | `TempVar(0)`, `PermVar(0)`, `Clocked(τ, 0)`, `Seq(B, 0)`, `ZeroPad(0, h)`, `delay1(0)`, `delay(0, d)` | `0` |
/// | `float_cast(0)` | `real(0)` |
/// | `select2(c, 0, 0)` | `0` |
///
/// These are folds the signal simplifier (`normalize`) applies to bare
/// literals; a differentiation pass needs them earlier and through the
/// clock-domain wrappers, because it decides from the shape of a
/// derivative what a block must hold: a zero wrapped as `Clocked(τ, 0)`
/// became a held value read at the audio rate
/// (porting/fad-zero-tangent-folding-plan-2026-10-18-en.md). Only a zero
/// goes through a wrapper: `PermVar(c)` reads 0 before its block first
/// fires, so it equals `c` only for `c = 0`.
///
/// The folder walks the nodes it has not seen and stops at those it has
/// folded or been told to [`keep`](Self::keep): a pass that folds each
/// derivative it builds over already-folded operands, and keeps its
/// primals, visits only the nodes its rule just built. Folding is
/// structural (a node is replaced by an equal-valued node of the same
/// scope), so one memo is valid at every de Bruijn depth.
#[derive(Debug, Default)]
pub struct DerivativeZeroFolder {
    memo: HashMap<SigId, SigId>,
}

impl DerivativeZeroFolder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks `sig` as a fixed point: the folder will not look inside it.
    pub fn keep(&mut self, sig: SigId) {
        self.memo.entry(sig).or_insert(sig);
    }

    /// The derivative expression `sig` with its literal zeros folded.
    pub fn fold(&mut self, arena: &mut TreeArena, sig: SigId) -> SigId {
        if let Some(&folded) = self.memo.get(&sig) {
            return folded;
        }
        let folded = match match_sig(arena, sig) {
            SigMatch::BinOp(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div), x, y) => {
                let fx = self.fold(arena, x);
                let fy = self.fold(arena, y);
                let zx = is_literal_zero(arena, fx);
                let zy = is_literal_zero(arena, fy);
                match op {
                    BinOp::Add if zx => fy,
                    BinOp::Add | BinOp::Sub if zy => fx,
                    BinOp::Mul | BinOp::Div if zx => fx,
                    BinOp::Mul if zy => fy,
                    _ if fx == x && fy == y => sig,
                    _ => SigBuilder::new(arena).binop(op, fx, fy),
                }
            }
            SigMatch::TempVar(u)
            | SigMatch::PermVar(u)
            | SigMatch::Clocked(_, u)
            | SigMatch::Seq(_, u)
            | SigMatch::ZeroPad(u, _)
            | SigMatch::Delay1(u)
            | SigMatch::Delay(u, _) => {
                let fu = self.fold(arena, u);
                if is_literal_zero(arena, fu) { fu } else { sig }
            }
            SigMatch::FloatCast(u) => {
                let fu = self.fold(arena, u);
                if is_literal_zero(arena, fu) {
                    SigBuilder::new(arena).real(0.0)
                } else {
                    sig
                }
            }
            SigMatch::Select2(_, a, b) => {
                let fa = self.fold(arena, a);
                let fb = self.fold(arena, b);
                if is_literal_zero(arena, fa) && is_literal_zero(arena, fb) {
                    fa
                } else {
                    sig
                }
            }
            _ => sig,
        };
        self.memo.insert(sig, folded);
        folded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SigBuilder, dump_sig_readable};
    use tlib::TreeArena;

    #[test]
    fn derivative_zero_folder_folds_zeros_through_arithmetic_and_wrappers() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let zero = b.real(0.0);
        let one = b.real(1.0);
        let x = b.input(0);
        let env = b.clock_env_token(0);
        let wrapped = b.clocked(env, zero);
        let held = b.perm_var(wrapped);
        let snap = b.temp_var(held);
        let prod = b.mul(x, snap);
        let sum = b.add(prod, one);
        let quot = b.div(snap, x);
        let diff = b.sub(x, quot);
        let both = b.select2(x, snap, zero);
        let padded = b.zero_pad(wrapped, one);
        let cast = b.float_cast(padded);
        let delayed = b.delay1(cast);
        let mut folder = DerivativeZeroFolder::new();
        assert_eq!(folder.fold(&mut arena, snap), zero);
        assert_eq!(folder.fold(&mut arena, sum), one, "x·0 + 1 is 1");
        assert_eq!(folder.fold(&mut arena, diff), x, "x − 0/x is x");
        assert_eq!(folder.fold(&mut arena, both), zero);
        assert_eq!(folder.fold(&mut arena, delayed), zero);
    }

    #[test]
    fn derivative_zero_folder_keeps_non_zero_holds_and_kept_nodes() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let zero = b.real(0.0);
        let one = b.real(1.0);
        let x = b.input(0);
        let env = b.clock_env_token(0);
        let wrapped_one = b.clocked(env, one);
        let held_one = b.perm_var(wrapped_one);
        let zero_minus_x = b.sub(zero, x);
        let primal = b.mul(x, zero);
        let tangent = b.add(primal, one);
        let mut folder = DerivativeZeroFolder::new();
        assert_eq!(
            folder.fold(&mut arena, held_one),
            held_one,
            "a held 1 reads 0 before its first firing: not the literal 1"
        );
        assert_eq!(folder.fold(&mut arena, zero_minus_x), zero_minus_x);
        folder.keep(primal);
        assert_eq!(
            folder.fold(&mut arena, tangent),
            tangent,
            "a kept node is not looked into"
        );
    }

    #[test]
    fn ad_rules_classify_unary_and_binary_math_nodes() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.real(0.5);
        let y = b.real(2.0);
        let sin = b.sin(x);
        let pow = b.pow(x, y);

        assert_eq!(
            rad_unary_math_rule(&crate::match_sig(&arena, sin)),
            Some((RadUnaryMathRule::Sin, x))
        );
        assert_eq!(
            rad_binary_math_rule(&crate::match_sig(&arena, pow)),
            Some((RadBinaryMathRule::Pow, x, y))
        );
    }

    #[test]
    fn ad_rules_classify_discrete_binops_as_zero() {
        assert_eq!(rad_binop_rule(BinOp::Add), RadBinOpRule::Add);
        assert_eq!(rad_binop_rule(BinOp::Rem), RadBinOpRule::Rem);
        assert_eq!(rad_binop_rule(BinOp::Lt), RadBinOpRule::Zero);
        assert_eq!(rad_binop_rule(BinOp::And), RadBinOpRule::Zero);
    }

    #[test]
    fn ad_rules_pow_formula_uses_stable_base_derivative() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.real(0.5);
        let y = b.real(2.0);
        let y_bar = b.real(3.0);
        let primal = b.pow(x, y);

        let (x_bar, y_bar_out) =
            rad_binary_contributions(&mut b, RadBinaryMathRule::Pow, x, y, primal, y_bar);

        let x_dump = dump_sig_readable(&arena, x_bar);
        assert!(
            x_dump.contains("SIGPOW")
                && x_dump.contains("op=sub (-)")
                && x_dump.contains("0x3ff0000000000000"),
            "pow base derivative should use x^(y-1), got {x_dump}"
        );
        assert!(
            !x_dump.contains("op=div (/)"),
            "pow base derivative should not use x^y / x form, got {x_dump}"
        );

        let y_dump = dump_sig_readable(&arena, y_bar_out);
        assert!(
            y_dump.contains("SIGPOW") && y_dump.contains("SIGLOG"),
            "pow exponent derivative should use primal * log(x), got {y_dump}"
        );
    }

    #[test]
    fn ad_rules_unary_formulas_emit_expected_shapes() {
        let mut arena = TreeArena::new();
        let (x, y_bar) = {
            let mut b = SigBuilder::new(&mut arena);
            (b.real(4.0), b.real(2.0))
        };
        let sqrt_grad = {
            let mut b = SigBuilder::new(&mut arena);
            let sqrt_primal = b.sqrt(x);
            rad_unary_contribution(&mut b, RadUnaryMathRule::Sqrt, x, sqrt_primal, y_bar)
        };
        let sqrt_dump = dump_sig_readable(&arena, sqrt_grad);
        assert!(
            sqrt_dump.contains("SIGSQRT") && sqrt_dump.contains("op=div (/)"),
            "sqrt derivative should divide by the primal sqrt value, got {sqrt_dump}"
        );

        let asin_grad = {
            let mut b = SigBuilder::new(&mut arena);
            rad_unary_contribution(&mut b, RadUnaryMathRule::Asin, x, x, y_bar)
        };
        let asin_dump = dump_sig_readable(&arena, asin_grad);
        assert!(
            asin_dump.contains("SIGSQRT")
                && asin_dump.contains("op=sub (-)")
                && asin_dump.contains("op=mul (*)"),
            "asin derivative should use sqrt(1 - x*x), got {asin_dump}"
        );
    }

    /// Plain-`f64` builder so the shared formulas can be evaluated directly.
    struct F64Builder;

    impl RadFormulaBuilder for F64Builder {
        type Value = f64;
        fn zero(&mut self) -> f64 {
            0.0
        }
        fn one(&mut self) -> f64 {
            1.0
        }
        fn ln_10(&mut self) -> f64 {
            std::f64::consts::LN_10
        }
        fn add(&mut self, x: f64, y: f64) -> f64 {
            x + y
        }
        fn sub(&mut self, x: f64, y: f64) -> f64 {
            x - y
        }
        fn mul(&mut self, x: f64, y: f64) -> f64 {
            x * y
        }
        fn div(&mut self, x: f64, y: f64) -> f64 {
            x / y
        }
        fn pow(&mut self, x: f64, y: f64) -> f64 {
            x.powf(y)
        }
        fn log(&mut self, x: f64) -> f64 {
            x.ln()
        }
        fn cos(&mut self, x: f64) -> f64 {
            x.cos()
        }
        fn sin(&mut self, x: f64) -> f64 {
            x.sin()
        }
        fn sqrt(&mut self, x: f64) -> f64 {
            x.sqrt()
        }
        fn abs(&mut self, x: f64) -> f64 {
            x.abs()
        }
        fn floor(&mut self, x: f64) -> f64 {
            x.floor()
        }
        fn round(&mut self, x: f64) -> f64 {
            x.round()
        }
        fn lt(&mut self, x: f64, y: f64) -> f64 {
            f64::from(u8::from(x < y))
        }
        fn le(&mut self, x: f64, y: f64) -> f64 {
            f64::from(u8::from(x <= y))
        }
        fn gt(&mut self, x: f64, y: f64) -> f64 {
            f64::from(u8::from(x > y))
        }
        fn ge(&mut self, x: f64, y: f64) -> f64 {
            f64::from(u8::from(x >= y))
        }
        fn select_nonzero(&mut self, cond: f64, when_true: f64, when_false: f64) -> f64 {
            if cond != 0.0 { when_true } else { when_false }
        }
    }

    /// `tan' = 1 + tan²` built on the primal agrees with the analytic
    /// `1 / cos²` at points spread over `(-π/2, π/2)`, including near the
    /// poles where both grow large.
    #[test]
    fn ad_rules_tan_contribution_matches_analytic_derivative() {
        for x in [-1.5, -0.9, -0.25, 0.0, 0.3, 0.785, 1.2, 1.55] {
            let y_bar = 0.75;
            let primal = f64::tan(x);
            let got =
                rad_unary_contribution(&mut F64Builder, RadUnaryMathRule::Tan, x, primal, y_bar);
            let expected = y_bar / (x.cos() * x.cos());
            let rel = ((got - expected) / expected).abs();
            assert!(
                rel < 1e-12,
                "tan'({x}): got {got}, expected {expected}, rel {rel}"
            );
        }
    }

    /// The symbolic `tan` contribution reads the primal and never calls `cos`.
    #[test]
    fn ad_rules_tan_contribution_reuses_primal_without_cos() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let y_bar = b.real(2.0);
        let primal = b.tan(x);
        let grad = rad_unary_contribution(&mut b, RadUnaryMathRule::Tan, x, primal, y_bar);
        let dump = dump_sig_readable(&arena, grad);
        assert!(
            !dump.contains("SIGCOS") && !dump.contains("op=div (/)"),
            "tan derivative should not rebuild cos(x)^2 or divide, got {dump}"
        );
        assert!(
            dump.contains("SIGTAN") && dump.contains("op=add (+)"),
            "tan derivative should be y_bar * (1 + tan(x)^2), got {dump}"
        );
    }

    #[test]
    fn ad_rules_min_max_select_the_expected_branches() {
        let mut arena = TreeArena::new();
        let (x, y, y_bar) = {
            let mut b = SigBuilder::new(&mut arena);
            (b.real(1.0), b.real(2.0), b.real(3.0))
        };
        let (min_x, min_y) = {
            let mut b = SigBuilder::new(&mut arena);
            rad_binary_contributions(&mut b, RadBinaryMathRule::Min, x, y, x, y_bar)
        };
        let min_x_dump = dump_sig_readable(&arena, min_x);
        let min_y_dump = dump_sig_readable(&arena, min_y);
        assert!(
            min_x_dump.contains("SIGSELECT2")
                && min_x_dump.contains("op=le (<=)")
                && min_x_dump.contains("0x0000000000000000")
                && min_x_dump.contains("0x4008000000000000"),
            "min lhs derivative should receive y_bar on lhs <= rhs, got {min_x_dump}"
        );
        assert!(
            min_y_dump.contains("SIGSELECT2")
                && min_y_dump.contains("op=le (<=)")
                && min_y_dump.contains("0x4008000000000000")
                && min_y_dump.contains("0x0000000000000000"),
            "min rhs derivative should receive zero on lhs <= rhs, got {min_y_dump}"
        );

        let (max_x, max_y) = {
            let mut b = SigBuilder::new(&mut arena);
            rad_binary_contributions(&mut b, RadBinaryMathRule::Max, x, y, x, y_bar)
        };
        let max_x_dump = dump_sig_readable(&arena, max_x);
        let max_y_dump = dump_sig_readable(&arena, max_y);
        assert!(
            max_x_dump.contains("SIGSELECT2")
                && max_x_dump.contains("op=ge (>=)")
                && max_x_dump.contains("0x0000000000000000")
                && max_x_dump.contains("0x4008000000000000"),
            "max lhs derivative should receive y_bar on lhs >= rhs, got {max_x_dump}"
        );
        assert!(
            max_y_dump.contains("SIGSELECT2")
                && max_y_dump.contains("op=ge (>=)")
                && max_y_dump.contains("0x4008000000000000")
                && max_y_dump.contains("0x0000000000000000"),
            "max rhs derivative should receive zero on lhs >= rhs, got {max_y_dump}"
        );
    }

    #[test]
    fn ad_rules_binop_formula_shapes_are_shared() {
        let mut arena = TreeArena::new();
        let (x, y, y_bar) = {
            let mut b = SigBuilder::new(&mut arena);
            (b.real(6.0), b.real(2.0), b.real(3.0))
        };
        let ((lhs, rhs), zero_contrib) = {
            let mut b = SigBuilder::new(&mut arena);
            (
                rad_binop_contributions(&mut b, RadBinOpRule::Div, x, y, y_bar).unwrap(),
                rad_binop_contributions(&mut b, RadBinOpRule::Zero, x, y, y_bar),
            )
        };
        let lhs_dump = dump_sig_readable(&arena, lhs);
        assert!(
            lhs_dump.contains("op=div (/)")
                && lhs_dump.contains("0x4008000000000000")
                && lhs_dump.contains("0x4000000000000000"),
            "division lhs derivative should use y_bar / rhs, got {lhs_dump}"
        );
        let rhs_dump = dump_sig_readable(&arena, rhs);
        assert!(
            rhs_dump.contains("op=sub (-)")
                && rhs_dump.contains("op=mul (*)")
                && rhs_dump.contains("0x4018000000000000")
                && rhs_dump.contains("0x4000000000000000"),
            "division rhs derivative should use -x/(y*y), got {rhs_dump}"
        );

        assert!(zero_contrib.is_none());
    }
}
