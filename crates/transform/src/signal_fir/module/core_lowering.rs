//! Central `lower_signal` dispatcher and leaf signal lowering.
//!
//! The [`SignalToFirLower::lower_signal`] method defined here is the single
//! recursive entry point for the signal-to-FIR traversal.  It dispatches on
//! the signal-tree shape via `match_sig` and delegates each case to a
//! specialised helper: constants, inputs, delay chains, casts, BinOps, math
//! functions, FFI calls, and recursion projections.
//!
//! Results are memoized in [`SignalToFirLower::cache`] for DAG sharing.

use super::*;

impl<'a> SignalToFirLower<'a> {
    /// Lowers every node owned by one hierarchical graph in the accepted
    /// dependencies-first order. Recursive calls remain responsible for
    /// expression construction, while the previsit fixes the order in which
    /// independent nodes first enter the region-local caches and statements.
    pub(super) fn lower_scheduled_graph(
        &mut self,
        key: crate::hgraph::GraphKey,
    ) -> Result<(), SignalFirError> {
        let schedule = self
            .scalar_schedule
            .as_ref()
            .and_then(|schedule| schedule.schedule(key))
            .unwrap_or(&[])
            .to_vec();
        for sig in schedule {
            if self.fixed_ad_internal_signals.contains(&sig)
                || matches!(
                    match_sig(self.arena, sig),
                    SigMatch::BlockReverseAD { .. } | SigMatch::ReverseTimeRec(_)
                )
            {
                continue;
            }
            if matches!(
                key,
                crate::hgraph::GraphKey::Control | crate::hgraph::GraphKey::Top
            ) && self.clocked_payload_signals.contains(&sig)
            {
                continue;
            }
            if matches!(key, crate::hgraph::GraphKey::Wrapper(_))
                && self.clocked_payload_signals.contains(&sig)
            {
                let payload = match match_sig(self.arena, sig) {
                    SigMatch::Clocked(_, payload) => payload,
                    _ => sig,
                };
                let _ = self.lower_clocked_payload(payload)?;
            } else {
                let _ = self.lower_signal(sig)?;
            }
        }
        Ok(())
    }

    /// Central dispatcher: lowers one signal node to a FIR value expression.
    ///
    /// Results are memoized in [`Self::cache`] for DAG sharing.  As a side
    /// effect, successful lowering may append declarations and assignments to
    /// lifecycle section accumulators (e.g. sample-loop phase statements,
    /// state declarations to
    /// [`Self::sections`]).
    ///
    /// Returns a typed `FRS-SFIR-*` error for unsupported signal families.
    pub(super) fn lower_signal(&mut self, sig: SigId) -> Result<FirId, SignalFirError> {
        if let Some(id) = self.cache.get_at(self.regions.effective_depth(), sig) {
            return Ok(id);
        }

        // P3 clocked emission: a signal computed in a strict-ancestor clock
        // domain must have its statements placed in the ancestor's region,
        // even while a guarded block is open (see `clocked.rs`). The
        // recursive re-entry cannot loop: with the redirection installed,
        // the effective domain equals the signal's domain.
        if !self.suppress_clocked_redirect
            && let Some(depth) = self.clocked_redirect_target(sig)
        {
            let previous = self.regions.set_redirect(Some(depth));
            let result = self.lower_signal(sig);
            self.regions.set_redirect(previous);
            return result;
        }

        let lowered = match match_sig(self.arena, sig) {
            SigMatch::Int(value) => self.lower_int32_const(value),
            // Real constant: emitted at internal precision (Float32 or Float64).
            SigMatch::Real(value) => self.float_const(value),
            SigMatch::Input(index) => self.lower_input(index)?,
            SigMatch::Output(_, inner) => self.lower_signal(inner)?,
            SigMatch::Delay1(value) => {
                // Recursion delay chains that ultimately read from an active
                // recursion carrier are lowered through that carrier directly.
                // Standalone Delay1 nodes keep using the dedicated fast path
                // when the shift strategy is enabled.
                if self.resolve_recursion_delay_ref(value)?.is_none()
                    && self.delay.max_copy_delay() >= 1
                {
                    self.lower_shift_delay1(sig, value)?
                } else {
                    let init = self.zero_value_for_signal(sig)?;
                    self.lower_delay_state(sig, value, init)?
                }
            }
            SigMatch::Delay(value, amount) => self.lower_delay(sig, value, amount)?,
            SigMatch::Prefix(init_sig, value) => {
                let init = self.initial_state_from_signal(init_sig);
                self.lower_delay_state(sig, value, init)?
            }
            SigMatch::IntCast(value) => self.lower_cast(FirType::Int32, value)?,
            // BitCast and FloatCast convert to the internal real type, not to
            // FaustFloat: they are integer↔float reinterpretation/coercion
            // operations used in internal DSP computation.
            SigMatch::BitCast(value) => self.lower_bitcast(self.real_ty(), value)?,
            SigMatch::FloatCast(value) => self.lower_cast(self.real_ty(), value)?,
            SigMatch::Select2(cond, else_value, then_value) => {
                self.lower_select2(sig, cond, then_value, else_value)?
            }
            SigMatch::Proj(index, group) => self.lower_proj(sig, index, group)?,
            SigMatch::BinOp(op, lhs, rhs) => self.lower_binop(sig, op, lhs, rhs)?,
            SigMatch::Pow(lhs, rhs) => self.lower_math2(FirMathOp::Pow, lhs, rhs)?,
            SigMatch::Min(lhs, rhs) => self.lower_minmax(sig, lhs, rhs, true)?,
            SigMatch::Max(lhs, rhs) => self.lower_minmax(sig, lhs, rhs, false)?,
            SigMatch::Sin(value) => self.lower_math1(FirMathOp::Sin, value)?,
            SigMatch::Cos(value) => self.lower_math1(FirMathOp::Cos, value)?,
            SigMatch::Acos(value) => self.lower_math1(FirMathOp::Acos, value)?,
            SigMatch::Asin(value) => self.lower_math1(FirMathOp::Asin, value)?,
            SigMatch::Atan(value) => self.lower_math1(FirMathOp::Atan, value)?,
            SigMatch::Atan2(lhs, rhs) => self.lower_math2(FirMathOp::Atan2, lhs, rhs)?,
            SigMatch::Tan(value) => self.lower_math1(FirMathOp::Tan, value)?,
            SigMatch::Exp(value) => self.lower_math1(FirMathOp::Exp, value)?,
            SigMatch::Exp10(value) => self.lower_math1(FirMathOp::Exp10, value)?,
            SigMatch::Log(value) => self.lower_math1(FirMathOp::Log, value)?,
            SigMatch::Log10(value) => self.lower_math1(FirMathOp::Log10, value)?,
            SigMatch::Sqrt(value) => self.lower_math1(FirMathOp::Sqrt, value)?,
            SigMatch::Abs(value) => self.lower_abs(sig, value)?,
            SigMatch::Fmod(lhs, rhs) => self.lower_math2(FirMathOp::Fmod, lhs, rhs)?,
            SigMatch::Remainder(lhs, rhs) => self.lower_math2(FirMathOp::Remainder, lhs, rhs)?,
            SigMatch::Floor(value) => self.lower_math1(FirMathOp::Floor, value)?,
            SigMatch::Ceil(value) => self.lower_math1(FirMathOp::Ceil, value)?,
            SigMatch::Rint(value) => self.lower_math1(FirMathOp::Rint, value)?,
            SigMatch::Round(value) => self.lower_math1(FirMathOp::Round, value)?,
            SigMatch::Lowest(value) => self.lower_signal(value)?,
            SigMatch::Highest(value) => self.lower_signal(value)?,
            SigMatch::FConst(_, name, _) => self.lower_fconst(sig, name)?,
            SigMatch::RdTbl(tbl, ridx) => self.lower_rdtbl(sig, tbl, ridx)?,
            SigMatch::WrTbl(size, generator, widx, wsig) => {
                self.lower_wrtbl(sig, size, generator, widx, wsig)?
            }
            SigMatch::Waveform(values) => self.lower_waveform(sig, values)?,
            SigMatch::Button(control) => self.lower_button(control, ButtonType::Button)?,
            SigMatch::Checkbox(control) => self.lower_button(control, ButtonType::Checkbox)?,
            SigMatch::VSlider(control) => self.lower_slider(control, SliderType::Vertical)?,
            SigMatch::HSlider(control) => self.lower_slider(control, SliderType::Horizontal)?,
            SigMatch::NumEntry(control) => self.lower_slider(control, SliderType::NumEntry)?,
            SigMatch::VBargraph(control, value) => {
                self.lower_bargraph(control, value, BargraphType::Vertical)?
            }
            SigMatch::HBargraph(control, value) => {
                self.lower_bargraph(control, value, BargraphType::Horizontal)?
            }
            SigMatch::Attach(lhs, rhs) => {
                let _ = self.lower_signal(rhs)?;
                self.lower_signal(lhs)?
            }
            SigMatch::Enable(lhs, rhs) => {
                let zero = self.zero_value_for_signal(sig)?;
                let lhs = self.lower_signal(lhs)?;
                let cond = self.lower_signal(rhs)?;
                let real_ty = self.signal_fir_type(sig)?;
                let mut b = FirBuilder::new(&mut self.store);
                b.select2(cond, lhs, zero, real_ty)
            }
            SigMatch::Control(lhs, rhs) => {
                let _ = self.lower_signal(rhs)?;
                self.lower_signal(lhs)?
            }
            SigMatch::FFun(ff, largs) => self.lower_ffun(sig, ff, largs)?,
            SigMatch::FVar(kind, name, file) => self.lower_fvar(sig, kind, name, file)?,
            SigMatch::Soundfile(control) => self.lower_soundfile(control)?,
            SigMatch::SoundfileLength(sf, part) => self.lower_soundfile_length(sf, part)?,
            SigMatch::SoundfileRate(sf, part) => self.lower_soundfile_rate(sf, part)?,
            SigMatch::SoundfileBuffer(sf, chan, part, ridx) => {
                self.lower_soundfile_buffer(sig, sf, chan, part, ridx)?
            }
            // Clocked machinery under an active clocked-lowering state
            // (roadmap P3, boolean-ondemand slice — see `clocked.rs`):
            // `Seq(od, y)` compiles the block then reads the hold;
            // `Clocked`/`TempVar` are annotations/snapshots (passthrough);
            // `PermVar` reads its registered hold field; `OnDemand` emits
            // the guarded block.
            SigMatch::Seq(x, y) if self.clocked.is_some() => {
                let _ = self.lower_signal(x)?;
                self.lower_signal(y)?
            }
            SigMatch::Clocked(_, inner) if self.clocked.is_some() => self.lower_signal(inner)?,
            SigMatch::TempVar(inner) if self.clocked.is_some() => {
                if self.suppress_clocked_redirect {
                    self.lower_clocked_temp_var(inner)?
                } else {
                    self.lower_signal(inner)?
                }
            }
            SigMatch::PermVar(_) if self.clocked.is_some() => self.lower_perm_var_read(sig)?,
            SigMatch::ZeroPad(value, _) if self.clocked.is_some() => {
                self.lower_zero_pad_clocked(sig, value)?
            }
            SigMatch::OnDemand(_) | SigMatch::Upsampling(_) | SigMatch::Downsampling(_)
                if self.clocked.is_some() =>
            {
                self.ensure_guarded_block(sig)?
            }
            // Clocked machinery (`ondemand` / `upsampling` / `downsampling`
            // wrappers and their glue nodes): accepted by propagation and
            // `signal_prepare`, but the guarded-block lowering (roadmap
            // P1–P3) has not landed. Reject with the dedicated structured
            // code so no clocked program falls into the generic
            // `FRS-SFIR-0004` bucket or panics (roadmap P0.1).
            SigMatch::OnDemand(_)
            | SigMatch::Upsampling(_)
            | SigMatch::Downsampling(_)
            | SigMatch::Clocked(_, _)
            | SigMatch::ClockEnvToken(_)
            | SigMatch::Seq(_, _)
            | SigMatch::TempVar(_)
            | SigMatch::PermVar(_)
            | SigMatch::ZeroPad(_, _) => {
                let construct = match match_sig(self.arena, sig) {
                    SigMatch::OnDemand(_) => "ondemand",
                    SigMatch::Upsampling(_) => "upsampling",
                    SigMatch::Downsampling(_) => "downsampling",
                    SigMatch::Clocked(_, _) => "clocked wrapper",
                    SigMatch::ClockEnvToken(_) => "clock-env token",
                    SigMatch::Seq(_, _) => "clocked sequencing (Seq)",
                    SigMatch::TempVar(_) => "clock-boundary input (TempVar)",
                    SigMatch::PermVar(_) => "clock-boundary output (PermVar)",
                    _ => "zero-padded upsampling input (ZeroPad)",
                };
                return Err(SignalFirError::new(
                    SignalFirErrorCode::ClockedNotLowered,
                    format!(
                        "{construct} is not lowered to FIR yet: the clock-domain \
                         back half (inference, guarded blocks, per-domain local \
                         time — roadmap P1–P3) has not been ported"
                    ),
                ));
            }
            other => {
                return Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!(
                        "unsupported signal node in Step 2C: {other:?} (expr={})",
                        dump_sig_readable(self.arena, sig)
                    ),
                ));
            }
        };

        // ── Variability-driven placement (Phase 1) ──────────────────────
        //
        // Non-trivial expressions whose variability is slower than sample
        // rate are hoisted into the appropriate execution-tier bucket:
        //   Konst → constants_statements (instanceConstants, once at init)
        //   Block → control_statements   (compute preamble, once per call)
        //   Samp  → shared nodes are materialized at their Hsched position
        //
        // To avoid creating unnecessary temporaries for intermediate
        // sub-expressions, only nodes referenced ≥ 2 times in the signal
        // DAG are materialized into named variables (`iConst*`/`fConst*`,
        // `iSlow*`/`fSlow*`).
        // Single-use nodes at the same variability tier stay inline inside
        // their parent's expression.  This matches C++ Faust behavior
        // where compound expressions like `fConst6 * cos(fConst7 * fSlow2)`
        // are emitted as one variable instead of three.
        //
        // However, at a **variability boundary** (Block→Samp or
        // Konst→Block/Samp), even single-use nodes must be materialized
        // to ensure they execute in the correct bucket.  Without this,
        // a single-use Block-rate sub-expression of a Samp parent would
        // be inlined into the per-sample loop body, re-evaluated every
        // sample.
        //
        // Guards:
        // - Trivial nodes (literals, loads) are never hoisted — they are
        //   free to duplicate and hoisting them wastes a variable name.
        // - Recursive projections must stay in the sample loop; the type
        //   system ensures they are always Samp, but the guard is kept as
        //   a defensive check.
        // - SIGWRTBL nodes: the type system assigns Konst variability
        //   (from `make_table_type`) reflecting the static table content,
        //   but `lower_wrtbl` returns the write signal's value which may
        //   reference Samp-rate state (e.g. `iWave*` cycling counters).
        //   Hoisting would place `LoadVar("iWave*")` inside
        //   `instanceConstants`, before `instanceClear` has initialized it.
        let sig_shared = self
            .placement
            .sig_ref_counts
            .get(&sig)
            .copied()
            .unwrap_or(0)
            >= 2;
        let at_boundary = self.placement.sig_at_boundary.contains(&sig);
        let lowered = if !is_trivial_fir(&self.store, lowered)
            && !self.is_recursive_projection(sig)
            && !matches!(match_sig(self.arena, sig), SigMatch::WrTbl(..))
            && (sig_shared || at_boundary)
        {
            match self.variability_of(sig) {
                Some(Variability::Konst) => {
                    self.materialize_in_bucket(sig, lowered, Bucket::Constants)
                }
                Some(Variability::Block) => {
                    self.materialize_in_bucket(sig, lowered, Bucket::Control)
                }
                Some(Variability::Samp) if self.scalar_schedule.is_some() => {
                    self.materialize_scheduled_sample(lowered)
                }
                _ => lowered,
            }
        } else {
            lowered
        };

        if !self.is_recursive_projection(sig) {
            // First materialization of `sig`: the sole cache-insertion site
            // and therefore the exact P3 conformance trace compared with the
            // selected Hsched (`crate::signal_fir::shadow`).
            self.cache
                .insert_at(self.regions.effective_depth(), sig, lowered);
            if self.emission_seen.insert(sig) {
                self.emission_order.push(sig);
            }
        }
        Ok(lowered)
    }

    /// Lowers one top-level signal into the currently active sample-loop
    /// accumulator.
    ///
    /// The caller controls which sample loop is active by clearing
    /// the current compute region between forward and reverse scheduling slices.
    /// Output signals are cast at the external FaustFloat boundary and stored
    /// into `outputN[i0]`; non-output surplus signals are evaluated and dropped.
    pub(super) fn lower_output_signal(
        &mut self,
        signal_index: usize,
        sig: SigId,
        num_outputs: usize,
    ) -> Result<(), SignalFirError> {
        let mut value = self.lower_signal(sig)?;
        if signal_index < num_outputs {
            let needs_output_cast = self.store.value_type(value) != Some(FirType::FaustFloat);
            let mut b = FirBuilder::new(&mut self.store);
            if needs_output_cast {
                value = b.cast(FirType::FaustFloat, value);
            }
            let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
            self.regions
                .current_phases_mut()
                .immediate
                .push(b.store_table(
                    format!("output{signal_index}"),
                    AccessType::Stack,
                    i0,
                    value,
                ));
        } else {
            let mut b = FirBuilder::new(&mut self.store);
            self.regions
                .current_phases_mut()
                .immediate
                .push(b.drop_(value));
        }
        Ok(())
    }

    /// Clears per-loop scheduling state before building another sample loop.
    pub(super) fn reset_sample_loop_state(&mut self, next_kind: region::RegionKind) {
        self.regions.begin_sibling(next_kind);
        self.scheduled_state_updates.clear();
        self.recursion.scheduled_groups.clear();
    }

    /// Lowers supported foreign constants.
    ///
    /// Active parity slice mirrors the C++ fast-lane special-case for
    /// `fSamplingFreq`, which loads the persistent `fSampleRate` struct field.
    ///
    /// `fSamplingFreq` is typed as Int in the signal domain, so its FIR type is
    /// always `Int32`.  If it appears in a Real context the promoter wraps it in a
    /// `FloatCast` node, which is lowered separately by `lower_cast`.  No implicit
    /// cast is needed here.
    pub(super) fn lower_fconst(
        &mut self,
        sig: SigId,
        name: SigId,
    ) -> Result<FirId, SignalFirError> {
        let name = self.label_text(name);
        if name == "fSamplingFreq" || name == "fSamplingRate" {
            // The Faust runtime stores the sample rate as a 32-bit integer
            // (`fSampleRate` struct field, type `int`).  However, when the
            // Faust signal tree uses this constant in floating-point arithmetic
            // (e.g. `si.smoo` → `tau2pole` → `exp(-2π/ma.SR)`), the prepared
            // type of the FConst node is `Real`.  Emitting a bare `Int32` load
            // there causes a FIR type-mismatch error at verify time.
            //
            // Fix: load as `Int32` and then cast to the expected FIR type when
            // the signal's prepared type is `Real`.
            let int_val = {
                let mut b = FirBuilder::new(&mut self.store);
                b.load_var("fSampleRate", AccessType::Struct, FirType::Int32)
            };
            let expected_ty = self.signal_fir_type(sig)?;
            if expected_ty == FirType::Int32 {
                return Ok(int_val);
            }
            let mut b = FirBuilder::new(&mut self.store);
            return Ok(b.cast(expected_ty, int_val));
        }
        self.unsupported_node(
            sig,
            &format!("unsupported foreign constant `{name}` in Step 2C"),
        )
    }

    /// Lowers one foreign variable load.
    ///
    /// Active parity slice mirrors `InstructionsCompiler::generateFVar`:
    /// - `count` is a special Faust runtime symbol (`fFullCount` in the C++
    ///   generator), not a normal extern. In scalar `compute(int count, ...)`
    ///   codegen it denotes the current block size, so we must lower it to the
    ///   existing FIR function argument rather than emitting a separate global.
    /// - any other foreign variable is treated as an extern global and loaded
    ///   through `AccessType::Global`, with one declaration emitted per symbol.
    ///
    /// Source provenance (C++):
    /// - `compiler/generator/instructions_compiler.cpp` (`generateFVar`)
    pub(super) fn lower_fvar(
        &mut self,
        _sig: SigId,
        kind: SigId,
        name: SigId,
        _file: SigId,
    ) -> Result<FirId, SignalFirError> {
        let name = self.label_text(name);
        let typ = self.foreign_sig_type(kind);
        let mut b = FirBuilder::new(&mut self.store);

        if name == "count" {
            return Ok(b.load_var(name, AccessType::FunArgs, typ));
        }

        if !self.used_protos.foreign_vars.contains_key(&name) {
            let decl = b.declare_var(name.to_owned(), typ.clone(), AccessType::Global, None);
            self.sections.global_declarations.push(decl);
            self.used_protos
                .foreign_vars
                .insert(name.to_owned(), typ.clone());
        }

        Ok(b.load_var(name, AccessType::Global, typ))
    }

    /// Lowers one foreign function call to a FIR `FunCall` plus extern prototype.
    ///
    /// Source provenance (C++):
    /// - `compiler/signals/prim2.cpp` (`ffname`, `ffrestype`, `ffargtype`)
    /// - `compiler/generator/instructions_compiler.cpp` (`generateFFun`)
    pub(super) fn lower_ffun(
        &mut self,
        sig: SigId,
        ff: SigId,
        largs: SigId,
    ) -> Result<FirId, SignalFirError> {
        let proto = self.decode_foreign_fun_proto(ff)?;
        let args = list_to_vec(self.arena, largs).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "malformed SIGFFUN argument list in Step 2C (expr={})",
                    dump_sig_readable(self.arena, sig)
                ),
            )
        })?;
        if args.len() != proto.args.len() {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "foreign function `{}` arity mismatch in Step 2C: expected {}, got {}",
                    proto.name,
                    proto.args.len(),
                    args.len()
                ),
            ));
        }

        let mut lowered_args = Vec::with_capacity(args.len());
        for arg in args {
            lowered_args.push(self.lower_signal(arg)?);
        }
        self.used_protos
            .foreign_fun_protos
            .entry(proto.name.clone())
            .or_insert_with(|| proto.clone());

        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.fun_call(proto.name, &lowered_args, proto.ret))
    }

    /// Decodes one Faust `FFUN(signature, incfile, libfile)` descriptor.
    /// Extracts a [`ForeignFunProto`] from a Faust `FFUN(signature, _, _)` descriptor.
    ///
    /// The `signature` list has the layout `[ret_type, [name_f32, name_f64], arg0_type, …]`:
    /// index 0 is the return type code, index 1 is the name list (0=float32 name,
    /// 1=float64 name), and indices 2+ are argument type codes.  Type codes follow
    /// `foreign_sig_type`: `0` → `Int32`, any other value → `real_ty`.
    pub(super) fn decode_foreign_fun_proto(
        &self,
        ff: SigId,
    ) -> Result<ForeignFunProto, SignalFirError> {
        let Some((signature, _, _)) = match_ffunction_node(self.arena, ff) else {
            return self.unsupported_node(ff, "SIGFFUN descriptor is not an FFUNCTION node");
        };
        let items = list_to_vec(self.arena, signature).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "malformed foreign function signature list in Step 2C",
            )
        })?;
        if items.len() < 2 {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "foreign function signature list must contain return type and names",
            ));
        }
        let names = list_to_vec(self.arena, items[1]).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "malformed foreign function name list in Step 2C",
            )
        })?;
        let name_index = match self.real_ty() {
            FirType::Float32 => 0,
            FirType::Float64 => 1,
            _ => 0,
        };
        let name = names
            .get(name_index)
            .and_then(|id| tree_to_str(self.arena, *id))
            .ok_or_else(|| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    "foreign function name slot missing in Step 2C",
                )
            })?
            .to_owned();
        let ret = self.foreign_sig_type(items[0]);
        let args = items[2..]
            .iter()
            .copied()
            .map(|ty| self.foreign_sig_type(ty))
            .collect();
        Ok(ForeignFunProto { name, ret, args })
    }

    /// Decodes one Faust foreign signature type code (`0=int`, otherwise real).
    pub(super) fn foreign_sig_type(&self, ty: SigId) -> FirType {
        match tree_to_int(self.arena, ty) {
            Some(0) => FirType::Int32,
            Some(_) | None => self.real_ty(),
        }
    }

    /// Lowers one input signal by materializing channel-pointer aliases once
    /// and generating a per-sample table load (`inputN[i0]`).
    pub(super) fn lower_input(&mut self, index: i32) -> Result<FirId, SignalFirError> {
        let index = usize::try_from(index).map_err(|_| {
            SignalFirError::new(
                SignalFirErrorCode::InputIndexOutOfRange,
                "input index conversion overflow",
            )
        })?;
        if index >= self.num_inputs {
            return Err(SignalFirError::new(
                SignalFirErrorCode::InputIndexOutOfRange,
                format!(
                    "input index {index} is out of range for num_inputs={}",
                    self.num_inputs
                ),
            ));
        }

        let alias = if let Some(alias) = self.input_ptr_aliases.get(&index) {
            alias.clone()
        } else {
            let alias = format!("input{index}");
            let mut b = FirBuilder::new(&mut self.store);
            let chan = b.int32(i32::try_from(index).expect("validated input index fits i32"));
            let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
            let load_chan_ptr = b.load_table("inputs", AccessType::FunArgs, chan, ptr_ty.clone());
            self.sections.control_statements.push(b.declare_var(
                alias.clone(),
                ptr_ty,
                AccessType::Stack,
                Some(load_chan_ptr),
            ));
            self.input_ptr_aliases.insert(index, alias.clone());
            alias
        };

        // Load the sample from the external FAUSTFLOAT buffer, then cast to the
        // internal real type so all downstream computation uses real_ty.
        let real_ty = self.real_ty();
        let mut b = FirBuilder::new(&mut self.store);
        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let raw = b.load_table(alias, AccessType::Stack, i0, FirType::FaustFloat);
        Ok(b.cast(real_ty, raw))
    }

    /// Lowers general `SIGDELAY` using a fixed-size circular delay line.
    ///
    /// Source provenance (C++):
    /// - `signalFIRCompiler.cpp::compileSigDelay(...)`
    /// - `signalFIRCompiler.hh::writeReadDelay(...)`
    ///
    /// Active Rust parity slice:
    /// - constant integer amount only,
    /// - zero-delay fast path,
    /// - one typed DSP-struct array per delayed carried signal,
    /// - masked circular indexing driven by persistent `fIOTA`.
    ///
    /// For variable-rate amounts (e.g., UI sliders), the delay line is sized to
    /// the interval upper bound from `sig_types`; the runtime index expression
    /// is the lowered amount signal evaluated each sample.
    pub(super) fn lower_delay(
        &mut self,
        node: SigId,
        value: SigId,
        amount: SigId,
    ) -> Result<FirId, SignalFirError> {
        match delay_size_for_amount(self.arena, self.sig_types, amount)? {
            Some(0) => self.lower_signal(value),
            Some(delay) => self.lower_fixed_delay(node, value, amount, delay),
            None => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "SIGDELAY requires a constant integer amount or a signal with a \
                     bounded non-negative interval (expr={})",
                    dump_sig_readable(self.arena, amount)
                ),
            )),
        }
    }

    /// Lowers a fixed-size `SIGDELAY(value, amount)` using the canonical delay
    /// line pre-allocated by [`Self::prepare_delay_lines`].
    ///
    /// Strategy-specific FIR emission is delegated to `delay/` through
    /// `emit_fixed_delay_for_line`, while this method keeps:
    ///
    /// - recursion-carrier reuse for merged `Delay1^k(Proj(...))` chains
    /// - evaluation of the runtime `amount` expression
    /// - per-carrier write scheduling
    pub(super) fn lower_fixed_delay(
        &mut self,
        node: SigId,
        value: SigId,
        amount: SigId,
        delay: i32,
    ) -> Result<FirId, SignalFirError> {
        // ── Merged recursion delay ──
        //
        // When `value` is a `Delay1^k(Proj(i, active_group))` chain, the scan pass has
        // already sized the recursion array to hold the full delay chain.
        // Read directly from the recursion array at offset `amount + k`,
        // eliminating the separate fVec buffer and per-sample copy.
        if let Some(rec_delay_ref) = self.resolve_recursion_delay_ref(value)? {
            let total_delay =
                usize::try_from(delay).unwrap_or(usize::MAX) + rec_delay_ref.implicit_delay;
            match rec_delay_ref.carrier.strategy {
                RecursionStorageStrategy::Circular => {
                    // The recursion array was upsized — the merge is active.
                    // Use the runtime amount expression (which may be variable,
                    // e.g. slider-driven), not the constant sizing bound.
                    // Total offset = explicit amount + the carried implicit delay chain.
                    let amount_value = self.lower_signal(amount)?;
                    let carried_delay = self.lower_int32_const(
                        i32::try_from(rec_delay_ref.implicit_delay).unwrap_or(i32::MAX),
                    );
                    let total_offset = {
                        let mut b = FirBuilder::new(&mut self.store);
                        b.binop(FirBinOp::Add, amount_value, carried_delay, FirType::Int32)
                    };
                    let read_index = self.global_circular_delayed_index(
                        total_offset,
                        rec_delay_ref.carrier.info.size,
                    );
                    let read_ty = self.signal_fir_type(node)?;
                    let mut b = FirBuilder::new(&mut self.store);
                    return Ok(b.load_table(
                        rec_delay_ref.carrier.info.name,
                        AccessType::Struct,
                        read_index,
                        read_ty,
                    ));
                }
                RecursionStorageStrategy::SingleScalar if total_delay == 1 => {
                    let read_ty = self.signal_fir_type(node)?;
                    let mut b = FirBuilder::new(&mut self.store);
                    return Ok(b.load_var(
                        rec_delay_ref.carrier.info.name,
                        AccessType::Struct,
                        read_ty,
                    ));
                }
                RecursionStorageStrategy::ExactShift => {
                    let read_ty = self.signal_fir_type(node)?;
                    let prev_index =
                        self.lower_int32_const(i32::try_from(total_delay).unwrap_or(i32::MAX));
                    let mut b = FirBuilder::new(&mut self.store);
                    return Ok(b.load_table(
                        rec_delay_ref.carrier.info.name,
                        AccessType::Struct,
                        prev_index,
                        read_ty,
                    ));
                }
                RecursionStorageStrategy::SingleScalar => {}
            }
        }

        let line = self.delay_line_info_for_current_region(value)?;
        let current = self.lower_signal(value)?;
        let read_ty = self.signal_fir_type(node)?;
        let amount_value = self.lower_signal(amount)?;
        let schedule_write = self.delay.schedule_delay_write(value);
        let phases = self.regions.current_phases_mut();
        let mut delay_ctx = DelayLoweringCtx {
            store: &mut self.store,
            immediate_statements: &mut phases.immediate,
            post_output_statements: &mut phases.post_output,
            next_loop_var_id: &mut self.name_gen.next_loop_var_id,
        };
        Ok(emit_fixed_delay_for_line(
            &mut delay_ctx,
            &line,
            current,
            amount_value,
            read_ty,
            schedule_write,
        ))
    }

    /// Lowers one single-sample state edge (`delay1`/`prefix`).
    ///
    /// **Recursion feedback optimization**: if the carried `value` is
    /// `Proj(i, SYMREC/SYMREF)` pointing into the currently active recursion
    /// context (detected by `recursion_feedback_info`), the group's existing
    /// recursion array is reused directly — no separate state variable is
    /// allocated and no extra write is emitted.  The previous-sample value is
    /// read as `rec_array[(fIOTA - 1) & 1]`, which is always valid because the
    /// recursion body writes `rec_array[0]` earlier in the same sample and a
    /// deferred copy updates `rec_array[1]` after outputs are stored.
    ///
    /// For all other `value` signals the normal path applies:
    ///
    /// - Write: `state[fIOTA & 1] = next` (immediate, in sample body)
    /// - Read:  `state[(fIOTA - 1) & 1]`   (returns previous sample)
    pub(super) fn lower_delay_state(
        &mut self,
        node: SigId,
        value: SigId,
        init: FirId,
    ) -> Result<FirId, SignalFirError> {
        if self.rad_reverse.lowering_reverse_loop
            && let Some(replayed) =
                self.lower_forward_output_delay1_for_reverse_loop(node, value)?
        {
            return Ok(replayed);
        }
        if let Some(rec_delay_ref) = self.resolve_recursion_delay_ref(value)? {
            let out_ty = self.signal_fir_type(node)?;
            debug_assert_eq!(
                rec_delay_ref.carrier.info.typ, out_ty,
                "prepared recursion feedback type should match delay1 output type"
            );
            let total_offset = rec_delay_ref.implicit_delay.saturating_add(1);
            match rec_delay_ref.carrier.strategy {
                RecursionStorageStrategy::SingleScalar => {
                    debug_assert_eq!(
                        total_offset, 1,
                        "scalar recursion carriers must not serve delays beyond one sample"
                    );
                    let mut b = FirBuilder::new(&mut self.store);
                    return Ok(b.load_var(
                        rec_delay_ref.carrier.info.name,
                        AccessType::Struct,
                        rec_delay_ref.carrier.info.typ.clone(),
                    ));
                }
                RecursionStorageStrategy::ExactShift => {
                    let prev_index =
                        self.lower_int32_const(i32::try_from(total_offset).unwrap_or(i32::MAX));
                    let mut b = FirBuilder::new(&mut self.store);
                    return Ok(b.load_table(
                        rec_delay_ref.carrier.info.name,
                        AccessType::Struct,
                        prev_index,
                        rec_delay_ref.carrier.info.typ.clone(),
                    ));
                }
                RecursionStorageStrategy::Circular => {
                    let total_offset =
                        self.lower_int32_const(i32::try_from(total_offset).unwrap_or(i32::MAX));
                    let prev_index = self.global_circular_delayed_index(
                        total_offset,
                        rec_delay_ref.carrier.info.size,
                    );
                    let mut b = FirBuilder::new(&mut self.store);
                    return Ok(b.load_table(
                        rec_delay_ref.carrier.info.name,
                        AccessType::Struct,
                        prev_index,
                        rec_delay_ref.carrier.info.typ.clone(),
                    ));
                }
            }
        }
        let state_ty = self.signal_fir_type(value)?;
        let name = self.ensure_state_slot(node, state_ty.clone(), init);
        // Read previous value: state[(fIOTA - 1) & 1]
        let one = self.lower_int32_const(1);
        let read_index = self.global_circular_delayed_index(one, 2);
        let out = {
            let mut b = FirBuilder::new(&mut self.store);
            b.load_table(name.clone(), AccessType::Struct, read_index, state_ty)
        };
        // Write current value: state[fIOTA & 1] = next (immediate)
        if self.scheduled_state_updates.insert(node) {
            let next = self.lower_signal(value)?;
            let write_index = self.global_circular_current_index(2);
            let mut b = FirBuilder::new(&mut self.store);
            self.regions
                .current_phases_mut()
                .immediate
                .push(b.store_table(name, AccessType::Struct, write_index, next));
        }
        Ok(out)
    }

    /// Replays `Delay1(primal_output)` while lowering a reverse-time RAD loop.
    ///
    /// In split RAD bundles, forward primals are emitted before reverse
    /// gradients. A feedback-coefficient contribution such as
    /// `adjoint[n] * y[n-1]` must read the primal state at the matching forward
    /// frame, not advance a recursion carrier while iterating backward. For
    /// primals present in the public output bundle, the forward output buffer is
    /// the block-local tape: frame `0` returns the delay initializer `0`, and
    /// later frames read `output_primal[i0 - 1]`.
    pub(super) fn lower_forward_output_delay1_for_reverse_loop(
        &mut self,
        node: SigId,
        value: SigId,
    ) -> Result<Option<FirId>, SignalFirError> {
        let output_index = self
            .rad_reverse
            .forward_output_by_sig
            .get(&value)
            .copied()
            .or_else(|| {
                self.rad_reverse
                    .forward_output_by_sig_key
                    .get(&dump_sig_readable(self.arena, value))
                    .copied()
            });
        let Some(output_index) = output_index else {
            return Ok(None);
        };
        let out_ty = self.signal_fir_type(node)?;
        if !matches!(out_ty, FirType::Int32 | FirType::Float32 | FirType::Float64) {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "unsupported reverse RAD primal replay type for {}: {out_ty:?}",
                    dump_sig_readable(self.arena, node)
                ),
            ));
        }
        let mut b = FirBuilder::new(&mut self.store);
        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let zero_index = b.int32(0);
        let has_previous = b.binop(FirBinOp::Gt, i0, zero_index, FirType::Int32);
        let one = b.int32(1);
        let raw_previous_index = b.binop(FirBinOp::Sub, i0, one, FirType::Int32);
        let previous_index = b.binop(
            FirBinOp::Mul,
            has_previous,
            raw_previous_index,
            FirType::Int32,
        );
        let previous = b.load_table(
            format!("output{output_index}"),
            AccessType::Stack,
            previous_index,
            FirType::FaustFloat,
        );
        let previous = b.cast(out_ty.clone(), previous);
        let mask = b.cast(out_ty.clone(), has_previous);
        let masked_previous = b.binop(FirBinOp::Mul, previous, mask, out_ty);
        Ok(Some(masked_previous))
    }

    /// Lowers a standalone `Delay1(value)` node using the canonical
    /// preplanned strategy for its carried signal.
    ///
    /// When the carried signal owns a `Shift` delay line, this matches the
    /// reference C++ Faust pattern:
    /// ```text
    /// buf[0] = value;       // immediate write
    /// output = buf[1];      // read previous sample
    /// buf[1] = buf[0];      // deferred shift (after output stores)
    /// ```
    ///
    /// The same `Delay1(value)` may also reuse a preplanned `CircularPow2` or
    /// `IfWrapping` line when the carried signal shares storage with a larger
    /// `SIGDELAY(value, N)`. In all cases the concrete write/read sequence is
    /// delegated to `emit_delay1_for_line`.
    ///
    /// Only called when `max_copy_delay >= 1` and `value` is not a recursion
    /// feedback projection.
    pub(super) fn lower_shift_delay1(
        &mut self,
        node: SigId,
        value: SigId,
    ) -> Result<FirId, SignalFirError> {
        let line = self.delay_line_info_for_current_region(value)?;
        let read_ty = self.signal_fir_type(node)?;
        let current = self.lower_signal(value)?;
        let schedule_write = self.delay.schedule_delay_write(value);
        let phases = self.regions.current_phases_mut();
        let mut delay_ctx = DelayLoweringCtx {
            store: &mut self.store,
            immediate_statements: &mut phases.immediate,
            post_output_statements: &mut phases.post_output,
            next_loop_var_id: &mut self.name_gen.next_loop_var_id,
        };
        Ok(emit_delay1_for_line(
            &mut delay_ctx,
            &line,
            current,
            read_ty,
            schedule_write,
        ))
    }
}
