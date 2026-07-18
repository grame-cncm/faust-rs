//! Constructor and shared lowering utilities for [`SignalToFirLower`].
//!
//! Defines [`NameGen`] (monotonic counters for generated variable names) and
//! [`PlacementInfo`] (read-only placement analysis results: ref counts,
//! boundary set, konst escapes).
//!
//! Contains `SignalToFirLower::new` together with the helper methods that are
//! used across multiple lowering concerns: sample-rate variable creation,
//! delay-line preparation, type resolution, signal variability analysis,
//! bucket materialization, and FIR-type mapping.

use super::*;

/// Monotonic counters for all generated variable names.
#[derive(Default)]
pub(super) struct NameGen {
    /// Monotonic counter for generating unique loop-variable names.
    pub(super) next_loop_var_id: usize,
    /// Monotonic counter for `fConst*` init-time float constant variable names.
    pub(super) fconst_counter: u32,
    /// Monotonic counter for `iConst*` init-time integer constant variable names.
    pub(super) iconst_counter: u32,
    /// Monotonic counter for `fSlow*` block-rate float variable names.
    pub(super) fslow_counter: u32,
    /// Monotonic counter for `iSlow*` block-rate integer variable names.
    pub(super) islow_counter: u32,
    /// Monotonic counter for scheduled `fTemp*` sample-rate variables.
    pub(super) ftemp_counter: u32,
    /// Monotonic counter for scheduled `iTemp*` sample-rate variables.
    pub(super) itemp_counter: u32,
}

/// Read-only placement analysis results, computed once before lowering begins.
pub(super) struct PlacementInfo {
    /// Signal-level reference counts: how many parent nodes reference each `SigId`.
    ///
    /// Used by Phase 1 variability-driven placement to gate materialization:
    /// only nodes with `ref_count >= 2` are hoisted into a named variable.
    /// Single-use nodes stay inline, avoiding unnecessary temporaries.
    pub(super) sig_ref_counts: HashMap<SigId, usize>,
    /// Signal nodes that sit at a variability boundary (at least one parent has
    /// strictly higher variability).  These must be materialized even if
    /// single-use, to ensure they execute in the correct bucket.
    pub(super) sig_at_boundary: HashSet<SigId>,
    /// `Konst` signal nodes whose value is consumed outside `instanceConstants`.
    ///
    /// These hoists need persistent `Struct` storage; init-only `Konst` hoists
    /// can stay stack-local inside `instanceConstants()`.
    pub(super) konst_escapes: HashSet<SigId>,
}

impl PlacementInfo {
    pub(super) fn new(
        sig_ref_counts: HashMap<SigId, usize>,
        sig_at_boundary: HashSet<SigId>,
        konst_escapes: HashSet<SigId>,
    ) -> Self {
        Self {
            sig_ref_counts,
            sig_at_boundary,
            konst_escapes,
        }
    }
}

impl<'a> SignalToFirLower<'a> {
    /// Creates a fresh lowering state for one [`build_module`] call.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        arena: &'a TreeArena,
        ui_program: &'a UiProgram,
        types: &'a HashMap<SigId, SimpleSigType>,
        sig_types: &'a HashMap<SigId, SigType>,
        num_inputs: usize,
        real_ty: FirType,
        placement: PlacementInfo,
        delay_opts: DelayOptions,
    ) -> Self {
        Self {
            arena,
            ui_program,
            types,
            sig_types,
            num_inputs,
            real_ty,
            store: FirStore::new(),
            cache: region::RegionCache::new(),
            sections: state::ModuleSections::default(),
            regions: region::RegionTree::new(region::RegionKind::SampleLoop),
            clocked: None,
            suppress_clocked_redirect: false,
            domain_counters: DomainCounters::default(),
            state_name_by_node: HashMap::new(),
            recursion: RecursionState::default(),
            scheduled_state_updates: HashSet::new(),
            delay: DelayManager::new(delay_opts),
            uses_iota: false,
            ui: ui_lowering::UiLoweringState::default(),
            input_ptr_aliases: HashMap::new(),
            used_protos: arithmetic::UsedPrototypes::default(),
            name_gen: NameGen::default(),
            placement,
            rad_reverse: build::RadReverseState::default(),
            bra: bra::BraState::default(),
            emission_order: Vec::new(),
            emission_seen: HashSet::new(),
            scalar_schedule: None,
            clocked_payload_signals: HashSet::new(),
            fixed_ad_internal_signals: HashSet::new(),
        }
    }

    /// Registers every symbolic recursion binder before scheduled lowering.
    ///
    /// C++ uses `Tree` identity and vector-name properties for this binding.
    /// Rust records it explicitly so a delayed `Proj(SYMREF)` can reserve the
    /// owning carrier before `Proj(SYMREC)` emits the group update.
    pub(super) fn register_symbolic_recursion_groups(
        &mut self,
        outputs: &[SigId],
    ) -> Result<(), SignalFirError> {
        let mut stack = outputs.to_vec();
        let mut visited = HashSet::new();
        while let Some(sig) = stack.pop() {
            if !visited.insert(sig) {
                continue;
            }
            if let Some((var, _)) = match_sym_rec(self.arena, sig)
                && let Some(previous) = self.recursion.register_group(var, sig)
                && previous != sig
            {
                return Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!(
                        "symbolic recursion variable {} belongs to both groups {} and {}",
                        var.as_u32(),
                        previous.as_u32(),
                        sig.as_u32()
                    ),
                ));
            }
            if let Some(children) = self.arena.children(sig) {
                stack.extend(children.iter().copied());
            }
        }
        Ok(())
    }

    /// Ensures the canonical DSP sample-rate field is present in the FIR struct.
    ///
    /// Backends should consume this field directly instead of synthesizing their
    /// own `fSampleRate` side channel.
    pub(super) fn ensure_sample_rate_var(&mut self) {
        self.ensure_named_struct_var("fSampleRate", FirType::Int32, None);
    }

    /// Pre-scans the output signal forest and allocates all delay lines before
    /// lowering begins.
    ///
    /// This preparation step is a single pass: [`plan_delays`] walks the reachable
    /// signal forest once and returns a `DelayPlan` holding both the per-carrier
    /// maximum owned delays (the standalone lines to allocate) and the
    /// recursion-output sizing metadata.
    ///
    /// Multiple `SIGDELAY(x, n)` nodes sharing the same carried signal `x`
    /// reuse one delay line sized to the largest delay seen. Standalone
    /// `Delay1(x)` nodes that use the shift strategy are included in the same
    /// pre-pass so delay-line geometry is decided exactly once up front.
    ///
    /// Recursion carriers are not allocated here directly; their size is
    /// planned by the accumulated delay analysis and consumed later by
    /// `ensure_recursion_array_for_group`.
    ///
    /// This pre-pass ensures all resource-sizing decisions are registered
    /// before reads are emitted during lowering.
    pub(super) fn prepare_delay_lines(&mut self, outputs: &[SigId]) -> Result<(), SignalFirError> {
        let plan = plan_delays(self.arena, self.sig_types, outputs, &self.delay.options())?;
        self.delay.set_rec_output_analysis(plan.rec_outputs);
        for (carried, delay) in plan.lines {
            self.ensure_delay_line_decl(carried, delay)?;
        }
        Ok(())
    }

    /// Emits the BRA reverse update for a supported unary math node.
    ///
    /// Unlike the pure Signal RAD path, BRA cannot freely rebuild every
    /// operand expression during the reverse sweep: operands may be temporal,
    /// recursive, or otherwise already materialized in forward storage. This
    /// method therefore performs the tape-aware loads first, then delegates
    /// only the pointwise algebra to `ad_rules`. For formulas that can reuse the
    /// forward node output (`exp`, `sqrt`, `abs`), `sig` is loaded as `primal`
    /// so the local transpose uses the recorded forward value rather than a
    /// second computation.
    pub(super) fn propagate_bra_unary_math_adj(
        &mut self,
        rule: RadUnaryMathRule,
        sig: SigId,
        x: SigId,
        y_bar: FirId,
        adj: &mut std::collections::HashMap<SigId, FirId>,
    ) -> Result<(), SignalFirError> {
        let real_ty = self.real_ty.clone();
        let x_fir = self.load_bra_fwd_value(x)?;
        // The shared formula only sees values. For rules whose derivative can
        // reuse the forward output, pass the tape-loaded current node value so
        // the reverse sweep does not recompute non-trivial temporal operands.
        let primal = match rule {
            RadUnaryMathRule::Exp | RadUnaryMathRule::Sqrt | RadUnaryMathRule::Abs => {
                self.load_bra_fwd_value(sig)?
            }
            _ => x_fir,
        };
        let mut b = FirRadFormulaBuilder::new(self, real_ty.clone());
        let x_adj = rad_unary_contribution(&mut b, rule, x_fir, primal, y_bar);
        Self::add_to_adjoint(&mut self.store, adj, x, x_adj, real_ty);
        Ok(())
    }

    /// Emits the BRA reverse updates for a supported binary math node.
    ///
    /// This method is the FIR/BRA counterpart of `propagate_binary_math`: it
    /// loads both forward operand values from BRA storage, lets the shared
    /// `ad_rules` formula build the two local cotangents in FIR, then
    /// accumulates them into the reverse adjoint map. `pow` additionally needs
    /// the stored forward result of `sig` for its exponent contribution; other
    /// binary math rules depend only on the loaded operands and ignore the
    /// `primal` placeholder.
    pub(super) fn propagate_bra_binary_math_adj(
        &mut self,
        rule: RadBinaryMathRule,
        lhs: SigId,
        rhs: SigId,
        sig: SigId,
        y_bar: FirId,
        adj: &mut std::collections::HashMap<SigId, FirId>,
    ) -> Result<(), SignalFirError> {
        let real_ty = self.real_ty.clone();
        let lhs_fir = self.load_bra_fwd_value(lhs)?;
        let rhs_fir = self.load_bra_fwd_value(rhs)?;
        // `pow` needs its forward output for the exponent derivative. Other
        // binary rules compute their local transpose from operand values only,
        // so the placeholder is intentionally ignored by the shared helper.
        let primal = match rule {
            RadBinaryMathRule::Pow => self.load_bra_fwd_value(sig)?,
            _ => lhs_fir,
        };
        let mut b = FirRadFormulaBuilder::new(self, real_ty.clone());
        let (lhs_adj, rhs_adj) =
            rad_binary_contributions(&mut b, rule, lhs_fir, rhs_fir, primal, y_bar);
        Self::add_to_adjoint(&mut self.store, adj, lhs, lhs_adj, real_ty.clone());
        Self::add_to_adjoint(&mut self.store, adj, rhs, rhs_adj, real_ty);
        Ok(())
    }

    /// Returns a clone of the internal real computation type.
    ///
    /// Use this whenever a FIR node must carry the internal scalar precision
    /// (arithmetic result, state slot, math call, real constant, …).
    /// For external interface points (audio buffer samples, UI zone variables)
    /// use `FirType::FaustFloat` directly instead.
    pub(super) fn real_ty(&self) -> FirType {
        self.real_ty.clone()
    }

    // ── Variability-driven statement placement (Phase 1) ──────────────────

    /// Returns the signal-level variability for a node, if type info exists.
    ///
    /// Variability drives the execution-tier placement of the resulting FIR
    /// expression:
    /// - [`Variability::Konst`] → `constants_statements` (once at init)
    /// - [`Variability::Block`] → `control_statements` (once per `compute()`)
    /// - [`Variability::Samp`]  → sample-loop immediate phase
    pub(super) fn variability_of(&self, sig: SigId) -> Option<Variability> {
        self.sig_types.get(&sig).map(|t| t.variability())
    }

    /// Returns `true` when a hoisted `Konst` value must remain persistent
    /// beyond `instanceConstants()`.
    pub(super) fn konst_escapes(&self, sig: SigId) -> bool {
        self.placement.konst_escapes.contains(&sig)
    }

    /// Returns the typed prefix used for one materialized scalar value.
    pub(super) fn typed_prefix_for(bucket: Bucket, typ: &FirType) -> &'static str {
        let is_int_like = matches!(typ, FirType::Int32 | FirType::Int64 | FirType::Bool);
        match (bucket, is_int_like) {
            (Bucket::Constants, true) => "iConst",
            (Bucket::Constants, false) => "fConst",
            (Bucket::Control, true) => "iSlow",
            (Bucket::Control, false) => "fSlow",
        }
    }

    /// Returns the next numeric suffix for one typed materialization prefix.
    pub(super) fn next_materialized_counter(&mut self, prefix: &str) -> u32 {
        match prefix {
            "fConst" => {
                let n = self.name_gen.fconst_counter;
                self.name_gen.fconst_counter += 1;
                n
            }
            "iConst" => {
                let n = self.name_gen.iconst_counter;
                self.name_gen.iconst_counter += 1;
                n
            }
            "fSlow" => {
                let n = self.name_gen.fslow_counter;
                self.name_gen.fslow_counter += 1;
                n
            }
            "iSlow" => {
                let n = self.name_gen.islow_counter;
                self.name_gen.islow_counter += 1;
                n
            }
            other => panic!("unsupported materialized prefix `{other}`"),
        }
    }

    /// Returns `true` when the signal is a direct `Proj(i, SYMREC)` read.
    ///
    /// The type system (after the `update_rec_types` variability-join fix)
    /// guarantees that such nodes always carry at least `Samp` variability, so
    /// they would not be hoisted by the placement logic anyway.  This guard is
    /// kept as a defensive check against future regressions.
    pub(super) fn is_recursive_projection(&self, sig: SigId) -> bool {
        if let SigMatch::Proj(_, group) = match_sig(self.arena, sig) {
            let group = match match_sig(self.arena, group) {
                SigMatch::ReverseTimeRec(body) => body,
                _ => group,
            };
            match_sym_rec(self.arena, group).is_some()
                || match_sym_ref(self.arena, group).is_some()
                || tlib::match_de_bruijn_ref(self.arena, group).is_some()
        } else {
            false
        }
    }

    /// Materializes a FIR value expression into a named variable in the
    /// given execution-tier bucket.
    ///
    /// Returns a [`FirId`] for the `LoadVar` that reads the materialized
    /// variable.  The corresponding `DeclareVar` (with initializer) is
    /// appended to the appropriate lifecycle accumulator:
    ///
    /// | Bucket | Prefix | Access | Lifecycle section |
    /// |--------|--------|--------|-------------------|
    /// | `Constants` | `iConst` / `fConst` | [`AccessType::Stack`] for init-local, [`AccessType::Struct`] for escaping values | `instanceConstants` |
    /// | `Control` | `iSlow` / `fSlow` | [`AccessType::Stack`] | `compute` preamble |
    ///
    /// `Konst` variables that feed `compute()` use struct storage because they
    /// are written in `instanceConstants()` and read later; init-only `Konst`
    /// temporaries and all `Block` variables stay stack-local.
    pub(super) fn materialize_in_bucket(
        &mut self,
        sig: SigId,
        value: FirId,
        bucket: Bucket,
    ) -> FirId {
        let typ = self
            .store
            .value_type(value)
            .unwrap_or_else(|| self.real_ty());
        let prefix = Self::typed_prefix_for(bucket, &typ);
        let n = self.next_materialized_counter(prefix);
        let access = match bucket {
            Bucket::Constants if self.konst_escapes(sig) => AccessType::Struct,
            Bucket::Constants | Bucket::Control => AccessType::Stack,
        };
        let name = format!("{prefix}{n}");

        match bucket {
            Bucket::Constants if access == AccessType::Struct => {
                self.ensure_named_struct_var(&name, typ.clone(), None);
                let mut b = FirBuilder::new(&mut self.store);
                self.sections.constants_statements.push(b.store_var(
                    &name,
                    AccessType::Struct,
                    value,
                ));
            }
            Bucket::Constants => {
                let mut b = FirBuilder::new(&mut self.store);
                self.sections.constants_statements.push(b.declare_var(
                    &name,
                    typ.clone(),
                    AccessType::Stack,
                    Some(value),
                ));
            }
            Bucket::Control => {
                let mut b = FirBuilder::new(&mut self.store);
                self.sections.control_statements.push(b.declare_var(
                    &name,
                    typ.clone(),
                    AccessType::Stack,
                    Some(value),
                ));
            }
        }

        let mut b = FirBuilder::new(&mut self.store);
        b.load_var(name, access, typ)
    }

    /// Materializes one shared sample-rate signal at its selected schedule
    /// position.
    ///
    /// C++ provenance: `InstructionsCompiler::CS` visits `Hsched` and
    /// `generateCacheCode` immediately delegates multi-occurrence sample
    /// signals to `generateVariableStore`. Performing this before FIR CSE is
    /// what makes `-ss` observable in the generated sample loop while keeping
    /// the signal semantics unchanged.
    pub(super) fn materialize_scheduled_sample(&mut self, value: FirId) -> FirId {
        let typ = self
            .store
            .value_type(value)
            .unwrap_or_else(|| self.real_ty());
        let name = if typ == FirType::Int32 {
            let counter = self.name_gen.itemp_counter;
            self.name_gen.itemp_counter += 1;
            format!("iTemp{counter}")
        } else {
            let counter = self.name_gen.ftemp_counter;
            self.name_gen.ftemp_counter += 1;
            format!("fTemp{counter}")
        };
        let mut b = FirBuilder::new(&mut self.store);
        self.regions
            .current_phases_mut()
            .immediate
            .push(b.declare_var(name.clone(), typ.clone(), AccessType::Stack, Some(value)));
        b.load_var(name, AccessType::Stack, typ)
    }

    /// Returns the reduced prepared signal type attached to one signal node.
    ///
    /// The fast-lane relies on the pre-FIR `signal_prepare` boundary to decide
    /// whether one value/state/table should stay integer or use the internal
    /// real computation type, mirroring the reduced
    /// `deBruijn2Sym -> typeAnnotation -> signalPromote` contract.
    pub(super) fn simple_type(&self, sig: SigId) -> Result<SimpleSigType, SignalFirError> {
        self.types.get(&sig).copied().ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("missing prepared type for signal {}", sig.as_u32()),
            )
        })
    }

    /// Maps one prepared signal type to the FIR value type used by lowering.
    pub(super) fn signal_fir_type(&self, sig: SigId) -> Result<FirType, SignalFirError> {
        match self.simple_type(sig)? {
            SimpleSigType::Int => Ok(FirType::Int32),
            SimpleSigType::Real => Ok(self.real_ty()),
            SimpleSigType::Sound => Ok(FirType::Sound),
        }
    }

    /// Returns the typed zero initializer used for state slots and table
    /// declarations.
    pub(super) fn zero_value_for_signal(&mut self, sig: SigId) -> Result<FirId, SignalFirError> {
        match self.simple_type(sig)? {
            SimpleSigType::Int => Ok(self.lower_int32_const(0)),
            SimpleSigType::Real => Ok(self.float_const(0.0)),
            SimpleSigType::Sound => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "signal {} cannot use a soundfile handle as delay/table state",
                    sig.as_u32()
                ),
            )),
        }
    }
}
