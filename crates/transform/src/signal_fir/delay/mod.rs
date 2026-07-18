//! Circular delay-line sizing, geometry, and state management for the FIR fast-lane.
//!
//! Faust's `@(n)` operator maps to one of three delay-line strategies, selected
//! by the delay amount relative to two thresholds (`-mcd` / `-dlt`):
//!
//! ```text
//! [1, max_copy_delay)                    → Shift         (shift/copy; no fIOTA)
//! [max_copy_delay, delay_line_threshold) → CircularPow2  (shared fIOTA + mask)
//! [delay_line_threshold, ∞)              → IfWrapping    (per-line counter)
//! ```
//!
//! This module provides:
//!
//! 1. The pure *sizing/analysis layer*: deciding how large a buffer needs to be
//!    and resolving the delay amount from the signal tree.
//! 2. The [`DelayManager`] component: owns all delay-exclusive state (allocated
//!    lines, recursion-merged delays, write-scheduling dedup) and provides the
//!    scan and allocation entry points.
//! 3. [`DelayFirCtx`]: a borrow bundle assembled from disjoint fields of
//!    `SignalToFirLower` and passed to `DelayManager` methods that emit FIR nodes.
//! 4. [`DelayKind`]: the single cohesive strategy enum — carries its own buffer
//!    sizing, fixed-delay emission, and single-sample emission.  Replaces the
//!    former `RingDelayModel` / `DelayStrategyEmitter` trait split.
//! 5. [`DelayLoweringCtx`]: borrow bundle for lowering-time FIR emission; the
//!    two public thin wrappers [`emit_fixed_delay_for_line`] /
//!    [`emit_delay1_for_line`] delegate to `DelayKind` methods.
//!
//! # Module layout
//!
//! The subsystem is split across the `delay/` directory. This `mod.rs` holds the
//! spine — the [`DelayKind`] strategy enum, [`DelayLineInfo`], and the
//! `emit_*_for_line` lowering wrappers — plus the module re-exports. The rest is
//! one concern per file:
//!
//! - `manager.rs` — [`DelayManager`] state, sizing decisions, allocation.
//! - `plan.rs` — [`DelayPlan`] + the `plan_delays` / `DelayPlanner` single-pass walk.
//! - `context.rs` — [`DelayFirCtx`] / [`DelayLoweringCtx`] borrow bundles.
//! - `shift.rs` / `circular_pow2.rs` / `if_wrapping.rs` — the three strategies.
//! - `sizing.rs` — pure delay-amount → size resolution.
//! - `options.rs` — [`DelayOptions`] + the strategy selector.
//! - `arith.rs` — the shared `DelayArith` FIR-expression helper.
//!
//! # Strategy descriptions
//!
//! ## Shift/copy (`DelayKind::Shift`, delays ≤ `-mcd`, default 16)
//!
//! Each sample, all buffer elements are shifted one slot toward the high end,
//! and the new value is placed at index 0.  Read is a direct load at index
//! equal to the delay amount.  No `fIOTA` is used.
//!
//! ```text
//! buf[size-1] = buf[size-2]; ... buf[1] = buf[0];  (shift loop)
//! buf[0] = current_value;
//! read:  buf[N]
//! ```
//!
//! ## Power-of-two circular (`DelayKind::CircularPow2`, default middle range)
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
//!
//! ## If-based wrapping (`DelayKind::IfWrapping`, delays > `-dlt`)
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
//!
//! # Recursion + delay merging
//!
//! When a recursion output is consumed through a delay chain
//! `Delay1^k(Proj(i, group))` — either from an active `SYMREF` feedback edge or
//! from a top-level `SYMREC` projection — the recursion array for output `i`
//! is sized to hold the full delayed history so no separate `fVec` is needed.
//!
//! The planning split is:
//!
//! - [`plan_delays`] produces a [`DelayPlan`] with both per-carrier max delays
//!   and recursion-output sizing metadata in one DAG walk.
//! - `prepare_delay_lines` allocates each line through [`DelayManager::ensure_delay_line`].
//! - `ensure_recursion_array_for_group` in `module/` consumes the recursion-output
//!   analysis to size recursion carriers.
//!
//! Standalone `Delay1(x)` nodes that use the shift strategy are also recorded
//! during the same planning walk so their buffer geometry is chosen once up front and
//! later reused by the lowering phase without allocation side effects.
//!
//! # Scope of this module
//!
//! | Item | Kind | Purpose |
//! |------|------|---------|
//! | [`pow2limit_for_delay`] | free fn | `next_power_of_two(N + 1)` with overflow checks |
//! | [`constant_delay_amount`] | free fn | Extract a literal `SIGINT` delay amount |
//! | [`variable_delay_max_bound`] | free fn | Derive a bound from interval analysis |
//! | [`min_const_upper_bound`] | free fn | Structural fallback: `SIGMIN(SigInt(n), _)` |
//! | [`delay_size_for_amount`] | free fn | Unified resolver (tries all three above) |
//! | [`DelayOptions`] | struct | `-mcd` / `-dlt` threshold options |
//! | [`DelayKind`] | enum | Strategy + all per-strategy FIR emission behaviour |
//! | [`DelayLineInfo`] | struct | Metadata for one allocated delay buffer |
//! | [`DelayManager`] | struct | Owns delay-exclusive state; scan + alloc methods |
//! | [`DelayFirCtx`] | struct | Borrow bundle for FIR-emitting methods |
//! | [`GlobalCircularCursor`] | struct | Shared `fIOTA` cursor service for circular storage |
//! | [`plan_delays`] | free fn | Unified single-pass delay planner |
//! | [`DelayPlan`] | struct | Pure-data delay decision; input to allocation |
//!
//! The complementary **stateful orchestration** layer (`lower_fixed_delay`,
//! `lower_delay_state`, recursion-carrier resolution, and `lower_signal`
//! dispatch) remains in `module/`.
//!
//! Source provenance (C++):
//! - `compiler/transform/signalFIRCompiler.hh` — `DelayLine`, `allocateDelayLineAux`
//! - `compiler/transform/signalFIRCompiler.cpp` — `compileSigDelay`, `writeReadDelay`,
//!   `checkDelayInterval`

use fir::{FirId, FirType};

use super::error::{SignalFirError, SignalFirErrorCode};

// ─── Sub-modules ──────────────────────────────────────────────────────────────

mod arith;
mod circular_pow2;
mod context;
mod domain_counters;
mod if_wrapping;
mod manager;
mod options;
mod plan;
mod shift;
mod sizing;

pub(super) use circular_pow2::GlobalCircularCursor;
pub(crate) use circular_pow2::{cursor_current_index, cursor_delayed_index};
pub(super) use context::{DelayFirCtx, DelayLoweringCtx};
pub(super) use domain_counters::DomainCounters;
pub(crate) use if_wrapping::emit_if_wrapping_advance;
pub(super) use manager::DelayManager;
pub(super) use options::DelayOptions;
pub(super) use plan::plan_delays;
pub(super) use sizing::{delay_size_for_amount, pow2limit_for_delay};

// ─── DelayLineInfo ────────────────────────────────────────────────────────────

/// Metadata for one allocated delay-line DSP-struct array.
///
/// The array is named `fVec<id>` (real) or `iVec<id>` (integer), declared
/// during `prepare_delay_lines` and zeroed in `instanceClear`.
///
/// Source provenance (C++):
/// - `compiler/transform/signalFIRCompiler.hh` (`DelayLine`, `allocateDelayLineAux`)
/// - `compiler/transform/signalFIRCompiler.cpp` (`compileSigDelay`, `writeReadDelay`)
#[derive(Clone, Debug)]
pub(super) struct DelayLineInfo {
    /// Generated DSP-struct array variable name (e.g. `fVec42`).
    pub(super) name: String,
    /// Allocated buffer size in elements.
    ///
    /// For `CircularPow2` this is always a power of two ≥ 1.
    /// For `Shift` and `IfWrapping` this is `max_delay + 1` (exact).
    pub(super) size: usize,
    /// Buffer-geometry strategy selected for this line.
    pub(super) strategy: DelayKind,
    /// Circular-cursor override for `CircularPow2` lines whose carrier lives
    /// inside a clock domain: the per-domain `fIOTA_d<i>` field name
    /// (roadmap P3). `None` = the shared global `fIOTA`.
    pub(super) cursor: Option<String>,
    /// `true` when the carrier lives inside a clocked block: the line's
    /// end-of-sample maintenance is emitted inside the guarded region
    /// (fire-gated), not at the top sample end (roadmap P3 slice 4).
    pub(super) inner_clocked: bool,
}

// ─── DelayKind ────────────────────────────────────────────────────────────────

/// Buffer-geometry strategy for a single allocated delay line, with all
/// per-strategy FIR emission behaviour attached.
///
/// This is the single source of truth for each strategy: buffer sizing, fixed
/// delay reads/writes, single-sample reads/writes, and end-of-sample advances
/// all live here.  There are no shared cross-strategy enums or impossible
/// branches — each arm only ever touches the state it owns.
///
/// | Variant | Buffer | Pointer |
/// |---------|--------|---------|
/// | `Shift` | exact `N+1` | none (shift loop) |
/// | `CircularPow2` | `next_pow2(N+1)` | shared `fIOTA`, masked |
/// | `IfWrapping` | exact `N+1` | per-line `fIdx*`, if-wrap |
#[derive(Clone, Debug)]
pub(super) enum DelayKind {
    /// Shift/copy: buffer shifted one slot per sample; new value at index 0;
    /// read at index = delay amount.  No `fIOTA`.  Buffer size = `delay + 1`.
    Shift,
    /// Power-of-two circular buffer driven by the shared `fIOTA` counter.
    /// Buffer size = `next_power_of_two(delay + 1)`.
    CircularPow2,
    /// Per-line if-based wrapping counter; exact buffer size (`delay + 1`).
    /// Each line has its own `fIdx<sig_id>` struct variable.
    IfWrapping {
        /// Name of the per-line counter variable, e.g. `fIdx42`.
        counter_name: String,
    },
}

impl DelayKind {
    // ── Sizing ────────────────────────────────────────────────────────────────

    /// Minimum buffer size for a maximum delay of `max_delay` samples.
    pub(super) fn buffer_size(&self, max_delay: i32) -> Result<usize, SignalFirError> {
        match self {
            DelayKind::Shift => shift::buffer_size(max_delay),
            DelayKind::CircularPow2 => pow2limit_for_delay(max_delay),
            DelayKind::IfWrapping { .. } => if_wrapping::buffer_size(max_delay),
        }
    }

    // ── Lowering ──────────────────────────────────────────────────────────────

    /// Emits one `SIGDELAY(value, amount)` read/write sequence.
    ///
    /// When `schedule_write` is true, emits the store before the read and
    /// schedules any shift copies / advances.
    pub(super) fn emit_fixed_delay(
        &self,
        ctx: &mut DelayLoweringCtx<'_>,
        line: &DelayLineInfo,
        current: FirId,
        amount: FirId,
        read_ty: FirType,
        schedule_write: bool,
    ) -> FirId {
        match self {
            DelayKind::Shift => {
                shift::emit_fixed_delay(ctx, line, current, amount, read_ty, schedule_write)
            }
            DelayKind::CircularPow2 => {
                circular_pow2::emit_fixed_delay(ctx, line, current, amount, read_ty, schedule_write)
            }
            DelayKind::IfWrapping { counter_name } => if_wrapping::emit_fixed_delay(
                ctx,
                line,
                current,
                amount,
                read_ty,
                schedule_write,
                counter_name,
            ),
        }
    }

    /// Emits one `Delay1(value)` read/write sequence (amount = 1 sample).
    pub(super) fn emit_delay1(
        &self,
        ctx: &mut DelayLoweringCtx<'_>,
        line: &DelayLineInfo,
        current: FirId,
        read_ty: FirType,
        schedule_write: bool,
    ) -> FirId {
        match self {
            DelayKind::Shift => shift::emit_delay1(ctx, line, current, read_ty, schedule_write),
            DelayKind::CircularPow2 => {
                circular_pow2::emit_delay1(ctx, line, current, read_ty, schedule_write)
            }
            DelayKind::IfWrapping { counter_name } => {
                if_wrapping::emit_delay1(ctx, line, current, read_ty, schedule_write, counter_name)
            }
        }
    }
}

/// Thin wrapper: emits one `SIGDELAY(value, amount)` sequence.
///
/// Delegates immediately to [`DelayKind::emit_fixed_delay`]; callers in
/// `module/` are unchanged.
pub(super) fn emit_fixed_delay_for_line(
    ctx: &mut DelayLoweringCtx<'_>,
    line: &DelayLineInfo,
    current: FirId,
    amount: FirId,
    read_ty: FirType,
    schedule_write: bool,
) -> FirId {
    line.strategy
        .emit_fixed_delay(ctx, line, current, amount, read_ty, schedule_write)
}

/// Thin wrapper: emits one `Delay1(value)` sequence.
///
/// Delegates immediately to [`DelayKind::emit_delay1`]; callers in
/// `module/` are unchanged.
pub(super) fn emit_delay1_for_line(
    ctx: &mut DelayLoweringCtx<'_>,
    line: &DelayLineInfo,
    current: FirId,
    read_ty: FirType,
    schedule_write: bool,
) -> FirId {
    line.strategy
        .emit_delay1(ctx, line, current, read_ty, schedule_write)
}
