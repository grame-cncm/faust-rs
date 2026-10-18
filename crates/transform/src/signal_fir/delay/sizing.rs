//! Pure delay-line sizing free functions.
//!
//! These functions have no FIR side-effects; they only resolve delay amounts
//! and compute buffer sizes from signal trees and type annotations.

use std::collections::HashMap;

use signals::{SigId, SigMatch, match_sig};
use sigtype::{SigType, check_delay_interval};
use tlib::TreeArena;

use super::SignalFirError;
use super::SignalFirErrorCode;

// ─── pow2limit_for_delay ─────────────────────────────────────────────────────

/// Computes `next_power_of_two(delay + 1)` — the circular buffer size for
/// a given maximum delay in samples.
///
/// A delay of `N` samples requires reading `N` positions behind the write
/// pointer.  With a power-of-two size `S`, the mask `S - 1` covers all valid
/// offsets `0..=N`, so the minimum `S` is `next_power_of_two(N + 1)`.
///
/// # Errors
///
/// Returns [`SignalFirErrorCode::UnsupportedSignalNode`] if `delay` is
/// negative, if `delay + 1` overflows `usize`, or if the result would exceed
/// `usize::MAX` (i.e. `delay >= usize::MAX / 2`).
///
/// | `delay` | result |
/// |---------|--------|
/// | `0` | `1` (passthrough needs 1 slot) |
/// | `1` | `2` |
/// | `3` | `4` |
/// | `4` | `8` |
/// | `10` | `16` |
pub(crate) fn pow2limit_for_delay(delay: i32) -> Result<usize, SignalFirError> {
    let delay = usize::try_from(delay).map_err(|_| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!("SIGDELAY amount must be >= 0, got {delay}"),
        )
    })?;
    let requested = delay.checked_add(1).ok_or_else(|| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            "SIGDELAY amount overflow while sizing delay line",
        )
    })?;
    requested.checked_next_power_of_two().ok_or_else(|| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!("SIGDELAY amount too large to size delay line: {delay}"),
        )
    })
}

// ─── Delay amount resolution ──────────────────────────────────────────────────

/// Returns the constant integer value of `sig` if it is a `SIGINT` literal,
/// otherwise `None`.
///
/// This is the fast path for compile-time constant delay amounts (e.g. `x @ 3`
/// after constant propagation).  The returned value is the exact delay in
/// samples; callers should pass it directly to [`pow2limit_for_delay`].
pub(crate) fn constant_delay_amount(
    arena: &TreeArena,
    sig: SigId,
) -> Result<Option<i32>, SignalFirError> {
    match match_sig(arena, sig) {
        SigMatch::Int(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}

/// Returns the interval upper-bound used to size the delay line for a
/// variable delay amount, mirroring C++ `checkDelayInterval`.
///
/// Accepts any signal whose full type annotation has a non-empty, bounded
/// (finite `hi`), non-negative interval.  `hi == 0` is the zero-delay
/// passthrough case.
///
/// Returns `None` when:
/// - `sig` has no type entry in `sig_types`,
/// - the interval is empty or unbounded (infinite `hi`), or
/// - `hi < 0` (negative delay, semantically invalid).
///
/// # C++ correspondence
///
/// Mirrors `compiler/signals/sigtype.cpp::checkDelayInterval` (Rust:
/// `sigtype::check_delay_interval`), which rejects any
/// delay with an interval whose upper bound cannot be determined or is
/// negative.
pub(crate) fn variable_delay_max_bound(
    sig_types: &HashMap<SigId, SigType>,
    sig: SigId,
) -> Option<i32> {
    let ty = sig_types.get(&sig)?;
    if ty.interval().hi() < 0.0 {
        return None;
    }
    check_delay_interval(ty).ok()
}

/// Returns a structural upper bound for a delay expression when interval
/// analysis cannot determine a finite bound.
///
/// If `sig` is `SIGMIN(SigInt(n), _)` or `SIGMIN(_, SigInt(n))` with
/// `n >= 0`, returns `n` as a conservative upper bound.  This covers the
/// standard `de.delay(n, d, x) = x @ min(n, max(0, d))` pattern, where
/// the first argument to `min` is an explicit compile-time ceiling.
///
/// # When this fires
///
/// With correct `FConst` typing (`Interval::new_default()` rather than
/// `empty()`), `fSamplingFreq`-based expressions (e.g. `ma.SR`) produce a
/// finite bounded interval through standard interval algebra and do not reach
/// this fallback.  This method acts as defence-in-depth for any remaining
/// case where interval analysis yields an empty or unbounded result.
pub(crate) fn min_const_upper_bound(arena: &TreeArena, sig: SigId) -> Option<i32> {
    let SigMatch::Min(lhs, rhs) = match_sig(arena, sig) else {
        return None;
    };
    let as_nonneg_int = |id: SigId| -> Option<i32> {
        if let SigMatch::Int(n) = match_sig(arena, id)
            && n >= 0
        {
            return Some(n);
        }
        None
    };
    as_nonneg_int(lhs).or_else(|| as_nonneg_int(rhs))
}

/// Resolves the delay line allocation size for a delay `amount` signal.
///
/// Tries three strategies in order of specificity:
///
/// 1. **Literal `SIGINT`** — exact compile-time constant via
///    [`constant_delay_amount`].
/// 2. **Bounded interval** — interval upper bound from the type annotator
///    via [`variable_delay_max_bound`].  Covers slider-driven and
///    `fSamplingFreq`-derived amounts after type propagation.
/// 3. **Structural `SIGMIN` fallback** — conservative upper bound from
///    `SIGMIN(SigInt(n), _)` patterns via [`min_const_upper_bound`].
///    Defence-in-depth for cases where interval analysis still yields empty.
///
/// Returns `None` when no bound can be determined; the caller should report
/// an `UnsupportedSignalNode` error.
///
/// Returns `Some(0)` for a zero-delay (passthrough) amount.
pub(crate) fn delay_size_for_amount(
    arena: &TreeArena,
    sig_types: &HashMap<SigId, SigType>,
    amount: SigId,
) -> Result<Option<i32>, SignalFirError> {
    if let Some(c) = constant_delay_amount(arena, amount)? {
        return Ok(Some(c));
    }
    if let Some(b) = variable_delay_max_bound(sig_types, amount) {
        return Ok(Some(b));
    }
    Ok(min_const_upper_bound(arena, amount))
}
