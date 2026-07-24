//! Signal lowering (producer): the pure-vector lowerer and the program
//! entry points. The terminal step calls the boundary/body checks in
//! `check.rs`, so every admission guard there also binds the producer
//! (plan §4.8).

use super::check::collect_prepared_ids;
use super::check::{verify_plan_prepared_boundary, verify_pure_vector_bodies};
use super::program::*;
use super::tables::mutable_table_name;
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::ControlRateMode;
use crate::signal_fir::module::map_binop;
use crate::signal_fir::vector::analysis::wrtbl_is_readonly;
use crate::signal_fir::vector::clock_ad::{ClockGuard, VerifiedVectorClockAdPlan};
use crate::signal_fir::vector::cse::{
    materialize_shared_values, materialize_shared_values_promoted,
};
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::recursion::{decode_group_projection, decode_symbolic_group_bodies};
use crate::signal_fir::vector::route::{
    RouteResolution, VectorRegion, VectorRouteSession, value_fir_type,
};
use crate::signal_fir::vector::siggen::interpret_generator;
use crate::signal_fir::vector::state::{VectorDelayStorage, VerifiedVectorStatePlan};
use crate::signal_fir::vector::verify::{Placement, SignalRecord, ValueType};
use crate::signal_prepare::VerifiedPreparedSignals;
use fir::{
    AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirMathOp, FirStore, FirType, match_fir,
};
use signals::{BinOp, SigId, SigMatch, dump_sig_readable, match_sig};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use tlib::{match_sym_ref, tree_to_int, tree_to_str};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LowerScope {
    Control,
    Loop(u64),
}
impl LowerScope {
    fn region(self) -> VectorRegion {
        match self {
            Self::Control => VectorRegion::Control,
            Self::Loop(loop_id) => VectorRegion::Loop(loop_id),
        }
    }
}
struct PureVectorLowerer<'a> {
    prepared: &'a VerifiedPreparedSignals,
    ui: &'a ui::UiProgram,
    session: VectorRouteSession<'a>,
    store: FirStore,
    real_type: FirType,
    num_inputs: usize,
    signal_ids: BTreeMap<u64, SigId>,
    input_declarations: Vec<FirId>,
    input_aliases: BTreeSet<usize>,
    static_declarations: Vec<FirId>,
    waveform_tables: BTreeMap<u64, String>,
    readonly_tables: BTreeMap<u64, (String, usize, FirType)>,
    mutable_tables: BTreeMap<u64, (String, usize, FirType)>,
    table_declarations: Vec<FirId>,
    table_init_statements: Vec<FirId>,
    table_stores: BTreeMap<u64, Vec<FirId>>,
    math_ops: HashSet<FirMathOp>,
    int_helpers: BTreeSet<&'static str>,
    state_plan: Option<&'a VerifiedVectorStatePlan>,
    ui_stores: BTreeMap<LowerScope, Vec<FirId>>,
    upsampling_domains: BTreeMap<u64, u64>,
    /// Control-rate evaluation scheduling (`-ec`), plan phase 5.
    control_rate_mode: ControlRateMode,
    /// Per-zone UI snapshot names under `-ec` (zone name → snapshot field).
    ui_snapshots: BTreeMap<String, String>,
    /// Snapshot stores emitted into `control` under `-ec`.
    snapshot_stores: Vec<FirId>,
    /// DSP struct fields created by `-ec` promotion.
    promoted_control_fields: Vec<(String, FirType)>,
    /// Counter for `fSlow` snapshot names.
    snapshot_counter: u32,
}
/// Lowers actual effect-free prepared signals into P4.4 vector regions.
///
/// CSE is run once per control/loop region with loop-id-derived temporary names.
/// No stateful or effectful node is accepted, and this artifact is not yet
/// connected to backend module assembly.
pub fn lower_pure_vector_program(
    prepared: &VerifiedPreparedSignals,
    verified_plan: &VerifiedVectorPlan,
    strategy: SchedulingStrategy,
    real_type: FirType,
    num_inputs: usize,
) -> Result<VerifiedPureVectorProgram, PureVectorLowerError> {
    let ui = ui::UiProgram::empty();
    let context = VectorLoweringContext {
        ui: &ui,
        strategy,
        real_type,
        num_inputs,
        control_rate_mode: ControlRateMode::InlinePerBlock,
    };
    lower_vector_program_impl(prepared, verified_plan, None, None, &context)
}
/// Lowers the P6-supported vector subset using authoritative state and clock
/// artifacts. Forward AD needs no special carrier after propagation and enters
/// through the ordinary pointwise cases below.
pub fn lower_vector_program(
    prepared: &VerifiedPreparedSignals,
    verified_plan: &VerifiedVectorPlan,
    state_plan: &VerifiedVectorStatePlan,
    clock_plan: &VerifiedVectorClockAdPlan,
    context: &VectorLoweringContext<'_>,
) -> Result<VerifiedPureVectorProgram, PureVectorLowerError> {
    if state_plan.vector_plan() != verified_plan.plan()
        || clock_plan.vector_plan() != verified_plan.plan()
    {
        return Err(PureVectorLowerError::BodyEvidence {
            detail: "P6 artifacts do not belong to the selected vector plan".to_owned(),
        });
    }
    lower_vector_program_impl(
        prepared,
        verified_plan,
        Some(state_plan),
        Some(clock_plan),
        context,
    )
}
pub(super) fn lower_vector_program_impl<'a>(
    prepared: &'a VerifiedPreparedSignals,
    verified_plan: &'a VerifiedVectorPlan,
    state_plan: Option<&'a VerifiedVectorStatePlan>,
    clock_plan: Option<&'a VerifiedVectorClockAdPlan>,
    context: &VectorLoweringContext<'a>,
) -> Result<VerifiedPureVectorProgram, PureVectorLowerError> {
    let timing_enabled = std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some();
    let mut stage_started = std::time::Instant::now();
    let mut trace_stage = |stage: &str| {
        if timing_enabled {
            eprintln!(
                "[vector-lower-stage] {stage}: {:.3}s",
                stage_started.elapsed().as_secs_f64()
            );
        }
        stage_started = std::time::Instant::now();
    };
    if !matches!(context.real_type, FirType::Float32 | FirType::Float64) {
        return Err(PureVectorLowerError::InvalidRealType(
            context.real_type.clone(),
        ));
    }
    let signal_ids = collect_prepared_ids(prepared);
    verify_plan_prepared_boundary(
        prepared,
        context.ui,
        verified_plan.plan(),
        &signal_ids,
        state_plan,
        clock_plan,
    )?;
    trace_stage("prepared-boundary");
    let mut store = FirStore::new();
    let (session, transport_declarations) = if let Some(clock_plan) = clock_plan {
        VectorRouteSession::new_with_clock_plan(
            verified_plan,
            clock_plan,
            context.strategy,
            context.real_type.clone(),
            &mut store,
        )?
    } else {
        VectorRouteSession::new(
            verified_plan,
            context.strategy,
            context.real_type.clone(),
            &mut store,
        )?
    };
    trace_stage("route-session");
    // Signals whose inferred clock environment is a counted upsampling
    // domain; ZeroPad gating needs that domain's fire index.
    let upsampling_domains = clock_plan
        .map(|clock_plan| {
            clock_plan
                .plan()
                .clock_islands
                .iter()
                .filter(|island| island.guard == ClockGuard::CountedUpsampling)
                .flat_map(|island| {
                    island
                        .signal_ids
                        .iter()
                        .map(|&signal_id| (signal_id, island.domain_id))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut lowerer = PureVectorLowerer {
        prepared,
        ui: context.ui,
        session,
        store,
        real_type: context.real_type.clone(),
        num_inputs: context.num_inputs,
        signal_ids,
        input_declarations: Vec::new(),
        input_aliases: BTreeSet::new(),
        static_declarations: Vec::new(),
        waveform_tables: BTreeMap::new(),
        readonly_tables: BTreeMap::new(),
        mutable_tables: BTreeMap::new(),
        table_declarations: Vec::new(),
        table_init_statements: Vec::new(),
        table_stores: BTreeMap::new(),
        math_ops: HashSet::new(),
        int_helpers: BTreeSet::new(),
        state_plan,
        ui_stores: BTreeMap::new(),
        upsampling_domains,
        control_rate_mode: context.control_rate_mode,
        ui_snapshots: BTreeMap::new(),
        snapshot_stores: Vec::new(),
        promoted_control_fields: Vec::new(),
        snapshot_counter: 0,
    };

    let mut control_cache = BTreeMap::new();
    let mut active = BTreeSet::new();
    let control_ids = lowerer
        .session
        .plan()
        .signals
        .iter()
        .filter_map(|record| (record.placement == Placement::Control).then_some(record.signal_id))
        .collect::<Vec<_>>();
    let mut control_values = Vec::with_capacity(control_ids.len());
    for &signal_id in &control_ids {
        let sig = lowerer.sig(signal_id)?;
        match lowerer.lower_control(sig, &mut control_cache, &mut active) {
            Ok(value) => control_values.push(value),
            Err(error) => {
                trace_stage("control-lowering-failed");
                return Err(error);
            }
        }
    }
    trace_stage("control-lowering");
    // Phase 5: under external control the shared control-root temporaries are
    // promoted to DSP struct fields (their stores move to `control`, while
    // vector transport fills in `compute` read them back), mirroring the
    // scalar konst-escape promotion (§4.4). Classic mode keeps stack locals.
    let external_control = lowerer.control_rate_mode.is_external();
    let (mut control_statements, rewritten_control_values) = if external_control {
        let (statements, rewritten, fields) = materialize_region_roots_promoted(
            &mut lowerer.store,
            &control_values,
            VectorRegion::Control,
        )?;
        lowerer.promoted_control_fields.extend(fields);
        (statements, rewritten)
    } else {
        materialize_region_roots(&mut lowerer.store, &control_values, VectorRegion::Control)?
    };
    control_statements.extend(
        lowerer
            .ui_stores
            .remove(&LowerScope::Control)
            .unwrap_or_default(),
    );
    for (&signal_id, &value) in control_ids.iter().zip(&rewritten_control_values) {
        lowerer
            .session
            .define_control(signal_id, value, &lowerer.store)?;
    }

    let layout = lowerer.session.layout().loops().to_vec();
    let mut regions = Vec::with_capacity(layout.len());
    for region in layout {
        let mut local_cache = BTreeMap::new();
        let mut active = BTreeSet::new();
        let mut materialized_roots = Vec::with_capacity(region.roots.len());
        for &root in &region.roots {
            let sig = lowerer.sig(root)?;
            let structural_tuple = lowerer
                .session
                .plan()
                .signals
                .iter()
                .find(|signal| signal.signal_id == root)
                .is_some_and(|signal| signal.structural);
            // Symbolic recursion groups and references are structural tuple
            // carriers. Their selected bodies are scheduled as independent
            // executable roots, so evaluating the carrier here would duplicate
            // those bodies in the carrier's loop and invent cross-loop uses.
            if structural_tuple {
                continue;
            }
            let value =
                lowerer.lower_in_loop(region.loop_id, sig, &mut local_cache, &mut active)?;
            materialized_roots.push((root, value));
        }
        let root_values = materialized_roots
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        let (mut statements, rewritten_roots) = materialize_region_roots_with_prefix(
            &mut lowerer.store,
            &root_values,
            VectorRegion::Loop(region.loop_id),
            lowerer
                .table_stores
                .remove(&region.loop_id)
                .unwrap_or_default(),
            &format!("fVecL{}Temp", region.loop_id),
            &format!("iVecL{}Temp", region.loop_id),
        )?;
        for ((root, _), &value) in materialized_roots.iter().zip(&rewritten_roots) {
            local_cache.insert(*root, value);
        }

        let mut stores = Vec::new();
        for (&signal_id, &value) in &local_cache {
            stores.extend(lowerer.session.define_in_loop(
                region.loop_id,
                signal_id,
                value,
                &mut lowerer.store,
            )?);
        }
        let transported_values = stores
            .iter()
            .filter_map(|statement| match match_fir(&lowerer.store, *statement) {
                FirMatch::StoreTable { value, .. } => Some(value),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        statements.retain(|statement| {
            !matches!(
                match_fir(&lowerer.store, *statement),
                FirMatch::Drop(value) if transported_values.contains(&value)
            )
        });
        statements.extend(
            lowerer
                .ui_stores
                .remove(&LowerScope::Loop(region.loop_id))
                .unwrap_or_default(),
        );
        statements.extend(stores);
        regions.push(PureVectorRegionBody {
            loop_id: region.loop_id,
            statements,
        });
    }
    trace_stage("loop-lowering");

    // Phase 5 split: under external control the externalizable statements
    // (UI snapshots first, then promoted control roots and control-scope UI
    // stores) form the `control` body; only the input channel aliases stay
    // in the compute preamble. Classic mode keeps the single interleaved
    // list with the aliases spliced in front, byte-identical to before.
    let (control_statements, external_control_statements) = if external_control {
        let mut external = lowerer.snapshot_stores.clone();
        external.extend(control_statements.iter().copied());
        (lowerer.input_declarations.clone(), external)
    } else {
        control_statements.splice(0..0, lowerer.input_declarations.iter().copied());
        (control_statements, Vec::new())
    };
    let routed = lowerer.session.finish(&lowerer.store)?;
    if timing_enabled {
        for transport in &verified_plan.plan().transports {
            eprintln!(
                "[vector-lower-transport] id={} signal={} producer={} consumer={}",
                transport.transport_id,
                transport.signal_id,
                transport.producer_loop,
                transport.consumer_loop
            );
        }
    }
    let verified_control_section: Vec<FirId> = external_control_statements
        .iter()
        .chain(control_statements.iter())
        .copied()
        .collect();
    verify_pure_vector_bodies(
        verified_plan.plan(),
        &routed,
        &transport_declarations,
        &verified_control_section,
        &regions,
        state_plan,
        &lowerer.store,
    )?;
    trace_stage("route-and-body-verification");
    Ok(VerifiedPureVectorProgram {
        store: lowerer.store,
        static_declarations: lowerer.static_declarations,
        table_declarations: lowerer.table_declarations,
        table_init_statements: lowerer.table_init_statements,
        mutable_tables: lowerer.mutable_tables,
        transport_declarations,
        control_statements,
        external_control_statements,
        control_state_fields: lowerer.promoted_control_fields,
        regions,
        routed,
        math_ops: lowerer.math_ops,
        int_helpers: lowerer.int_helpers,
    })
}
impl PureVectorLowerer<'_> {
    fn sig(&self, signal_id: u64) -> Result<SigId, PureVectorLowerError> {
        self.signal_ids
            .get(&signal_id)
            .copied()
            .ok_or(PureVectorLowerError::MissingPreparedSignal { signal_id })
    }

    fn record(&self, signal_id: u64) -> Result<SignalRecord, PureVectorLowerError> {
        self.session
            .plan()
            .signals
            .iter()
            .find(|record| record.signal_id == signal_id)
            .cloned()
            .ok_or(PureVectorLowerError::MissingPreparedSignal { signal_id })
    }

    fn lower_control(
        &mut self,
        sig: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let signal_id = u64::from(sig.as_u32());
        if let Some(value) = cache.get(&signal_id).copied() {
            return Ok(value);
        }
        let record = self.record(signal_id)?;
        if record.placement != Placement::Control {
            return Err(PureVectorLowerError::InvalidControlDependency { signal_id });
        }
        let scope = LowerScope::Control;
        if !active.insert((scope, signal_id)) {
            return Err(PureVectorLowerError::PureCycle {
                signal_id,
                region: scope.region(),
            });
        }
        let value = self.lower_raw(scope, sig, cache, active)?;
        active.remove(&(scope, signal_id));
        self.check_type(signal_id, value)?;
        cache.insert(signal_id, value);
        Ok(value)
    }

    fn lower_in_loop(
        &mut self,
        loop_id: u64,
        sig: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let signal_id = u64::from(sig.as_u32());
        let record = self.record(signal_id)?;
        match record.placement {
            Placement::Control => {
                match self
                    .session
                    .resolve_in_loop(loop_id, signal_id, &mut self.store)?
                {
                    RouteResolution::Value(value) => return Ok(value),
                    RouteResolution::NeedsInlineLowering => unreachable!("control is never inline"),
                }
            }
            Placement::Owned(owner) if owner != loop_id => {
                return match self
                    .session
                    .resolve_in_loop(loop_id, signal_id, &mut self.store)?
                {
                    RouteResolution::Value(value) => Ok(value),
                    RouteResolution::NeedsInlineLowering => {
                        unreachable!("owned value is never inline")
                    }
                };
            }
            Placement::Inline | Placement::Owned(_) => {}
        }
        if let Some(value) = cache.get(&signal_id).copied() {
            return Ok(value);
        }
        let scope = LowerScope::Loop(loop_id);
        if !active.insert((scope, signal_id)) {
            return Err(PureVectorLowerError::PureCycle {
                signal_id,
                region: scope.region(),
            });
        }
        let value = self.lower_raw(scope, sig, cache, active)?;
        active.remove(&(scope, signal_id));
        self.check_type(signal_id, value)?;
        cache.insert(signal_id, value);
        Ok(value)
    }

    fn lower_dep(
        &mut self,
        scope: LowerScope,
        sig: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        match scope {
            LowerScope::Control => self.lower_control(sig, cache, active),
            LowerScope::Loop(loop_id) => self.lower_in_loop(loop_id, sig, cache, active),
        }
    }

    fn lower_raw(
        &mut self,
        scope: LowerScope,
        sig: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let signal_id = u64::from(sig.as_u32());
        if let Some((_var, bodies)) = decode_symbolic_group_bodies(self.prepared.arena(), sig) {
            let mut values = Vec::with_capacity(bodies.len());
            for body in bodies {
                values.push(self.lower_dep(scope, body, cache, active)?);
            }
            let typ = self.fir_type(signal_id)?;
            return Ok(FirBuilder::new(&mut self.store).value_array(&values, typ));
        }
        if let Some(var) = match_sym_ref(self.prepared.arena(), sig) {
            return self.lower_symbolic_ref(scope, signal_id, var, cache, active);
        }
        let value = match match_sig(self.prepared.arena(), sig) {
            SigMatch::Int(value) => FirBuilder::new(&mut self.store).int32(value),
            SigMatch::Real(value) => self.float_const(value),
            SigMatch::FConst(_, name, _) => self.lower_fconst(signal_id, name)?,
            SigMatch::FVar(kind, name, _) => self.lower_fvar(signal_id, kind, name)?,
            SigMatch::Input(index) => self.lower_input(index)?,
            SigMatch::Button(control) => self.lower_ui_input(control, ui::ControlKind::Button)?,
            SigMatch::Checkbox(control) => {
                self.lower_ui_input(control, ui::ControlKind::Checkbox)?
            }
            SigMatch::VSlider(control) => self.lower_ui_input(control, ui::ControlKind::VSlider)?,
            SigMatch::HSlider(control) => self.lower_ui_input(control, ui::ControlKind::HSlider)?,
            SigMatch::NumEntry(control) => {
                self.lower_ui_input(control, ui::ControlKind::NumEntry)?
            }
            SigMatch::Soundfile(control) => self.lower_soundfile_handle(control)?,
            SigMatch::SoundfileLength(sf, part) => {
                let var = self.soundfile_zone_name(sf)?;
                let _ = self.lower_dep(scope, sf, cache, active)?;
                let part = self.lower_dep(scope, part, cache, active)?;
                FirBuilder::new(&mut self.store).load_soundfile_length(var, part)
            }
            SigMatch::SoundfileRate(sf, part) => {
                let var = self.soundfile_zone_name(sf)?;
                let _ = self.lower_dep(scope, sf, cache, active)?;
                let part = self.lower_dep(scope, part, cache, active)?;
                FirBuilder::new(&mut self.store).load_soundfile_rate(var, part)
            }
            SigMatch::SoundfileBuffer(sf, chan, part, ridx) => {
                let var = self.soundfile_zone_name(sf)?;
                let _ = self.lower_dep(scope, sf, cache, active)?;
                let chan = self.lower_dep(scope, chan, cache, active)?;
                let part = self.lower_dep(scope, part, cache, active)?;
                let idx = self.lower_dep(scope, ridx, cache, active)?;
                let typ = self.fir_type(signal_id)?;
                FirBuilder::new(&mut self.store).load_soundfile_buffer(var, chan, part, idx, typ)
            }
            SigMatch::VBargraph(control, inner) => self.lower_bargraph(
                scope,
                control,
                ui::ControlKind::VBargraph,
                inner,
                cache,
                active,
            )?,
            SigMatch::HBargraph(control, inner) => self.lower_bargraph(
                scope,
                control,
                ui::ControlKind::HBargraph,
                inner,
                cache,
                active,
            )?,
            SigMatch::Output(_, inner) => self.lower_dep(scope, inner, cache, active)?,
            SigMatch::Delay1(value) => self.lower_delay_read(scope, value, 1, cache, active)?,
            SigMatch::Delay(value, amount) => match match_sig(self.prepared.arena(), amount) {
                SigMatch::Int(amount_literal) if amount_literal >= 0 => {
                    if amount_literal == 0 {
                        self.lower_dep(scope, value, cache, active)?
                    } else {
                        self.lower_delay_read(
                            scope,
                            value,
                            u64::try_from(amount_literal).expect("non-negative i32 fits u64"),
                            cache,
                            active,
                        )?
                    }
                }
                _ => {
                    let max_delay =
                        sigtype::check_delay_interval(self.prepared.sig_ty(amount).ok_or_else(
                            || PureVectorLowerError::UnsupportedSignal {
                                signal_id,
                                expression: "variable delay amount has no prepared type".to_owned(),
                            },
                        )?)
                        .map_err(|error| {
                            PureVectorLowerError::UnsupportedSignal {
                                signal_id,
                                expression: format!("invalid variable delay interval: {error}"),
                            }
                        })?;
                    if max_delay == 0 {
                        self.lower_dep(scope, value, cache, active)?
                    } else {
                        let amount_value = self.lower_dep(scope, amount, cache, active)?;
                        self.lower_delay_read_value(
                            value,
                            amount_value,
                            u64::try_from(max_delay).map_err(|_| {
                                PureVectorLowerError::UnsupportedSignal {
                                    signal_id,
                                    expression: "variable delay bound is negative".to_owned(),
                                }
                            })?,
                        )?
                    }
                }
            },
            SigMatch::Prefix(_, value) => {
                self.lower_prefix(scope, signal_id, value, cache, active)?
            }
            SigMatch::Waveform(values) => self.lower_waveform(scope, signal_id, values)?,
            SigMatch::Gen(_) => self.lower_table_generator(signal_id)?,
            SigMatch::RdTbl(table, index) => {
                self.lower_readonly_table(scope, signal_id, table, index, cache, active)?
            }
            SigMatch::WrTbl(size, generator, write_index, write_value) => self
                .lower_table_definition(
                    scope,
                    signal_id,
                    size,
                    generator,
                    write_index,
                    write_value,
                    cache,
                    active,
                )?,
            SigMatch::Proj(index, group) => {
                if let Some(var) = match_sym_ref(self.prepared.arena(), group) {
                    let bodies = self.symbolic_bodies_for_var(signal_id, var)?;
                    let index = usize::try_from(index).map_err(|_| {
                        PureVectorLowerError::UnsupportedSignal {
                            signal_id,
                            expression: "negative symbolic recursion projection".to_owned(),
                        }
                    })?;
                    let canonical = if bodies.len() == 1 { 0 } else { index };
                    let body = bodies.get(canonical).copied().ok_or_else(|| {
                        PureVectorLowerError::UnsupportedSignal {
                            signal_id,
                            expression: "symbolic recursion projection is out of bounds".to_owned(),
                        }
                    })?;
                    // C++ `getSignalDependencies` gives a symbolic back-edge
                    // previous-sample semantics even when the selected body
                    // has no explicit `sigDelay` occurrence. Use that implicit
                    // history only when the accepted P6.1 plan proves this is
                    // the X2b cross-loop alias shape; ordinary same-loop
                    // recursion keeps its established lowering.
                    let body_id = u64::from(body.as_u32());
                    let cross_loop = matches!(
                        (
                            self.record(signal_id)?.placement,
                            self.record(body_id)?.placement,
                        ),
                        (Placement::Owned(from), Placement::Owned(to)) if from != to
                    );
                    let has_implicit_history = cross_loop
                        && self.state_plan.is_some_and(|plan| {
                            plan.plan()
                                .delays
                                .iter()
                                .any(|transition| transition.signal_id == body_id)
                        });
                    return if has_implicit_history {
                        self.lower_delay_read(scope, body, 1, cache, active)
                    } else {
                        self.lower_dep(scope, body, cache, active)
                    };
                }
                let projection = decode_group_projection(self.prepared.arena(), sig, index, group)
                    .map_err(|error| PureVectorLowerError::UnsupportedSignal {
                        signal_id,
                        expression: error.to_string(),
                    })?;
                self.lower_dep(
                    scope,
                    projection.bodies[projection.canonical_index],
                    cache,
                    active,
                )?
            }
            SigMatch::IntCast(value) => {
                let value = self.lower_dep(scope, value, cache, active)?;
                FirBuilder::new(&mut self.store).cast(FirType::Int32, value)
            }
            SigMatch::FloatCast(value) => {
                let value = self.lower_dep(scope, value, cache, active)?;
                FirBuilder::new(&mut self.store).cast(self.real_type.clone(), value)
            }
            SigMatch::BitCast(value) => {
                let value = self.lower_dep(scope, value, cache, active)?;
                FirBuilder::new(&mut self.store).bitcast(self.real_type.clone(), value)
            }
            SigMatch::Select2(cond, else_value, then_value) => {
                let cond = self.lower_dep(scope, cond, cache, active)?;
                let then_value = self.lower_dep(scope, then_value, cache, active)?;
                let else_value = self.lower_dep(scope, else_value, cache, active)?;
                let typ = self.fir_type(signal_id)?;
                FirBuilder::new(&mut self.store).select2(cond, then_value, else_value, typ)
            }
            SigMatch::BinOp(op, lhs, rhs) => {
                self.lower_binop(scope, signal_id, op, (lhs, rhs), cache, active)?
            }
            SigMatch::Pow(lhs, rhs) => {
                self.lower_math2(scope, FirMathOp::Pow, lhs, rhs, cache, active)?
            }
            SigMatch::Min(lhs, rhs) => {
                self.lower_minmax(scope, signal_id, (lhs, rhs), true, cache, active)?
            }
            SigMatch::Max(lhs, rhs) => {
                self.lower_minmax(scope, signal_id, (lhs, rhs), false, cache, active)?
            }
            SigMatch::Sin(value) => {
                self.lower_math1(scope, FirMathOp::Sin, value, cache, active)?
            }
            SigMatch::Cos(value) => {
                self.lower_math1(scope, FirMathOp::Cos, value, cache, active)?
            }
            SigMatch::Acos(value) => {
                self.lower_math1(scope, FirMathOp::Acos, value, cache, active)?
            }
            SigMatch::Asin(value) => {
                self.lower_math1(scope, FirMathOp::Asin, value, cache, active)?
            }
            SigMatch::Atan(value) => {
                self.lower_math1(scope, FirMathOp::Atan, value, cache, active)?
            }
            SigMatch::Atan2(lhs, rhs) => {
                self.lower_math2(scope, FirMathOp::Atan2, lhs, rhs, cache, active)?
            }
            SigMatch::Tan(value) => {
                self.lower_math1(scope, FirMathOp::Tan, value, cache, active)?
            }
            SigMatch::Exp(value) => {
                self.lower_math1(scope, FirMathOp::Exp, value, cache, active)?
            }
            SigMatch::Exp10(value) => {
                self.lower_math1(scope, FirMathOp::Exp10, value, cache, active)?
            }
            SigMatch::Log(value) => {
                self.lower_math1(scope, FirMathOp::Log, value, cache, active)?
            }
            SigMatch::Log10(value) => {
                self.lower_math1(scope, FirMathOp::Log10, value, cache, active)?
            }
            SigMatch::Sqrt(value) => {
                self.lower_math1(scope, FirMathOp::Sqrt, value, cache, active)?
            }
            SigMatch::Abs(value) => self.lower_abs(scope, signal_id, value, cache, active)?,
            SigMatch::Fmod(lhs, rhs) => {
                self.lower_math2(scope, FirMathOp::Fmod, lhs, rhs, cache, active)?
            }
            SigMatch::Remainder(lhs, rhs) => {
                self.lower_math2(scope, FirMathOp::Remainder, lhs, rhs, cache, active)?
            }
            SigMatch::Floor(value) => {
                self.lower_math1(scope, FirMathOp::Floor, value, cache, active)?
            }
            SigMatch::Ceil(value) => {
                self.lower_math1(scope, FirMathOp::Ceil, value, cache, active)?
            }
            SigMatch::Rint(value) => {
                self.lower_math1(scope, FirMathOp::Rint, value, cache, active)?
            }
            SigMatch::Round(value) => {
                self.lower_math1(scope, FirMathOp::Round, value, cache, active)?
            }
            SigMatch::Lowest(value) | SigMatch::Highest(value) => {
                self.lower_dep(scope, value, cache, active)?
            }
            SigMatch::Attach(value, attached) => {
                // `attach` only forces the attached computation; its value is
                // never part of this expression. Lowering it here would
                // register a routing use - and demand a transport - for a
                // value the emitted body then discards. The attached branch
                // executes through its own placement: effectful branches are
                // rooted in their own loops by the plan's component sweep, and
                // a pure attach-only branch is semantically dead.
                let _ = attached;
                self.lower_dep(scope, value, cache, active)?
            }
            SigMatch::Enable(value, gate) => {
                let value = self.lower_dep(scope, value, cache, active)?;
                let gate = self.lower_dep(scope, gate, cache, active)?;
                let typ = self.fir_type(signal_id)?;
                let zero = self.zero_value(&typ)?;
                FirBuilder::new(&mut self.store).select2(gate, value, zero, typ)
            }
            SigMatch::Control(value, gate) => {
                let _ = self.lower_dep(scope, gate, cache, active)?;
                self.lower_dep(scope, value, cache, active)?
            }
            SigMatch::Seq(block, held) => {
                let _ = self.lower_dep(scope, block, cache, active)?;
                self.lower_dep(scope, held, cache, active)?
            }
            SigMatch::Clocked(_, inner) | SigMatch::TempVar(inner) | SigMatch::PermVar(inner) => {
                self.lower_dep(scope, inner, cache, active)?
            }
            SigMatch::ZeroPad(value, amount) => {
                self.lower_zero_pad(scope, signal_id, value, amount, cache, active)?
            }
            SigMatch::OnDemand(children)
            | SigMatch::Upsampling(children)
            | SigMatch::Downsampling(children) => {
                let Some((&last, prefix)) = children.split_last() else {
                    return Err(PureVectorLowerError::UnsupportedSignal {
                        signal_id,
                        expression: "empty clock wrapper".to_owned(),
                    });
                };
                for &child in prefix {
                    let _ = self.lower_dep(scope, child, cache, active)?;
                }
                self.lower_dep(scope, last, cache, active)?
            }
            SigMatch::ClockEnvToken(domain) => {
                FirBuilder::new(&mut self.store).int32(i32::try_from(domain).map_err(|_| {
                    PureVectorLowerError::UnsupportedSignal {
                        signal_id,
                        expression: "clock domain identity exceeds FIR i32".to_owned(),
                    }
                })?)
            }
            _ => {
                return Err(PureVectorLowerError::UnsupportedSignal {
                    signal_id,
                    expression: dump_sig_readable(self.prepared.arena(), sig),
                });
            }
        };
        Ok(value)
    }

    /// Loads the `Sound` struct handle for one soundfile control.
    ///
    /// Soundfile data is immutable at compute time, so like the sliders this
    /// is a pure zone read; the data accessors below address the handle by
    /// its zone name exactly as the scalar template does.
    fn lower_soundfile_handle(
        &mut self,
        control: ui::ControlId,
    ) -> Result<FirId, PureVectorLowerError> {
        let zone = crate::signal_fir::vector::ui::control_zone(self.ui, control).map_err(
            |expression| PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression,
            },
        )?;
        if zone.kind != ui::ControlKind::Soundfile {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression: format!(
                    "soundfile control {control} kind mismatch: got {:?}",
                    zone.kind
                ),
            });
        }
        Ok(
            FirBuilder::new(&mut self.store).load_var(
                zone.name,
                AccessType::Struct,
                FirType::Sound,
            ),
        )
    }

    /// Resolves the zone name of a `SIGSOUNDFILE` operand.
    fn soundfile_zone_name(&mut self, sf: SigId) -> Result<String, PureVectorLowerError> {
        let signal_id = u64::from(sf.as_u32());
        let SigMatch::Soundfile(control) = match_sig(self.prepared.arena(), sf) else {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "soundfile accessor operand is not a SIGSOUNDFILE".to_owned(),
            });
        };
        let zone = crate::signal_fir::vector::ui::control_zone(self.ui, control).map_err(
            |expression| PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression,
            },
        )?;
        if zone.kind != ui::ControlKind::Soundfile {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: format!(
                    "soundfile control {control} kind mismatch: got {:?}",
                    zone.kind
                ),
            });
        }
        Ok(zone.name)
    }

    fn lower_ui_input(
        &mut self,
        control: ui::ControlId,
        expected: ui::ControlKind,
    ) -> Result<FirId, PureVectorLowerError> {
        let zone = crate::signal_fir::vector::ui::control_zone(self.ui, control).map_err(
            |expression| PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression,
            },
        )?;
        if zone.kind != expected {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression: format!(
                    "UI control {control} kind mismatch: expected {expected:?}, got {:?}",
                    zone.kind
                ),
            });
        }
        // Execution-options port phase 5: under external control, compute
        // must not observe host-mutated UI zones directly. Each zone read
        // goes through a promoted snapshot field written once per `control`
        // invocation; the classic mode keeps the direct zone read inline.
        if self.control_rate_mode.is_external() {
            let snapshot = if let Some(existing) = self.ui_snapshots.get(&zone.name) {
                existing.clone()
            } else {
                let name = format!("fSlow{}", self.snapshot_counter);
                self.snapshot_counter += 1;
                let mut b = FirBuilder::new(&mut self.store);
                let raw = b.load_var(zone.name.clone(), AccessType::Struct, FirType::FaustFloat);
                let cast = b.cast(self.real_type.clone(), raw);
                let store = b.store_var(&name, AccessType::Struct, cast);
                self.snapshot_stores.push(store);
                self.promoted_control_fields
                    .push((name.clone(), self.real_type.clone()));
                self.ui_snapshots.insert(zone.name.clone(), name.clone());
                name
            };
            return Ok(FirBuilder::new(&mut self.store).load_var(
                snapshot,
                AccessType::Struct,
                self.real_type.clone(),
            ));
        }
        let raw = FirBuilder::new(&mut self.store).load_var(
            zone.name,
            AccessType::Struct,
            FirType::FaustFloat,
        );
        Ok(FirBuilder::new(&mut self.store).cast(self.real_type.clone(), raw))
    }

    fn lower_bargraph(
        &mut self,
        scope: LowerScope,
        control: ui::ControlId,
        expected: ui::ControlKind,
        inner: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let zone = crate::signal_fir::vector::ui::control_zone(self.ui, control).map_err(
            |expression| PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression,
            },
        )?;
        if zone.kind != expected {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression: format!(
                    "bargraph {control} kind mismatch: expected {expected:?}, got {:?}",
                    zone.kind
                ),
            });
        }
        let value = self.lower_dep(scope, inner, cache, active)?;
        let external = FirBuilder::new(&mut self.store).cast(FirType::FaustFloat, value);
        let store =
            FirBuilder::new(&mut self.store).store_var(zone.name, AccessType::Struct, external);
        self.ui_stores.entry(scope).or_default().push(store);
        Ok(value)
    }

    fn lower_delay_read(
        &mut self,
        scope: LowerScope,
        carrier: SigId,
        delay: u64,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        if delay == 0 {
            return self.lower_dep(scope, carrier, cache, active);
        }
        let amount =
            FirBuilder::new(&mut self.store).int32(i32::try_from(delay).map_err(|_| {
                PureVectorLowerError::UnsupportedSignal {
                    signal_id: u64::from(carrier.as_u32()),
                    expression: "delay amount exceeds FIR i32".to_owned(),
                }
            })?);
        self.lower_delay_read_value(carrier, amount, delay)
    }

    fn lower_delay_read_value(
        &mut self,
        carrier: SigId,
        amount: FirId,
        max_delay: u64,
    ) -> Result<FirId, PureVectorLowerError> {
        let carrier_id = u64::from(carrier.as_u32());
        let transition = self
            .state_plan
            .and_then(|plan| {
                plan.plan()
                    .delays
                    .iter()
                    .find(|transition| transition.signal_id == carrier_id)
            })
            .ok_or_else(|| PureVectorLowerError::UnsupportedSignal {
                signal_id: carrier_id,
                expression: "delay carrier has no accepted P6.1 storage transition".to_owned(),
            })?;
        if max_delay > transition.max_delay {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id: carrier_id,
                expression: format!(
                    "delay bound {max_delay} exceeds certified maximum {}",
                    transition.max_delay
                ),
            });
        }
        let typ = value_fir_type(&transition.value_type, self.real_type.clone());
        if let VectorDelayStorage::Register { local_name, .. } = &transition.storage {
            if !matches!(
                match_fir(&self.store, amount),
                FirMatch::Int32 { value: 1, .. }
            ) {
                return Err(PureVectorLowerError::UnsupportedSignal {
                    signal_id: carrier_id,
                    expression: "register-carried lockstep state requires a fixed delay of one"
                        .to_owned(),
                });
            }
            return Ok(FirBuilder::new(&mut self.store).load_var(
                local_name,
                AccessType::Stack,
                typ,
            ));
        }
        let mut builder = FirBuilder::new(&mut self.store);
        let index = match &transition.storage {
            VectorDelayStorage::Register { .. } => {
                unreachable!("register storage returned before indexed lowering")
            }
            VectorDelayStorage::Copy { history_length, .. } => {
                let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
                let vindex = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
                let local = builder.binop(fir::FirBinOp::Sub, i0, vindex, FirType::Int32);
                let history = builder.int32(i32::try_from(*history_length).map_err(|_| {
                    PureVectorLowerError::UnsupportedSignal {
                        signal_id: carrier_id,
                        expression: "copy-delay history exceeds FIR i32".to_owned(),
                    }
                })?);
                let current = builder.binop(fir::FirBinOp::Add, history, local, FirType::Int32);
                builder.binop(fir::FirBinOp::Sub, current, amount, FirType::Int32)
            }
            VectorDelayStorage::Ring {
                index_name, mask, ..
            } => {
                let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
                let vindex = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
                let local = builder.binop(fir::FirBinOp::Sub, i0, vindex, FirType::Int32);
                let base = builder.load_var(index_name, AccessType::Struct, FirType::Int32);
                let current = builder.binop(fir::FirBinOp::Add, base, local, FirType::Int32);
                let delayed = builder.binop(fir::FirBinOp::Sub, current, amount, FirType::Int32);
                let mask = builder.int32(i32::try_from(*mask).map_err(|_| {
                    PureVectorLowerError::UnsupportedSignal {
                        signal_id: carrier_id,
                        expression: "ring-delay mask exceeds FIR i32".to_owned(),
                    }
                })?);
                builder.binop(fir::FirBinOp::And, delayed, mask, FirType::Int32)
            }
            VectorDelayStorage::ClockRing {
                cursor_name, mask, ..
            } => {
                let cursor = builder.load_var(cursor_name, AccessType::Struct, FirType::Int32);
                let delayed = builder.binop(fir::FirBinOp::Sub, cursor, amount, FirType::Int32);
                let mask = builder.int32(i32::try_from(*mask).map_err(|_| {
                    PureVectorLowerError::UnsupportedSignal {
                        signal_id: carrier_id,
                        expression: "clock-ring mask exceeds FIR i32".to_owned(),
                    }
                })?);
                builder.binop(fir::FirBinOp::And, delayed, mask, FirType::Int32)
            }
        };
        Ok(match &transition.storage {
            VectorDelayStorage::Register { .. } => {
                unreachable!("register storage returned before table lowering")
            }
            VectorDelayStorage::Copy { temporary_name, .. } => {
                builder.load_table(temporary_name, AccessType::Stack, index, typ)
            }
            VectorDelayStorage::Ring { buffer_name, .. } => {
                builder.load_table(buffer_name, AccessType::Struct, index, typ)
            }
            VectorDelayStorage::ClockRing { buffer_name, .. } => {
                builder.load_table(buffer_name, AccessType::Struct, index, typ)
            }
        })
    }

    fn lower_prefix(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        value: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let LowerScope::Loop(loop_id) = scope else {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "prefix state cannot be read from control scope".to_owned(),
            });
        };
        let transition = self
            .state_plan
            .and_then(|plan| {
                plan.plan()
                    .prefixes
                    .iter()
                    .find(|transition| transition.signal_id == signal_id)
            })
            .ok_or_else(|| PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "prefix has no accepted P6.1 state transition".to_owned(),
            })?;
        if transition.loop_id != loop_id || transition.value_signal_id != u64::from(value.as_u32())
        {
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "prefix signal {signal_id} transition does not match loop {loop_id} and value {}",
                    value.as_u32()
                ),
            });
        }
        let state_name = transition.state_name.clone();
        let typ = value_fir_type(&transition.value_type, self.real_type.clone());
        let _ = self.lower_dep(scope, value, cache, active)?;
        Ok(FirBuilder::new(&mut self.store).load_var(state_name, AccessType::Struct, typ))
    }

    fn lower_waveform(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        values: &[SigId],
    ) -> Result<FirId, PureVectorLowerError> {
        let LowerScope::Loop(loop_id) = scope else {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "waveform state cannot be read from control scope".to_owned(),
            });
        };
        let transition = self
            .state_plan
            .and_then(|plan| {
                plan.plan()
                    .waveforms
                    .iter()
                    .find(|transition| transition.signal_id == signal_id)
            })
            .ok_or_else(|| PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "waveform has no accepted P6.1 state transition".to_owned(),
            })?;
        if transition.loop_id != loop_id
            || transition.length
                != u64::try_from(values.len()).map_err(|_| {
                    PureVectorLowerError::UnsupportedSignal {
                        signal_id,
                        expression: "waveform length exceeds u64".to_owned(),
                    }
                })?
            || values.is_empty()
        {
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "waveform signal {signal_id} transition does not match loop {loop_id} and length {}",
                    values.len()
                ),
            });
        }
        let index_name = transition.index_name.clone();
        let elem_type = value_fir_type(&transition.value_type, self.real_type.clone());
        let table_name = if let Some(name) = self.waveform_tables.get(&signal_id) {
            name.clone()
        } else {
            let prefix = if elem_type == FirType::Int32 {
                "iVecWave"
            } else {
                "fVecWave"
            };
            let name = format!("{prefix}{signal_id}");
            let mut literals = Vec::with_capacity(values.len());
            for &value in values {
                let literal = match (elem_type.clone(), match_sig(self.prepared.arena(), value)) {
                    (FirType::Int32, SigMatch::Int(value)) => {
                        FirBuilder::new(&mut self.store).int32(value)
                    }
                    (FirType::Float32 | FirType::Float64, SigMatch::Int(value)) => {
                        self.float_const(f64::from(value))
                    }
                    (FirType::Float32 | FirType::Float64, SigMatch::Real(value)) => {
                        self.float_const(value)
                    }
                    _ => {
                        return Err(PureVectorLowerError::UnsupportedSignal {
                            signal_id,
                            expression: "checked waveform tables require scalar numeric literals"
                                .to_owned(),
                        });
                    }
                };
                literals.push(literal);
            }
            let declaration = FirBuilder::new(&mut self.store).declare_table(
                name.clone(),
                AccessType::Static,
                elem_type.clone(),
                &literals,
            );
            self.static_declarations.push(declaration);
            self.waveform_tables.insert(signal_id, name.clone());
            name
        };
        let index = FirBuilder::new(&mut self.store).load_var(
            index_name,
            AccessType::Struct,
            FirType::Int32,
        );
        Ok(FirBuilder::new(&mut self.store).load_table(
            table_name,
            AccessType::Static,
            index,
            elem_type,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_table_definition(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        size: SigId,
        generator: SigId,
        write_index: SigId,
        write_value: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        if wrtbl_is_readonly(self.prepared.arena(), write_index, write_value) {
            let _ = self.ensure_readonly_table(signal_id, size, generator)?;
            let typ = self.fir_type(signal_id)?;
            return self.zero_value(&typ);
        }
        // A live-port table writes once per sample, so its store belongs to
        // the writer's own sample loop. The store statement is head-inserted
        // into that loop's body: CSE hoists the index and value definitions
        // before their first use, and every same-sample read materializes
        // after, which is the rwtable write-before-read contract the scalar
        // backend emits.
        let LowerScope::Loop(loop_id) = scope else {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "mutable table write outside a sample loop".to_owned(),
            });
        };
        let (name, length, elem_type) = self.ensure_mutable_table(signal_id, size, generator)?;
        let raw_index = self.lower_dep(scope, write_index, cache, active)?;
        if self.store.value_type(raw_index) != Some(FirType::Int32) {
            return Err(PureVectorLowerError::FirTypeMismatch {
                signal_id: u64::from(write_index.as_u32()),
                expected: FirType::Int32,
                actual: self.store.value_type(raw_index),
            });
        }
        let value = self.lower_dep(scope, write_value, cache, active)?;
        if self.store.value_type(value) != Some(elem_type.clone()) {
            return Err(PureVectorLowerError::FirTypeMismatch {
                signal_id: u64::from(write_value.as_u32()),
                expected: elem_type,
                actual: self.store.value_type(value),
            });
        }
        let length_i32 =
            i32::try_from(length).map_err(|_| PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "mutable table length exceeds FIR i32".to_owned(),
            })?;
        // The scalar backend wraps the write index as ((i % n) + n) % n;
        // mirror it exactly so both paths store to identical cells.
        let mut builder = FirBuilder::new(&mut self.store);
        let len = builder.int32(length_i32);
        let rem = builder.binop(fir::FirBinOp::Rem, raw_index, len, FirType::Int32);
        let shifted = builder.binop(fir::FirBinOp::Add, rem, len, FirType::Int32);
        let wrapped = builder.binop(fir::FirBinOp::Rem, shifted, len, FirType::Int32);
        let store = builder.store_table(name, AccessType::Struct, wrapped, value);
        self.table_stores.entry(loop_id).or_default().push(store);
        let typ = self.fir_type(signal_id)?;
        self.zero_value(&typ)
    }

    fn lower_table_generator(&mut self, signal_id: u64) -> Result<FirId, PureVectorLowerError> {
        // `Gen` is a lifecycle boundary: its content runs through the SIGGEN
        // interpreter at init, never at compute time, so the node itself is a
        // zero placeholder. Read-only and mutable owners both qualify; the
        // owning table's own lowering decides how the content is emitted.
        let is_table_generator = self.signal_ids.values().any(|&candidate| {
            matches!(
                match_sig(self.prepared.arena(), candidate),
                SigMatch::WrTbl(_, generator, _, _)
                    if u64::from(generator.as_u32()) == signal_id
            )
        });
        if !is_table_generator {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "generator is not owned by an accepted table".to_owned(),
            });
        }
        let typ = self.fir_type(signal_id)?;
        self.zero_value(&typ)
    }

    fn lower_readonly_table(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        table: SigId,
        index: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let table_id = u64::from(table.as_u32());
        let _ = self.lower_dep(scope, table, cache, active)?;
        let (name, length, elem_type, access) = if let Some((name, length, elem_type)) =
            self.readonly_tables.get(&table_id).cloned()
        {
            (name, length, elem_type, AccessType::Static)
        } else if let Some((name, length, elem_type)) = self.mutable_tables.get(&table_id).cloned()
        {
            (name, length, elem_type, AccessType::Struct)
        } else {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "table read source is not an accepted table".to_owned(),
            });
        };
        let raw_index = self.lower_dep(scope, index, cache, active)?;
        if self.store.value_type(raw_index) != Some(FirType::Int32) {
            return Err(PureVectorLowerError::FirTypeMismatch {
                signal_id: u64::from(index.as_u32()),
                expected: FirType::Int32,
                actual: self.store.value_type(raw_index),
            });
        }
        let checked_index = self.table_index_with_bounds(index, raw_index, length)?;
        let expected = self.fir_type(signal_id)?;
        if expected != elem_type {
            return Err(PureVectorLowerError::FirTypeMismatch {
                signal_id,
                expected,
                actual: Some(elem_type),
            });
        }
        Ok(FirBuilder::new(&mut self.store).load_table(name, access, checked_index, expected))
    }

    fn ensure_readonly_table(
        &mut self,
        signal_id: u64,
        size: SigId,
        generator: SigId,
    ) -> Result<(String, usize, FirType), PureVectorLowerError> {
        if let Some(table) = self.readonly_tables.get(&signal_id) {
            return Ok(table.clone());
        }
        let (length, elem_type, initializers) =
            self.table_initializers(signal_id, size, generator)?;
        let prefix = if elem_type == FirType::Int32 {
            "iVecTbl"
        } else {
            "fVecTbl"
        };
        let name = format!("{prefix}{signal_id}");
        let declaration = FirBuilder::new(&mut self.store).declare_table(
            name.clone(),
            AccessType::Static,
            elem_type.clone(),
            &initializers,
        );
        self.static_declarations.push(declaration);
        let table = (name, length, elem_type);
        self.readonly_tables.insert(signal_id, table.clone());
        Ok(table)
    }

    /// Evaluates a table's constant length, element type, and per-element
    /// initial content. Shared by the read-only and mutable table paths: both
    /// classes const-fold their generator through the same SIGGEN interpreter,
    /// differing only in where the declaration and the initial content land.
    fn table_initializers(
        &mut self,
        signal_id: u64,
        size: SigId,
        generator: SigId,
    ) -> Result<(usize, FirType, Vec<FirId>), PureVectorLowerError> {
        let length = match match_sig(self.prepared.arena(), size) {
            SigMatch::Int(value) if value > 0 => {
                usize::try_from(value).map_err(|_| PureVectorLowerError::UnsupportedSignal {
                    signal_id,
                    expression: format!("read-only table size {value} exceeds usize"),
                })?
            }
            _ => {
                return Err(PureVectorLowerError::UnsupportedSignal {
                    signal_id,
                    expression: "read-only table requires a positive literal size".to_owned(),
                });
            }
        };
        let elem_type = self.fir_type(signal_id)?;
        if !matches!(
            elem_type,
            FirType::Int32 | FirType::Float32 | FirType::Float64
        ) {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: format!("unsupported read-only table element type {elem_type:?}"),
            });
        }
        let inner = match match_sig(self.prepared.arena(), generator) {
            SigMatch::Gen(inner) => inner,
            _ => generator,
        };
        let mut initializers = Vec::with_capacity(length);
        match match_sig(self.prepared.arena(), inner) {
            SigMatch::Waveform(values) if !values.is_empty() => {
                for index in 0..length {
                    initializers
                        .push(self.table_literal(values[index % values.len()], &elem_type)?);
                }
            }
            SigMatch::Int(_) | SigMatch::Real(_) => {
                let value = self.table_literal(inner, &elem_type)?;
                initializers.resize(length, value);
            }
            _ => {
                let values =
                    interpret_generator(self.prepared.arena(), inner, length).map_err(|error| {
                        PureVectorLowerError::UnsupportedSignal {
                            signal_id,
                            expression: format!("read-only table generator failed: {error}"),
                        }
                    })?;
                for value in values {
                    initializers.push(self.table_value(value, &elem_type)?);
                }
            }
        }
        Ok((length, elem_type, initializers))
    }

    /// Declares one mutable table as a DSP-struct array field and queues its
    /// element-wise initial content for `instanceConstants`, mirroring the
    /// scalar lifecycle: runtime writes must persist across compute calls, so
    /// the content is written once at init, never per block.
    fn ensure_mutable_table(
        &mut self,
        signal_id: u64,
        size: SigId,
        generator: SigId,
    ) -> Result<(String, usize, FirType), PureVectorLowerError> {
        if let Some(table) = self.mutable_tables.get(&signal_id) {
            return Ok(table.clone());
        }
        let (length, elem_type, initializers) =
            self.table_initializers(signal_id, size, generator)?;
        let name = mutable_table_name(signal_id, &elem_type);
        let mut builder = FirBuilder::new(&mut self.store);
        let declaration = builder.declare_var(
            name.clone(),
            FirType::Array(Box::new(elem_type.clone()), length),
            AccessType::Struct,
            None,
        );
        let mut init_statements = Vec::with_capacity(length);
        for (index, &value) in initializers.iter().enumerate() {
            let index_i32 = i32::try_from(index).expect("table length fits i32");
            let position = builder.int32(index_i32);
            init_statements.push(builder.store_table(
                name.clone(),
                AccessType::Struct,
                position,
                value,
            ));
        }
        self.table_declarations.push(declaration);
        self.table_init_statements.extend(init_statements);
        let table = (name, length, elem_type);
        self.mutable_tables.insert(signal_id, table.clone());
        Ok(table)
    }

    fn table_literal(
        &mut self,
        signal: SigId,
        elem_type: &FirType,
    ) -> Result<FirId, PureVectorLowerError> {
        match match_sig(self.prepared.arena(), signal) {
            SigMatch::Int(value) => self.table_value(f64::from(value), elem_type),
            SigMatch::Real(value) => self.table_value(value, elem_type),
            _ => Err(PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(signal.as_u32()),
                expression: "read-only table literal is not numeric".to_owned(),
            }),
        }
    }

    fn table_value(
        &mut self,
        value: f64,
        elem_type: &FirType,
    ) -> Result<FirId, PureVectorLowerError> {
        let mut builder = FirBuilder::new(&mut self.store);
        match elem_type {
            FirType::Int32 => Ok(builder.int32(value as i32)),
            FirType::Float32 => Ok(builder.float32(value as f32)),
            FirType::Float64 => Ok(builder.float64(value)),
            _ => Err(PureVectorLowerError::InvalidRealType(elem_type.clone())),
        }
    }

    fn table_index_with_bounds(
        &mut self,
        index_signal: SigId,
        index: FirId,
        length: usize,
    ) -> Result<FirId, PureVectorLowerError> {
        let length_i32 =
            i32::try_from(length).map_err(|_| PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(index_signal.as_u32()),
                expression: "read-only table length exceeds FIR i32".to_owned(),
            })?;
        if let Some(interval) = self
            .prepared
            .sig_ty(index_signal)
            .map(sigtype::SigType::interval)
        {
            let lo = interval.lo();
            let hi = interval.hi();
            if lo.is_finite() && hi.is_finite() {
                let lo = lo as i64;
                let hi = hi as i64;
                let length = i64::from(length_i32);
                if lo >= 0 && hi >= 0 && hi < length {
                    return Ok(index);
                }
                let mut builder = FirBuilder::new(&mut self.store);
                let upper = builder.int32(length_i32 - 1);
                self.int_helpers.insert("min_i");
                let upper_clamped = builder.fun_call("min_i", &[upper, index], FirType::Int32);
                if lo >= 0 {
                    return Ok(upper_clamped);
                }
                let zero = builder.int32(0);
                self.int_helpers.insert("max_i");
                return Ok(builder.fun_call("max_i", &[upper_clamped, zero], FirType::Int32));
            }
        }
        let mut builder = FirBuilder::new(&mut self.store);
        let length = builder.int32(length_i32);
        let rem = builder.binop(fir::FirBinOp::Rem, index, length, FirType::Int32);
        let shifted = builder.binop(fir::FirBinOp::Add, rem, length, FirType::Int32);
        Ok(builder.binop(fir::FirBinOp::Rem, shifted, length, FirType::Int32))
    }

    fn lower_symbolic_ref(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        var: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let bodies = self.symbolic_bodies_for_var(signal_id, var)?;
        let mut values = Vec::with_capacity(bodies.len());
        for body in bodies {
            values.push(self.lower_dep(scope, body, cache, active)?);
        }
        let typ = self.fir_type(signal_id)?;
        Ok(FirBuilder::new(&mut self.store).value_array(&values, typ))
    }

    fn symbolic_bodies_for_var(
        &self,
        signal_id: u64,
        var: SigId,
    ) -> Result<Vec<SigId>, PureVectorLowerError> {
        self.signal_ids
            .values()
            .find_map(|&candidate| {
                let (bound, bodies) =
                    decode_symbolic_group_bodies(self.prepared.arena(), candidate)?;
                (bound == var).then_some(bodies)
            })
            .ok_or_else(|| PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "symbolic recursion reference has no reachable binder".to_owned(),
            })
    }

    /// Lowers `ZeroPad(x, h)` under its counted upsampling island as
    /// `((vclock_d<N>_fire == h - 1) ? x : 0)`, the scalar `generateZeroPad`
    /// gating: the outer-rate input enters on the last fire only. Passing `x`
    /// through unguarded feeds it on every fire and, e.g., accumulates an
    /// impulse `h` times.
    fn lower_zero_pad(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        value: SigId,
        amount: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let Some(&domain_id) = self.upsampling_domains.get(&signal_id) else {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "ZeroPad outside a counted upsampling island (zero-stuffed \
                             inputs are only legal under an upsampling fire loop)"
                    .to_owned(),
            });
        };
        let value = self.lower_dep(scope, value, cache, active)?;
        let amount = self.lower_dep(scope, amount, cache, active)?;
        let typ = self.fir_type(signal_id)?;
        let zero = self.zero_value(&typ)?;
        let mut b = FirBuilder::new(&mut self.store);
        let idx = b.load_var(
            format!("vclock_d{domain_id}_fire"),
            AccessType::Loop,
            FirType::Int32,
        );
        let one = b.int32(1);
        let last = b.binop(FirBinOp::Sub, amount, one, FirType::Int32);
        let is_last = b.binop(FirBinOp::Eq, idx, last, FirType::Int32);
        Ok(b.select2(is_last, value, zero, typ))
    }

    fn zero_value(&mut self, typ: &FirType) -> Result<FirId, PureVectorLowerError> {
        Ok(match typ {
            FirType::Int32 | FirType::Bool => FirBuilder::new(&mut self.store).int32(0),
            FirType::Float32 => FirBuilder::new(&mut self.store).float32(0.0),
            FirType::Float64 => FirBuilder::new(&mut self.store).float64(0.0),
            other => {
                return Err(PureVectorLowerError::InvalidRealType(other.clone()));
            }
        })
    }

    fn lower_binop(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        op: BinOp,
        operands: (SigId, SigId),
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let lhs = self.lower_dep(scope, operands.0, cache, active)?;
        let rhs = self.lower_dep(scope, operands.1, cache, active)?;
        let result_type = self.fir_type(signal_id)?;
        let (fir_op, typ) = map_binop(op, result_type.clone()).ok_or_else(|| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: format!("unsupported binary operator {}", op.name()),
            }
        })?;
        let lhs_type = self.store.value_type(lhs);
        let rhs_type = self.store.value_type(rhs);
        let operands_ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                lhs_type == Some(result_type.clone()) && rhs_type == Some(result_type)
            }
            BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Lsh | BinOp::ARsh | BinOp::LRsh => {
                lhs_type == Some(FirType::Int32) && rhs_type == Some(FirType::Int32)
            }
            BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne => {
                lhs_type == rhs_type
                    && matches!(
                        lhs_type,
                        Some(FirType::Int32 | FirType::Float32 | FirType::Float64)
                    )
            }
        };
        if !operands_ok {
            return Err(PureVectorLowerError::FirTypeMismatch {
                signal_id,
                expected: typ,
                actual: lhs_type,
            });
        }
        Ok(FirBuilder::new(&mut self.store).binop(fir_op, lhs, rhs, typ))
    }

    fn lower_math1(
        &mut self,
        scope: LowerScope,
        op: FirMathOp,
        value: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let value = self.lower_dep(scope, value, cache, active)?;
        self.math_ops.insert(op);
        Ok(FirBuilder::new(&mut self.store).math_call(op, &[value], self.real_type.clone()))
    }

    fn lower_math2(
        &mut self,
        scope: LowerScope,
        op: FirMathOp,
        lhs: SigId,
        rhs: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        let lhs = self.lower_dep(scope, lhs, cache, active)?;
        let rhs = self.lower_dep(scope, rhs, cache, active)?;
        self.math_ops.insert(op);
        Ok(FirBuilder::new(&mut self.store).math_call(op, &[lhs, rhs], self.real_type.clone()))
    }

    fn lower_minmax(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        operands: (SigId, SigId),
        is_min: bool,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        if self.fir_type(signal_id)? == FirType::Int32 {
            let lhs = self.lower_dep(scope, operands.0, cache, active)?;
            let rhs = self.lower_dep(scope, operands.1, cache, active)?;
            let name = if is_min { "min_i" } else { "max_i" };
            self.int_helpers.insert(name);
            return Ok(FirBuilder::new(&mut self.store).fun_call(
                name,
                &[lhs, rhs],
                FirType::Int32,
            ));
        }
        self.lower_math2(
            scope,
            if is_min {
                FirMathOp::Min
            } else {
                FirMathOp::Max
            },
            operands.0,
            operands.1,
            cache,
            active,
        )
    }

    fn lower_abs(
        &mut self,
        scope: LowerScope,
        signal_id: u64,
        value: SigId,
        cache: &mut BTreeMap<u64, FirId>,
        active: &mut BTreeSet<(LowerScope, u64)>,
    ) -> Result<FirId, PureVectorLowerError> {
        if self.fir_type(signal_id)? == FirType::Int32 {
            let value = self.lower_dep(scope, value, cache, active)?;
            self.int_helpers.insert("abs");
            return Ok(FirBuilder::new(&mut self.store).fun_call("abs", &[value], FirType::Int32));
        }
        self.lower_math1(scope, FirMathOp::Abs, value, cache, active)
    }

    fn lower_input(&mut self, index: i32) -> Result<FirId, PureVectorLowerError> {
        let Ok(index_usize) = usize::try_from(index) else {
            return Err(PureVectorLowerError::InputIndexOutOfRange {
                index,
                num_inputs: self.num_inputs,
            });
        };
        if index_usize >= self.num_inputs {
            return Err(PureVectorLowerError::InputIndexOutOfRange {
                index,
                num_inputs: self.num_inputs,
            });
        }
        let alias = format!("input{index_usize}");
        if self.input_aliases.insert(index_usize) {
            let mut builder = FirBuilder::new(&mut self.store);
            let channel = builder.int32(index);
            let pointer_type = FirType::Ptr(Box::new(FirType::FaustFloat));
            let pointer =
                builder.load_table("inputs", AccessType::FunArgs, channel, pointer_type.clone());
            self.input_declarations.push(builder.declare_var(
                alias.clone(),
                pointer_type,
                AccessType::Stack,
                Some(pointer),
            ));
        }
        let mut builder = FirBuilder::new(&mut self.store);
        let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
        let raw = builder.load_table(alias, AccessType::Stack, i0, FirType::FaustFloat);
        Ok(builder.cast(self.real_type.clone(), raw))
    }

    fn float_const(&mut self, value: f64) -> FirId {
        let mut builder = FirBuilder::new(&mut self.store);
        match self.real_type {
            FirType::Float32 => builder.float32(value as f32),
            FirType::Float64 => builder.float64(value),
            _ => unreachable!("real type checked at entry"),
        }
    }

    /// Mirrors scalar `SignalFirLower::lower_fconst` for the canonical Faust
    /// sampling-rate aliases. The persistent field is initialized by the
    /// shared vector lifecycle assembler before `compute` executes.
    fn lower_fconst(&mut self, signal_id: u64, name: SigId) -> Result<FirId, PureVectorLowerError> {
        let name = tree_to_str(self.prepared.arena(), name).ok_or_else(|| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "foreign constant name is not a symbol".to_owned(),
            }
        })?;
        if name != "fSamplingFreq" && name != "fSamplingRate" {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: format!("unsupported foreign constant `{name}`"),
            });
        }
        let expected = self.fir_type(signal_id)?;
        let mut builder = FirBuilder::new(&mut self.store);
        let sample_rate = builder.load_var("fSampleRate", AccessType::Struct, FirType::Int32);
        Ok(if expected == FirType::Int32 {
            sample_rate
        } else {
            builder.cast(expected, sample_rate)
        })
    }

    /// Mirrors the scalar special case for Faust's block-size foreign
    /// variable. Other extern globals remain outside the checked vector module
    /// until their declarations are represented in the final artifact.
    fn lower_fvar(
        &mut self,
        signal_id: u64,
        kind: SigId,
        name: SigId,
    ) -> Result<FirId, PureVectorLowerError> {
        let name = tree_to_str(self.prepared.arena(), name).ok_or_else(|| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "foreign variable name is not a symbol".to_owned(),
            }
        })?;
        if name != "count" {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: format!("unsupported foreign variable `{name}`"),
            });
        }
        let kind = tree_to_int(self.prepared.arena(), kind).ok_or_else(|| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression: "foreign variable type is not an integer code".to_owned(),
            }
        })?;
        let declared = if kind == 0 {
            FirType::Int32
        } else {
            self.real_type.clone()
        };
        let expected = self.fir_type(signal_id)?;
        if expected != declared {
            return Err(PureVectorLowerError::PlannedTypeMismatch {
                signal_id,
                planned: self.record(signal_id)?.value_type,
                prepared: self.prepared.ty(self.sig(signal_id)?),
            });
        }
        Ok(FirBuilder::new(&mut self.store).load_var("count", AccessType::FunArgs, declared))
    }

    fn fir_type(&self, signal_id: u64) -> Result<FirType, PureVectorLowerError> {
        let record = self.record(signal_id)?;
        value_type_to_fir(&record.value_type, &self.real_type).ok_or(
            PureVectorLowerError::PlannedTypeMismatch {
                signal_id,
                planned: record.value_type,
                prepared: self.prepared.ty(self.sig(signal_id)?),
            },
        )
    }

    fn check_type(&self, signal_id: u64, value: FirId) -> Result<(), PureVectorLowerError> {
        let expected = self.fir_type(signal_id)?;
        let actual = self.store.value_type(value);
        if actual == Some(expected.clone()) {
            Ok(())
        } else {
            Err(PureVectorLowerError::FirTypeMismatch {
                signal_id,
                expected,
                actual,
            })
        }
    }
}
pub(super) fn value_type_to_fir(value_type: &ValueType, real_type: &FirType) -> Option<FirType> {
    match value_type {
        ValueType::Int => Some(FirType::Int32),
        ValueType::Real => Some(real_type.clone()),
        ValueType::Sound => Some(FirType::Sound),
        ValueType::Tuple(_) => Some(value_fir_type(value_type, real_type.clone())),
    }
}
pub(super) fn materialize_region_roots(
    store: &mut FirStore,
    values: &[FirId],
    region: VectorRegion,
) -> Result<(Vec<FirId>, Vec<FirId>), PureVectorLowerError> {
    materialize_region_roots_with_prefix(
        store,
        values,
        region,
        Vec::new(),
        "fVecControlTemp",
        "iVecControlTemp",
    )
}

/// Result of [`materialize_region_roots_promoted`]: the materialized
/// statement list, the rewritten root values, and the promoted struct
/// fields.
pub(super) type PromotedRegionRoots = (Vec<FirId>, Vec<FirId>, Vec<(String, FirType)>);

/// `-ec` variant of [`materialize_region_roots`]: shared temporaries become
/// DSP struct fields written in place (plan phase 5, vector control-root
/// promotion), so their stores can move to `control` while compute-side
/// consumers load them across the function boundary.
pub(super) fn materialize_region_roots_promoted(
    store: &mut FirStore,
    values: &[FirId],
    region: VectorRegion,
) -> Result<PromotedRegionRoots, PureVectorLowerError> {
    let mut builder = FirBuilder::new(store);
    let mut statements: Vec<FirId> = values.iter().map(|&value| builder.drop_(value)).collect();
    let mut fields = materialize_shared_values_promoted(
        store,
        &mut statements,
        "fVecControlTemp",
        0,
        "iVecControlTemp",
        0,
    );
    let mut field_names = fields
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    let mut float_counter = next_promoted_counter(&field_names, "fVecControlTemp");
    let mut int_counter = next_promoted_counter(&field_names, "iVecControlTemp");
    let mut rewritten = Vec::with_capacity(values.len());
    let mut promoted_statements = Vec::with_capacity(statements.len());
    for statement in statements {
        let FirMatch::Drop(value) = match_fir(store, statement) else {
            promoted_statements.push(statement);
            continue;
        };
        if let FirMatch::LoadVar {
            name,
            access: AccessType::Struct,
            ..
        } = match_fir(store, value)
            && field_names.contains(&name)
        {
            // CSE already emitted the corresponding struct store. Retain the
            // Drop temporarily because the independent body checker uses it
            // as root evidence; final module materialization removes it.
            promoted_statements.push(statement);
            rewritten.push(value);
            continue;
        }
        let typ = store
            .value_type(value)
            .ok_or(PureVectorLowerError::CseRootCoverage { region })?;
        let (prefix, counter) = if matches!(typ, FirType::Int32 | FirType::Int64 | FirType::Bool) {
            ("iVecControlTemp", &mut int_counter)
        } else {
            ("fVecControlTemp", &mut float_counter)
        };
        let name = loop {
            let candidate = format!("{prefix}{counter}");
            *counter += 1;
            if field_names.insert(candidate.clone()) {
                break candidate;
            }
        };
        fields.push((name.clone(), typ.clone()));
        let store_root = FirBuilder::new(store).store_var(&name, AccessType::Struct, value);
        let load_root = FirBuilder::new(store).load_var(name, AccessType::Struct, typ);
        let evidence_root = FirBuilder::new(store).drop_(load_root);
        promoted_statements.push(store_root);
        promoted_statements.push(evidence_root);
        rewritten.push(load_root);
    }
    if rewritten.len() != values.len() {
        return Err(PureVectorLowerError::CseRootCoverage { region });
    }
    Ok((promoted_statements, rewritten, fields))
}

fn next_promoted_counter(names: &BTreeSet<String>, prefix: &str) -> u32 {
    names
        .iter()
        .filter_map(|name| name.strip_prefix(prefix)?.parse::<u32>().ok())
        .max()
        .map_or(0, |value| value.saturating_add(1))
}
pub(super) fn materialize_region_roots_with_prefix(
    store: &mut FirStore,
    values: &[FirId],
    region: VectorRegion,
    head_statements: Vec<FirId>,
    float_prefix: &str,
    int_prefix: &str,
) -> Result<(Vec<FirId>, Vec<FirId>), PureVectorLowerError> {
    // Head statements run before every root of the region. Mutable-table
    // stores are placed here: shared-value materialization inserts each
    // definition before its first use, so a store's index and value land
    // above it while every dependent read materializes below.
    let mut builder = FirBuilder::new(store);
    let mut statements = head_statements;
    statements.extend(values.iter().map(|&value| builder.drop_(value)));
    materialize_shared_values(store, &mut statements, float_prefix, 0, int_prefix, 0);
    let rewritten = statements
        .iter()
        .filter_map(|statement| match match_fir(store, *statement) {
            FirMatch::Drop(value) => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    if rewritten.len() != values.len() {
        return Err(PureVectorLowerError::CseRootCoverage { region });
    }
    Ok((statements, rewritten))
}
