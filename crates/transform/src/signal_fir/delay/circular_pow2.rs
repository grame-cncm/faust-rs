//! Power-of-two circular delay-line strategy.
//!
//! Every active delay line is backed by one DSP-struct array (`fVec*` or
//! `iVec*`) of size `S = next_power_of_two(max_delay + 1)`.  A shared
//! integer counter `fIOTA` advances by 1 each sample and serves as the write
//! pointer.  Reads use a masked offset: `array[(fIOTA - N) & (S - 1)]`.
//!
//! ```text
//! write: array[fIOTA & (S-1)]  = current_value;
//! read:  array[(fIOTA - N) & (S-1)]
//! end-of-sample: fIOTA = fIOTA + 1;
//! ```

use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirStore, FirType};

use super::arith::DelayArith;
use super::{DelayFirCtx, DelayLineInfo, DelayLoweringCtx};

// ─── masked_delay_index ───────────────────────────────────────────────────────

/// Applies the power-of-two ring-buffer mask: `index & (size - 1)`.
pub(crate) fn masked_delay_index(store: &mut FirStore, index: FirId, size: usize) -> FirId {
    let mut e = DelayArith(store);
    let mask = e.i32c(i32::try_from(size.saturating_sub(1)).unwrap_or(i32::MAX));
    e.and_mask(index, mask)
}

// ─── GlobalCircularCursor ─────────────────────────────────────────────────────

/// Shared runtime cursor used by all global masked circular-storage paths.
///
/// Today this is materialized as the persistent struct field `fIOTA`. It is
/// shared by `CircularPow2` delay lines and by circular recursion carriers
/// lowered from `module/`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GlobalCircularCursor;

impl GlobalCircularCursor {
    /// Declares and clears the shared `fIOTA` state, idempotent.
    pub(crate) fn ensure_state(self, ctx: &mut DelayFirCtx<'_>) {
        if *ctx.uses_iota {
            return;
        }
        *ctx.uses_iota = true;
        let zero = {
            let mut b = FirBuilder::new(ctx.store);
            b.int32(0)
        };
        let decl = {
            let mut b = FirBuilder::new(ctx.store);
            b.declare_var("fIOTA", FirType::Int32, AccessType::Struct, None)
        };
        ctx.struct_declarations.push(decl);
        if ctx.clear_init_seen.insert("fIOTA".to_owned()) {
            let mut b = FirBuilder::new(ctx.store);
            ctx.clear_statements
                .push(b.store_var("fIOTA", AccessType::Struct, zero));
        }
    }

    /// Emits `fIOTA = fIOTA + 1` to advance the cursor by one sample.
    pub(crate) fn emit_advance(self, store: &mut FirStore) -> FirId {
        cursor_advance(store, "fIOTA")
    }
}

// ─── Name-parametrized cursor primitives (roadmap P3: per-domain IOTA) ───────

/// Loads one circular cursor field.
pub(crate) fn cursor_load(store: &mut FirStore, name: &str) -> FirId {
    let mut b = FirBuilder::new(store);
    b.load_var(name, AccessType::Struct, FirType::Int32)
}

/// Masked current write index `cursor & (size - 1)`.
pub(crate) fn cursor_current_index(store: &mut FirStore, name: &str, size: usize) -> FirId {
    let cursor = cursor_load(store, name);
    masked_delay_index(store, cursor, size)
}

/// Masked delayed read index `(cursor - amount) & (size - 1)`.
pub(crate) fn cursor_delayed_index(
    store: &mut FirStore,
    name: &str,
    amount: FirId,
    size: usize,
) -> FirId {
    let cursor = cursor_load(store, name);
    let raw = {
        let mut b = FirBuilder::new(store);
        b.binop(FirBinOp::Sub, cursor, amount, FirType::Int32)
    };
    masked_delay_index(store, raw, size)
}

/// Emits `cursor = cursor + 1`.
pub(crate) fn cursor_advance(store: &mut FirStore, name: &str) -> FirId {
    let next = {
        let cursor = cursor_load(store, name);
        let one = {
            let mut b = FirBuilder::new(store);
            b.int32(1)
        };
        let mut b = FirBuilder::new(store);
        b.binop(FirBinOp::Add, cursor, one, FirType::Int32)
    };
    let mut b = FirBuilder::new(store);
    b.store_var(name, AccessType::Struct, next)
}

// ─── Strategy emit functions ─────────────────────────────────────────────────

/// Emits one `SIGDELAY(value, amount)` read/write sequence for the CircularPow2 strategy.
pub(super) fn emit_fixed_delay(
    ctx: &mut DelayLoweringCtx<'_>,
    line: &DelayLineInfo,
    current: FirId,
    amount: FirId,
    read_ty: FirType,
    schedule_write: bool,
) -> FirId {
    let cursor = line.cursor.as_deref().unwrap_or("fIOTA");
    if schedule_write {
        let write_index = cursor_current_index(ctx.store, cursor, line.size);
        let mut b = FirBuilder::new(ctx.store);
        ctx.immediate_statements.push(b.store_table(
            line.name.clone(),
            AccessType::Struct,
            write_index,
            current,
        ));
    }
    let read_index = cursor_delayed_index(ctx.store, cursor, amount, line.size);
    let mut b = FirBuilder::new(ctx.store);
    b.load_table(line.name.clone(), AccessType::Struct, read_index, read_ty)
}

/// Emits one `Delay1(value)` read/write sequence for the CircularPow2 strategy.
///
/// Delay1 is fixed_delay with amount = 1.
pub(super) fn emit_delay1(
    ctx: &mut DelayLoweringCtx<'_>,
    line: &DelayLineInfo,
    current: FirId,
    read_ty: FirType,
    schedule_write: bool,
) -> FirId {
    let one = {
        let mut b = FirBuilder::new(ctx.store);
        b.int32(1)
    };
    emit_fixed_delay(ctx, line, current, one, read_ty, schedule_write)
}
