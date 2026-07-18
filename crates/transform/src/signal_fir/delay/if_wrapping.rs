//! If-based wrapping delay-line strategy.
//!
//! Uses an exact-size buffer (size = `max_delay + 1`) with a dedicated
//! per-line integer counter.  The counter wraps to zero via an `if` comparison
//! instead of a bitmask, saving memory for non-power-of-two delay sizes at the
//! cost of a branch per write.
//!
//! ```text
//! buf[idx] = current_value;
//! read:  buf[(idx + size - N) select2-wrapped]
//! end-of-sample: idx = (idx + 1 >= size) ? 0 : idx + 1;
//! ```

use fir::{AccessType, FirBuilder, FirId, FirStore, FirType};

use super::SignalFirError;
use super::SignalFirErrorCode;
use super::arith::DelayArith;
use super::{DelayLineInfo, DelayLoweringCtx};

// ─── buffer_size ─────────────────────────────────────────────────────────────

/// Minimum buffer size for the IfWrapping strategy: `delay + 1` (exact).
pub(super) fn buffer_size(max_delay: i32) -> Result<usize, SignalFirError> {
    usize::try_from(max_delay).map(|d| d + 1).map_err(|_| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!("SIGDELAY amount overflow: {max_delay}"),
        )
    })
}

// ─── Emission ─────────────────────────────────────────────────────────────────

/// Computes the read index for an `IfWrapping` delay line:
/// `(counter + size - amount)` with if-based wrap when `≥ size`.
pub(super) fn if_wrapping_read_index(
    store: &mut FirStore,
    counter_name: &str,
    amount: FirId,
    size: usize,
) -> FirId {
    let size_i32 = i32::try_from(size).unwrap_or(i32::MAX);
    let mut e = DelayArith(store);
    let counter = e.load_counter(counter_name);
    let size_fir = e.i32c(size_i32);
    let plus_size = e.add(counter, size_fir);
    let raw = e.sub(plus_size, amount);
    let size_fir2 = e.i32c(size_i32);
    let cond = e.ge(raw, size_fir2);
    let size_fir3 = e.i32c(size_i32);
    let adjusted = e.sub(raw, size_fir3);
    e.select2(cond, adjusted, raw)
}

/// Emits `counter = (counter + 1 >= size) ? 0 : counter + 1` for an
/// `IfWrapping` delay line counter advance.
pub(super) fn bump_if_wrapping_counter(
    store: &mut FirStore,
    counter_name: &str,
    size: usize,
) -> FirId {
    let size_i32 = i32::try_from(size).unwrap_or(i32::MAX);
    let mut e = DelayArith(store);
    let counter = e.load_counter(counter_name);
    let one = e.i32c(1);
    let next = e.add(counter, one);
    let size_fir = e.i32c(size_i32);
    let cond = e.ge(next, size_fir);
    let zero = e.i32c(0);
    let wrapped = e.select2(cond, zero, next);
    e.store_counter(counter_name, wrapped)
}

/// Emits the end-of-sample counter advance for one `IfWrapping` delay line.
pub(crate) fn emit_if_wrapping_advance(
    store: &mut FirStore,
    counter_name: &str,
    size: usize,
) -> FirId {
    bump_if_wrapping_counter(store, counter_name, size)
}

// ─── Strategy emit functions ─────────────────────────────────────────────────

/// Emits one `SIGDELAY(value, amount)` read/write sequence for the IfWrapping strategy.
pub(super) fn emit_fixed_delay(
    ctx: &mut DelayLoweringCtx<'_>,
    line: &DelayLineInfo,
    current: FirId,
    amount: FirId,
    read_ty: FirType,
    schedule_write: bool,
    counter_name: &str,
) -> FirId {
    if schedule_write {
        let write_index = {
            let mut b = FirBuilder::new(ctx.store);
            b.load_var(counter_name, AccessType::Struct, FirType::Int32)
        };
        let mut b = FirBuilder::new(ctx.store);
        ctx.immediate_statements.push(b.store_table(
            line.name.clone(),
            AccessType::Struct,
            write_index,
            current,
        ));
    }
    let read_index = if_wrapping_read_index(ctx.store, counter_name, amount, line.size);
    let mut b = FirBuilder::new(ctx.store);
    b.load_table(line.name.clone(), AccessType::Struct, read_index, read_ty)
}

/// Emits one `Delay1(value)` read/write sequence for the IfWrapping strategy.
///
/// Delay1 is fixed_delay with amount = 1.
pub(super) fn emit_delay1(
    ctx: &mut DelayLoweringCtx<'_>,
    line: &DelayLineInfo,
    current: FirId,
    read_ty: FirType,
    schedule_write: bool,
    counter_name: &str,
) -> FirId {
    let one = {
        let mut b = FirBuilder::new(ctx.store);
        b.int32(1)
    };
    emit_fixed_delay(
        ctx,
        line,
        current,
        one,
        read_ty,
        schedule_write,
        counter_name,
    )
}
