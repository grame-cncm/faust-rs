//! Stateless leaf emission shared by the two production lowerers.
//!
//! The scalar lowerer (`module/`) and the checked vector lowerer
//! (`vector/lower/`) recurse, cache, and place values differently, but once
//! the operands of a stateless node are lowered, the FIR they emit is
//! identical. This module is that shared grammar: binary operators with the
//! fast-lane typing contract, unary/binary math intrinsics, integer-vs-real
//! `min`/`max`/`abs` selection, and real constants at the internal
//! precision. Each function is context-free — it sees a [`FirStore`],
//! already-lowered operand ids, and the result type — so a fix to a leaf
//! rule can no longer land on one path only.
//!
//! # Scope
//! Only families whose emitted FIR is provably identical on both paths live
//! here. Anything touching state, placement, caching, regions, UI, tables,
//! delays, or recursion stays with its lowerer, and `select2` stays out
//! until its sharing is proven the same way.
//!
//! # Doctrine
//! This is **producer-side vocabulary**, exactly like [`FirBuilder`] itself:
//! the vector pipeline's independent checkers keep re-deriving their own
//! evidence and never call these emitters (producer/checker doctrine in
//! `vector/mod.rs`).

use fir::{FirBinOp, FirBuilder, FirId, FirMathOp, FirStore, FirType};
use signals::BinOp;

/// Where a lowerer records the math intrinsics and integer helper functions
/// its module ends up using, so exactly the needed prototypes are declared.
///
/// Each lowerer keeps its own containers; the emitters only *note* usage.
pub(in crate::signal_fir) trait LeafPrototypes {
    /// Records one used math intrinsic.
    fn note_math_op(&mut self, op: FirMathOp);
    /// Records one used integer helper (`abs`, `min_i`, `max_i`).
    fn note_int_helper(&mut self, name: &'static str);
}

/// Maps one signal binary operator to its FIR operator and result type:
/// arithmetic keeps the internal real (or integer) type, comparisons produce
/// the C++-parity "boolean int", bitwise operators are `Int32` throughout.
pub(in crate::signal_fir) fn map_binop(op: BinOp, real_ty: FirType) -> Option<(FirBinOp, FirType)> {
    match op {
        // Arithmetic operators: result is the internal real type.
        BinOp::Add => Some((FirBinOp::Add, real_ty)),
        BinOp::Sub => Some((FirBinOp::Sub, real_ty)),
        BinOp::Mul => Some((FirBinOp::Mul, real_ty)),
        BinOp::Div => Some((FirBinOp::Div, real_ty)),
        BinOp::Rem => Some((FirBinOp::Rem, real_ty)),
        // Comparison operators: result is Int32 ("boolean int") for parity
        // with the standard C++ signal typing path.
        BinOp::Gt => Some((FirBinOp::Gt, FirType::Int32)),
        BinOp::Lt => Some((FirBinOp::Lt, FirType::Int32)),
        BinOp::Ge => Some((FirBinOp::Ge, FirType::Int32)),
        BinOp::Le => Some((FirBinOp::Le, FirType::Int32)),
        BinOp::Eq => Some((FirBinOp::Eq, FirType::Int32)),
        BinOp::Ne => Some((FirBinOp::Ne, FirType::Int32)),
        // Bitwise operators: result is Int32 — independent of real_ty.
        BinOp::And => Some((FirBinOp::And, FirType::Int32)),
        BinOp::Or => Some((FirBinOp::Or, FirType::Int32)),
        BinOp::Xor => Some((FirBinOp::Xor, FirType::Int32)),
        BinOp::Lsh => Some((FirBinOp::Lsh, FirType::Int32)),
        BinOp::ARsh => Some((FirBinOp::ARsh, FirType::Int32)),
        BinOp::LRsh => Some((FirBinOp::LRsh, FirType::Int32)),
    }
}

/// Why [`emit_binop`] refused to emit. Each lowerer maps this back to its
/// own error type, preserving its exact current diagnostics.
pub(in crate::signal_fir) enum LeafBinopError {
    /// [`map_binop`] has no mapping for the operator.
    UnsupportedOperator,
    /// An operand carries no FIR value type.
    MissingOperandType {
        /// `true` when the left operand is the one missing a type.
        is_lhs: bool,
        /// Left operand type, when known.
        lhs: Option<FirType>,
        /// The mapped result type the contract would have checked against.
        expected: FirType,
    },
    /// Operand types violate the fast-lane typing contract.
    OperandContract {
        /// Left operand type.
        lhs: FirType,
        /// Right operand type.
        rhs: FirType,
        /// The mapped result type.
        expected: FirType,
    },
}

/// Emits one binary operation over already-lowered operands.
///
/// Relies on the promoter invariant: every `BinOp` operand already has the
/// correct domain type (mixed Int/Real pairs wrapped in `FloatCast`; bitwise
/// and shift operands in `IntCast`; `Div` operands always Real). Comparisons
/// keep same-typed numeric operands and produce `Int32` results for C++
/// parity. No implicit coercion is performed here; a violation is refused,
/// never repaired.
pub(in crate::signal_fir) fn emit_binop(
    store: &mut FirStore,
    op: BinOp,
    result_ty: FirType,
    lhs: FirId,
    rhs: FirId,
) -> Result<FirId, LeafBinopError> {
    let (fir_op, typ) = map_binop(op, result_ty).ok_or(LeafBinopError::UnsupportedOperator)?;
    let lhs_ty = store
        .value_type(lhs)
        .ok_or_else(|| LeafBinopError::MissingOperandType {
            is_lhs: true,
            lhs: None,
            expected: typ.clone(),
        })?;
    let rhs_ty = store
        .value_type(rhs)
        .ok_or_else(|| LeafBinopError::MissingOperandType {
            is_lhs: false,
            lhs: Some(lhs_ty.clone()),
            expected: typ.clone(),
        })?;
    let operands_ok = match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            lhs_ty == typ && rhs_ty == typ
        }
        BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Lsh | BinOp::ARsh | BinOp::LRsh => {
            lhs_ty == FirType::Int32 && rhs_ty == FirType::Int32
        }
        BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne => {
            lhs_ty == rhs_ty
                && matches!(lhs_ty, FirType::Int32 | FirType::Float32 | FirType::Float64)
        }
    };
    if !operands_ok {
        return Err(LeafBinopError::OperandContract {
            lhs: lhs_ty,
            rhs: rhs_ty,
            expected: typ,
        });
    }
    Ok(FirBuilder::new(store).binop(fir_op, lhs, rhs, typ))
}

/// Emits one unary math intrinsic call at the internal real precision and
/// notes the used prototype.
pub(in crate::signal_fir) fn emit_math_call1(
    store: &mut FirStore,
    protos: &mut impl LeafPrototypes,
    op: FirMathOp,
    value: FirId,
    real_ty: FirType,
) -> FirId {
    protos.note_math_op(op);
    FirBuilder::new(store).math_call(op, &[value], real_ty)
}

/// Emits one binary math intrinsic call at the internal real precision and
/// notes the used prototype.
pub(in crate::signal_fir) fn emit_math_call2(
    store: &mut FirStore,
    protos: &mut impl LeafPrototypes,
    op: FirMathOp,
    lhs: FirId,
    rhs: FirId,
    real_ty: FirType,
) -> FirId {
    protos.note_math_op(op);
    FirBuilder::new(store).math_call(op, &[lhs, rhs], real_ty)
}

/// Emits `min`/`max` over already-lowered operands: an explicit `min_i` /
/// `max_i` call when the result stays integer (so backends keep the C++
/// target-local renaming policy instead of a hardwired branch synthesis),
/// the math intrinsic otherwise.
pub(in crate::signal_fir) fn emit_minmax(
    store: &mut FirStore,
    protos: &mut impl LeafPrototypes,
    is_min: bool,
    result_ty: &FirType,
    real_ty: FirType,
    lhs: FirId,
    rhs: FirId,
) -> FirId {
    if *result_ty == FirType::Int32 {
        let name = if is_min { "min_i" } else { "max_i" };
        protos.note_int_helper(name);
        return FirBuilder::new(store).fun_call(name, &[lhs, rhs], FirType::Int32);
    }
    emit_math_call2(
        store,
        protos,
        if is_min {
            FirMathOp::Min
        } else {
            FirMathOp::Max
        },
        lhs,
        rhs,
        real_ty,
    )
}

/// Emits `abs` over an already-lowered operand: an explicit `abs` call when
/// the result stays integer (preserving the target-local parity spelling and
/// overflow contract), the math intrinsic otherwise.
pub(in crate::signal_fir) fn emit_abs(
    store: &mut FirStore,
    protos: &mut impl LeafPrototypes,
    result_ty: &FirType,
    real_ty: FirType,
    value: FirId,
) -> FirId {
    if *result_ty == FirType::Int32 {
        protos.note_int_helper("abs");
        return FirBuilder::new(store).fun_call("abs", &[value], FirType::Int32);
    }
    emit_math_call1(store, protos, FirMathOp::Abs, value, real_ty)
}

/// Emits one floating-point constant at the internal real precision
/// (`Float32` or `Float64`, never `FaustFloat` — that type is reserved for
/// external interface points).
pub(in crate::signal_fir) fn emit_real_const(
    store: &mut FirStore,
    real_ty: &FirType,
    value: f64,
) -> FirId {
    let mut b = FirBuilder::new(store);
    match real_ty {
        FirType::Float64 => b.float64(value),
        _ => b.float32(value as f32),
    }
}
