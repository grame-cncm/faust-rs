//! Delay-line strategy selection options.
//!
//! [`DelayOptions`] mirrors the Faust `-mcd` / `-dlt` compiler options and
//! drives the strategy selector inside [`super::DelayManager::ensure_delay_line`].

use signals::SigId;

use super::DelayKind;

// ─── DelayOptions ─────────────────────────────────────────────────────────────

/// Delay-line strategy selection thresholds.
///
/// Mirror of the Faust `-mcd` / `-dlt` compiler options:
///
/// - `-mcd N` (max-copy-delay, default 16): delays < N use the shift/copy
///   strategy (no `fIOTA`).
/// - `-dlt N` (delay-line threshold, default `u32::MAX`): delays ≥ N use the
///   if-based wrapping strategy; delays in `[mcd, dlt)` use the default
///   power-of-two circular strategy.
#[derive(Clone, Debug)]
pub(crate) struct DelayOptions {
    /// Shift/copy model upper bound (exclusive).  Default: 16.
    pub(crate) max_copy_delay: u32,
    /// If-based wrapping model lower bound (inclusive).  Default: `u32::MAX`
    /// (disabled; all non-copy delays use the circular-pow2 model).
    pub(crate) delay_line_threshold: u32,
}

impl Default for DelayOptions {
    fn default() -> Self {
        Self {
            max_copy_delay: 16,
            delay_line_threshold: u32::MAX,
        }
    }
}

// ─── Strategy selector ────────────────────────────────────────────────────────

/// Selects the [`DelayKind`] strategy for a delay amount `delay_u` based on
/// the configured thresholds and the carried signal id.
///
/// - `delay_u < max_copy_delay` → [`DelayKind::Shift`]
/// - `max_copy_delay ≤ delay_u < delay_line_threshold` → [`DelayKind::CircularPow2`]
/// - `delay_u ≥ delay_line_threshold` → [`DelayKind::IfWrapping`]
pub(super) fn select_delay_kind(
    delay_u: u32,
    options: &DelayOptions,
    carried: SigId,
    clock_context: Option<u32>,
) -> DelayKind {
    if delay_u < options.max_copy_delay {
        DelayKind::Shift
    } else if delay_u < options.delay_line_threshold {
        DelayKind::CircularPow2
    } else {
        DelayKind::IfWrapping {
            counter_name: contextual_name("fIdx", carried, clock_context),
        }
    }
}

pub(super) fn contextual_name(prefix: &str, signal: SigId, clock_context: Option<u32>) -> String {
    match clock_context {
        Some(domain) => format!("{prefix}{}_d{domain}", signal.as_u32()),
        None => format!("{prefix}{}", signal.as_u32()),
    }
}
