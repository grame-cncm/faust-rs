//! Per-clock-domain runtime counters (roadmap P2.3).
//!
//! # Source provenance (C++)
//! - `compiler/generator/compile_scal.cpp` (`declareRetrieveIotaName`,
//!   `declareRetrieveDSName`, branch `master-dev-ocpp-od-fir-2-FIR19`,
//!   commit `8eebea429`)
//!
//! # What this provides
//! Clocked blocks advance **their own local time**: delay lines inside an
//! `ondemand`/`upsampling`/`downsampling` block index with a per-domain
//! `IOTA` cursor that only advances when the block fires, and each
//! `downsampling` block owns a modulo counter (`DSCounter`) implementing the
//! `1/H` firing guard. Both are persistent struct fields, cleared to 0,
//! **keyed by [`ClockDomainId`]** — the same clock domain always retrieves
//! the same field, two domains never share one (P0.2 made domain identity
//! per-instance, so hash-consing cannot alias them).
//!
//! The top-level (audio-rate) cursor stays the historical `fIOTA` field
//! owned by `GlobalCircularCursor`; this module only manages the clocked
//! domains. Increment statements are returned as plain FIR statements so the
//! P3 guarded-block lowering can append them to the block's post-code
//! (plan §2.4 reference code).

use ahash::AHashMap;
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirStore, FirType};
use propagate::ClockDomainId;

use super::DelayFirCtx;

/// Declare/retrieve registry for per-domain `IOTA` and `DSCounter` fields.
///
/// Owned by the lowering state; consulted by delay planning (per-domain
/// delay lines, P3.1) and by guarded-block emission (post-code increments,
/// P3.2).
#[derive(Debug)]
pub(crate) struct DomainCounters {
    iota_names: AHashMap<ClockDomainId, String>,
    ds_names: AHashMap<ClockDomainId, String>,
}

impl Default for DomainCounters {
    fn default() -> Self {
        Self {
            iota_names: AHashMap::new(),
            ds_names: AHashMap::new(),
        }
    }
}

#[allow(
    dead_code,
    reason = "wired into guarded-block lowering by roadmap P3; unit-tested now (P2.3)"
)]
impl DomainCounters {
    /// Creates an empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// C++ `declareRetrieveIotaName(clock)`: returns the per-domain `IOTA`
    /// field name, declaring the struct field (cleared to 0) on first use.
    pub(crate) fn declare_retrieve_iota(
        &mut self,
        domain: ClockDomainId,
        ctx: &mut DelayFirCtx<'_>,
    ) -> String {
        if let Some(name) = self.iota_names.get(&domain) {
            return name.clone();
        }
        let name = format!("fIOTA_d{}", domain.as_u32());
        declare_cleared_int_field(&name, ctx);
        self.iota_names.insert(domain, name.clone());
        name
    }

    /// C++ `declareRetrieveDSName(clock)`: returns the per-domain
    /// `DSCounter` field name, declaring the struct field (cleared to 0) on
    /// first use.
    pub(crate) fn declare_retrieve_ds_counter(
        &mut self,
        domain: ClockDomainId,
        ctx: &mut DelayFirCtx<'_>,
    ) -> String {
        if let Some(name) = self.ds_names.get(&domain) {
            return name.clone();
        }
        let name = format!("fDSCounter_d{}", domain.as_u32());
        declare_cleared_int_field(&name, ctx);
        self.ds_names.insert(domain, name.clone());
        name
    }

    /// Retrieves an already-declared per-domain `IOTA` name, if any.
    pub(crate) fn iota_name(&self, domain: ClockDomainId) -> Option<&str> {
        self.iota_names.get(&domain).map(String::as_str)
    }

    /// Retrieves an already-declared per-domain `DSCounter` name, if any.
    pub(crate) fn ds_counter_name(&self, domain: ClockDomainId) -> Option<&str> {
        self.ds_names.get(&domain).map(String::as_str)
    }

    /// Emits `counter = counter + 1` for one per-domain counter.
    ///
    /// The caller (P3 guarded-block lowering) appends the statement to the
    /// guarded block's post-code, so the counter advances only when the
    /// block fires — this is what makes the domain's time *local*.
    pub(crate) fn emit_increment(store: &mut FirStore, name: &str) -> FirId {
        let current = {
            let mut b = FirBuilder::new(store);
            b.load_var(name, AccessType::Struct, FirType::Int32)
        };
        let one = {
            let mut b = FirBuilder::new(store);
            b.int32(1)
        };
        let next = {
            let mut b = FirBuilder::new(store);
            b.binop(FirBinOp::Add, current, one, FirType::Int32)
        };
        let mut b = FirBuilder::new(store);
        b.store_var(name, AccessType::Struct, next)
    }

    /// Emits the wrap-to-zero form `counter = (counter + 1) % modulo` used by
    /// `downsampling` firing guards.
    pub(crate) fn emit_wrapping_increment(
        store: &mut FirStore,
        name: &str,
        modulo: FirId,
    ) -> FirId {
        let current = {
            let mut b = FirBuilder::new(store);
            b.load_var(name, AccessType::Struct, FirType::Int32)
        };
        let one = {
            let mut b = FirBuilder::new(store);
            b.int32(1)
        };
        let next = {
            let mut b = FirBuilder::new(store);
            b.binop(FirBinOp::Add, current, one, FirType::Int32)
        };
        let wrapped = {
            let mut b = FirBuilder::new(store);
            b.binop(FirBinOp::Rem, next, modulo, FirType::Int32)
        };
        let mut b = FirBuilder::new(store);
        b.store_var(name, AccessType::Struct, wrapped)
    }
}

/// Declares one persistent `Int32` struct field cleared to 0 in
/// `instanceClear`, idempotent through `clear_init_seen` (same pattern as the
/// global `fIOTA` in `circular_pow2.rs`).
fn declare_cleared_int_field(name: &str, ctx: &mut DelayFirCtx<'_>) {
    let decl = {
        let mut b = FirBuilder::new(ctx.store);
        b.declare_var(name, FirType::Int32, AccessType::Struct, None)
    };
    ctx.struct_declarations.push(decl);
    if ctx.clear_init_seen.insert(name.to_owned()) {
        let zero = {
            let mut b = FirBuilder::new(ctx.store);
            b.int32(0)
        };
        let mut b = FirBuilder::new(ctx.store);
        ctx.clear_statements
            .push(b.store_var(name, AccessType::Struct, zero));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use fir::{FirMatch, FirStore, match_fir};

    use super::*;

    fn with_ctx<R>(f: impl FnOnce(&mut DomainCounters, &mut DelayFirCtx<'_>) -> R) -> (R, Ctx) {
        let mut store = FirStore::default();
        let types = HashMap::new();
        let mut struct_declarations = Vec::new();
        let mut clear_statements = Vec::new();
        let mut clear_init_seen = HashSet::new();
        let mut next_loop_var_id = 0usize;
        let mut uses_iota = false;
        let mut counters = DomainCounters::new();
        let result = {
            let mut ctx = DelayFirCtx {
                store: &mut store,
                real_ty: FirType::Float32,
                types: &types,
                struct_declarations: &mut struct_declarations,
                clear_statements: &mut clear_statements,
                clear_init_seen: &mut clear_init_seen,
                next_loop_var_id: &mut next_loop_var_id,
                uses_iota: &mut uses_iota,
            };
            f(&mut counters, &mut ctx)
        };
        (
            result,
            Ctx {
                store,
                struct_declarations,
                clear_statements,
            },
        )
    }

    struct Ctx {
        store: FirStore,
        struct_declarations: Vec<FirId>,
        clear_statements: Vec<FirId>,
    }

    fn domain(index: u32) -> ClockDomainId {
        ClockDomainId::from_u32(index)
    }

    #[test]
    fn iota_declare_retrieve_is_idempotent_and_per_domain() {
        let ((first, second, other), ctx) = with_ctx(|counters, fir_ctx| {
            let first = counters.declare_retrieve_iota(domain(0), fir_ctx);
            let second = counters.declare_retrieve_iota(domain(0), fir_ctx);
            let other = counters.declare_retrieve_iota(domain(1), fir_ctx);
            (first, second, other)
        });
        assert_eq!(first, "fIOTA_d0");
        assert_eq!(first, second, "same domain retrieves the same field");
        assert_eq!(other, "fIOTA_d1", "distinct domains get distinct fields");
        // One declaration + one clear per distinct field.
        assert_eq!(ctx.struct_declarations.len(), 2);
        assert_eq!(ctx.clear_statements.len(), 2);
    }

    #[test]
    fn ds_counter_is_distinct_from_iota() {
        let ((iota, ds), ctx) = with_ctx(|counters, fir_ctx| {
            let iota = counters.declare_retrieve_iota(domain(3), fir_ctx);
            let ds = counters.declare_retrieve_ds_counter(domain(3), fir_ctx);
            (iota, ds)
        });
        assert_eq!(iota, "fIOTA_d3");
        assert_eq!(ds, "fDSCounter_d3");
        assert_eq!(ctx.struct_declarations.len(), 2);
        assert_eq!(ctx.clear_statements.len(), 2);
    }

    #[test]
    fn clear_statement_stores_zero_to_the_field() {
        let (name, ctx) =
            with_ctx(|counters, fir_ctx| counters.declare_retrieve_iota(domain(0), fir_ctx));
        let FirMatch::StoreVar {
            name: stored,
            access: AccessType::Struct,
            value,
        } = match_fir(&ctx.store, ctx.clear_statements[0])
        else {
            panic!("clear must be a struct StoreVar");
        };
        assert_eq!(stored, name);
        assert!(matches!(
            match_fir(&ctx.store, value),
            FirMatch::Int32 { value: 0, .. }
        ));
    }

    #[test]
    fn increment_emits_store_of_plus_one() {
        let mut store = FirStore::default();
        let stmt = DomainCounters::emit_increment(&mut store, "fIOTA_d0");
        let FirMatch::StoreVar {
            name,
            access: AccessType::Struct,
            value,
        } = match_fir(&store, stmt)
        else {
            panic!("increment must be a struct StoreVar");
        };
        assert_eq!(name, "fIOTA_d0");
        let FirMatch::BinOp {
            op: FirBinOp::Add,
            lhs,
            rhs,
            ..
        } = match_fir(&store, value)
        else {
            panic!("increment value must be an Add");
        };
        assert!(matches!(
            match_fir(&store, lhs),
            FirMatch::LoadVar { name, .. } if name == "fIOTA_d0"
        ));
        assert!(matches!(
            match_fir(&store, rhs),
            FirMatch::Int32 { value: 1, .. }
        ));
    }

    #[test]
    fn wrapping_increment_applies_modulo() {
        let mut store = FirStore::default();
        let modulo = {
            let mut b = FirBuilder::new(&mut store);
            b.int32(4)
        };
        let stmt = DomainCounters::emit_wrapping_increment(&mut store, "fDSCounter_d0", modulo);
        let FirMatch::StoreVar { value, .. } = match_fir(&store, stmt) else {
            panic!("wrapping increment must be a StoreVar");
        };
        assert!(matches!(
            match_fir(&store, value),
            FirMatch::BinOp {
                op: FirBinOp::Rem,
                ..
            }
        ));
    }
}
