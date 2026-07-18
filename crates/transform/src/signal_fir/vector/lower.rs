//! P5.2/P6.5 signal-closure lowering into verified vector regions.
//!
//! C++ `DAGInstructionsCompiler::compileMultiSignal` recursively lowers one
//! loop root and its inline closure while its current loop owns cache lookup.
//! This adapted Rust slice consumes the already verified prepared forest and
//! P4.4 plan, plus P6.1/P6.2 state and clock policies when requested. It runs
//! CSE independently in each routed region, then checks the final bodies
//! against P5.1 routing evidence. Storage and transport geometry are never
//! inferred here: fixed or bounded-variable delays, symbolic recursion, and
//! clock wrappers are lowered only through their accepted P6 artifacts.
//! Tables, UI, foreign calls, and reverse AD remain fail-closed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;

use fir::{AccessType, FirBuilder, FirId, FirMatch, FirMathOp, FirStore, FirType, match_fir};
use signals::{BinOp, SigId, SigMatch, dump_sig_readable, match_sig};
use tlib::{match_sym_ref, tree_to_int, tree_to_str};

use crate::schedule::SchedulingStrategy;
use crate::signal_prepare::{SimpleSigType, VerifiedPreparedSignals};

use super::super::module::map_binop;
use super::cse::materialize_shared_values;
use super::recursion::{decode_group_projection, decode_symbolic_group_bodies};
use super::siggen::interpret_generator;
use super::vector_analysis::{EffectAtom, wrtbl_is_readonly};
use super::vector_clock_ad::VerifiedVectorClockAdPlan;
use super::vector_plan::VerifiedVectorPlan;
use super::vector_route::{
    RouteResolution, RoutedUseSource, VectorRegion, VectorRouteError, VectorRouteSession,
    VerifiedRoutedFir, value_fir_type,
};
use super::vector_state::{VectorDelayStorage, VerifiedVectorStatePlan};
use super::vector_verify::{Placement, SignalRecord, ValueType, VectorPlan};

/// One scheduled vector loop and its final CSE-rewritten FIR body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PureVectorRegionBody {
    loop_id: u64,
    statements: Vec<FirId>,
}

impl PureVectorRegionBody {
    /// Stable P4.4 loop identity.
    #[must_use]
    pub fn loop_id(&self) -> u64 {
        self.loop_id
    }

    /// Final statements in execution order.
    #[must_use]
    pub fn statements(&self) -> &[FirId] {
        &self.statements
    }
}

/// Opaque P5.2/P6.5 result accepted by routing and region-body verification.
///
/// The historical `Pure` name is retained for source compatibility. The
/// representation now also carries programs accepted through explicit P6.1
/// state and P6.2 clock policies; it does not imply that those programs are
/// pure.
pub struct VerifiedPureVectorProgram {
    store: FirStore,
    static_declarations: Vec<FirId>,
    table_declarations: Vec<FirId>,
    table_init_statements: Vec<FirId>,
    mutable_tables: BTreeMap<u64, (String, usize, FirType)>,
    transport_declarations: Vec<FirId>,
    control_statements: Vec<FirId>,
    regions: Vec<PureVectorRegionBody>,
    routed: VerifiedRoutedFir,
    math_ops: HashSet<FirMathOp>,
    int_helpers: BTreeSet<&'static str>,
}

impl VerifiedPureVectorProgram {
    /// FIR store owning every returned id.
    #[must_use]
    pub fn store(&self) -> &FirStore {
        &self.store
    }

    /// Mutable store access reserved for the checked final-module assembler.
    pub(crate) fn store_mut(&mut self) -> &mut FirStore {
        &mut self.store
    }

    /// Consumes the checked program after final module assembly.
    pub(crate) fn into_store(self) -> FirStore {
        self.store
    }

    /// Canonical transport declarations emitted before region bodies.
    #[must_use]
    pub fn transport_declarations(&self) -> &[FirId] {
        &self.transport_declarations
    }

    /// Immutable literal tables required by checked waveform reads.
    #[must_use]
    pub fn static_declarations(&self) -> &[FirId] {
        &self.static_declarations
    }

    /// Mutable table DSP-struct field declarations.
    #[must_use]
    pub fn table_declarations(&self) -> &[FirId] {
        &self.table_declarations
    }

    /// Element-wise mutable-table initialization for `instanceConstants`.
    #[must_use]
    pub fn table_init_statements(&self) -> &[FirId] {
        &self.table_init_statements
    }

    /// Accepted mutable tables by signal id: field name, length, element type.
    #[must_use]
    pub fn mutable_tables(&self) -> &BTreeMap<u64, (String, usize, FirType)> {
        &self.mutable_tables
    }

    /// Fixed control-scope statements, including input pointer aliases.
    #[must_use]
    pub fn control_statements(&self) -> &[FirId] {
        &self.control_statements
    }

    /// Loop bodies in the selected strategy-dependent schedule order.
    #[must_use]
    pub fn regions(&self) -> &[PureVectorRegionBody] {
        &self.regions
    }

    /// Independently accepted P5.1 route evidence.
    #[must_use]
    pub fn routed(&self) -> &VerifiedRoutedFir {
        &self.routed
    }

    /// Math prototypes required when this artifact is assembled as a module.
    #[must_use]
    pub fn math_ops(&self) -> &HashSet<FirMathOp> {
        &self.math_ops
    }

    /// Integer helper prototypes required by `min`, `max`, or `abs`.
    #[must_use]
    pub fn int_helpers(&self) -> &BTreeSet<&'static str> {
        &self.int_helpers
    }
}

/// P5.2 lowering or final-body verification failure.
#[derive(Clone, Debug, PartialEq)]
pub enum PureVectorLowerError {
    /// P5.1 route construction or verification failed.
    Route(VectorRouteError),
    /// Internal real precision is outside the active fast-lane contract.
    InvalidRealType(FirType),
    /// A P4.4 signal id is absent from the verified prepared forest.
    MissingPreparedSignal { signal_id: u64 },
    /// Prepared and planned scalar types disagree.
    PlannedTypeMismatch {
        signal_id: u64,
        planned: ValueType,
        prepared: Option<SimpleSigType>,
    },
    /// The pure P5.2 slice cannot execute an effect-bearing signal.
    EffectfulSignal {
        signal_id: u64,
        expression: String,
        effects: Vec<EffectAtom>,
    },
    /// The pure P5.2 slice has no state/effect semantics for this node.
    UnsupportedSignal { signal_id: u64, expression: String },
    /// A control expression depended on a sample-region value.
    InvalidControlDependency { signal_id: u64 },
    /// A pure signal cycle escaped the P4/P6 recursion boundary.
    PureCycle {
        signal_id: u64,
        region: VectorRegion,
    },
    /// Audio input index is invalid for the declared module arity.
    InputIndexOutOfRange { index: i32, num_inputs: usize },
    /// FIR operands or result violate the prepared typing contract.
    FirTypeMismatch {
        signal_id: u64,
        expected: FirType,
        actual: Option<FirType>,
    },
    /// Region-local CSE did not preserve one sink per requested root.
    CseRootCoverage { region: VectorRegion },
    /// Final bodies do not contain the evidence accepted by P5.1.
    BodyEvidence { detail: String },
}

impl fmt::Display for PureVectorLowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route(error) => write!(f, "vector routing failed: {error}"),
            Self::InvalidRealType(typ) => write!(f, "unsupported vector real type {typ:?}"),
            Self::MissingPreparedSignal { signal_id } => {
                write!(
                    f,
                    "vector plan signal {signal_id} is absent from the prepared forest"
                )
            }
            Self::PlannedTypeMismatch {
                signal_id,
                planned,
                prepared,
            } => write!(
                f,
                "signal {signal_id} planned type {planned:?} disagrees with prepared type {prepared:?}"
            ),
            Self::EffectfulSignal {
                signal_id,
                expression,
                effects,
            } => {
                write!(
                    f,
                    "signal {signal_id} is effectful and cannot enter pure P5.2 lowering: {expression}; effects={effects:?}"
                )
            }
            Self::UnsupportedSignal {
                signal_id,
                expression,
            } => write!(
                f,
                "signal {signal_id} is outside the pure P5.2 node set: {expression}"
            ),
            Self::InvalidControlDependency { signal_id } => {
                write!(f, "control lowering reached sample signal {signal_id}")
            }
            Self::PureCycle { signal_id, region } => {
                write!(f, "pure signal cycle at signal {signal_id} in {region:?}")
            }
            Self::InputIndexOutOfRange { index, num_inputs } => {
                write!(f, "input index {index} is outside num_inputs={num_inputs}")
            }
            Self::FirTypeMismatch {
                signal_id,
                expected,
                actual,
            } => write!(
                f,
                "signal {signal_id} FIR type {actual:?} does not match {expected:?}"
            ),
            Self::CseRootCoverage { region } => {
                write!(f, "CSE changed root-sink coverage in {region:?}")
            }
            Self::BodyEvidence { detail } => {
                write!(f, "routed region-body verification failed: {detail}")
            }
        }
    }
}

impl std::error::Error for PureVectorLowerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Route(error) => Some(error),
            _ => None,
        }
    }
}

impl From<VectorRouteError> for PureVectorLowerError {
    fn from(value: VectorRouteError) -> Self {
        Self::Route(value)
    }
}

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
    };
    lower_vector_program_impl(prepared, verified_plan, None, None, &context)
}

/// Shared immutable configuration for one vector-region lowering pipeline.
///
/// State and clock certificates remain explicit arguments because they are
/// independently verified artifacts. This context groups the execution policy
/// and module-interface parameters consumed throughout lowering.
pub struct VectorLoweringContext<'a> {
    /// Canonical grouped UI program associated with the prepared forest.
    pub ui: &'a ui::UiProgram,
    /// Per-epoch scheduling strategy.
    pub strategy: SchedulingStrategy,
    /// Internal FIR real type.
    pub real_type: FirType,
    /// Number of audio inputs exposed by the module contract.
    pub num_inputs: usize,
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

fn lower_vector_program_impl<'a>(
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
    let (mut control_statements, rewritten_control_values) =
        materialize_region_roots(&mut lowerer.store, &control_values, VectorRegion::Control)?;
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

    control_statements.splice(0..0, lowerer.input_declarations.iter().copied());
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
    verify_pure_vector_bodies(
        verified_plan.plan(),
        &routed,
        &transport_declarations,
        &control_statements,
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
                    return self.lower_dep(scope, body, cache, active);
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
                let _ = self.lower_dep(scope, attached, cache, active)?;
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
            SigMatch::Clocked(_, inner)
            | SigMatch::TempVar(inner)
            | SigMatch::PermVar(inner)
            | SigMatch::ZeroPad(inner, _) => self.lower_dep(scope, inner, cache, active)?,
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
        let zone = super::vector_ui::control_zone(self.ui, control).map_err(|expression| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression,
            }
        })?;
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
        let zone = super::vector_ui::control_zone(self.ui, control).map_err(|expression| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id,
                expression,
            }
        })?;
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
        let zone = super::vector_ui::control_zone(self.ui, control).map_err(|expression| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression,
            }
        })?;
        if zone.kind != expected {
            return Err(PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression: format!(
                    "UI control {control} kind mismatch: expected {expected:?}, got {:?}",
                    zone.kind
                ),
            });
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
        let zone = super::vector_ui::control_zone(self.ui, control).map_err(|expression| {
            PureVectorLowerError::UnsupportedSignal {
                signal_id: u64::from(control),
                expression,
            }
        })?;
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

fn collect_prepared_ids(prepared: &VerifiedPreparedSignals) -> BTreeMap<u64, SigId> {
    let mut ids = BTreeMap::new();
    let mut stack = prepared.outputs().to_vec();
    while let Some(id) = stack.pop() {
        if ids.insert(u64::from(id.as_u32()), id).is_some() {
            continue;
        }
        if let Some(children) = prepared.arena().children(id) {
            stack.extend(children.iter().copied());
        }
    }
    ids
}

fn verify_plan_prepared_boundary(
    prepared: &VerifiedPreparedSignals,
    ui: &ui::UiProgram,
    plan: &VectorPlan,
    ids: &BTreeMap<u64, SigId>,
    state_plan: Option<&VerifiedVectorStatePlan>,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), PureVectorLowerError> {
    let mut managed_resources = state_plan
        .map(VerifiedVectorStatePlan::managed_resources)
        .unwrap_or_default();
    if let Some(clock_plan) = clock_plan {
        managed_resources.extend(clock_plan.managed_state_resources());
    }
    let output_channels = ids
        .values()
        .filter_map(|&sig| match match_sig(prepared.arena(), sig) {
            SigMatch::Output(channel, _) if channel >= 0 => {
                Some(u32::try_from(channel).expect("nonnegative output channel fits u32"))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for record in &plan.signals {
        let sig = ids.get(&record.signal_id).copied().ok_or(
            PureVectorLowerError::MissingPreparedSignal {
                signal_id: record.signal_id,
            },
        )?;
        let prepared_type = prepared.ty(sig);
        let matches = match (&record.value_type, prepared_type) {
            (ValueType::Int, Some(SimpleSigType::Int))
            | (ValueType::Real, Some(SimpleSigType::Real))
            | (ValueType::Sound, Some(SimpleSigType::Sound)) => true,
            (ValueType::Tuple(_), _) => {
                decode_symbolic_group_bodies(prepared.arena(), sig).is_some()
                    || match_sym_ref(prepared.arena(), sig).is_some()
            }
            _ => false,
        };
        if !matches {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                eprintln!(
                    "[vector-lower-type-mismatch] signal={} planned={:?} prepared={prepared_type:?} expr={}",
                    record.signal_id,
                    record.value_type,
                    dump_sig_readable(prepared.arena(), sig)
                );
            }
            return Err(PureVectorLowerError::PlannedTypeMismatch {
                signal_id: record.signal_id,
                planned: record.value_type.clone(),
                prepared: prepared_type,
            });
        }
        let effects_supported = record.effects.iter().all(|effect| match effect {
            EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource) => {
                managed_resources.contains(resource)
            }
            EffectAtom::ReadTable(table) | EffectAtom::WriteTable(table) => {
                readonly_table_signal(prepared, ids, u64::from(*table))
                    || mutable_table_signal(prepared, ids, u64::from(*table))
            }
            EffectAtom::WriteOutput(channel) => output_channels.contains(channel),
            EffectAtom::WriteUi(control) => ui.control(*control).is_some_and(|spec| {
                matches!(
                    spec.kind,
                    ui::ControlKind::VBargraph | ui::ControlKind::HBargraph
                ) && spec.id == *control
            }),
            _ => false,
        });
        if !record.effects.is_empty() && !effects_supported {
            return Err(PureVectorLowerError::EffectfulSignal {
                signal_id: record.signal_id,
                expression: dump_sig_readable(prepared.arena(), sig),
                effects: record.effects.to_vec(),
            });
        }
    }
    Ok(())
}

/// Canonical DSP-struct field name for one mutable table.
///
/// Shared by the lowerer and the final-module verifier so the emitted
/// declaration, the compute stores, and the attribution check cannot drift
/// onto different names.
pub(super) fn mutable_table_name(signal_id: u64, elem_type: &FirType) -> String {
    let prefix = if *elem_type == FirType::Int32 {
        "iVecMutTbl"
    } else {
        "fVecMutTbl"
    };
    format!("{prefix}{signal_id}")
}

fn mutable_table_signal(
    prepared: &VerifiedPreparedSignals,
    ids: &BTreeMap<u64, SigId>,
    signal_id: u64,
) -> bool {
    ids.get(&signal_id).is_some_and(|&sig| {
        matches!(
            match_sig(prepared.arena(), sig),
            SigMatch::WrTbl(_, _, write_index, write_value)
                if !wrtbl_is_readonly(prepared.arena(), write_index, write_value)
        )
    })
}

fn readonly_table_signal(
    prepared: &VerifiedPreparedSignals,
    ids: &BTreeMap<u64, SigId>,
    signal_id: u64,
) -> bool {
    ids.get(&signal_id).is_some_and(|&sig| {
        matches!(
            match_sig(prepared.arena(), sig),
            SigMatch::WrTbl(_, _, write_index, write_value)
                if prepared.arena().is_nil(write_index)
                    && prepared.arena().is_nil(write_value)
        )
    })
}

fn value_type_to_fir(value_type: &ValueType, real_type: &FirType) -> Option<FirType> {
    match value_type {
        ValueType::Int => Some(FirType::Int32),
        ValueType::Real => Some(real_type.clone()),
        ValueType::Sound => Some(FirType::Sound),
        ValueType::Tuple(_) => Some(value_fir_type(value_type, real_type.clone())),
    }
}

fn materialize_region_roots(
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

fn materialize_region_roots_with_prefix(
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

/// Independently reconnects final region bodies to the accepted P5.1 route.
pub fn verify_pure_vector_bodies(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    transport_declarations: &[FirId],
    control_statements: &[FirId],
    regions: &[PureVectorRegionBody],
    state_plan: Option<&VerifiedVectorStatePlan>,
    store: &FirStore,
) -> Result<(), PureVectorLowerError> {
    // Prefix writes are assembled from the accepted state plan after this
    // region-body check. Their routed input therefore has an explicit checked
    // consumer even though it is not reachable from the prefix read value.
    let state_consumed_uses = state_plan
        .into_iter()
        .flat_map(|state| state.plan().prefixes.iter())
        .map(|transition| (transition.loop_id, transition.value_signal_id))
        .collect::<BTreeSet<_>>();
    let expected_order = routed
        .layout()
        .loops()
        .iter()
        .map(|region| region.loop_id)
        .collect::<Vec<_>>();
    let actual_order = regions
        .iter()
        .map(PureVectorRegionBody::loop_id)
        .collect::<Vec<_>>();
    if actual_order != expected_order {
        return Err(PureVectorLowerError::BodyEvidence {
            detail: "region order differs from the verified schedule".to_owned(),
        });
    }
    let routed_transports = routed.trace().transports();
    if transport_declarations.len() != plan.transports.len()
        || routed_transports.len() != plan.transports.len()
    {
        return Err(PureVectorLowerError::BodyEvidence {
            detail: "transport declaration coverage differs from the plan".to_owned(),
        });
    }
    for (index, transport) in plan.transports.iter().enumerate() {
        if transport_declarations[index] != routed_transports[index].declaration {
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "transport {} declaration is not authoritative",
                    transport.transport_id
                ),
            });
        }
        let producer = region_by_id(regions, transport.producer_loop)?;
        let consumer = region_by_id(regions, transport.consumer_loop)?;
        let store_id =
            routed_transports[index]
                .store
                .ok_or_else(|| PureVectorLowerError::BodyEvidence {
                    detail: format!("transport {} has no producer store", transport.transport_id),
                })?;
        let load_id =
            routed_transports[index]
                .load
                .ok_or_else(|| PureVectorLowerError::BodyEvidence {
                    detail: format!("transport {} has no consumer load", transport.transport_id),
                })?;
        if producer
            .statements
            .iter()
            .filter(|&&id| id == store_id)
            .count()
            != 1
        {
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "transport {} store is not emitted exactly once",
                    transport.transport_id
                ),
            });
        }
        if !body_contains(store, &consumer.statements, load_id)
            && !body_contains_equivalent_table_load(store, &consumer.statements, load_id)
            && !body_contains_equivalent_scalar_load(store, &consumer.statements, load_id)
            && !state_consumed_uses.contains(&(transport.consumer_loop, transport.signal_id))
        {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                for statement in &consumer.statements {
                    eprintln!(
                        "[vector-lower-consumer-body] loop={} {}",
                        transport.consumer_loop,
                        fir::dump_fir(store, *statement)
                    );
                }
            }
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "transport {} for signal {} load is absent from its consumer body",
                    transport.transport_id, transport.signal_id
                ),
            });
        }
    }
    for definition in routed.trace().definitions() {
        let visible = match definition.region {
            VectorRegion::Control => body_contains(store, control_statements, definition.value),
            VectorRegion::Loop(loop_id) => body_contains(
                store,
                &region_by_id(regions, loop_id)?.statements,
                definition.value,
            ),
        };
        let structural_tuple = plan
            .signals
            .iter()
            .find(|signal| signal.signal_id == definition.signal_id)
            .is_some_and(|signal| {
                matches!(signal.value_type, ValueType::Tuple(_))
                    && !plan
                        .transports
                        .iter()
                        .any(|transport| transport.signal_id == definition.signal_id)
            });
        // Region-local CSE may rebuild an inline expression around temporary
        // loads, so its pre-CSE FIR id need not remain reachable. It is safe to
        // accept only non-transported `Inline` values here: owned and routed
        // definitions retain exact independent visibility requirements.
        let cse_local_inline = plan
            .signals
            .iter()
            .find(|signal| signal.signal_id == definition.signal_id)
            .is_some_and(|signal| signal.placement == Placement::Inline)
            && !plan
                .transports
                .iter()
                .any(|transport| transport.signal_id == definition.signal_id);
        if !visible && !structural_tuple && !cse_local_inline {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                eprintln!(
                    "[vector-lower-missing-definition] signal={} region={:?} fir={}",
                    definition.signal_id,
                    definition.region,
                    fir::dump_fir(store, definition.value)
                );
            }
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "signal {} definition is absent from {:?}",
                    definition.signal_id, definition.region
                ),
            });
        }
    }
    for routed_use in routed.trace().uses() {
        if let RoutedUseSource::Transport(transport_id) = routed_use.source {
            let consumer = region_by_id(regions, routed_use.consumer_loop)?;
            let transport_load = routed
                .trace()
                .transports()
                .iter()
                .find(|transport| transport.transport_id == transport_id)
                .and_then(|transport| transport.load);
            if !body_contains(store, &consumer.statements, routed_use.value)
                && transport_load.is_none_or(|load| {
                    !body_contains_equivalent_table_load(store, &consumer.statements, load)
                        && !body_contains_equivalent_scalar_load(store, &consumer.statements, load)
                })
                && !state_consumed_uses.contains(&(routed_use.consumer_loop, routed_use.signal_id))
            {
                return Err(PureVectorLowerError::BodyEvidence {
                    detail: format!(
                        "signal {} routed load is absent from loop {}",
                        routed_use.signal_id, routed_use.consumer_loop
                    ),
                });
            }
        }
    }
    Ok(())
}

fn region_by_id(
    regions: &[PureVectorRegionBody],
    loop_id: u64,
) -> Result<&PureVectorRegionBody, PureVectorLowerError> {
    regions
        .iter()
        .find(|region| region.loop_id == loop_id)
        .ok_or_else(|| PureVectorLowerError::BodyEvidence {
            detail: format!("missing loop body {loop_id}"),
        })
}

fn body_contains(store: &FirStore, roots: &[FirId], needle: FirId) -> bool {
    let mut stack = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        if value == needle {
            return true;
        }
        if seen.insert(value) {
            stack.extend(fir_children(store, value));
        }
    }
    false
}

fn body_contains_equivalent_table_load(store: &FirStore, roots: &[FirId], expected: FirId) -> bool {
    let FirMatch::LoadTable {
        name: expected_name,
        access: expected_access,
        typ: expected_type,
        ..
    } = match_fir(store, expected)
    else {
        return false;
    };
    let mut stack = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        if !seen.insert(value) {
            continue;
        }
        if matches!(
            match_fir(store, value),
            FirMatch::LoadTable { name, access, typ, .. }
                if name == expected_name && access == expected_access && typ == expected_type
        ) {
            return true;
        }
        stack.extend(fir_children(store, value));
    }
    false
}

fn body_contains_equivalent_scalar_load(
    store: &FirStore,
    roots: &[FirId],
    expected: FirId,
) -> bool {
    let FirMatch::LoadVar {
        name: expected_name,
        access: expected_access,
        typ: expected_type,
    } = match_fir(store, expected)
    else {
        return false;
    };
    let mut stack = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        if !seen.insert(value) {
            continue;
        }
        if matches!(
            match_fir(store, value),
            FirMatch::LoadVar { name, access, typ }
                if name == expected_name && access == expected_access && typ == expected_type
        ) {
            return true;
        }
        stack.extend(fir_children(store, value));
    }
    false
}

fn fir_children(store: &FirStore, value: FirId) -> Vec<FirId> {
    match match_fir(store, value) {
        FirMatch::BinOp { lhs, rhs, .. } => vec![lhs, rhs],
        FirMatch::Neg { value, .. }
        | FirMatch::Cast { value, .. }
        | FirMatch::Bitcast { value, .. }
        | FirMatch::Drop(value) => vec![value],
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => vec![cond, then_value, else_value],
        FirMatch::FunCall { args, .. } => args,
        FirMatch::LoadTable { index, .. } => vec![index],
        FirMatch::LoadSoundfileLength { part, .. } | FirMatch::LoadSoundfileRate { part, .. } => {
            vec![part]
        }
        FirMatch::LoadSoundfileBuffer {
            chan, part, idx, ..
        } => vec![chan, part, idx],
        FirMatch::DeclareVar { init, .. } => init.into_iter().collect(),
        FirMatch::StoreTable { index, value, .. } => vec![index, value],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use propagate::ClockDomainTable;
    use signals::SigBuilder;
    use sigtype::{Nature, Variability, Vectorability as SigVectorability};
    use tlib::TreeArena;

    use super::*;
    use crate::clk_env::annotate;
    use crate::signal_fir::decoration_verify::{CanonicalSigType, certify_decorations};
    use crate::signal_fir::vector_plan::verified_vector_plan_for_test;
    use crate::signal_fir::vector_verify::{
        EpochRecord, LoopEdge, LoopKind, LoopRecord, Rate, TransportRecord, VecSafeWitness,
        VectorPlan, Vectorability, WitnessKind,
    };
    use crate::signal_prepare::prepare_signals_for_fir_verified;

    fn pure_fixture() -> (VerifiedPreparedSignals, VerifiedVectorPlan) {
        let mut arena = TreeArena::new();
        let root = {
            let mut builder = SigBuilder::new(&mut arena);
            let input0 = builder.input(0);
            let input1 = builder.input(1);
            let sum = builder.binop(BinOp::Add, input0, input1);
            let producer = builder.atan2(sum, sum);
            builder.binop(BinOp::Add, producer, input0)
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
        let producer =
            find_repeated_atan2(prepared.arena(), prepared.outputs()[0]).unwrap_or_else(|| {
                panic!(
                    "prepared pure fixture changed shape: {}",
                    dump_sig_readable(prepared.arena(), prepared.outputs()[0])
                )
            });
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        let consumer = prepared.outputs()[0];
        let signals = decorations
            .certificate()
            .records
            .iter()
            .map(|record| SignalRecord {
                signal_id: u64::from(record.signal_id),
                value_type: canonical_value_type(&record.sig_type),
                structural: record.is_symbolic_recursion_carrier,
                rate: match record.variability {
                    Variability::Konst => Rate::Konst,
                    Variability::Block => Rate::Block,
                    Variability::Samp => Rate::Samp,
                },
                vectorability: match record.vectorability {
                    SigVectorability::Vect => Vectorability::Vect,
                    SigVectorability::Scal => Vectorability::Scal,
                    SigVectorability::TrueScal => Vectorability::TrueScal,
                },
                clock_id: 0,
                effects: record.effects.clone(),
                direct_effects: record.direct_effects.clone(),
                placement: if record.signal_id == producer.as_u32() {
                    Placement::Owned(0)
                } else if record.signal_id == consumer.as_u32() {
                    Placement::Owned(1)
                } else if record.variability == Variability::Samp {
                    Placement::Inline
                } else {
                    Placement::Control
                },
                duplicable: true,
            })
            .collect();
        let plan = verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 16,
            signals,
            loops: vec![
                LoopRecord {
                    loop_id: 0,
                    stable_name: "loop_pure_producer".to_owned(),
                    kind: LoopKind::Vectorizable,
                    roots: vec![u64::from(producer.as_u32())],
                    epoch_id: 0,
                },
                LoopRecord {
                    loop_id: 1,
                    stable_name: "loop_pure_consumer".to_owned(),
                    kind: LoopKind::Vectorizable,
                    roots: vec![u64::from(consumer.as_u32())],
                    epoch_id: 0,
                },
            ],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1],
            }],
            transports: vec![TransportRecord {
                transport_id: 0,
                stable_name: "transport_pure_producer".to_owned(),
                signal_id: u64::from(producer.as_u32()),
                producer_loop: 0,
                consumer_loop: 1,
                element_type: ValueType::Real,
                length: 16,
                layout: crate::signal_fir::vector_verify::TransportLayout::Planar,
            }],
            data_edges: vec![LoopEdge {
                consumer: 1,
                dependency: 0,
            }],
            effect_edges: vec![],
            vec_safe_witnesses: vec![
                VecSafeWitness {
                    loop_id: 0,
                    witness_kind: WitnessKind::Pointwise,
                },
                VecSafeWitness {
                    loop_id: 1,
                    witness_kind: WitnessKind::Pointwise,
                },
            ],
            fused_serial_groups: vec![],
        });
        (prepared, plan)
    }

    fn find_repeated_atan2(arena: &TreeArena, root: SigId) -> Option<SigId> {
        let mut stack = vec![root];
        let mut seen = BTreeSet::new();
        while let Some(signal) = stack.pop() {
            if !seen.insert(signal) {
                continue;
            }
            if matches!(
                match_sig(arena, signal),
                SigMatch::Atan2(lhs, rhs) if lhs == rhs
            ) {
                return Some(signal);
            }
            if let Some(children) = arena.children(signal) {
                stack.extend(children.iter().copied());
            }
        }
        None
    }

    fn canonical_value_type(sig_type: &CanonicalSigType) -> ValueType {
        match sig_type {
            CanonicalSigType::Sound => ValueType::Sound,
            CanonicalSigType::Simple { nature, .. } => match nature {
                Nature::Int => ValueType::Int,
                Nature::Real | Nature::Any => ValueType::Real,
            },
            CanonicalSigType::Table { content, .. } => canonical_value_type(content),
            CanonicalSigType::Tuplet { components, .. } => {
                ValueType::Tuple(components.iter().map(canonical_value_type).collect())
            }
        }
    }

    #[test]
    fn lowers_actual_pure_closures_with_local_cse_and_transport() {
        let (prepared, plan) = pure_fixture();
        let program = lower_pure_vector_program(
            &prepared,
            &plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            2,
        )
        .unwrap();
        assert_eq!(program.regions().len(), 2);
        assert!(program.regions()[0].statements().iter().any(|statement| {
            matches!(
                match_fir(program.store(), *statement),
                FirMatch::DeclareVar { name, .. } if name == "fVecL0Temp0"
            )
        }));
        assert!(!program.regions()[1].statements().iter().any(|statement| {
            matches!(
                match_fir(program.store(), *statement),
                FirMatch::DeclareVar { name, .. } if name.starts_with("fVecL0Temp")
            )
        }));
        assert!(body_contains(
            program.store(),
            program.regions()[1].statements(),
            program.routed().trace().transports()[0].load.unwrap()
        ));
        assert_eq!(program.routed().trace().transports().len(), 1);
    }

    #[test]
    fn all_scheduling_strategies_keep_region_local_storage_names() {
        let (prepared, plan) = pure_fixture();
        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let program =
                lower_pure_vector_program(&prepared, &plan, strategy, FirType::Float64, 2).unwrap();
            assert_eq!(program.routed().trace().transports()[0].transport_id, 0);
            assert!(program.regions().iter().any(|region| {
                region.statements().iter().any(|statement| {
                    matches!(
                        match_fir(program.store(), *statement),
                        FirMatch::DeclareVar { name, .. } if name == "fVecL0Temp0"
                    )
                })
            }));
        }
    }

    #[test]
    fn final_body_verifier_rejects_a_missing_consumer_body() {
        let (prepared, plan) = pure_fixture();
        let program = lower_pure_vector_program(
            &prepared,
            &plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            2,
        )
        .unwrap();
        let mut regions = program.regions().to_vec();
        regions[1].statements.clear();
        assert!(matches!(
            verify_pure_vector_bodies(
                plan.plan(),
                program.routed(),
                program.transport_declarations(),
                program.control_statements(),
                &regions,
                None,
                program.store()
            ),
            Err(PureVectorLowerError::BodyEvidence { .. })
        ));
    }

    #[test]
    fn body_transport_load_evidence_survives_cse_index_rewriting() {
        let mut store = FirStore::new();
        let expected = {
            let mut builder = FirBuilder::new(&mut store);
            let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
            builder.load_table(
                "transport_s1_l0_l1",
                AccessType::Stack,
                i0,
                FirType::Float32,
            )
        };
        let actual = {
            let mut builder = FirBuilder::new(&mut store);
            let index = builder.load_var("iTemp0", AccessType::Stack, FirType::Int32);
            let load = builder.load_table(
                "transport_s1_l0_l1",
                AccessType::Stack,
                index,
                FirType::Float32,
            );
            builder.drop_(load)
        };
        assert!(body_contains_equivalent_table_load(
            &store,
            &[actual],
            expected
        ));

        let wrong = {
            let mut builder = FirBuilder::new(&mut store);
            let index = builder.load_var("iTemp0", AccessType::Stack, FirType::Int32);
            let load = builder.load_table(
                "transport_s2_l0_l1",
                AccessType::Stack,
                index,
                FirType::Float32,
            );
            builder.drop_(load)
        };
        assert!(!body_contains_equivalent_table_load(
            &store,
            &[wrong],
            expected
        ));
    }

    #[test]
    fn fused_scalar_transport_load_evidence_survives_cse_rebuilding() {
        let mut store = FirStore::new();
        let expected = FirBuilder::new(&mut store).load_var(
            "transport_s1_l0_l1",
            AccessType::Stack,
            FirType::Float32,
        );
        let actual_load = FirBuilder::new(&mut store).load_var(
            "transport_s1_l0_l1",
            AccessType::Stack,
            FirType::Float32,
        );
        let actual = FirBuilder::new(&mut store).drop_(actual_load);
        assert!(body_contains_equivalent_scalar_load(
            &store,
            &[actual],
            expected
        ));

        let wrong_load = FirBuilder::new(&mut store).load_var(
            "transport_s2_l0_l1",
            AccessType::Stack,
            FirType::Float32,
        );
        let wrong = FirBuilder::new(&mut store).drop_(wrong_load);
        assert!(!body_contains_equivalent_scalar_load(
            &store,
            &[wrong],
            expected
        ));
    }

    #[test]
    fn stateful_signal_fails_closed_before_region_lowering() {
        let mut arena = TreeArena::new();
        let root = {
            let mut builder = SigBuilder::new(&mut arena);
            let input = builder.input(0);
            builder.delay1(input)
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        let plan = crate::signal_fir::vector_plan::build_vector_plan(&decorations, 16).unwrap();
        assert!(matches!(
            lower_pure_vector_program(
                &prepared,
                &plan,
                SchedulingStrategy::DepthFirst,
                FirType::Float32,
                1
            ),
            Err(PureVectorLowerError::EffectfulSignal { .. })
                | Err(PureVectorLowerError::UnsupportedSignal { .. })
        ));
    }
}
