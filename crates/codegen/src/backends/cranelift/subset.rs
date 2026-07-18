//! Cranelift lowering subset matcher and diagnostics.
//!
//! The backend can emit a no-op stub while lowering coverage is incomplete.
//! This module provides the deterministic pre-checks and first-gap reasons used
//! by strict validation and progress reports.

use super::*;
use crate::backends::purity::is_obviously_side_effect_free_value;

/// Fast pre-check: returns `true` when the current subset matcher accepts the
/// FIR `compute` body, `false` when the backend should fall back to a stub.
///
/// This is implemented as a thin wrapper over
/// [`compute_body_subset_gap_reason_from_compute_decl`] so the backend can keep
/// a cheap boolean decision while diagnostics tooling can request the reason.
pub(crate) fn function_body_matches_current_subset(
    store: &FirStore,
    function_decl: FirId,
    extern_data_symbols: &HashMap<String, *const c_void>,
    extern_function_symbols: &HashMap<String, *const c_void>,
) -> bool {
    function_body_subset_gap_reason_from_decl(
        store,
        function_decl,
        extern_data_symbols,
        extern_function_symbols,
    )
    .is_none()
}

/// Returns the first subset-gap reason for a FIR `compute` declaration.
///
/// `None` means the `compute` body matches the currently supported lowering
/// subset. `Some(reason)` captures the first unsupported shape encountered while
/// recursively walking statements/expressions.
///
/// The reason string is intentionally human-readable and may contain FIR debug
/// formatting; it is meant for diagnostics and prioritization, not a stable ABI.
pub(crate) fn compute_body_subset_gap_reason_from_compute_decl(
    store: &FirStore,
    compute_decl: FirId,
    extern_data_symbols: &HashMap<String, *const c_void>,
    extern_function_symbols: &HashMap<String, *const c_void>,
) -> Option<String> {
    function_body_subset_gap_reason_from_decl(
        store,
        compute_decl,
        extern_data_symbols,
        extern_function_symbols,
    )
}

pub(crate) fn function_body_subset_gap_reason_from_decl(
    store: &FirStore,
    function_decl: FirId,
    extern_data_symbols: &HashMap<String, *const c_void>,
    extern_function_symbols: &HashMap<String, *const c_void>,
) -> Option<String> {
    let body = match match_fir(store, function_decl) {
        FirMatch::DeclareFun {
            body: Some(body), ..
        } => body,
        other => return Some(format!("unsupported function declaration shape: {other:?}")),
    };
    subset_stmt_gap_reason(store, body, extern_data_symbols, extern_function_symbols)
}

/// Recursive subset matcher for FIR statements used by stub-fallback diagnostics.
///
/// The function returns the first unsupported statement/expression shape found
/// in depth-first order. This "first gap" policy keeps diagnostics concise and
/// deterministic, which is useful for corpus scans and progress tracking.
pub(crate) fn subset_stmt_gap_reason(
    store: &FirStore,
    id: FirId,
    extern_data_symbols: &HashMap<String, *const c_void>,
    extern_function_symbols: &HashMap<String, *const c_void>,
) -> Option<String> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items.into_iter().find_map(|x| {
            subset_stmt_gap_reason(store, x, extern_data_symbols, extern_function_symbols)
        }),
        FirMatch::DeclareVar {
            access: AccessType::Stack | AccessType::Loop,
            init: Some(init),
            ..
        } => subset_expr_gap_reason(store, init, extern_data_symbols, extern_function_symbols),
        FirMatch::DeclareVar {
            access: AccessType::Stack | AccessType::Loop,
            init: None,
            ..
        } => None,
        FirMatch::Label(_) => None,
        FirMatch::StoreVar {
            access: AccessType::Struct,
            value,
            ..
        } => subset_expr_gap_reason(store, value, extern_data_symbols, extern_function_symbols),
        FirMatch::StoreVar {
            access: AccessType::Stack | AccessType::Loop,
            value,
            ..
        } => subset_expr_gap_reason(store, value, extern_data_symbols, extern_function_symbols),
        FirMatch::ShiftArrayVar {
            access: AccessType::Struct,
            ..
        } => None,
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => subset_expr_gap_reason(store, cond, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                subset_stmt_gap_reason(
                    store,
                    then_block,
                    extern_data_symbols,
                    extern_function_symbols,
                )
            })
            .or_else(|| {
                else_block.and_then(|b| {
                    subset_stmt_gap_reason(store, b, extern_data_symbols, extern_function_symbols)
                })
            }),
        FirMatch::Control { cond, stmt } => {
            subset_expr_gap_reason(store, cond, extern_data_symbols, extern_function_symbols)
                .or_else(|| {
                    subset_stmt_gap_reason(
                        store,
                        stmt,
                        extern_data_symbols,
                        extern_function_symbols,
                    )
                })
        }
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => subset_expr_gap_reason(store, cond, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                cases.into_iter().find_map(|(_, stmt)| {
                    subset_stmt_gap_reason(
                        store,
                        stmt,
                        extern_data_symbols,
                        extern_function_symbols,
                    )
                })
            })
            .or_else(|| {
                default.and_then(|stmt| {
                    subset_stmt_gap_reason(
                        store,
                        stmt,
                        extern_data_symbols,
                        extern_function_symbols,
                    )
                })
            }),
        FirMatch::SimpleForLoop { upper, body, .. } => {
            subset_expr_gap_reason(store, upper, extern_data_symbols, extern_function_symbols)
                .or_else(|| {
                    subset_stmt_gap_reason(
                        store,
                        body,
                        extern_data_symbols,
                        extern_function_symbols,
                    )
                })
        }
        FirMatch::ForLoop {
            init,
            end,
            step,
            body,
            ..
        } => subset_expr_gap_reason(store, init, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                subset_expr_gap_reason(store, end, extern_data_symbols, extern_function_symbols)
            })
            .or_else(|| {
                subset_expr_gap_reason(store, step, extern_data_symbols, extern_function_symbols)
            })
            .or_else(|| {
                subset_stmt_gap_reason(store, body, extern_data_symbols, extern_function_symbols)
            }),
        FirMatch::WhileLoop { cond, body } => {
            subset_expr_gap_reason(store, cond, extern_data_symbols, extern_function_symbols)
                .or_else(|| {
                    subset_stmt_gap_reason(
                        store,
                        body,
                        extern_data_symbols,
                        extern_function_symbols,
                    )
                })
        }
        FirMatch::StoreTable {
            access: AccessType::Stack,
            index,
            value,
            ..
        } => subset_expr_gap_reason(store, index, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                subset_expr_gap_reason(store, value, extern_data_symbols, extern_function_symbols)
            }),
        FirMatch::StoreTable {
            access: AccessType::Struct,
            index,
            value,
            ..
        } => subset_expr_gap_reason(store, index, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                subset_expr_gap_reason(store, value, extern_data_symbols, extern_function_symbols)
            }),
        FirMatch::Drop(v) if is_obviously_side_effect_free_value(store, v) => None,
        FirMatch::Drop(v) => {
            subset_expr_gap_reason(store, v, extern_data_symbols, extern_function_symbols)
        }
        FirMatch::NullStatement | FirMatch::Return(None) => None,
        other => Some(format!("unsupported stmt variant in subset: {other:?}")),
    }
}

/// Recursive subset matcher for FIR expressions used by stub-fallback diagnostics.
///
/// This matcher intentionally mirrors the expression coverage expected by the
/// current lowering implementation (`ComputeLowering::lower_expr` and friends).
/// When new lowering support is added, this function should be updated in the
/// same change so subset pre-checks and diagnostics stay aligned.
pub(crate) fn subset_expr_gap_reason(
    store: &FirStore,
    id: FirId,
    extern_data_symbols: &HashMap<String, *const c_void>,
    extern_function_symbols: &HashMap<String, *const c_void>,
) -> Option<String> {
    match match_fir(store, id) {
        FirMatch::Int32 { .. }
        | FirMatch::Bool { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. } => None,
        FirMatch::DeclareVar {
            access: AccessType::Stack | AccessType::Loop,
            init: Some(init),
            ..
        } => subset_expr_gap_reason(store, init, extern_data_symbols, extern_function_symbols),
        FirMatch::DeclareVar {
            access: AccessType::Stack | AccessType::Loop,
            init: None,
            ..
        } => None,
        FirMatch::LoadVar {
            name,
            access: AccessType::Global,
            ..
        } => {
            if extern_data_symbols.contains_key(&name) {
                None
            } else {
                Some(format!(
                    "external data symbol `{name}` not found in Cranelift options"
                ))
            }
        }
        FirMatch::LoadVar {
            access: AccessType::Stack | AccessType::FunArgs | AccessType::Loop | AccessType::Struct,
            ..
        } => None,
        FirMatch::LoadTable {
            access: AccessType::Stack,
            index,
            ..
        } => subset_expr_gap_reason(store, index, extern_data_symbols, extern_function_symbols),
        FirMatch::LoadTable {
            access: AccessType::FunArgs,
            index,
            ..
        } => subset_expr_gap_reason(store, index, extern_data_symbols, extern_function_symbols),
        FirMatch::LoadTable {
            access: AccessType::Struct,
            index,
            ..
        } => subset_expr_gap_reason(store, index, extern_data_symbols, extern_function_symbols),
        FirMatch::LoadTable {
            access: AccessType::Static,
            index,
            ..
        } => subset_expr_gap_reason(store, index, extern_data_symbols, extern_function_symbols),
        FirMatch::BinOp { lhs, rhs, .. } => {
            subset_expr_gap_reason(store, lhs, extern_data_symbols, extern_function_symbols)
                .or_else(|| {
                    subset_expr_gap_reason(store, rhs, extern_data_symbols, extern_function_symbols)
                })
        }
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => subset_expr_gap_reason(store, cond, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                subset_expr_gap_reason(
                    store,
                    then_value,
                    extern_data_symbols,
                    extern_function_symbols,
                )
            })
            .or_else(|| {
                subset_expr_gap_reason(
                    store,
                    else_value,
                    extern_data_symbols,
                    extern_function_symbols,
                )
            }),
        FirMatch::Neg { value, .. } => {
            subset_expr_gap_reason(store, value, extern_data_symbols, extern_function_symbols)
        }
        FirMatch::FunCall { name, args, .. } => {
            if extern_function_symbols.contains_key(&name) {
                args.into_iter().find_map(|x| {
                    subset_expr_gap_reason(store, x, extern_data_symbols, extern_function_symbols)
                })
            } else if fir::FirMathOp::from_symbol(&name).is_none()
                && !matches!(
                    name.as_str(),
                    "abs"
                        | "min_i"
                        | "max_i"
                        | "isnanf"
                        | "isnan"
                        | "isinff"
                        | "isinf"
                        | "copysignf"
                        | "copysign"
                        | "acoshf"
                        | "acosh"
                        | "asinhf"
                        | "asinh"
                        | "atanhf"
                        | "atanh"
                        | "coshf"
                        | "cosh"
                        | "sinhf"
                        | "sinh"
                        | "tanhf"
                        | "tanh"
                )
            {
                Some(format!("unsupported math call in subset: {name}"))
            } else {
                args.into_iter().find_map(|x| {
                    subset_expr_gap_reason(store, x, extern_data_symbols, extern_function_symbols)
                })
            }
        }
        FirMatch::Cast { value, .. } => {
            subset_expr_gap_reason(store, value, extern_data_symbols, extern_function_symbols)
        }
        FirMatch::LoadSoundfileLength { part, .. } => {
            subset_expr_gap_reason(store, part, extern_data_symbols, extern_function_symbols)
        }
        FirMatch::LoadSoundfileRate { part, .. } => {
            subset_expr_gap_reason(store, part, extern_data_symbols, extern_function_symbols)
        }
        FirMatch::LoadSoundfileBuffer {
            chan, part, idx, ..
        } => subset_expr_gap_reason(store, chan, extern_data_symbols, extern_function_symbols)
            .or_else(|| {
                subset_expr_gap_reason(store, part, extern_data_symbols, extern_function_symbols)
            })
            .or_else(|| {
                subset_expr_gap_reason(store, idx, extern_data_symbols, extern_function_symbols)
            }),
        other => Some(format!("unsupported expr variant in subset: {other:?}")),
    }
}
