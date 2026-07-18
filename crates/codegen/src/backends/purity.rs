//! Conservative FIR value-purity predicate shared by textual backends.
//!
//! C++ provenance: `DropInst` roots are often introduced while the upstream
//! instruction compiler materializes a shared expression DAG. A backend may
//! omit such a root only when evaluating its value is structurally unable to
//! perform a write, allocation, or foreign call. This is intentionally not a
//! general mathematical-purity analysis: unknown nodes and every `FunCall`
//! remain effectful by default.

use fir::{FirId, FirMatch, FirStore, match_fir};

/// Returns `true` only for FIR values whose evaluation cannot have an effect.
///
/// A false result merely preserves redundant target text. A false positive
/// could erase an observable foreign call or write, so unsupported nodes are
/// always retained.
pub(super) fn is_obviously_side_effect_free_value(store: &FirStore, value: FirId) -> bool {
    match match_fir(store, value) {
        FirMatch::Int32 { .. }
        | FirMatch::Int64 { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. }
        | FirMatch::Bool { .. }
        | FirMatch::Quad { .. }
        | FirMatch::FixedPoint { .. }
        | FirMatch::Int32Array { .. }
        | FirMatch::Float32Array { .. }
        | FirMatch::Float64Array { .. }
        | FirMatch::QuadArray { .. }
        | FirMatch::FixedPointArray { .. }
        | FirMatch::LoadVar { .. }
        | FirMatch::LoadVarAddress { .. }
        | FirMatch::NullValue { .. } => true,
        FirMatch::LoadTable { index, .. } => is_obviously_side_effect_free_value(store, index),
        FirMatch::ValueArray { values, .. } => values
            .iter()
            .all(|&item| is_obviously_side_effect_free_value(store, item)),
        FirMatch::BinOp { lhs, rhs, .. } => {
            is_obviously_side_effect_free_value(store, lhs)
                && is_obviously_side_effect_free_value(store, rhs)
        }
        FirMatch::Neg { value, .. }
        | FirMatch::Cast { value, .. }
        | FirMatch::Bitcast { value, .. } => is_obviously_side_effect_free_value(store, value),
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            is_obviously_side_effect_free_value(store, cond)
                && is_obviously_side_effect_free_value(store, then_value)
                && is_obviously_side_effect_free_value(store, else_value)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_obviously_side_effect_free_value;
    use fir::{AccessType, FirBinOp, FirBuilder, FirStore, FirType};

    #[test]
    fn predicate_retains_foreign_calls_and_accepts_plain_arithmetic() {
        let mut store = FirStore::new();
        let mut builder = FirBuilder::new(&mut store);
        let one = builder.float32(1.0);
        let local = builder.load_var("local", AccessType::Stack, FirType::Float32);
        let sum = builder.binop(FirBinOp::Add, one, local, FirType::Float32);
        let foreign = builder.fun_call("foreign_side_effect", &[], FirType::Float32);

        assert!(is_obviously_side_effect_free_value(&store, sum));
        assert!(!is_obviously_side_effect_free_value(&store, foreign));
    }
}
