//! [`DelayManager`]: owns all delay-line exclusive state and provides
//! scan + allocation entry points for the FIR fast-lane delay subsystem.

use std::collections::{HashMap, HashSet};

use fir::{FirId, FirStore};
use signals::SigId;

use super::circular_pow2::GlobalCircularCursor;
use super::context::DelayFirCtx;
use super::if_wrapping;
use super::options::{self, DelayOptions};
use super::plan::DelayAnalysisEntry;
use super::{DelayKind, DelayLineInfo};
use super::{SignalFirError, SignalFirErrorCode};

// ─── DelayManager ─────────────────────────────────────────────────────────────

/// Owns all delay-line exclusive state and provides scan + allocation entry points.
///
/// Four fields form the delay manager's state:
///
/// | Field | Type | Role |
/// |-------|------|------|
/// | `options` | [`DelayOptions`] | `-mcd` / `-dlt` strategy thresholds |
/// | `delay_lines` | `HashMap<SigId, DelayLineInfo>` | Allocated buffers, keyed by carried signal |
/// | `rec_output_analysis` | `HashMap<(u32, usize), DelayAnalysisEntry>` | Read-only accumulated delay metadata per recursion output |
/// | `scheduled_delay_writes` | `HashSet<SigId>` | Dedup guard for per-sample delay writes |
///
/// # Planning / allocation flow
///
/// 1. `SignalToFirLower::prepare_delay_lines` calls [`plan_delays`] once to build
///    a [`DelayPlan`] (per-carrier max delays + recursion-output sizing metadata),
///    then stores the recursion-output map via [`Self::set_rec_output_analysis`].
/// 2. `prepare_delay_lines` allocates each owned delay line from `plan.lines`
///    through [`Self::ensure_delay_line`] using a [`DelayFirCtx`].
/// 3. During lowering, the orchestration in `module/` delegates strategy-local
///    FIR emission to [`emit_fixed_delay_for_line`] / [`emit_delay1_for_line`].
/// 4. `ensure_recursion_array_for_group` consumes the recursion-output analysis
///    to size recursion arrays that also serve as merged delay buffers.
///
/// [`plan_delays`]: super::plan::plan_delays
/// [`DelayPlan`]: super::plan::DelayPlan
/// [`emit_fixed_delay_for_line`]: super::emit_fixed_delay_for_line
/// [`emit_delay1_for_line`]: super::emit_delay1_for_line
pub(crate) struct DelayManager {
    /// Strategy selection thresholds (`-mcd` / `-dlt` options).
    options: DelayOptions,
    /// Allocated delay buffers, keyed by carried-signal id.
    delay_lines: HashMap<SigId, DelayLineInfo>,
    /// Read-only accumulated delay metadata keyed by `(rec_var_id, proj_index)`.
    rec_output_analysis: HashMap<(u32, usize), DelayAnalysisEntry>,
    /// Dedup guard: ensures the delay-write store for a given carried signal is
    /// emitted at most once per sample, even when the same signal is used by
    /// multiple `SIGDELAY` reads.
    scheduled_delay_writes: HashSet<SigId>,
}

impl DelayManager {
    /// Creates a fresh `DelayManager` for one module compilation.
    pub(crate) fn new(options: DelayOptions) -> Self {
        Self {
            options,
            delay_lines: HashMap::new(),
            rec_output_analysis: HashMap::new(),
            scheduled_delay_writes: HashSet::new(),
        }
    }

    /// Returns the configured maximum copy-shift delay threshold.
    pub(crate) fn max_copy_delay(&self) -> u32 {
        self.options.max_copy_delay
    }

    /// Returns a clone of the delay options.
    pub(crate) fn options(&self) -> DelayOptions {
        self.options.clone()
    }

    // ── Allocation ───────────────────────────────────────────────────────────

    /// Declares the struct array for one delay line, idempotent.
    ///
    /// Selects a [`DelayKind`] based on `delay` and [`DelayOptions`]:
    ///
    /// - `delay < max_copy_delay` → [`DelayKind::Shift`] (exact size, no fIOTA)
    /// - `max_copy_delay ≤ delay < delay_line_threshold` → [`DelayKind::CircularPow2`]
    ///   (power-of-two size, fIOTA declared via `ctx`)
    /// - `delay ≥ delay_line_threshold` → [`DelayKind::IfWrapping`] (exact size,
    ///   per-line `fIdx<id>` counter declared via `ctx`)
    ///
    /// On first call for `carried`, emits the struct declaration and registers an
    /// `instanceClear` zeroing loop via `ctx`.  Subsequent calls for the same
    /// `carried` return the cached info; an error is returned if the cached size is
    /// smaller than what the current delay requires.
    pub(crate) fn ensure_delay_line(
        &mut self,
        carried: SigId,
        delay: i32,
        ctx: &mut DelayFirCtx<'_>,
    ) -> Result<DelayLineInfo, SignalFirError> {
        if delay < 0 {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("SIGDELAY amount must be >= 0, got {delay}"),
            ));
        }
        // Select strategy based on delay amount and options.
        let delay_u = delay as u32;
        let strategy = options::select_delay_kind(delay_u, &self.options, carried);

        // Compute required buffer size via the unified DelayKind method.
        let required_size = strategy.buffer_size(delay)?;

        let elem_type = ctx.signal_elem_type(carried)?;

        if let Some(existing) = self.delay_lines.get(&carried) {
            if existing.size < required_size {
                return Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!(
                        "internal fast-lane delay-line sizing mismatch for signal {}: \
                         existing size {} < required {}",
                        carried.as_u32(),
                        existing.size,
                        required_size
                    ),
                ));
            }
            return Ok(existing.clone());
        }

        // Strategy-specific ancillary declarations. The global `fIOTA`
        // cursor is declared lazily (roadmap P3 slice 4): a `CircularPow2`
        // line whose carrier lives in a clock domain is reassigned to a
        // per-domain `fIOTA_d<i>` cursor before emission, so declaring the
        // global cursor here would leave a dead field + advance. It is
        // instead ensured by `finalize_global_cursor` for the lines that
        // actually keep it, plus lazily by circular recursion/delay-state
        // lowering at the top rate.
        match &strategy {
            DelayKind::CircularPow2 => {}
            DelayKind::IfWrapping { counter_name } => {
                ctx.ensure_if_wrapping_counter(counter_name.clone());
            }
            DelayKind::Shift => {}
        }

        let prefix = if elem_type == fir::FirType::Int32 {
            "iVec"
        } else {
            "fVec"
        };
        let name = format!("{prefix}{}", carried.as_u32());
        let array_ty = fir::FirType::Array(Box::new(elem_type), required_size);
        let decl = {
            let mut b = fir::FirBuilder::new(ctx.store);
            b.declare_var(name.clone(), array_ty, fir::AccessType::Struct, None)
        };
        ctx.struct_declarations.push(decl);
        ctx.register_delay_clear(name.clone(), required_size, carried)?;
        let info = DelayLineInfo {
            name,
            size: required_size,
            strategy,
            cursor: None,
            inner_clocked: false,
        };
        self.delay_lines.insert(carried, info.clone());
        Ok(info)
    }

    /// Iterates the planned delay lines (carried signal, line info).
    pub(in crate::signal_fir) fn lines(&self) -> impl Iterator<Item = (&SigId, &DelayLineInfo)> {
        self.delay_lines.iter()
    }

    /// Overrides the circular cursor of one planned line (per-domain IOTA,
    /// roadmap P3). Later `get_delay_line` clones observe the new cursor.
    pub(in crate::signal_fir) fn set_line_cursor(&mut self, carried: SigId, cursor: String) {
        if let Some(info) = self.delay_lines.get_mut(&carried) {
            info.cursor = Some(cursor);
        }
    }

    /// Returns the carriers of every planned `CircularPow2` line still using
    /// the shared global cursor (`cursor == None`) — i.e. the lines that
    /// require the global `fIOTA` to be declared/advanced (roadmap P3 slice 4).
    pub(in crate::signal_fir) fn global_circular_carriers(&self) -> Vec<SigId> {
        self.delay_lines
            .iter()
            .filter(|(_, info)| {
                info.cursor.is_none() && matches!(info.strategy, DelayKind::CircularPow2)
            })
            .map(|(&carried, _)| carried)
            .collect()
    }

    /// Marks one planned line as living inside a clocked block: its
    /// end-of-sample maintenance moves into the guarded region (roadmap P3
    /// slice 4), so `emit_sample_end_updates` skips it at the top level.
    pub(in crate::signal_fir) fn mark_line_inner(&mut self, carried: SigId) {
        if let Some(info) = self.delay_lines.get_mut(&carried) {
            info.inner_clocked = true;
        }
    }

    pub(in crate::signal_fir) fn is_line_inner(&self, carried: SigId) -> bool {
        self.delay_lines
            .get(&carried)
            .is_some_and(|info| info.inner_clocked)
    }

    /// Emits all generic delay-subsystem end-of-sample updates.
    ///
    /// This centralizes the runtime maintenance required by delay strategies
    /// and by the shared global circular cursor:
    ///
    /// - advance the shared `fIOTA` counter when any circular-pow2 line exists
    /// - advance every per-line `IfWrapping` counter
    pub(crate) fn emit_sample_end_updates(
        &self,
        store: &mut FirStore,
        uses_iota: bool,
    ) -> Vec<FirId> {
        let mut updates = Vec::new();
        if uses_iota {
            updates.push(GlobalCircularCursor.emit_advance(store));
        }
        updates.extend(self.delay_lines.values().filter_map(|info| {
            // Inner (in-block) IfWrapping counters advance inside the guarded
            // region, not at the top sample end (roadmap P3 slice 4).
            if info.inner_clocked {
                return None;
            }
            if let DelayKind::IfWrapping { counter_name } = &info.strategy {
                Some(if_wrapping::emit_if_wrapping_advance(
                    store,
                    counter_name,
                    info.size,
                ))
            } else {
                None
            }
        }));
        updates
    }

    // ── Query / dedup accessors ───────────────────────────────────────────────

    /// Schedules the delay write for `carried` if not yet scheduled.
    ///
    /// Returns `true` on the first call for a given `carried` (the write store
    /// should be emitted); `false` on subsequent calls (dedup — write already
    /// scheduled earlier in this sample).
    pub(crate) fn schedule_delay_write(&mut self, carried: SigId) -> bool {
        self.scheduled_delay_writes.insert(carried)
    }

    /// Returns the allocated delay line for `carried`, if any.
    pub(crate) fn get_delay_line(&self, carried: SigId) -> Option<&DelayLineInfo> {
        self.delay_lines.get(&carried)
    }

    /// Returns read-only delay-analysis metadata for one recursion output.
    pub(crate) fn rec_output_analysis(
        &self,
        var_id: u32,
        index: usize,
    ) -> Option<&DelayAnalysisEntry> {
        self.rec_output_analysis.get(&(var_id, index))
    }

    /// Replaces the internal rec-output analysis map with the one from a [`DelayPlan`].
    ///
    /// Called by `prepare_delay_lines` after switching to the unified [`plan_delays`] walk.
    ///
    /// [`DelayPlan`]: super::plan::DelayPlan
    /// [`plan_delays`]: super::plan::plan_delays
    pub(crate) fn set_rec_output_analysis(
        &mut self,
        rec_outputs: HashMap<(u32, usize), DelayAnalysisEntry>,
    ) {
        self.rec_output_analysis = rec_outputs;
    }
}
