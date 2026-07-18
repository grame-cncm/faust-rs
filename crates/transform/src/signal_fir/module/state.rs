//! Delay and recursion state helpers, and register-init logic.
//!
//! Defines [`ModuleSections`], the sub-state struct that holds the FIR
//! statement buckets for each Faust lifecycle section (`struct_declarations`,
//! `static_declarations`, `global_declarations`, `constants_statements`,
//! `reset_statements`, `clear_statements`, `control_statements`, and their
//! dedup guards).
//!
//! Provides the methods that manage persistent DSP state across the Faust
//! lifecycle: recursion-carrier resolution and allocation, delay-state slot
//! lowering (shift strategy and ring-buffer strategy), and the helpers that
//! register state variables in `instanceClear` and `instanceConstants`
//! through the named-struct / register-init mechanism.

use super::*;

/// The FIR statement buckets for each Faust lifecycle section.
#[derive(Default)]
pub(super) struct ModuleSections {
    /// DSP struct field declarations (arrays, scalars, UI zones).
    pub(super) struct_declarations: Vec<FirId>,
    /// Constant waveform table declarations emitted at file scope (`const static`
    /// in C++/C) rather than inside the DSP struct.  These are tables whose
    /// content is fully determined at compile time (waveform literals) and is
    /// shared across all DSP instances.
    pub(super) static_declarations: Vec<FirId>,
    /// Extern global variable declarations requested by `SIGFVAR` lowering.
    pub(super) global_declarations: Vec<FirId>,
    /// `instanceConstants` body: table initializations and compile-time constants.
    pub(super) constants_statements: Vec<FirId>,
    /// `instanceResetUserInterface` body: UI zone reset assignments.
    pub(super) reset_statements: Vec<FirId>,
    /// `instanceClear` body: delay-line and recursion-state zero-init loops.
    pub(super) clear_statements: Vec<FirId>,
    /// `compute` preamble: channel-pointer aliases and diagnostic labels.
    pub(super) control_statements: Vec<FirId>,
    /// Dedup guard for named struct-var declarations (prevents double-emit).
    pub(super) named_struct_vars: HashSet<String>,
    /// Dedup guard for `instanceResetUserInterface` assignments.
    pub(super) reset_init_seen: HashSet<String>,
    /// Dedup guard for `instanceClear` assignments and loops.
    pub(super) clear_init_seen: HashSet<String>,
}

impl<'a> SignalToFirLower<'a> {
    /// Returns the resolved recursion-delay reference for `value`.
    ///
    /// Examples:
    ///
    /// - `Proj(i, group)` → delay chain `0`
    /// - `Delay1(Proj(i, group))` → delay chain `1`
    /// - `Delay1(Delay1(Proj(i, group)))` → delay chain `2`
    ///
    /// Pure state-based resolution lives in `recursion.rs`; this wrapper only
    /// allocates missing carrier storage. Recursion-body computation remains
    /// controlled by the global signal schedule.
    pub(super) fn resolve_recursion_delay_ref(
        &mut self,
        value: SigId,
    ) -> Result<Option<RecursionDelayRef>, SignalFirError> {
        if let Some(delay_ref) = self.recursion.resolve_delay_ref(self.arena, value)? {
            return Ok(Some(delay_ref));
        }
        let Some(key) = match_recursion_delay_key(self.arena, value) else {
            return Ok(None);
        };
        let Some(rec_info) =
            self.resolve_recursion_carrier(key.proj_node, key.proj_index, key.group)?
        else {
            return Ok(None);
        };
        Ok(Some(RecursionDelayRef {
            carrier: rec_info,
            implicit_delay: key.implicit_delay,
        }))
    }

    /// Returns the canonical recursion carrier for `Proj(index, group)` whether
    /// the projection points to the active feedback reference (`SYMREF`) or to
    /// the materialized top-level recursion group (`SYMREC`).
    ///
    /// Pure active/materialized lookup lives in `recursion.rs`; this wrapper
    /// only performs allocation-only materialization when needed. In
    /// particular, it must not emit recursion-body stores: delayed edges do not
    /// constrain the C++ loop DAG, so a delay read may be lowered before the
    /// corresponding projection node.
    pub(super) fn resolve_recursion_carrier(
        &mut self,
        _proj_node: SigId,
        index: i32,
        group: SigId,
    ) -> Result<Option<RecursionCarrierRef>, SignalFirError> {
        let index_usize = usize::try_from(index).map_err(|_| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("negative SIGPROJ index {index} in recursion carrier lookup"),
            )
        })?;
        if let Some(info) = self
            .recursion
            .resolve_carrier(self.arena, group, index_usize)?
        {
            return Ok(Some(info));
        }
        let Some(canonical_group) = self.recursion.canonical_group(self.arena, group) else {
            return Ok(None);
        };
        if match_sym_rec(self.arena, canonical_group).is_none() {
            return Ok(None);
        }

        // Reserve the canonical carrier name and storage without compiling the
        // recurrence body. This mirrors C++ `ensureVectorNameProperty`: delayed
        // reads may reserve a carrier before the scheduled projection computes
        // and stores its next value.
        let _ = self.ensure_recursion_group_carriers(canonical_group)?;
        self.recursion
            .resolve_carrier(self.arena, canonical_group, index_usize)
    }

    /// Declares a stack-local current-sample binding for one scalar recursion
    /// carrier and records it under the canonical `(group, index)` key.
    pub(super) fn bind_scalar_recursion_current_value(
        &mut self,
        group: SigId,
        index: usize,
        info: &RecArrayInfo,
        value: FirId,
    ) -> String {
        let prefix = if info.typ == FirType::Int32 {
            "iRecCur"
        } else {
            "fRecCur"
        };
        let name = if index == 0 {
            format!("{prefix}{}", group.as_u32())
        } else {
            format!("{prefix}{}_{}", group.as_u32(), index)
        };
        let mut b = FirBuilder::new(&mut self.store);
        self.regions
            .current_phases_mut()
            .immediate
            .push(b.declare_var(
                name.clone(),
                info.typ.clone(),
                AccessType::Stack,
                Some(value),
            ));
        self.recursion.set_current_value_binding(
            group,
            index,
            RecursionCurrentValueBinding {
                name: name.clone(),
                typ: info.typ.clone(),
            },
        );
        name
    }

    /// Loads the current-sample value of a scalar recursion carrier through its
    /// stack-local binding.
    pub(super) fn load_scalar_recursion_current_value(
        &mut self,
        group: SigId,
        index: usize,
    ) -> Result<Option<FirId>, SignalFirError> {
        let Some(binding) = self
            .recursion
            .current_value_binding(self.arena, group, index)
        else {
            return Ok(None);
        };
        let mut b = FirBuilder::new(&mut self.store);
        Ok(Some(b.load_var(
            binding.name,
            AccessType::Stack,
            binding.typ.clone(),
        )))
    }

    /// Ensures a 2-element circular buffer state slot exists for `node`,
    /// idempotent.  On first call, declares `[typ; 2]` in the struct
    /// (prefixed `iRec` for `Int32`, `fRec` otherwise) and registers an
    /// `instanceClear` zeroing loop.  Returns the generated variable name.
    ///
    /// Keyed by `node` SigId in `state_name_by_node` — separate from
    /// `rec_array_by_group_index` to avoid aliasing (see `build_module` doc).
    pub(super) fn ensure_state_slot(&mut self, node: SigId, typ: FirType, init: FirId) -> String {
        if let Some(name) = self.state_name_by_node.get(&node) {
            return name.clone();
        }
        let prefix = if typ == FirType::Int32 {
            "iRec"
        } else {
            "fRec"
        };
        let name = format!("{prefix}{}", node.as_u32());
        // Allocate a 2-element circular buffer (matching C++ signalFIRCompiler DelayLine).
        let array_ty = FirType::Array(Box::new(typ), 2);
        let mut b = FirBuilder::new(&mut self.store);
        let dec = b.declare_var(name.clone(), array_ty, AccessType::Struct, None);
        self.sections.struct_declarations.push(dec);
        self.register_clear_recursion_array(name.clone(), init, 2);
        self.state_name_by_node.insert(node, name.clone());
        name
    }

    /// Declares the struct array for one circular delay line, idempotent.
    ///
    /// Thin delegate to [`DelayManager::ensure_delay_line`]; constructs a
    /// [`DelayFirCtx`] from disjoint fields via split-borrow struct literal so
    /// that `self.delay` can be borrowed simultaneously.
    pub(super) fn ensure_delay_line_decl(
        &mut self,
        carried: SigId,
        delay: i32,
    ) -> Result<DelayLineInfo, SignalFirError> {
        // Explicit field-level split borrows: `self.delay` is NOT included here,
        // so it can be mutably borrowed below without conflict.
        let mut ctx = DelayFirCtx {
            store: &mut self.store,
            real_ty: self.real_ty.clone(),
            types: self.types,
            struct_declarations: &mut self.sections.struct_declarations,
            clear_statements: &mut self.sections.clear_statements,
            clear_init_seen: &mut self.sections.clear_init_seen,
            next_loop_var_id: &mut self.name_gen.next_loop_var_id,
            uses_iota: &mut self.uses_iota,
        };
        self.delay.ensure_delay_line(carried, delay, &mut ctx)
    }

    /// Returns the canonical pre-allocated delay line for `carried`.
    ///
    /// Delay-line strategy and geometry are chosen during
    /// [`Self::prepare_delay_lines`]. Lowering paths should only query that
    /// decision, not allocate new delay lines opportunistically.
    pub(super) fn delay_line_info(&self, carried: SigId) -> Result<DelayLineInfo, SignalFirError> {
        self.delay.get_delay_line(carried).cloned().ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "internal fast-lane missing pre-allocated delay line for signal {}",
                    carried.as_u32()
                ),
            )
        })
    }

    /// Declares the shared global circular cursor state (`fIOTA`), idempotent.
    ///
    pub(super) fn ensure_global_circular_cursor(&mut self) {
        let mut ctx = DelayFirCtx {
            store: &mut self.store,
            real_ty: self.real_ty.clone(),
            types: self.types,
            struct_declarations: &mut self.sections.struct_declarations,
            clear_statements: &mut self.sections.clear_statements,
            clear_init_seen: &mut self.sections.clear_init_seen,
            next_loop_var_id: &mut self.name_gen.next_loop_var_id,
            uses_iota: &mut self.uses_iota,
        };
        GlobalCircularCursor.ensure_state(&mut ctx);
    }

    /// Declares/advances the global `fIOTA` cursor iff some planned delay
    /// line still uses it (roadmap P3 slice 4). Run after
    /// `assign_clocked_delay_cursors` so in-domain `CircularPow2` lines that
    /// moved to a per-domain `fIOTA_d<i>` cursor no longer force a dead global
    /// field + advance.
    pub(super) fn finalize_global_cursor(&mut self) {
        if !self.delay.global_circular_carriers().is_empty() {
            self.ensure_global_circular_cursor();
        }
    }

    /// Returns the masked current write index for a circular structure lowered
    /// in the current append-target region.
    ///
    /// At the top rate this is the shared `fIOTA`; inside a guarded clocked
    /// block it is the effective domain's per-domain `fIOTA_d<i>` cursor
    /// (roadmap P3 slice 4), so circular recursion carriers and delay-states
    /// inside a block advance in fire time.
    pub(super) fn global_circular_current_index(&mut self, size: usize) -> FirId {
        let cursor = self.active_circular_cursor_name();
        if cursor == "fIOTA" {
            self.ensure_global_circular_cursor();
        }
        cursor_current_index(&mut self.store, &cursor, size)
    }

    /// Returns the masked delayed read index for a circular structure lowered
    /// in the current append-target region (see
    /// [`Self::global_circular_current_index`]).
    pub(super) fn global_circular_delayed_index(&mut self, amount: FirId, size: usize) -> FirId {
        let cursor = self.active_circular_cursor_name();
        if cursor == "fIOTA" {
            self.ensure_global_circular_cursor();
        }
        cursor_delayed_index(&mut self.store, &cursor, amount, size)
    }

    /// Runs `f` with one recursion group pushed onto the active recursion stack.
    ///
    /// This centralizes the push/pop discipline for the active recursion-group
    /// stack, which must stay perfectly balanced even when lowering
    /// fails partway through a recursive body.
    pub(super) fn with_active_recursion_group<R>(
        &mut self,
        var: SigId,
        arrays: Vec<RecArrayInfo>,
        f: impl FnOnce(&mut Self, &[RecArrayInfo]) -> Result<R, SignalFirError>,
    ) -> Result<R, SignalFirError> {
        self.recursion.push_active_group(var, arrays.clone());
        let result = f(self, &arrays);
        self.recursion.pop_active_group();
        result
    }

    /// Emits an `instanceClear` zeroing loop for a two-slot recursion array.
    ///
    /// Idempotent: subsequent calls for the same `name` are silently ignored.
    pub(super) fn register_clear_recursion_array(
        &mut self,
        name: String,
        init: FirId,
        size: usize,
    ) {
        if !self.sections.clear_init_seen.insert(name.clone()) {
            return;
        }
        let loop_var = self.fresh_loop_var("lRec");
        let upper = {
            let mut b = FirBuilder::new(&mut self.store);
            b.int32(i32::try_from(size).unwrap_or(i32::MAX))
        };
        let body = {
            let index = {
                let mut b = FirBuilder::new(&mut self.store);
                b.load_var(loop_var.clone(), AccessType::Loop, FirType::Int32)
            };
            let store = {
                let mut b = FirBuilder::new(&mut self.store);
                b.store_table(name, AccessType::Struct, index, init)
            };
            let mut b = FirBuilder::new(&mut self.store);
            b.block(&[store])
        };
        let mut b = FirBuilder::new(&mut self.store);
        self.sections
            .clear_statements
            .push(b.simple_for_loop(loop_var, upper, body, false));
    }

    /// Generates a unique loop variable name using a monotonic counter.
    pub(super) fn fresh_loop_var(&mut self, prefix: &str) -> String {
        let name = format!("{prefix}{}", self.name_gen.next_loop_var_id);
        self.name_gen.next_loop_var_id += 1;
        name
    }

    /// Declares one named struct variable once.
    pub(super) fn ensure_named_struct_var(
        &mut self,
        name: &str,
        typ: FirType,
        init: Option<FirId>,
    ) {
        if self.sections.named_struct_vars.contains(name) {
            return;
        }
        let mut b = FirBuilder::new(&mut self.store);
        let dec = b.declare_var(name.to_owned(), typ, AccessType::Struct, None);
        self.sections.struct_declarations.push(dec);
        self.sections.named_struct_vars.insert(name.to_owned());
        if let Some(init) = init {
            self.register_reset_init(name.to_owned(), init);
        }
    }

    /// Registers one reset-time assignment for UI controls (`instanceResetUserInterface`).
    pub(super) fn register_reset_init(&mut self, name: String, init: FirId) {
        if !self.sections.reset_init_seen.insert(name.clone()) {
            return;
        }
        let mut b = FirBuilder::new(&mut self.store);
        self.sections
            .reset_statements
            .push(b.store_var(name, AccessType::Struct, init));
    }

    /// Registers one clear-time assignment for runtime state (`instanceClear`).
    pub(super) fn register_clear_init(&mut self, name: String, init: FirId) {
        if !self.sections.clear_init_seen.insert(name.clone()) {
            return;
        }
        let mut b = FirBuilder::new(&mut self.store);
        self.sections
            .clear_statements
            .push(b.store_var(name, AccessType::Struct, init));
    }

    /// Registers one per-instance table initialization block for
    /// `instanceConstants`.
    pub(super) fn register_constant_table_init(
        &mut self,
        name: String,
        access: AccessType,
        values: &[FirId],
    ) {
        if values.is_empty() {
            return;
        }
        let mut stores = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let idx = {
                let mut b = FirBuilder::new(&mut self.store);
                b.int32(i32::try_from(index).unwrap_or(i32::MAX))
            };
            let store = {
                let mut b = FirBuilder::new(&mut self.store);
                b.store_table(name.clone(), access, idx, *value)
            };
            stores.push(store);
        }
        let mut b = FirBuilder::new(&mut self.store);
        self.sections.constants_statements.push(b.block(&stores));
    }
}
