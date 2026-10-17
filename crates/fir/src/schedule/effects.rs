//! The alias model of the W2/W3 producers (design note §2).
//!
//! `schedule::check` has its own implementation of the same model, so that a
//! mistake here is not repeated by the checker.

use crate::{AccessType, FirId, FirMatch, FirMathOp, FirStore, match_fir};

/// A memory or local location.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Addr {
    /// A struct or static scalar, or the whole struct/static array.
    Field(String),
    /// Element `k` (a FIR `Int32` constant) of a struct/static array.
    Elem(String, i32),
    /// A stack, loop or argument scalar local.
    Local(String),
    /// Every stack table, audio buffers included (they may alias in place).
    Stack,
    /// Anything: an unknown statement or an impure call.
    All,
}

impl Addr {
    /// Whether a write of `self` may change what a read of `other` returns.
    /// The producers index their maps by name instead; the tests check that
    /// both agree.
    #[cfg(test)]
    pub(crate) fn conflicts(&self, other: &Addr) -> bool {
        match (self, other) {
            (Addr::All, _) | (_, Addr::All) => true,
            (Addr::Field(a), Addr::Field(b) | Addr::Elem(b, _))
            | (Addr::Elem(a, _), Addr::Field(b)) => a == b,
            (Addr::Elem(a, i), Addr::Elem(b, j)) => a == b && i == j,
            (Addr::Local(a), Addr::Local(b)) => a == b,
            (Addr::Stack, Addr::Stack) => true,
            _ => false,
        }
    }
}

/// Reads and writes of a value or a statement subtree.
#[derive(Default, Debug)]
pub(crate) struct Effects {
    pub(crate) reads: Vec<Addr>,
    pub(crate) writes: Vec<Addr>,
}

/// Pure C functions the backends call: math operators and math imports.
pub(crate) fn is_pure_function(name: &str) -> bool {
    FirMathOp::from_symbol(name).is_some()
        || matches!(
            name,
            "abs"
                | "max_i"
                | "min_i"
                | "acosh"
                | "acoshf"
                | "asinh"
                | "asinhf"
                | "atanh"
                | "atanhf"
                | "cosh"
                | "coshf"
                | "sinh"
                | "sinhf"
                | "tanh"
                | "tanhf"
                | "copysign"
                | "copysignf"
                | "isinf"
                | "isinff"
                | "isnan"
                | "isnanf"
        )
}

/// The address a struct/static table access touches.
pub(crate) fn table_addr(store: &FirStore, name: &str, index: FirId) -> Addr {
    match match_fir(store, index) {
        FirMatch::Int32 { value, .. } => Addr::Elem(name.to_owned(), value),
        _ => Addr::Field(name.to_owned()),
    }
}

/// Collects the reads of value `id`; returns whether it is pure. An impure
/// value also records a write of `All`.
pub(crate) fn value_effects(store: &FirStore, id: FirId, eff: &mut Effects) -> bool {
    match match_fir(store, id) {
        FirMatch::Bool { .. }
        | FirMatch::Int32 { .. }
        | FirMatch::Int64 { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. }
        | FirMatch::NullValue { .. } => true,
        FirMatch::LoadVar { name, access, .. } => match access {
            AccessType::Struct | AccessType::Static => {
                eff.reads.push(Addr::Field(name));
                true
            }
            AccessType::Stack | AccessType::Loop | AccessType::FunArgs => {
                eff.reads.push(Addr::Local(name));
                true
            }
            AccessType::Global => {
                eff.writes.push(Addr::All);
                false
            }
        },
        FirMatch::LoadTable {
            name,
            access,
            index,
            ..
        } => {
            let index_pure = value_effects(store, index, eff);
            match access {
                AccessType::Struct | AccessType::Static => {
                    eff.reads.push(table_addr(store, &name, index));
                }
                AccessType::Stack => {
                    eff.reads.push(Addr::Local(name));
                    eff.reads.push(Addr::Stack);
                }
                // The `inputs`/`outputs` pointer arrays: never written by compute.
                AccessType::FunArgs => {}
                AccessType::Loop | AccessType::Global => {
                    eff.writes.push(Addr::All);
                    return false;
                }
            }
            index_pure
        }
        FirMatch::BinOp { lhs, rhs, .. } => {
            let l = value_effects(store, lhs, eff);
            let r = value_effects(store, rhs, eff);
            l && r
        }
        FirMatch::Neg { value, .. }
        | FirMatch::Cast { value, .. }
        | FirMatch::Bitcast { value, .. } => value_effects(store, value, eff),
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            let c = value_effects(store, cond, eff);
            let t = value_effects(store, then_value, eff);
            let e = value_effects(store, else_value, eff);
            c && t && e
        }
        FirMatch::FunCall { name, args, .. } => {
            let mut pure = is_pure_function(&name);
            for arg in args {
                pure &= value_effects(store, arg, eff);
            }
            if !pure {
                eff.writes.push(Addr::All);
            }
            pure
        }
        _ => {
            eff.writes.push(Addr::All);
            false
        }
    }
}

/// Collects the reads and writes of statement `id`, with its whole subtree.
pub(crate) fn stmt_effects(store: &FirStore, id: FirId, eff: &mut Effects) {
    match match_fir(store, id) {
        FirMatch::Block(items) => {
            for item in items {
                stmt_effects(store, item, eff);
            }
        }
        FirMatch::DeclareVar {
            name,
            access: AccessType::Stack | AccessType::Loop,
            init,
            ..
        } => {
            if let Some(init) = init {
                value_effects(store, init, eff);
            }
            eff.writes.push(Addr::Local(name));
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            value_effects(store, value, eff);
            match access {
                AccessType::Struct | AccessType::Static => eff.writes.push(Addr::Field(name)),
                AccessType::Stack | AccessType::Loop => eff.writes.push(Addr::Local(name)),
                _ => eff.writes.push(Addr::All),
            }
        }
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => {
            value_effects(store, index, eff);
            value_effects(store, value, eff);
            match access {
                AccessType::Struct | AccessType::Static => {
                    eff.writes.push(table_addr(store, &name, index));
                }
                AccessType::Stack => {
                    eff.reads.push(Addr::Local(name));
                    eff.writes.push(Addr::Stack);
                }
                _ => eff.writes.push(Addr::All),
            }
        }
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            value_effects(store, cond, eff);
            stmt_effects(store, then_block, eff);
            if let Some(else_block) = else_block {
                stmt_effects(store, else_block, eff);
            }
        }
        FirMatch::Control { cond, stmt } => {
            value_effects(store, cond, eff);
            stmt_effects(store, stmt, eff);
        }
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            value_effects(store, cond, eff);
            for (_, block) in cases {
                stmt_effects(store, block, eff);
            }
            if let Some(default) = default {
                stmt_effects(store, default, eff);
            }
        }
        FirMatch::SimpleForLoop {
            var, upper, body, ..
        } => {
            value_effects(store, upper, eff);
            eff.writes.push(Addr::Local(var.clone()));
            eff.reads.push(Addr::Local(var));
            stmt_effects(store, body, eff);
        }
        FirMatch::ForLoop {
            var,
            init,
            end,
            step,
            body,
            ..
        } => {
            value_effects(store, init, eff);
            value_effects(store, end, eff);
            value_effects(store, step, eff);
            eff.writes.push(Addr::Local(var.clone()));
            eff.reads.push(Addr::Local(var));
            stmt_effects(store, body, eff);
        }
        FirMatch::WhileLoop { cond, body } => {
            value_effects(store, cond, eff);
            stmt_effects(store, body, eff);
        }
        FirMatch::Drop(value) => {
            value_effects(store, value, eff);
        }
        FirMatch::Label(_) | FirMatch::NullStatement => {}
        _ => eff.writes.push(Addr::All),
    }
}

/// The effects of statement `id`.
pub(crate) fn effects_of(store: &FirStore, id: FirId) -> Effects {
    let mut eff = Effects::default();
    stmt_effects(store, id, &mut eff);
    eff
}
