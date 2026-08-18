//! `types` rules of the FIR checker.
//!
//! The type lattice this checker reasons with: inference, compatibility, promotion, and width.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// Returns the element type for pointer/array/vector containers.
    pub(super) fn is_indexable_container_type(&self, typ: &FirType) -> Option<FirType> {
        match typ {
            FirType::Ptr(inner) => Some((**inner).clone()),
            FirType::Array(inner, _) => Some((**inner).clone()),
            FirType::Vector(inner, _) => Some((**inner).clone()),
            _ => None,
        }
    }
    /// Infers the semantic value type of one FIR value node.
    ///
    /// The checker prefers symbol table information when available, but falls
    /// back to the explicit type encoded on the FIR node to remain robust to
    /// partial symbol information (notably some `kStruct` accesses).
    pub(super) fn infer_value_type(&self, id: FirId) -> Option<FirType> {
        match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, typ } => {
                // For `kStruct`, `resolve()` intentionally carries a placeholder
                // type because field names are unavailable at the type level.
                // Prefer the explicit FIR node type in that case.
                if access == AccessType::Struct {
                    Some(typ)
                } else {
                    self.resolve(&name, access).map(|e| e.typ).or(Some(typ))
                }
            }
            FirMatch::LoadTable {
                name, access, typ, ..
            } => {
                if access == AccessType::Struct {
                    Some(typ)
                } else {
                    self.resolve(&name, access)
                        .map(|e| {
                            if e.is_table {
                                e.typ
                            } else {
                                self.is_indexable_container_type(&e.typ).unwrap_or(e.typ)
                            }
                        })
                        .or(Some(typ))
                }
            }
            FirMatch::FunCall { name, typ, .. } => self
                .symbols
                .functions
                .get(&name)
                .map(|sig| sig.return_type.clone())
                .or(Some(typ)),
            _ => self.store.value_type(id),
        }
    }
    /// Returns `true` for integer scalar types accepted by index/condition rules.
    pub(super) fn is_integer_type(&self, typ: &FirType) -> bool {
        matches!(typ, FirType::Int32 | FirType::Int64)
    }
    /// Returns `true` for scalar numeric-like types used by arithmetic checks.
    ///
    /// `Bool` is intentionally included because some FIR arithmetic/logical
    /// operations allow explicit bool/int mixtures.
    pub(super) fn is_numeric_type(&self, typ: &FirType) -> bool {
        matches!(
            typ,
            FirType::Int32
                | FirType::Int64
                | FirType::Float32
                | FirType::Float64
                | FirType::FaustFloat
                | FirType::Quad
                | FirType::FixedPoint
                | FirType::Bool
        )
    }
    /// Returns `true` for floating-point-like scalar types.
    pub(super) fn is_float_like_type(&self, typ: &FirType) -> bool {
        matches!(
            typ,
            FirType::Float32 | FirType::Float64 | FirType::FaustFloat | FirType::Quad
        )
    }
    /// Returns `true` when a type is accepted as an integer/boolean condition.
    pub(super) fn is_int_or_bool_type(&self, typ: &FirType) -> bool {
        self.is_integer_type(typ) || *typ == FirType::Bool
    }
    /// Returns `true` when operands are identical or one of the allowed bool/int mixes.
    pub(super) fn same_or_int_bool_mix(&self, lhs: &FirType, rhs: &FirType) -> bool {
        lhs == rhs
            || matches!(
                (lhs, rhs),
                (FirType::Bool, FirType::Int32)
                    | (FirType::Bool, FirType::Int64)
                    | (FirType::Int32, FirType::Bool)
                    | (FirType::Int64, FirType::Bool)
            )
    }
    /// Compatibility relation used for function-call argument warnings (`FC03`).
    ///
    /// This is intentionally broader than exact type equality and allows
    /// numeric-to-numeric calls, while binops remain stricter (`B01`).
    pub(super) fn types_compatible(&self, actual: &FirType, expected: &FirType) -> bool {
        actual == expected || (self.is_numeric_type(actual) && self.is_numeric_type(expected))
    }
    /// Best-effort bit width lookup for bitcast validation.
    pub(super) fn bit_width(&self, typ: &FirType) -> Option<u32> {
        match typ {
            FirType::Bool => Some(1),
            FirType::Int32 | FirType::Float32 => Some(32),
            FirType::Int64 | FirType::Float64 => Some(64),
            FirType::Quad => Some(128),
            FirType::Ptr(_) | FirType::Obj | FirType::Sound | FirType::UI | FirType::Meta => {
                Some(64)
            }
            _ => None,
        }
    }
    /// Computes a checker-side numeric promotion target for diagnostics.
    ///
    /// This does not rewrite FIR; it is used only to evaluate whether the
    /// declared result type of operations is plausible (`B03`).
    pub(super) fn promoted_numeric_type(&self, lhs: &FirType, rhs: &FirType) -> Option<FirType> {
        if !self.is_numeric_type(lhs) || !self.is_numeric_type(rhs) {
            return None;
        }
        if lhs == rhs {
            return Some(lhs.clone());
        }
        if self.same_or_int_bool_mix(lhs, rhs) {
            return Some(
                if matches!(lhs, FirType::Int64) || matches!(rhs, FirType::Int64) {
                    FirType::Int64
                } else {
                    FirType::Int32
                },
            );
        }
        let rank = |t: &FirType| -> i32 {
            match t {
                FirType::Quad => 70,
                FirType::Float64 => 60,
                FirType::FaustFloat => 55,
                FirType::Float32 => 50,
                FirType::FixedPoint => 45,
                FirType::Int64 => 20,
                FirType::Int32 => 10,
                FirType::Bool => 0,
                _ => -1,
            }
        };
        let out = if rank(lhs) >= rank(rhs) { lhs } else { rhs };
        Some(out.clone())
    }
    /// Returns the expected result type for a binop given inferred operand types.
    pub(super) fn expected_binop_result_type(
        &self,
        op: FirBinOp,
        lhs: &FirType,
        rhs: &FirType,
    ) -> Option<FirType> {
        match op {
            FirBinOp::Eq
            | FirBinOp::Ne
            | FirBinOp::Lt
            | FirBinOp::Le
            | FirBinOp::Gt
            | FirBinOp::Ge => Some(FirType::Int32),
            FirBinOp::And | FirBinOp::Or | FirBinOp::Xor => {
                if *lhs == FirType::Bool && *rhs == FirType::Bool {
                    Some(FirType::Bool)
                } else {
                    self.promoted_numeric_type(lhs, rhs)
                }
            }
            _ => self.promoted_numeric_type(lhs, rhs),
        }
    }
    /// Detects literal zero values for division-by-zero diagnostics.
    pub(super) fn const_is_zero(&self, id: FirId) -> bool {
        match match_fir(self.store, id) {
            FirMatch::Int32 { value, .. } => value == 0,
            FirMatch::Int64 { value, .. } => value == 0,
            FirMatch::Float32 { value, .. } => value == 0.0,
            FirMatch::Float64 { value, .. } => value == 0.0,
            FirMatch::Bool { value, .. } => !value,
            _ => false,
        }
    }
}
