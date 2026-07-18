//! Recursive-group carrier data model for the `signal_fir` fast-lane.
//!
//! This module owns the explicit recursion abstractions used by `module/`
//! during recursive-group lowering:
//!
//! - carrier storage strategy
//! - carrier metadata
//! - canonical resolved carrier references
//! - canonical resolved delayed recursion reads
//!
//! It also owns most of the recursion-specific helper logic needed by
//! `module/`:
//!
//! - active-vs-materialized carrier resolution
//! - delayed recursion-chain matching (`Delay1^k(Proj(...))`)
//! - recursive-group projection decoding/validation
//! - carrier allocation and clear-loop registration
//! - recursion-specific FIR helper emission for current writes/finalization
//!
//! `module/` still owns the final orchestration decisions:
//!
//! - when a top-level recursion group must be materialized,
//! - recursive body evaluation order,
//! - integration of recursion updates into sample phases.

use std::collections::{HashMap, HashSet};

use fir::helpers::{emit_reverse_array_shift_loop, fresh_loop_var};
use fir::{AccessType, FirBuilder, FirId, FirStore, FirType};
use signals::{SigId, SigMatch, match_sig};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};

use super::delay::{DelayManager, pow2limit_for_delay};
use super::error::{SignalFirError, SignalFirErrorCode};

/// Storage strategy used by one recursion carrier.
///
/// This names the two concrete runtime representations used by the fast-lane:
///
/// - a single scalar state cell for the simplest one-sample feedback loops,
/// - an exact-size shift-style carrier for small delayed-feedback cases,
/// - a larger circular carrier when delay analysis found deeper delayed reads
///   beyond the copy-delay threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecursionStorageStrategy {
    /// One scalar state cell holding the previous sample.
    ///
    /// The current sample value is materialized separately as a stack-local
    /// binding during the sample iteration, then committed back to the struct
    /// field in `PostOutput`.
    SingleScalar,
    /// Exact-size shift carrier:
    /// - current sample in slot `0`
    /// - delayed reads at slot `N`
    /// - post-output finalization shifts `slot[k + 1] = slot[k]`
    ExactShift,
    /// Circular carrier larger than the copy-delay threshold, indexed by the
    /// shared global circular cursor (`fIOTA`).
    Circular,
}

/// Carrier metadata for one output of a recursive group (`SIGPROJ(i, SYMREC(…))`).
///
/// Each output body in a recursion group gets its own carrier declaration.
/// The carrier uses one of three storage strategies:
///
/// - [`RecursionStorageStrategy::SingleScalar`] for eligible simple unary
///   feedback loops
/// - [`RecursionStorageStrategy::ExactShift`] for the default small shifted
///   case
/// - [`RecursionStorageStrategy::Circular`] when accumulated delay analysis
///   upsizes the carrier beyond the copy-delay threshold
///
/// Source provenance (C++): `signalFIRCompiler.cpp` (`generateRecProj`,
/// `generateRec`), emitted as `fRecN[2]` / `iRecN[2]`.
#[derive(Clone, Debug)]
pub(super) struct RecArrayInfo {
    /// Generated DSP-struct array variable name (e.g. `fRec7`).
    pub(super) name: String,
    /// Element type (`Int32` for integer recursion, `Float32`/`Float64` otherwise).
    pub(super) typ: FirType,
    /// Allocated carrier size in elements.
    ///
    /// - `1` for the scalar fast path,
    /// - `>1` for exact-shift or circular carriers serving delayed reads
    ///   directly.
    pub(super) size: usize,
    /// Selected runtime storage strategy for this carrier.
    pub(super) strategy: RecursionStorageStrategy,
}

impl RecArrayInfo {
    /// Returns the already selected runtime storage strategy.
    pub(super) fn storage_strategy(&self) -> RecursionStorageStrategy {
        if self.size == 1 {
            RecursionStorageStrategy::SingleScalar
        } else {
            self.strategy
        }
    }
}

/// Stack-local current-sample binding used by scalar recursion carriers.
#[derive(Clone, Debug)]
pub(super) struct RecursionCurrentValueBinding {
    pub(super) name: String,
    pub(super) typ: FirType,
}

/// Canonically resolved recursion carrier.
///
/// This is the state-independent “answer object” returned by recursion lookup:
/// callers get both the concrete carrier metadata and its resolved storage
/// strategy without having to recompute that split themselves.
#[derive(Clone, Debug)]
pub(super) struct RecursionCarrierRef {
    pub(super) info: RecArrayInfo,
    pub(super) strategy: RecursionStorageStrategy,
}

impl RecursionCarrierRef {
    /// Builds a canonical resolved carrier from already allocated array info.
    pub(super) fn new(info: RecArrayInfo) -> Self {
        let strategy = info.storage_strategy();
        Self { info, strategy }
    }
}

/// Canonically resolved delayed recursion read.
///
/// Represents a successful match of a recursion-rooted delay chain such as
/// `Proj(i, group)`, `Delay1(Proj(...))`, or `Delay1^k(Proj(...))`, together
/// with the recursion carrier that should serve the read.
#[derive(Clone, Debug)]
pub(super) struct RecursionDelayRef {
    pub(super) carrier: RecursionCarrierRef,
    pub(super) implicit_delay: usize,
}

/// Recursion lookup input recovered from a `Proj(...)` optionally wrapped in
/// `Delay1^k(...)`.
#[derive(Clone, Copy, Debug)]
pub(super) struct RecursionDelayKey {
    pub(super) proj_node: SigId,
    pub(super) proj_index: i32,
    pub(super) group: SigId,
    pub(super) implicit_delay: usize,
}

/// Decoded and validated recursive-group projection shape.
///
/// `module/` uses this as the structural payload of `SIGPROJ(index, group)`
/// after validation:
///
/// - the symbolic recursion binder id,
/// - the full body list for the group,
/// - the canonical output index after unary-group normalization.
#[derive(Clone, Debug)]
pub(super) struct RecursionGroupProjection {
    pub(super) var: SigId,
    pub(super) bodies: Vec<SigId>,
    pub(super) canonical_index: usize,
}

/// Owned recursion-group state for the fast-lane lowerer.
///
/// This bundles all runtime-independent recursion bookkeeping that used to be
/// spread across `module/`:
///
/// - allocated carriers keyed by `(group, body index)`,
/// - the stack of currently active recursive groups while lowering bodies,
/// - the matching symbolic recursion variables for that stack,
/// - per-sample scheduling dedup for recursive body materialization.
#[derive(Default)]
pub(super) struct RecursionState {
    /// Maps `(group_id, body_index)` to the recursion array allocated for that
    /// output slot of a recursion group.
    pub(super) rec_array_by_group_index: HashMap<(u32, usize), RecArrayInfo>,
    /// Resolves a symbolic recursion variable to its owning `SYMREC` group.
    ///
    /// C++ uses `Tree` identity and vector-name properties for this binding.
    /// Rust records it explicitly so a scheduled `Proj(SYMREF)` can reserve or
    /// read the owning carrier before `Proj(SYMREC)` emits the group update.
    pub(super) group_by_var: HashMap<u32, SigId>,
    /// `group.as_u32()` values for groups that are `ReverseTimeRec` (LTI
    /// adjoint) wrappers.
    ///
    /// Dormant under the 2026-05-10 RAD dispatcher change; kept compilable for
    /// a future LTI fast-path revival.
    ///
    /// Only these carriers must be zeroed in the `compute()` preamble (via
    /// `emit_reverse_time_rec_compute_resets`).  Normal SYMREC primal carriers
    /// — including those allocated while lowering the forward body of a
    /// `SigBlockReverseAD` group — are persistent DSP state that must NOT be
    /// reset per-block.
    pub(super) reverse_time_rec_group_ids: HashSet<u32>,
    /// Stack of active recursion carrier groups, innermost last.
    pub(super) recursion_stack: Vec<Vec<RecArrayInfo>>,
    /// Stack of active symbolic recursion variables matching `recursion_stack`.
    pub(super) recursion_vars: Vec<SigId>,
    /// Current-sample bindings for scalar recursion carriers keyed by
    /// `(group_id, body_index)`.
    pub(super) current_value_by_group_index: HashMap<(u32, usize), RecursionCurrentValueBinding>,
    /// Groups whose recursive body pass has already been scheduled this sample.
    pub(super) scheduled_groups: HashSet<SigId>,
}

impl RecursionState {
    /// Registers one symbolic recursion binder before scheduled lowering.
    pub(super) fn register_group(&mut self, var: SigId, group: SigId) -> Option<SigId> {
        self.group_by_var.insert(var.as_u32(), group)
    }

    /// Resolves either a `SYMREC` group or a `SYMREF` variable to the owning
    /// canonical group used as the carrier-map key.
    pub(super) fn canonical_group(&self, arena: &TreeArena, group: SigId) -> Option<SigId> {
        if match_sym_rec(arena, group).is_some() {
            return Some(group);
        }
        if let SigMatch::ReverseTimeRec(body) = match_sig(arena, group)
            && match_sym_rec(arena, body).is_some()
        {
            return Some(group);
        }
        match_sym_ref(arena, group).and_then(|var| self.group_by_var.get(&var.as_u32()).copied())
    }

    /// Returns the already materialized carrier metadata for one recursion
    /// output slot, if that slot has been allocated.
    pub(super) fn carrier_info(&self, group: SigId, index: usize) -> Option<RecArrayInfo> {
        self.rec_array_by_group_index
            .get(&(group.as_u32(), index))
            .cloned()
    }

    /// Pushes one recursion group onto the active lowering stack.
    pub(super) fn push_active_group(&mut self, var: SigId, arrays: Vec<RecArrayInfo>) {
        self.recursion_vars.push(var);
        self.recursion_stack.push(arrays);
    }

    /// Pops the innermost recursion group from the active lowering stack.
    pub(super) fn pop_active_group(&mut self) {
        self.recursion_stack.pop();
        self.recursion_vars.pop();
    }

    /// Marks a recursion group as already scheduled for body lowering in the
    /// current sample and returns `true` only on the first mark.
    pub(super) fn mark_group_scheduled(&mut self, group: SigId) -> bool {
        self.scheduled_groups.insert(group)
    }

    /// Records the stack-local current-sample binding for one scalar carrier.
    pub(super) fn set_current_value_binding(
        &mut self,
        group: SigId,
        index: usize,
        binding: RecursionCurrentValueBinding,
    ) {
        self.current_value_by_group_index
            .insert((group.as_u32(), index), binding);
    }

    /// Returns the current-sample binding for one scalar carrier, if any.
    pub(super) fn current_value_binding(
        &self,
        arena: &TreeArena,
        group: SigId,
        index: usize,
    ) -> Option<RecursionCurrentValueBinding> {
        let group = self.canonical_group(arena, group)?;
        let canonical_index = canonical_group_index(arena, group, index)?;
        self.current_value_by_group_index
            .get(&(group.as_u32(), canonical_index))
            .cloned()
    }

    /// Resolves a carrier from already materialized recursion-group storage.
    ///
    /// This path is used for top-level `SYMREC` groups after `lower_proj(...)`
    /// has allocated their carriers.
    ///
    /// The projection index is first canonicalized so unary groups always map
    /// to slot `0` even if a structurally odd `Proj(k, group)` reaches here.
    pub(super) fn resolve_materialized_carrier(
        &self,
        arena: &TreeArena,
        group: SigId,
        index: usize,
    ) -> Option<RecursionCarrierRef> {
        let group = self.canonical_group(arena, group)?;
        let canonical_index = canonical_group_index(arena, group, index)?;
        self.carrier_info(group, canonical_index)
            .map(RecursionCarrierRef::new)
    }

    /// Resolves a recursion carrier from either the active lowering stack or
    /// the materialized carrier map.
    ///
    /// Active `SYMREF` recursion has priority so recursive bodies can break
    /// cycles by reading the carrier currently being constructed.
    pub(super) fn resolve_carrier(
        &self,
        arena: &TreeArena,
        group: SigId,
        index: usize,
    ) -> Result<Option<RecursionCarrierRef>, SignalFirError> {
        if let Some(carrier) = resolve_active_recursion_carrier(arena, self, group, index)? {
            return Ok(Some(carrier));
        }
        Ok(self.resolve_materialized_carrier(arena, group, index))
    }

    /// Resolves a delay chain rooted at a recursion projection against the
    /// current recursion state, without triggering new materialization.
    pub(super) fn resolve_delay_ref(
        &self,
        arena: &TreeArena,
        value: SigId,
    ) -> Result<Option<RecursionDelayRef>, SignalFirError> {
        let Some(key) = match_recursion_delay_key(arena, value) else {
            return Ok(None);
        };
        let proj_index = usize::try_from(key.proj_index).map_err(|_| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "negative SIGPROJ index {} in recursion delay lookup",
                    key.proj_index
                ),
            )
        })?;
        Ok(self
            .resolve_carrier(arena, key.group, proj_index)?
            .map(|carrier| RecursionDelayRef {
                carrier,
                implicit_delay: key.implicit_delay,
            }))
    }
}

/// Borrow bundle for recursion-specific FIR emission used by `module/`.
///
/// This keeps the recursion-specific load/store/finalize details out of
/// `module/` while still letting the module-level orchestrator decide when
/// those statements belong to the immediate or post-output sample phases.
pub(super) struct RecursionLoweringCtx<'a> {
    pub(super) store: &'a mut FirStore,
    pub(super) immediate_statements: &'a mut Vec<FirId>,
    pub(super) post_output_statements: &'a mut Vec<FirId>,
    pub(super) next_loop_var_id: &'a mut usize,
}

impl RecursionLoweringCtx<'_> {
    /// Chooses the runtime current-slot index for one array-backed carrier.
    ///
    /// Exact-shift carriers always write/read slot `0` for the current sample.
    /// Circular carriers use the caller-provided current circular index.
    pub(super) fn current_index_for_carrier(
        &mut self,
        info: &RecArrayInfo,
        zero_index: FirId,
        circular_index: FirId,
    ) -> FirId {
        match info.storage_strategy() {
            RecursionStorageStrategy::SingleScalar => zero_index,
            RecursionStorageStrategy::ExactShift => zero_index,
            RecursionStorageStrategy::Circular => circular_index,
        }
    }

    /// Emits a load of the feedback-visible carrier value.
    ///
    /// For scalar carriers this reads the struct field directly. For array
    /// carriers the caller supplies the already chosen runtime index.
    pub(super) fn load_feedback_carrier(
        &mut self,
        info: &RecArrayInfo,
        current_index: FirId,
        read_ty: FirType,
    ) -> FirId {
        let mut b = FirBuilder::new(self.store);
        match info.storage_strategy() {
            RecursionStorageStrategy::SingleScalar => {
                b.load_var(info.name.clone(), AccessType::Struct, read_ty)
            }
            RecursionStorageStrategy::ExactShift | RecursionStorageStrategy::Circular => b
                .load_table(
                    info.name.clone(),
                    AccessType::Struct,
                    current_index,
                    read_ty,
                ),
        }
    }

    /// Schedules the current-sample write into an array-backed recursion
    /// carrier.
    pub(super) fn emit_current_carrier_store(
        &mut self,
        info: &RecArrayInfo,
        current_index: FirId,
        value: FirId,
    ) {
        debug_assert_ne!(
            info.storage_strategy(),
            RecursionStorageStrategy::SingleScalar
        );
        let mut b = FirBuilder::new(self.store);
        self.immediate_statements.push(b.store_table(
            info.name.clone(),
            AccessType::Struct,
            current_index,
            value,
        ));
    }

    /// Schedules the post-output finalize shifts for one exact-shift recursion
    /// carrier.
    pub(super) fn emit_exact_shift_finalize_copies(&mut self, info: &RecArrayInfo) {
        debug_assert_eq!(
            info.storage_strategy(),
            RecursionStorageStrategy::ExactShift
        );
        let delay = info.size.saturating_sub(1);
        if delay <= 2 {
            for dst in (1..info.size).rev() {
                let src = dst - 1;
                let src_index = {
                    let mut b = FirBuilder::new(self.store);
                    b.int32(i32::try_from(src).unwrap_or(i32::MAX))
                };
                let src_value = {
                    let mut b = FirBuilder::new(self.store);
                    b.load_table(
                        info.name.clone(),
                        AccessType::Struct,
                        src_index,
                        info.typ.clone(),
                    )
                };
                let dst_index = {
                    let mut b = FirBuilder::new(self.store);
                    b.int32(i32::try_from(dst).unwrap_or(i32::MAX))
                };
                let shift_store = {
                    let mut b = FirBuilder::new(self.store);
                    b.store_table(info.name.clone(), AccessType::Struct, dst_index, src_value)
                };
                self.post_output_statements.push(shift_store);
            }
        } else {
            self.post_output_statements
                .push(emit_reverse_array_shift_loop(
                    self.store,
                    self.next_loop_var_id,
                    "jRec",
                    &info.name,
                    i32::try_from(delay).unwrap_or(i32::MAX),
                    info.typ.clone(),
                    AccessType::Struct,
                ));
        }
    }

    /// Emits all current writes and exact-shift finalize copies for one recursive
    /// group body pass.
    pub(super) fn emit_group_body_updates(
        &mut self,
        group_arrays: &[RecArrayInfo],
        body_values: &[FirId],
        current_indexes: &[FirId],
    ) {
        debug_assert_eq!(group_arrays.len(), body_values.len());
        debug_assert_eq!(group_arrays.len(), current_indexes.len());
        for i in 0..group_arrays.len() {
            let info = &group_arrays[i];
            match info.storage_strategy() {
                RecursionStorageStrategy::SingleScalar => {}
                RecursionStorageStrategy::ExactShift => {
                    self.emit_current_carrier_store(info, current_indexes[i], body_values[i]);
                    self.emit_exact_shift_finalize_copies(info);
                }
                RecursionStorageStrategy::Circular => {
                    self.emit_current_carrier_store(info, current_indexes[i], body_values[i]);
                }
            }
        }
    }
}

/// Borrow bundle for recursive-group carrier allocation and zero-init
/// registration.
///
/// This isolates the mutable pieces required to declare recursion arrays and
/// their `instanceClear` loops, so `module/` can request carrier allocation
/// without owning the low-level declaration details.
pub(super) struct RecursionAllocCtx<'a> {
    pub(super) arena: &'a TreeArena,
    pub(super) delay: &'a DelayManager,
    pub(super) store: &'a mut FirStore,
    pub(super) struct_declarations: &'a mut Vec<FirId>,
    pub(super) clear_statements: &'a mut Vec<FirId>,
    pub(super) clear_init_seen: &'a mut HashSet<String>,
    pub(super) next_loop_var_id: &'a mut usize,
    pub(super) recursion: &'a mut RecursionState,
}

impl RecursionAllocCtx<'_> {
    /// Generates a unique loop variable name for `instanceClear` helper loops.
    fn fresh_loop_var(&mut self, prefix: &str) -> String {
        fresh_loop_var(self.next_loop_var_id, prefix)
    }

    /// Registers the `instanceClear` zero-fill loop for one recursion array.
    ///
    /// The registration is idempotent so repeated allocation lookups cannot
    /// duplicate clear-time initialization.
    fn register_clear_recursion_array(&mut self, name: String, init: FirId, size: usize) {
        if !self.clear_init_seen.insert(name.clone()) {
            return;
        }
        let loop_var = self.fresh_loop_var("lRec");
        let upper = {
            let mut b = FirBuilder::new(self.store);
            b.int32(i32::try_from(size).unwrap_or(i32::MAX))
        };
        let body = {
            let index = {
                let mut b = FirBuilder::new(self.store);
                b.load_var(loop_var.clone(), AccessType::Loop, FirType::Int32)
            };
            let store_node = {
                let mut b = FirBuilder::new(self.store);
                b.store_table(name, AccessType::Struct, index, init)
            };
            let mut b = FirBuilder::new(self.store);
            b.block(&[store_node])
        };
        let mut b = FirBuilder::new(self.store);
        self.clear_statements
            .push(b.simple_for_loop(loop_var, upper, body, false));
    }

    /// Registers the `instanceClear` zero-init store for one scalar recursion
    /// carrier.
    fn register_clear_recursion_scalar(&mut self, name: String, init: FirId) {
        if !self.clear_init_seen.insert(name.clone()) {
            return;
        }
        let mut b = FirBuilder::new(self.store);
        self.clear_statements
            .push(b.store_var(name, AccessType::Struct, init));
    }

    /// Declares a recursion carrier for output slot `index` of recursion
    /// `group`, idempotent.
    ///
    /// The carrier is:
    ///
    /// - scalar (`size = 1`) for eligible unary feedback outputs whose maximum
    ///   observed delayed read does not exceed one sample,
    /// - exact-shift for small array-backed delayed-feedback paths,
    /// - circular when accumulated delay analysis recorded deeper delayed reads
    ///   beyond the configured copy-delay threshold.
    ///
    /// This is where recursion carriers pick up delay-analysis-driven upsizing
    /// from `delay/`.
    ///
    /// Naming follows the existing fast-lane convention:
    ///
    /// - first output slot: `fRec<group>` / `iRec<group>`
    /// - later slots: `fRec<group>_<index>` / `iRec<group>_<index>`
    pub(super) fn ensure_recursion_array_for_group(
        &mut self,
        group: SigId,
        index: usize,
        typ: FirType,
        init: FirId,
    ) -> Result<RecArrayInfo, SignalFirError> {
        let key = (group.as_u32(), index);
        if let Some(info) = self.recursion.rec_array_by_group_index.get(&key) {
            return Ok(info.clone());
        }
        let prefix = if typ == FirType::Int32 {
            "iRec"
        } else {
            "fRec"
        };
        let name = if index == 0 {
            format!("{prefix}{}", group.as_u32())
        } else {
            format!("{prefix}{}_{}", group.as_u32(), index)
        };
        let (size, strategy) = match decode_symbolic_group_bodies(self.arena, group) {
            Some((var, bodies)) => match self.delay.rec_output_analysis(var.as_u32(), index) {
                Some(analysis) if bodies.len() == 1 && analysis.max_delay <= 1 => {
                    (1, RecursionStorageStrategy::SingleScalar)
                }
                Some(analysis) => {
                    let exact_size = usize::try_from(analysis.max_delay).map_err(|_| {
                        SignalFirError::new(
                            SignalFirErrorCode::UnsupportedSignalNode,
                            format!(
                                "negative recursion delay analysis result {}",
                                analysis.max_delay
                            ),
                        )
                    })? + 1;
                    let copy_threshold =
                        usize::try_from(self.delay.max_copy_delay()).unwrap_or(usize::MAX);
                    if usize::try_from(analysis.max_delay).unwrap_or(usize::MAX) < copy_threshold {
                        (exact_size, RecursionStorageStrategy::ExactShift)
                    } else {
                        (
                            pow2limit_for_delay(analysis.max_delay)?,
                            RecursionStorageStrategy::Circular,
                        )
                    }
                }
                None if bodies.len() == 1 => (1, RecursionStorageStrategy::SingleScalar),
                None => (2, RecursionStorageStrategy::ExactShift),
            },
            None => (2, RecursionStorageStrategy::ExactShift),
        };
        let mut b = FirBuilder::new(self.store);
        let decl = if size == 1 {
            b.declare_var(name.clone(), typ.clone(), AccessType::Struct, None)
        } else {
            let array_ty = FirType::Array(Box::new(typ.clone()), size);
            b.declare_var(name.clone(), array_ty, AccessType::Struct, None)
        };
        self.struct_declarations.push(decl);
        if size == 1 {
            self.register_clear_recursion_scalar(name.clone(), init);
        } else {
            self.register_clear_recursion_array(name.clone(), init, size);
        }
        let info = RecArrayInfo {
            name,
            typ,
            size,
            strategy,
        };
        self.recursion
            .rec_array_by_group_index
            .insert(key, info.clone());
        Ok(info)
    }

    /// Allocates or reuses all carriers for one recursive group body list.
    ///
    /// When `group` is a `ReverseTimeRec(...)` node (the LTI adjoint wrapper),
    /// its id is recorded in `recursion.reverse_time_rec_group_ids` so that
    /// `emit_reverse_time_rec_compute_resets` can distinguish these adjoint
    /// carriers — which must be zeroed before each backward sweep — from normal
    /// SYMREC primal carriers, which are persistent DSP state.
    ///
    /// Dormant under the 2026-05-10 RAD dispatcher change; kept compilable for
    /// a future LTI fast-path revival.
    pub(super) fn allocate_group_arrays(
        &mut self,
        group: SigId,
        body_infos: &[(FirType, FirId)],
    ) -> Result<Vec<RecArrayInfo>, SignalFirError> {
        // Mark ReverseTimeRec groups so that emit_reverse_time_rec_compute_resets
        // can filter to only those carriers.
        if matches!(match_sig(self.arena, group), SigMatch::ReverseTimeRec(_)) {
            self.recursion
                .reverse_time_rec_group_ids
                .insert(group.as_u32());
        }
        let mut group_arrays = Vec::with_capacity(body_infos.len());
        for (index, (typ, init)) in body_infos.iter().enumerate() {
            group_arrays.push(self.ensure_recursion_array_for_group(
                group,
                index,
                typ.clone(),
                *init,
            )?);
        }
        Ok(group_arrays)
    }
}

/// Decodes a `SYMREC(var, body_list)` group to all its payload body signals.
///
/// Phase-E1 RAD may wrap the symbolic group in `ReverseTimeRec(body)`. That
/// wrapper keeps the same projection arity contract as a normal recursion, so
/// this helper deliberately unwraps it before decoding. The caller remains
/// responsible for choosing forward or reverse lowering semantics.
///
/// Returns `None` when the effective group is not a symbolic recursion binder
/// or when the body payload is not a proper list.
pub(super) fn decode_symbolic_group_bodies(
    arena: &TreeArena,
    group: SigId,
) -> Option<(SigId, Vec<SigId>)> {
    let effective_group = match match_sig(arena, group) {
        SigMatch::ReverseTimeRec(body) => body,
        _ => group,
    };
    let (var, body_list) = match_sym_rec(arena, effective_group)?;
    let bodies = list_to_vec(arena, body_list)?;
    Some((var, bodies))
}

/// Returns the canonical output index for one recursion projection.
///
/// Unary recursion groups normalize every `Proj(i, group)` to slot `0`, which
/// matches the rest of the fast-lane recursion handling.
pub(super) fn canonical_group_index(
    arena: &TreeArena,
    group: SigId,
    index: usize,
) -> Option<usize> {
    let (_var, bodies) = decode_symbolic_group_bodies(arena, group)?;
    Some(if bodies.len() == 1 { 0 } else { index })
}

/// Decodes one `SIGPROJ(index, group)` target into its recursion-group payload
/// and validates that the requested projection index is in bounds.
///
/// This is the structural front door used by `lower_proj(...)`: after this
/// function returns, the caller can allocate carriers and lower bodies without
/// re-checking symbolic-group shape or unary-group canonicalization.
pub(super) fn decode_group_projection(
    arena: &TreeArena,
    node: SigId,
    index: i32,
    group: SigId,
) -> Result<RecursionGroupProjection, SignalFirError> {
    let index_usize = usize::try_from(index).map_err(|_| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!("negative SIGPROJ index {index} in Step 2C.2"),
        )
    })?;
    let (var, bodies) = decode_symbolic_group_bodies(arena, group).ok_or_else(|| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!(
                "SIGPROJ group must be SYMREC/SYMREF after de_bruijn_to_sym in Step 2C.2 (expr={})",
                signals::dump_sig_readable(arena, node)
            ),
        )
    })?;
    let canonical_index = if bodies.len() == 1 { 0 } else { index_usize };
    if canonical_index >= bodies.len() {
        return Err(SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!(
                "SIGPROJ index {index} out of bounds for recursion group with {} bodies",
                bodies.len()
            ),
        ));
    }
    Ok(RecursionGroupProjection {
        var,
        bodies,
        canonical_index,
    })
}

/// Resolves a symbolic recursion group reference to its active carrier at a
/// given projection index.
///
/// This only handles the active-stack case (`SYMREF` bound by the current
/// recursive lowering context). Materialized top-level carriers are handled by
/// `RecursionState::resolve_materialized_carrier`.
pub(super) fn resolve_active_recursion_carrier(
    arena: &TreeArena,
    state: &RecursionState,
    group: SigId,
    proj_index: usize,
) -> Result<Option<RecursionCarrierRef>, SignalFirError> {
    let Some(var) = match_sym_ref(arena, group) else {
        return Ok(None);
    };
    let Some(depth) = state
        .recursion_vars
        .iter()
        .rposition(|bound| *bound == var)
        .map(|slot| state.recursion_vars.len() - slot)
    else {
        // Under global C++-style scheduling a recurrence descendant can be
        // visited outside the old lexical body-lowering stack. A registered
        // binder is therefore materialized through `group_by_var`; only a
        // genuinely unknown symbolic reference is an error.
        if state.group_by_var.contains_key(&var.as_u32()) {
            return Ok(None);
        }
        return Err(SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!("unbound symbolic recursion variable {}", var.as_u32()),
        ));
    };
    let group_arrays = &state.recursion_stack[state.recursion_stack.len() - depth];
    let canonical_index = if group_arrays.len() == 1 {
        0
    } else {
        proj_index
    };
    group_arrays
        .get(canonical_index)
        .cloned()
        .ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "projection index {proj_index} out of bounds for recursion group with {} outputs",
                    group_arrays.len()
                ),
            )
        })
        .map(RecursionCarrierRef::new)
        .map(Some)
}

/// Matches `Proj(i, group)` optionally wrapped in `Delay1^k(...)`.
///
/// This is the pure structural recognizer used before any materialization
/// fallback. It does not validate that the referenced group has already been
/// allocated.
pub(super) fn match_recursion_delay_key(
    arena: &TreeArena,
    value: SigId,
) -> Option<RecursionDelayKey> {
    let mut current = value;
    let mut carried_delay = 0usize;
    while let SigMatch::Delay1(inner) = match_sig(arena, current) {
        carried_delay = carried_delay.saturating_add(1);
        current = inner;
    }
    let SigMatch::Proj(index, group) = match_sig(arena, current) else {
        return None;
    };
    Some(RecursionDelayKey {
        proj_node: current,
        proj_index: index,
        group,
        implicit_delay: carried_delay,
    })
}
