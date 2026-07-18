//! Guarded-block lowering for clocked wrappers (roadmap P3, complete).
//!
//! # Source provenance (C++)
//! - `compiler/generator/compile_scal.cpp` (`generateOD`, `generateTempVar`,
//!   `generatePermVar`, `sigSeq`/`sigClocked` cases, 2181-2284; branch
//!   `master-dev-ocpp-od-fir-2-FIR19`, commit `8eebea429`)
//! - `compiler/parallelize/loop.cpp` (`CodeIFblock`)
//!
//! # Scope
//! Guard shapes (plan §3.8): boolean `ondemand` → `if (clock != 0) { … }`
//! (C++ `CodeIFblock`); integer `ondemand` / `upsampling` → counted
//! `SimpleForLoop`; `downsampling` → `if (fDSCounter == 0)` + modulo post-code.
//! All reuse the generic FIR `If` / `SimpleForLoop` / `Block` statements per
//! the P2.1 vocabulary decision.
//!
//! **State inside a block advances in fire time.** Every stateful shape whose
//! state lives in the domain has its end-of-sample maintenance routed into the
//! guarded region (roadmap P3 slices 3-4): shift delays and scalar recursion
//! emit their updates inline; `CircularPow2` lines and delay-states use the
//! domain's per-domain `fIOTA_d<i>` cursor (advanced once per fire); inner
//! `IfWrapping` lines advance their per-line counter in the block; a
//! `Waveform`'s read-index counter (`iWave*`) advances inside the block (see
//! the redirection exception below). The remaining `FRS-SFIR-0007` rejections
//! are genuinely-unsupported shapes only (non-boolean real OD clocks,
//! non-integer US/DS clocks).
//!
//! # Emission model (adaptation note)
//! Like C++, scalar emission is driven by the selected per-region `Hsched`.
//! Recursive expression construction remains demand-driven inside each
//! scheduled node. The **region redirection** rule still controls placement:
//! a signal inferred in a strict ancestor domain is appended to that
//! ancestor's region before the descendant guard closes.
//!
//! **Redirection exception — held payloads.** The payload of
//! `PermVar(Clocked(env, value))` is lowered *in the guarded block that writes
//! the hold field*, never redirected to an ancestor just because clock-env
//! inference gives `value` or one of its stateful children an ancestor rate.
//! This matches C++ `generateOD`/`generatePermVar`: only `TempVar` boundary
//! reads re-enable redirection for true outer-domain sources. This general rule
//! covers the original `Waveform` regression, where redirecting the `iWave*`
//! read-index update to the top rate made the scale advance every sample
//! instead of once per fire.
//!
//! # Node generators (plan §3.8)
//! - `Seq(od, y)` → `CS(od); return CS(y)`;
//! - `Clocked(env, y)` → passthrough (annotation only);
//! - `TempVar(x)` → passthrough: evaluated inside the guard, the expression
//!   reads the outer value exactly at the fire tick;
//! - `PermVar(Clocked(env, v))` → persistent struct field `fPerm<i>` cleared
//!   to 0, assigned inside the block, read as a plain field load;
//! - `OnDemand([Clocked(env, h), holds…])` → guarded `If` region.

use super::*;

/// Per-compilation clocked-lowering state (present only when the program
/// actually contains clocked wrappers).
pub(super) struct ClockedState<'a> {
    /// Propagation-owned clock-domain table (P0.2).
    pub(super) domains: &'a propagate::ClockDomainTable,
    /// Clock-environment side map from `clk_env::annotate` (P1.1).
    pub(super) envs: crate::clk_env::ClkEnvMap,
    /// Domains of the open guarded blocks, innermost last (parallel to
    /// `RegionTree::child_depth`).
    pub(super) open_domains: Vec<propagate::ClockDomainId>,
    /// Counted-loop context per open block (parallel to `open_domains`):
    /// `Some` for integer-OD/US blocks, `None` for If-shaped blocks.
    pub(super) open_loops: Vec<Option<LoopCtx>>,
    /// `PermVar` node → hold-field name (registered by the wrapper emission).
    pub(super) perm_fields: HashMap<SigId, String>,
    /// Wrapper nodes whose guarded block has been emitted.
    pub(super) emitted_blocks: HashSet<SigId>,
    /// Domains owning a per-domain circular cursor (`fIOTA_d<i>`): their
    /// guarded blocks advance it once per fire (roadmap P3).
    pub(super) iota_domains: HashSet<propagate::ClockDomainId>,
    /// Per-domain inner `IfWrapping` delay-line counters `(name, size)`: their
    /// advance is emitted inside the guarded block, not at the top sample end
    /// (roadmap P3 slice 4).
    pub(super) domain_ifwrap: HashMap<propagate::ClockDomainId, Vec<(String, usize)>>,
    /// Monotonic counter for `fPerm<i>` hold-field names.
    pub(super) next_perm_id: usize,
}

/// Analysis products handed to `build_module` for clocked programs.
pub(crate) struct ClockedPlan<'a> {
    pub(crate) domains: &'a propagate::ClockDomainTable,
    pub(crate) envs: crate::clk_env::ClkEnvMap,
}

impl<'a> ClockedState<'a> {
    pub(super) fn new(plan: ClockedPlan<'a>) -> Self {
        Self {
            domains: plan.domains,
            envs: plan.envs,
            open_domains: Vec::new(),
            open_loops: Vec::new(),
            perm_fields: HashMap::new(),
            emitted_blocks: HashSet::new(),
            iota_domains: HashSet::new(),
            domain_ifwrap: HashMap::new(),
            next_perm_id: 0,
        }
    }
}

/// Counted-loop context of one open guarded block (integer OD / US).
#[derive(Clone)]
pub(super) struct LoopCtx {
    /// Inner loop variable name (`lOd<i>`), readable by `ZeroPad` lowering.
    pub(super) var: String,
    /// Lowered loop bound (the integer clock value at this tick).
    pub(super) bound: FirId,
}

/// Guard statement shape selected from the wrapper kind and clock type
/// (plan §3.8: boolean OD `if`, integer OD / US counted loop, DS modulo).
enum GuardShape {
    /// `if (clock != 0) { body }`
    BoolIf,
    /// `for (l = 0; l < clock; l++) { body }`
    CountedLoop,
    /// `if (fDSCounter_d<i> == 0) { body }` + post `counter = (counter+1) % clock`
    DsModulo,
}

fn clocked_not_lowered(message: impl Into<String>) -> SignalFirError {
    SignalFirError::new(SignalFirErrorCode::ClockedNotLowered, message)
}

impl<'a> SignalToFirLower<'a> {
    /// Records the closure that must be materialized inside guarded hold
    /// payloads. This enforces the C++ `generatePermVar` exception before the
    /// top-level scalar schedule starts lowering ancestor-domain nodes.
    pub(super) fn prepare_clocked_payload_schedule(&mut self, roots: &[SigId]) {
        let mut reachable = roots.to_vec();
        let mut visited = HashSet::new();
        while let Some(sig) = reachable.pop() {
            if !visited.insert(sig) {
                continue;
            }
            match match_sig(self.arena, sig) {
                SigMatch::OnDemand(children)
                | SigMatch::Upsampling(children)
                | SigMatch::Downsampling(children) => {
                    for &hold in children.iter().skip(1) {
                        if let SigMatch::PermVar(inner) = match_sig(self.arena, hold) {
                            self.clocked_payload_signals.insert(inner);
                            let payload = match match_sig(self.arena, inner) {
                                SigMatch::Clocked(_, payload) => payload,
                                _ => inner,
                            };
                            self.collect_guarded_payload(payload);
                        }
                    }
                    reachable.extend(children.iter().copied());
                }
                _ => {
                    if let Some(children) = self.arena.children(sig) {
                        reachable.extend(children.iter().copied());
                    }
                }
            }
        }
    }

    fn collect_guarded_payload(&mut self, root: SigId) {
        let mut stack = vec![root];
        while let Some(sig) = stack.pop() {
            if !self.clocked_payload_signals.insert(sig) {
                continue;
            }
            if matches!(match_sig(self.arena, sig), SigMatch::TempVar(_)) {
                continue;
            }
            if let Some(children) = self.arena.children(sig) {
                stack.extend(children.iter().copied());
            }
        }
    }

    /// Computes the append-redirection depth for `sig`, when its inferred
    /// domain is a strict ancestor of the effective (append-target) domain.
    ///
    /// Returns `None` when no redirection is needed: not a clocked program,
    /// same domain, unknown node, or a *deeper* domain (deeper values are
    /// only reachable through their wrapper / hold fields, which have their
    /// own arms).
    pub(super) fn clocked_redirect_target(&self, sig: SigId) -> Option<usize> {
        let clocked = self.clocked.as_ref()?;

        let sig_env = self.clocked_env_of(sig)?;

        let effective_env = self.effective_domain();
        if sig_env == effective_env {
            return None;
        }
        if !crate::clk_env::is_ancestor_clk_env(clocked.domains, sig_env, effective_env) {
            return None;
        }
        match sig_env {
            None => Some(0),
            Some(id) => clocked
                .open_domains
                .iter()
                .position(|&d| d == id)
                .map(|index| index + 1),
        }
    }

    /// Lowers the payload of a held `PermVar(Clocked(env, value))` in the
    /// current guarded region.
    ///
    /// Clock inference may give `value` or some of its children an ancestor
    /// domain when they do not read a clocked input. That does not make the
    /// payload external: C++ `generateOD`/`generatePermVar` emits the whole
    /// held value inside the guarded block and only the resulting `PermVar` is
    /// read outside.
    pub(super) fn lower_clocked_payload(&mut self, inner: SigId) -> Result<FirId, SignalFirError> {
        let previous = self.suppress_clocked_redirect;
        self.suppress_clocked_redirect = true;
        let result = self.lower_signal(inner);
        self.suppress_clocked_redirect = previous;
        result
    }

    /// Lowers a `TempVar` source referenced from a clocked payload.
    ///
    /// `TempVar` is the boundary read: the use is inside the guarded block,
    /// but the source expression belongs to its inferred clock environment.
    /// Re-enable redirection while lowering the source so full-rate delay
    /// chains such as `serialize_in` stay outside the block.
    pub(super) fn lower_clocked_temp_var(&mut self, inner: SigId) -> Result<FirId, SignalFirError> {
        let previous = self.suppress_clocked_redirect;
        self.suppress_clocked_redirect = false;
        let result = self.lower_signal(inner);
        self.suppress_clocked_redirect = previous;
        result
    }

    pub(super) fn delay_line_info_for_current_region(
        &mut self,
        carried: SigId,
    ) -> Result<DelayLineInfo, SignalFirError> {
        let mut line = self.delay_line_info(carried)?;
        let Some(domain) = self.effective_domain() else {
            return Ok(line);
        };
        if !self.suppress_clocked_redirect {
            return Ok(line);
        }

        let was_inner = self.delay.is_line_inner(carried);
        self.delay.mark_line_inner(carried);
        match line.strategy.clone() {
            super::super::delay::DelayKind::Shift => {}
            super::super::delay::DelayKind::CircularPow2 => {
                let cursor = self.active_circular_cursor_name();
                self.delay.set_line_cursor(carried, cursor.clone());
                line.cursor = Some(cursor);
            }
            super::super::delay::DelayKind::IfWrapping { counter_name } => {
                if !was_inner {
                    self.clocked
                        .as_mut()
                        .expect("effective_domain returned Some only for clocked programs")
                        .domain_ifwrap
                        .entry(domain)
                        .or_default()
                        .push((counter_name, line.size));
                }
            }
        }
        Ok(line)
    }

    /// Clock environment of the current append-target region: `None` at the
    /// top rate (or when the program is not clocked), otherwise the domain of
    /// the effective open guarded block (honoring an active redirection).
    pub(super) fn effective_domain(&self) -> crate::clk_env::ClkEnv {
        let clocked = self.clocked.as_ref()?;
        let effective_depth = self
            .regions
            .redirect_depth()
            .unwrap_or_else(|| self.regions.child_depth());
        if effective_depth == 0 {
            None
        } else {
            Some(clocked.open_domains[effective_depth - 1])
        }
    }

    /// Name of the circular cursor to use for a circular structure lowered in
    /// the current append-target region (roadmap P3 slice 4): the shared
    /// `fIOTA` at the top rate, or the effective domain's `fIOTA_d<i>` inside
    /// a guarded block. Declares the per-domain field and marks its domain so
    /// the block advances the cursor once per fire.
    pub(super) fn active_circular_cursor_name(&mut self) -> String {
        match self.effective_domain() {
            None => "fIOTA".to_owned(),
            Some(domain) => {
                let cursor = {
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
                    self.domain_counters.declare_retrieve_iota(domain, &mut ctx)
                };
                self.clocked
                    .as_mut()
                    .expect("effective_domain returned Some only for clocked programs")
                    .iota_domains
                    .insert(domain);
                cursor
            }
        }
    }

    /// Inferred clock environment of one prepared node, when known.
    fn clocked_env_of(&self, sig: SigId) -> Option<crate::clk_env::ClkEnv> {
        self.clocked.as_ref()?.envs.env(sig)
    }

    /// Routes the end-of-sample maintenance of every delay line whose carrier
    /// lives inside a clock domain into that domain's guarded block, so it
    /// happens in **fire time** (roadmap P3 slice 4). Called once after delay
    /// planning:
    ///
    /// - `CircularPow2` lines switch from the shared `fIOTA` to the domain's
    ///   per-domain `fIOTA_d<i>` cursor (C++ `declareRetrieveIotaName`),
    ///   advanced once per fire by [`Self::ensure_guarded_block`];
    /// - `IfWrapping` lines are marked inner (so `emit_sample_end_updates`
    ///   skips them at the top level) and their per-line counter advance is
    ///   recorded per domain for the block to emit;
    /// - `Shift` lines are already correct — their shift is emitted inside the
    ///   guarded region during body lowering.
    pub(super) fn assign_clocked_delay_cursors(&mut self) -> Result<(), SignalFirError> {
        use super::super::delay::DelayKind;

        let Some(clocked) = self.clocked.as_ref() else {
            return Ok(());
        };
        let mut inner_lines: Vec<(SigId, propagate::ClockDomainId, DelayKind, usize)> = Vec::new();
        for (&carried, info) in self.delay.lines() {
            if let Some(Some(domain)) = clocked.envs.env(carried) {
                inner_lines.push((carried, domain, info.strategy.clone(), info.size));
            }
        }
        for (carried, domain, strategy, size) in inner_lines {
            self.delay.mark_line_inner(carried);
            match strategy {
                DelayKind::Shift => {}
                DelayKind::CircularPow2 => {
                    let cursor = {
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
                        self.domain_counters.declare_retrieve_iota(domain, &mut ctx)
                    };
                    self.delay.set_line_cursor(carried, cursor);
                    self.clocked
                        .as_mut()
                        .expect("clocked state present")
                        .iota_domains
                        .insert(domain);
                }
                DelayKind::IfWrapping { counter_name } => {
                    self.clocked
                        .as_mut()
                        .expect("clocked state present")
                        .domain_ifwrap
                        .entry(domain)
                        .or_default()
                        .push((counter_name, size));
                }
            }
        }
        Ok(())
    }

    /// Lowers `ZeroPad(x, h)` inside a counted upsampling block:
    /// `((loop_idx == bound - 1) ? x : 0)` — the input value is exposed on
    /// the **last** inner iteration only (plan §3.8 `generateZeroPad`).
    pub(super) fn lower_zero_pad_clocked(
        &mut self,
        sig: SigId,
        value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let effective_depth = self
            .regions
            .redirect_depth()
            .unwrap_or_else(|| self.regions.child_depth());
        let loop_ctx = self
            .clocked
            .as_ref()
            .and_then(|c| {
                effective_depth
                    .checked_sub(1)
                    .and_then(|index| c.open_loops.get(index))
            })
            .and_then(Clone::clone);
        let Some(loop_ctx) = loop_ctx else {
            return Err(clocked_not_lowered(
                "ZeroPad outside a counted upsampling block (zero-stuffed inputs \
                 are only legal inside integer-clock blocks)",
            ));
        };
        let value_fir = self.lower_signal(value)?;
        let ty = self.signal_fir_type(sig)?;
        let zero = if matches!(ty, FirType::Int32) {
            self.lower_int32_const(0)
        } else {
            self.float_const(0.0)
        };
        let one = self.lower_int32_const(1);
        let mut b = FirBuilder::new(&mut self.store);
        let idx = b.load_var(loop_ctx.var.clone(), AccessType::Stack, FirType::Int32);
        let last = b.binop(FirBinOp::Sub, loop_ctx.bound, one, FirType::Int32);
        let is_last = b.binop(FirBinOp::Eq, idx, last, FirType::Int32);
        Ok(b.select2(is_last, value_fir, zero, ty))
    }

    /// Reads one registered hold field (`PermVar` outside its block).
    pub(super) fn lower_perm_var_read(&mut self, sig: SigId) -> Result<FirId, SignalFirError> {
        let Some(name) = self
            .clocked
            .as_ref()
            .and_then(|c| c.perm_fields.get(&sig).cloned())
        else {
            return Err(clocked_not_lowered(
                "PermVar hold read before its clocked wrapper block was emitted \
                 (expected Seq(wrapper, hold) shape from propagation)",
            ));
        };
        let ty = self.signal_fir_type(sig)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.load_var(name, AccessType::Struct, ty))
    }

    /// Emits the guarded block of one `ondemand` wrapper, idempotent.
    ///
    /// Returns a dummy value: the wrapper "returns no expression" (C++);
    /// consumers read the hold fields through `Seq`.
    pub(super) fn ensure_guarded_block(&mut self, wrapper: SigId) -> Result<FirId, SignalFirError> {
        let dummy = self.lower_int32_const(0);
        if self
            .clocked
            .as_ref()
            .is_some_and(|c| c.emitted_blocks.contains(&wrapper))
        {
            return Ok(dummy);
        }

        // ── Decode the wrapper payload ───────────────────────────────────
        let (children, kind): (Vec<SigId>, propagate::ClockDomainKind) =
            match match_sig(self.arena, wrapper) {
                SigMatch::OnDemand(children) => {
                    (children.to_vec(), propagate::ClockDomainKind::OnDemand)
                }
                SigMatch::Upsampling(children) => {
                    (children.to_vec(), propagate::ClockDomainKind::Upsampling)
                }
                SigMatch::Downsampling(children) => {
                    (children.to_vec(), propagate::ClockDomainKind::Downsampling)
                }
                _ => {
                    return Err(clocked_not_lowered(
                        "ensure_guarded_block called on a non-wrapper signal",
                    ));
                }
            };
        let Some((&first, holds)) = children.split_first() else {
            return Err(clocked_not_lowered("clocked wrapper without children"));
        };
        let SigMatch::Clocked(env_tok, clock) = match_sig(self.arena, first) else {
            return Err(clocked_not_lowered(
                "clocked wrapper first child must be Clocked(env, clock)",
            ));
        };
        let SigMatch::ClockEnvToken(domain_id) = match_sig(self.arena, env_tok) else {
            return Err(clocked_not_lowered(
                "clocked wrapper carries a malformed clock-env token",
            ));
        };
        let domain = propagate::ClockDomainId::from_u32(domain_id);

        // ── Guard shape selection (plan §3.8) ────────────────────────────
        let clock_is_int = matches!(self.types.get(&clock), Some(SimpleSigType::Int));
        let shape = match kind {
            propagate::ClockDomainKind::OnDemand => {
                let clock_interval = self
                    .sig_types
                    .get(&clock)
                    .map(sigtype::SigType::interval)
                    .ok_or_else(|| clocked_not_lowered("clock signal has no type annotation"))?;
                let boolean = clock_interval.is_valid()
                    && clock_interval.lo() >= 0.0
                    && clock_interval.hi() <= 1.0;
                if boolean {
                    GuardShape::BoolIf
                } else if clock_is_int {
                    GuardShape::CountedLoop
                } else {
                    return Err(clocked_not_lowered(
                        "ondemand with a real-valued non-boolean clock is not supported \
                         (use a boolean gate or an integer repetition count)",
                    ));
                }
            }
            propagate::ClockDomainKind::Upsampling => {
                if !clock_is_int {
                    return Err(clocked_not_lowered(
                        "upsampling requires an integer clock (inner iteration count)",
                    ));
                }
                GuardShape::CountedLoop
            }
            propagate::ClockDomainKind::Downsampling => {
                if !clock_is_int {
                    return Err(clocked_not_lowered(
                        "downsampling requires an integer clock (decimation factor)",
                    ));
                }
                GuardShape::DsModulo
            }
        };

        // ── Clock and guard precondition (outer region) ──────────────────
        let clock_fir = self.lower_signal(clock)?;
        let guard_cond = match shape {
            GuardShape::BoolIf => {
                let zero = if clock_is_int {
                    self.lower_int32_const(0)
                } else {
                    self.float_const(0.0)
                };
                let mut b = FirBuilder::new(&mut self.store);
                Some(b.binop(FirBinOp::Ne, clock_fir, zero, FirType::Int32))
            }
            GuardShape::CountedLoop => None,
            GuardShape::DsModulo => {
                // C++ declareRetrieveDSName: per-domain modulo counter,
                // fires when the counter is 0.
                let counter = {
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
                    self.domain_counters
                        .declare_retrieve_ds_counter(domain, &mut ctx)
                };
                let zero = self.lower_int32_const(0);
                let mut b = FirBuilder::new(&mut self.store);
                let counter_value = b.load_var(counter, AccessType::Struct, FirType::Int32);
                Some(b.binop(FirBinOp::Eq, counter_value, zero, FirType::Int32))
            }
        };
        let loop_ctx = match shape {
            GuardShape::CountedLoop => Some(LoopCtx {
                var: self.fresh_loop_var("lOd"),
                bound: clock_fir,
            }),
            GuardShape::BoolIf | GuardShape::DsModulo => None,
        };

        // ── Hold fields: persistent struct fields cleared to 0 ───────────
        let mut hold_stores: Vec<(SigId, String)> = Vec::with_capacity(holds.len());
        for &hold in holds {
            let SigMatch::PermVar(inner) = match_sig(self.arena, hold) else {
                return Err(clocked_not_lowered(
                    "clocked wrapper output is not a PermVar hold",
                ));
            };
            let ty = self.signal_fir_type(hold)?;
            let clocked_state = self
                .clocked
                .as_mut()
                .expect("guarded blocks only emitted for clocked programs");
            let name = format!("fPerm{}", clocked_state.next_perm_id);
            clocked_state.next_perm_id += 1;
            clocked_state.perm_fields.insert(hold, name.clone());
            let is_int = matches!(ty, FirType::Int32);
            let decl = {
                let mut b = FirBuilder::new(&mut self.store);
                b.declare_var(name.clone(), ty, AccessType::Struct, None)
            };
            self.sections.struct_declarations.push(decl);
            if self.sections.clear_init_seen.insert(name.clone()) {
                let zero = if is_int {
                    self.lower_int32_const(0)
                } else {
                    self.float_const(0.0)
                };
                let mut b = FirBuilder::new(&mut self.store);
                let clear = b.store_var(name.clone(), AccessType::Struct, zero);
                self.sections.clear_statements.push(clear);
            }
            hold_stores.push((inner, name));
        }

        // ── Body: lower the held values inside the child region ──────────
        self.regions.open_child();
        self.cache.open_child();
        {
            let clocked_state = self.clocked.as_mut().expect("clocked state present");
            clocked_state.open_domains.push(domain);
            clocked_state.open_loops.push(loop_ctx.clone());
        }
        let prev_redirect = self.regions.set_redirect(None);

        let mut body_result: Result<(), SignalFirError> = Ok(());
        if let Err(error) = self.lower_scheduled_graph(crate::hgraph::GraphKey::Wrapper(wrapper)) {
            body_result = Err(error);
        }
        for (value, field) in &hold_stores {
            if body_result.is_err() {
                break;
            }
            // Propagation wraps every held value as Clocked(env, v); the
            // payload must be emitted in this guarded block, even when the
            // payload itself is inferred at an ancestor clock environment.
            let lowered = match match_sig(self.arena, *value) {
                SigMatch::Clocked(_, payload) => self.lower_clocked_payload(payload),
                _ => self.lower_signal(*value),
            };
            let lowered = match lowered {
                Ok(id) => id,
                Err(err) => {
                    body_result = Err(err);
                    break;
                }
            };
            let store_stmt = {
                let mut b = FirBuilder::new(&mut self.store);
                b.store_var(field.clone(), AccessType::Struct, lowered)
            };
            self.regions.current_phases_mut().immediate.push(store_stmt);
        }

        {
            let clocked_state = self.clocked.as_mut().expect("clocked state present");
            clocked_state.open_domains.pop();
            clocked_state.open_loops.pop();
        }
        let mut body_phases = self.regions.close_child();
        self.cache.close_child();
        self.regions.set_redirect(prev_redirect);
        body_result?;

        // Per-domain circular cursor: advance once per fire, after all
        // reads/writes of this tick (block sample-end phase).
        if self
            .clocked
            .as_ref()
            .is_some_and(|c| c.iota_domains.contains(&domain))
        {
            let cursor = self
                .domain_counters
                .iota_name(domain)
                .expect("per-domain cursor declared during delay-cursor assignment")
                .to_owned();
            let bump = DomainCounters::emit_increment(&mut self.store, &cursor);
            body_phases.sample_end.push(bump);
        }

        // Inner `IfWrapping` delay-line counters: advance once per fire, in
        // the block sample-end phase (they were skipped at the top level).
        let ifwrap = self
            .clocked
            .as_ref()
            .and_then(|c| c.domain_ifwrap.get(&domain))
            .cloned()
            .unwrap_or_default();
        for (counter_name, size) in ifwrap {
            let advance =
                super::super::delay::emit_if_wrapping_advance(&mut self.store, &counter_name, size);
            body_phases.sample_end.push(advance);
        }

        // ── Wrap the body in the guard and append to the outer region ────
        let body_stmts = body_phases.flattened();
        let guard = {
            let mut b = FirBuilder::new(&mut self.store);
            let block = b.block(&body_stmts);
            match (&shape, guard_cond, &loop_ctx) {
                (GuardShape::BoolIf | GuardShape::DsModulo, Some(cond), _) => {
                    b.if_(cond, block, None)
                }
                (GuardShape::CountedLoop, _, Some(ctx)) => {
                    b.simple_for_loop(ctx.var.clone(), ctx.bound, block, false)
                }
                _ => unreachable!("guard shape and condition are built together"),
            }
        };
        self.regions.current_phases_mut().immediate.push(guard);
        if matches!(shape, GuardShape::DsModulo) {
            // Post-code, every outer tick: counter = (counter + 1) % clock.
            let counter = self
                .domain_counters
                .ds_counter_name(domain)
                .expect("DS counter declared above")
                .to_owned();
            let bump =
                DomainCounters::emit_wrapping_increment(&mut self.store, &counter, clock_fir);
            self.regions.current_phases_mut().immediate.push(bump);
        }

        self.clocked
            .as_mut()
            .expect("clocked state present")
            .emitted_blocks
            .insert(wrapper);
        Ok(dummy)
    }
}
