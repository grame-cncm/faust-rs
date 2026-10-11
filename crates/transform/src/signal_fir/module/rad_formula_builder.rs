//! FIR adapter for backend-neutral local RAD formula emission.
//!
//! [`FirRadFormulaBuilder`] bridges the backend-agnostic `RadFormulaBuilder`
//! trait (used by shared differentiation-rule helpers) and the FIR store owned
//! by [`SignalToFirLower`].  It maps arithmetic operations and math calls to
//! typed FIR instructions while registering required math helpers through
//! `used_protos.math_ops`, and preserves the active internal real type.
use crate::signal_fir::FirId;
use crate::signal_fir::FirType;
use crate::signal_fir::module::FirBinOp;
use crate::signal_fir::module::FirBuilder;
use crate::signal_fir::module::FirMathOp;
use crate::signal_fir::module::RadFormulaBuilder;
use crate::signal_fir::module::SignalToFirLower;

/// FIR adapter for backend-neutral local RAD formulas.
///
/// This wrapper is deliberately small: it maps arithmetic and math calls into
/// FIR, registers required math helpers through `used_protos.math_ops`, and
/// preserves the active internal real type. It does not know which Signal node
/// is being differentiated and must not load BRA tapes; callers pass already
/// prepared `FirId` values into the shared formula helpers.
pub(super) struct FirRadFormulaBuilder<'lower, 'arena> {
    lower: &'lower mut SignalToFirLower<'arena>,
    real_ty: FirType,
}

impl<'lower, 'arena> FirRadFormulaBuilder<'lower, 'arena> {
    /// Wraps a mutable borrow of the active lowering state with the given real type.
    pub(super) fn new(lower: &'lower mut SignalToFirLower<'arena>, real_ty: FirType) -> Self {
        Self { lower, real_ty }
    }

    /// Emits a typed FIR binary operation into the lowering store.
    fn binop(&mut self, op: FirBinOp, x: FirId, y: FirId, typ: FirType) -> FirId {
        FirBuilder::new(&mut self.lower.store).binop(op, x, y, typ)
    }

    /// Emits a FIR math call, registering `op` in `used_protos.math_ops` so the
    /// backend knows to provide the corresponding math helper.
    fn math_call(&mut self, op: FirMathOp, args: &[FirId]) -> FirId {
        self.lower.used_protos.math_ops.insert(op);
        FirBuilder::new(&mut self.lower.store).fun_call(
            crate::signal_fir::leaf_emit::math_symbol(op, &self.real_ty),
            args,
            self.real_ty.clone(),
        )
    }
}

impl RadFormulaBuilder for FirRadFormulaBuilder<'_, '_> {
    type Value = FirId;

    fn zero(&mut self) -> Self::Value {
        self.lower.float_const(0.0)
    }

    fn one(&mut self) -> Self::Value {
        self.lower.float_const(1.0)
    }

    fn ln_10(&mut self) -> Self::Value {
        self.lower.float_const(std::f64::consts::LN_10)
    }

    fn add(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Add, x, y, self.real_ty.clone())
    }

    fn sub(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Sub, x, y, self.real_ty.clone())
    }

    fn mul(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Mul, x, y, self.real_ty.clone())
    }

    fn div(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Div, x, y, self.real_ty.clone())
    }

    fn pow(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Pow, &[x, y])
    }

    fn log(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Log, &[x])
    }

    fn cos(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Cos, &[x])
    }

    fn sin(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Sin, &[x])
    }

    fn sqrt(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Sqrt, &[x])
    }

    fn abs(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Abs, &[x])
    }

    fn floor(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Floor, &[x])
    }

    fn round(&mut self, x: Self::Value) -> Self::Value {
        self.math_call(FirMathOp::Round, &[x])
    }

    fn lt(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Lt, x, y, FirType::Int32)
    }

    fn le(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Le, x, y, FirType::Int32)
    }

    fn gt(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Gt, x, y, FirType::Int32)
    }

    fn ge(&mut self, x: Self::Value, y: Self::Value) -> Self::Value {
        self.binop(FirBinOp::Ge, x, y, FirType::Int32)
    }

    fn select_nonzero(
        &mut self,
        cond: Self::Value,
        when_true: Self::Value,
        when_false: Self::Value,
    ) -> Self::Value {
        // FIR `select2` already uses the natural order
        // `(cond, when_true, when_false)`, unlike Signal `select2`.
        FirBuilder::new(&mut self.lower.store).select2(
            cond,
            when_true,
            when_false,
            self.real_ty.clone(),
        )
    }
}
