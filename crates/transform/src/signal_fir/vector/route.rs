//! P5.1 region-aware vector routing and routed-FIR verification.
//!
//! # C++ provenance and adaptation
//! C++ `DAGInstructionsCompiler` combines loop ownership, value caching, and
//! vector buffer allocation while recursively compiling signals. Rust keeps
//! those concerns explicit: P4.4 freezes a [`VectorPlan`], this module resolves
//! values through three visibility scopes, and later lowering emits the loop
//! bodies. A cross-loop read can only use a transport already named by P4.4;
//! no buffer identity is allocated on demand.
//!
//! C++ compiles a recursive tuple through its individual `sigProj` values; it
//! does not allocate an inter-loop array of tuple objects. Rust retains the
//! tuple as a canonical typed `ValueArray` in routing evidence so simultaneous
//! recursion can be checked, but rejects tuple transports: only the scalar
//! projections may cross loop boundaries. The checker recursively validates
//! tuple arity and component types instead of trusting the outer FIR type.
//!
//! This is an additive P5 routing gate. It emits real FIR declarations,
//! stores, and loads for planned transports and independently verifies them,
//! but it is not connected to `build_module` yet. When a checked P6.2 plan is
//! supplied, declarations and accesses use its exact outer-chunk,
//! island-scalar, or held-output lifetime; P6.3b places those words in the
//! corresponding final region bodies.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

use crate::schedule::SchedulingStrategy;

use super::vector_clock_ad::{ClockTransportMode, ClockTransportPolicy, VerifiedVectorClockAdPlan};
use super::vector_plan::VerifiedVectorPlan;
use super::vector_schedule::{VectorScheduleError, schedule_verified_vector_plan};
use super::vector_verify::{
    LoopKind, Placement, TransportRecord, ValueType, VectorPlan, VectorPlanError,
    verify_vector_plan,
};

/// Lexical visibility scope used by the P5 value cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorRegion {
    /// Fixed control/lifecycle scope, visible from every loop.
    Control,
    /// One vector-plan loop region.
    Loop(u64),
}

/// One materialized loop region in strategy-dependent execution order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorLoopRegion {
    /// Stable loop id from the vector plan.
    pub loop_id: u64,
    /// Epoch containing the loop.
    pub epoch_id: u64,
    /// Dependency rank of the epoch.
    pub epoch_rank: u64,
    /// Vectorizable, recursive, or scalar-island loop classification.
    pub kind: LoopKind,
    /// Canonically ordered signal roots owned by the loop.
    pub roots: Vec<u64>,
}

/// Scheduled region layout. The plan remains strategy independent; only this
/// ordered projection varies with `-ss`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorRegionLayout {
    strategy: SchedulingStrategy,
    loops: Vec<VectorLoopRegion>,
}

impl VectorRegionLayout {
    #[must_use]
    pub fn strategy(&self) -> SchedulingStrategy {
        self.strategy
    }

    #[must_use]
    pub fn loops(&self) -> &[VectorLoopRegion] {
        &self.loops
    }
}

/// Result of one region-aware cache lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteResolution {
    /// A visible FIR value, direct or loaded from a planned transport.
    Value(FirId),
    /// An `Inline` signal has no value in this exact loop yet and must be
    /// lowered locally before being recorded with [`VectorRouteSession::define_in_loop`].
    NeedsInlineLowering,
}

/// Source selected for one recorded signal use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutedUseSource {
    Direct(VectorRegion),
    Transport(u64),
}

/// One FIR definition recorded at its legal visibility scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutedDefinition {
    /// Stable signal id.
    pub signal_id: u64,
    /// Region in which the FIR value is defined.
    pub region: VectorRegion,
    /// FIR value implementing the signal in that region.
    pub value: FirId,
}

/// One cache resolution made from a loop region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutedUse {
    /// Stable signal id.
    pub signal_id: u64,
    /// Loop reading the signal.
    pub consumer_loop: u64,
    /// Visibility path selected for the read.
    pub source: RoutedUseSource,
    /// Direct value or concrete transport load.
    pub value: FirId,
}

/// Concrete FIR evidence for one P4.4 transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedTransport {
    /// Stable transport id from P4.4.
    pub transport_id: u64,
    /// Concrete lifetime/indexing policy selected before FIR construction.
    pub mode: ClockTransportMode,
    /// Stack-array declaration with the canonical transport name and type.
    pub declaration: FirId,
    /// Value defined by the producer loop.
    pub producer_value: Option<FirId>,
    /// Producer-side table store.
    pub store: Option<FirId>,
    /// Consumer-side table load.
    pub load: Option<FirId>,
}

/// Canonical routed-FIR trace checked after lowering and before emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedFirTrace {
    definitions: Vec<RoutedDefinition>,
    uses: Vec<RoutedUse>,
    transports: Vec<RoutedTransport>,
}

impl RoutedFirTrace {
    #[must_use]
    pub fn definitions(&self) -> &[RoutedDefinition] {
        &self.definitions
    }

    #[must_use]
    pub fn uses(&self) -> &[RoutedUse] {
        &self.uses
    }

    #[must_use]
    pub fn transports(&self) -> &[RoutedTransport] {
        &self.transports
    }
}

/// Opaque evidence that [`verify_routed_fir`] accepted the routing trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRoutedFir {
    plan: VectorPlan,
    layout: VectorRegionLayout,
    trace: RoutedFirTrace,
}

impl VerifiedRoutedFir {
    /// Returns the exact verified plan whose stable identities own this route.
    #[must_use]
    pub fn plan(&self) -> &VectorPlan {
        &self.plan
    }

    #[must_use]
    pub fn layout(&self) -> &VectorRegionLayout {
        &self.layout
    }

    #[must_use]
    pub fn trace(&self) -> &RoutedFirTrace {
        &self.trace
    }

    #[must_use]
    pub fn into_trace(self) -> RoutedFirTrace {
        self.trace
    }
}

/// Typed P5 routing or routed-FIR verification failure.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorRouteError {
    Plan(VectorPlanError),
    Schedule(VectorScheduleError),
    UnknownSignal {
        signal_id: u64,
    },
    UnknownLoop {
        loop_id: u64,
    },
    WrongRegion {
        signal_id: u64,
        expected: Placement,
        actual: VectorRegion,
    },
    DuplicateDefinition {
        signal_id: u64,
        region: VectorRegion,
    },
    MissingDefinition {
        signal_id: u64,
        region: VectorRegion,
    },
    MissingInlineDefinition {
        signal_id: u64,
    },
    MissingTransport {
        signal_id: u64,
        producer_loop: u64,
        consumer_loop: u64,
    },
    UnsupportedTupleTransport {
        signal_id: u64,
    },
    TupleValueShape {
        signal_id: u64,
    },
    ValueTypeMismatch {
        signal_id: u64,
        expected: FirType,
        actual: Option<FirType>,
    },
    DefinitionCoverage {
        signal_id: u64,
    },
    TransportCoverage {
        transport_id: u64,
    },
    TransportDeclaration {
        transport_id: u64,
    },
    TransportStore {
        transport_id: u64,
    },
    TransportLoad {
        transport_id: u64,
    },
    InvalidUse {
        signal_id: u64,
        consumer_loop: u64,
    },
    ClockPlanMismatch,
    TransportPolicyCoverage {
        transport_id: u64,
    },
}

impl fmt::Display for VectorRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => write!(f, "vector plan verification failed: {error}"),
            Self::Schedule(error) => write!(f, "vector scheduling failed: {error}"),
            Self::UnknownSignal { signal_id } => write!(f, "unknown signal {signal_id}"),
            Self::UnknownLoop { loop_id } => write!(f, "unknown loop {loop_id}"),
            Self::WrongRegion {
                signal_id,
                expected,
                actual,
            } => write!(
                f,
                "signal {signal_id} has placement {expected:?}, not region {actual:?}"
            ),
            Self::DuplicateDefinition { signal_id, region } => {
                write!(f, "signal {signal_id} is defined twice in {region:?}")
            }
            Self::MissingDefinition { signal_id, region } => {
                write!(f, "signal {signal_id} has no definition in {region:?}")
            }
            Self::MissingInlineDefinition { signal_id } => {
                write!(f, "inline signal {signal_id} was never lowered")
            }
            Self::MissingTransport {
                signal_id,
                producer_loop,
                consumer_loop,
            } => write!(
                f,
                "no planned transport for signal {signal_id} from loop {producer_loop} to loop {consumer_loop}"
            ),
            Self::UnsupportedTupleTransport { signal_id } => write!(
                f,
                "tuple signal {signal_id} must be routed through its scalar projections"
            ),
            Self::TupleValueShape { signal_id } => write!(
                f,
                "tuple value for signal {signal_id} is not a canonical typed component array"
            ),
            Self::ValueTypeMismatch {
                signal_id,
                expected,
                actual,
            } => write!(
                f,
                "signal {signal_id} FIR type {actual:?} does not match {expected:?}"
            ),
            Self::DefinitionCoverage { signal_id } => {
                write!(f, "routed definitions do not cover signal {signal_id}")
            }
            Self::TransportCoverage { transport_id } => {
                write!(
                    f,
                    "routed trace does not exactly cover transport {transport_id}"
                )
            }
            Self::TransportDeclaration { transport_id } => {
                write!(f, "transport {transport_id} declaration is invalid")
            }
            Self::TransportStore { transport_id } => {
                write!(
                    f,
                    "transport {transport_id} producer store is invalid or missing"
                )
            }
            Self::TransportLoad { transport_id } => {
                write!(
                    f,
                    "transport {transport_id} consumer load is invalid or missing"
                )
            }
            Self::InvalidUse {
                signal_id,
                consumer_loop,
            } => write!(
                f,
                "signal {signal_id} is not visible from loop {consumer_loop} through the recorded source"
            ),
            Self::ClockPlanMismatch => {
                write!(f, "clock/AD plan does not belong to the routed vector plan")
            }
            Self::TransportPolicyCoverage { transport_id } => write!(
                f,
                "clock transport policy does not exactly cover transport {transport_id}"
            ),
        }
    }
}

impl std::error::Error for VectorRouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            Self::Schedule(error) => Some(error),
            _ => None,
        }
    }
}

impl From<VectorPlanError> for VectorRouteError {
    fn from(value: VectorPlanError) -> Self {
        Self::Plan(value)
    }
}

impl From<VectorScheduleError> for VectorRouteError {
    fn from(value: VectorScheduleError) -> Self {
        Self::Schedule(value)
    }
}

#[derive(Default)]
struct VectorValueCache {
    control: BTreeMap<u64, FirId>,
    owned: BTreeMap<u64, (u64, FirId)>,
    inline: BTreeMap<(u64, u64), FirId>,
    transport_loads: BTreeMap<u64, FirId>,
}

/// Stateful P5 routing surface used by a signal lowerer.
pub struct VectorRouteSession<'a> {
    plan: &'a VectorPlan,
    layout: VectorRegionLayout,
    real_type: FirType,
    cache: VectorValueCache,
    trace: RoutedFirTrace,
    transport_by_route: BTreeMap<(u64, u64, u64), usize>,
    loop_ids: BTreeSet<u64>,
    transport_policies: Vec<ClockTransportPolicy>,
}

impl<'a> VectorRouteSession<'a> {
    /// Creates scheduled vector regions and emits every transport declaration.
    /// No transport is created after this operation.
    pub fn new(
        verified: &'a VerifiedVectorPlan,
        strategy: SchedulingStrategy,
        real_type: FirType,
        store: &mut FirStore,
    ) -> Result<(Self, Vec<FirId>), VectorRouteError> {
        let policies = verified
            .plan()
            .transports
            .iter()
            .map(|transport| ClockTransportPolicy {
                transport_id: transport.transport_id,
                mode: fused_transport_group(verified.plan(), transport.transport_id)
                    .map_or(ClockTransportMode::OuterChunk, |group_id| {
                        ClockTransportMode::FusedScalar { group_id }
                    }),
            })
            .collect::<Vec<_>>();
        Self::new_with_policies(verified, strategy, real_type, &policies, store)
    }

    /// Creates routes using the exact P6.2 transport lifetimes.
    pub fn new_with_clock_plan(
        verified: &'a VerifiedVectorPlan,
        clock_plan: &VerifiedVectorClockAdPlan,
        strategy: SchedulingStrategy,
        real_type: FirType,
        store: &mut FirStore,
    ) -> Result<(Self, Vec<FirId>), VectorRouteError> {
        if clock_plan.vector_plan() != verified.plan() {
            return Err(VectorRouteError::ClockPlanMismatch);
        }
        Self::new_with_policies(
            verified,
            strategy,
            real_type,
            &clock_plan.plan().transports,
            store,
        )
    }

    fn new_with_policies(
        verified: &'a VerifiedVectorPlan,
        strategy: SchedulingStrategy,
        real_type: FirType,
        policies: &[ClockTransportPolicy],
        store: &mut FirStore,
    ) -> Result<(Self, Vec<FirId>), VectorRouteError> {
        let plan = verified.plan();
        verify_policy_coverage(plan, policies)?;
        let schedule = schedule_verified_vector_plan(verified, strategy)?;
        let loop_by_id = plan
            .loops
            .iter()
            .map(|record| (record.loop_id, record))
            .collect::<BTreeMap<_, _>>();
        let mut loops = Vec::with_capacity(plan.loops.len());
        for epoch in &schedule.epochs {
            for &loop_id in &epoch.loops {
                let record = loop_by_id[&loop_id];
                loops.push(VectorLoopRegion {
                    loop_id,
                    epoch_id: epoch.epoch_id,
                    epoch_rank: epoch.rank,
                    kind: record.kind,
                    roots: record.roots.clone(),
                });
            }
        }
        let layout = VectorRegionLayout { strategy, loops };
        let mut declarations = Vec::with_capacity(plan.transports.len());
        let mut routed_transports = Vec::with_capacity(plan.transports.len());
        let mut transport_by_route = BTreeMap::new();
        for (index, (transport, policy)) in plan.transports.iter().zip(policies).enumerate() {
            let elem = transport_fir_type(transport, real_type.clone())?;
            let declaration = declare_transport(store, transport, policy.mode, elem)?;
            declarations.push(declaration);
            routed_transports.push(RoutedTransport {
                transport_id: transport.transport_id,
                mode: policy.mode,
                declaration,
                producer_value: None,
                store: None,
                load: None,
            });
            transport_by_route.insert(
                (
                    transport.signal_id,
                    transport.producer_loop,
                    transport.consumer_loop,
                ),
                index,
            );
        }
        let loop_ids = plan.loops.iter().map(|record| record.loop_id).collect();
        Ok((
            Self {
                plan,
                layout,
                real_type,
                cache: VectorValueCache::default(),
                trace: RoutedFirTrace {
                    definitions: Vec::new(),
                    uses: Vec::new(),
                    transports: routed_transports,
                },
                transport_by_route,
                loop_ids,
                transport_policies: policies.to_vec(),
            },
            declarations,
        ))
    }

    #[must_use]
    pub fn layout(&self) -> &VectorRegionLayout {
        &self.layout
    }

    /// Returns the immutable P4.4 plan that owns all route identities.
    #[must_use]
    pub fn plan(&self) -> &VectorPlan {
        self.plan
    }

    /// Records a control value, visible from every vector loop.
    pub fn define_control(
        &mut self,
        signal_id: u64,
        value: FirId,
        store: &FirStore,
    ) -> Result<(), VectorRouteError> {
        let signal = self.signal(signal_id)?;
        if signal.placement != Placement::Control {
            return Err(VectorRouteError::WrongRegion {
                signal_id,
                expected: signal.placement,
                actual: VectorRegion::Control,
            });
        }
        check_value_type(signal_id, &signal.value_type, &self.real_type, value, store)?;
        if self.cache.control.insert(signal_id, value).is_some() {
            return Err(VectorRouteError::DuplicateDefinition {
                signal_id,
                region: VectorRegion::Control,
            });
        }
        self.trace.definitions.push(RoutedDefinition {
            signal_id,
            region: VectorRegion::Control,
            value,
        });
        Ok(())
    }

    /// Records an `Inline` instance or the unique `Owned(loop_id)` producer.
    /// Returns all pre-planned transport stores that the producer loop must
    /// append immediately after computing this value.
    pub fn define_in_loop(
        &mut self,
        loop_id: u64,
        signal_id: u64,
        value: FirId,
        store: &mut FirStore,
    ) -> Result<Vec<FirId>, VectorRouteError> {
        self.ensure_loop(loop_id)?;
        let signal = self.signal(signal_id)?;
        check_value_type(signal_id, &signal.value_type, &self.real_type, value, store)?;
        match signal.placement {
            Placement::Inline => {
                if self
                    .cache
                    .inline
                    .insert((loop_id, signal_id), value)
                    .is_some()
                {
                    return Err(VectorRouteError::DuplicateDefinition {
                        signal_id,
                        region: VectorRegion::Loop(loop_id),
                    });
                }
            }
            Placement::Owned(owner) if owner == loop_id => {
                if self
                    .cache
                    .owned
                    .insert(signal_id, (loop_id, value))
                    .is_some()
                {
                    return Err(VectorRouteError::DuplicateDefinition {
                        signal_id,
                        region: VectorRegion::Loop(loop_id),
                    });
                }
            }
            placement => {
                return Err(VectorRouteError::WrongRegion {
                    signal_id,
                    expected: placement,
                    actual: VectorRegion::Loop(loop_id),
                });
            }
        }
        self.trace.definitions.push(RoutedDefinition {
            signal_id,
            region: VectorRegion::Loop(loop_id),
            value,
        });

        let outgoing = self
            .plan
            .transports
            .iter()
            .enumerate()
            .filter(|(_, transport)| {
                transport.signal_id == signal_id && transport.producer_loop == loop_id
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut stores = Vec::with_capacity(outgoing.len());
        for index in outgoing {
            let transport = &self.plan.transports[index];
            let routed = &mut self.trace.transports[index];
            if routed.store.is_some() {
                return Err(VectorRouteError::TransportStore {
                    transport_id: transport.transport_id,
                });
            }
            let statement =
                store_transport(store, transport, self.transport_policies[index].mode, value);
            routed.producer_value = Some(value);
            routed.store = Some(statement);
            stores.push(statement);
        }
        Ok(stores)
    }

    /// Resolves one signal use from `consumer_loop` according to P5 visibility.
    pub fn resolve_in_loop(
        &mut self,
        consumer_loop: u64,
        signal_id: u64,
        store: &mut FirStore,
    ) -> Result<RouteResolution, VectorRouteError> {
        self.ensure_loop(consumer_loop)?;
        let signal = self.signal(signal_id)?;
        let (resolution, source) = match signal.placement {
            Placement::Control => {
                let value = self.cache.control.get(&signal_id).copied().ok_or(
                    VectorRouteError::MissingDefinition {
                        signal_id,
                        region: VectorRegion::Control,
                    },
                )?;
                (
                    RouteResolution::Value(value),
                    RoutedUseSource::Direct(VectorRegion::Control),
                )
            }
            Placement::Inline => {
                let Some(value) = self.cache.inline.get(&(consumer_loop, signal_id)).copied()
                else {
                    return Ok(RouteResolution::NeedsInlineLowering);
                };
                (
                    RouteResolution::Value(value),
                    RoutedUseSource::Direct(VectorRegion::Loop(consumer_loop)),
                )
            }
            Placement::Owned(owner) if owner == consumer_loop => {
                let value = self
                    .cache
                    .owned
                    .get(&signal_id)
                    .map(|(_, value)| *value)
                    .ok_or(VectorRouteError::MissingDefinition {
                        signal_id,
                        region: VectorRegion::Loop(owner),
                    })?;
                (
                    RouteResolution::Value(value),
                    RoutedUseSource::Direct(VectorRegion::Loop(owner)),
                )
            }
            Placement::Owned(owner) => {
                if !self.cache.owned.contains_key(&signal_id) {
                    return Err(VectorRouteError::MissingDefinition {
                        signal_id,
                        region: VectorRegion::Loop(owner),
                    });
                }
                let index = self
                    .transport_by_route
                    .get(&(signal_id, owner, consumer_loop))
                    .copied()
                    .ok_or(VectorRouteError::MissingTransport {
                        signal_id,
                        producer_loop: owner,
                        consumer_loop,
                    })?;
                let transport = &self.plan.transports[index];
                let value = if let Some(value) = self
                    .cache
                    .transport_loads
                    .get(&transport.transport_id)
                    .copied()
                {
                    value
                } else {
                    let elem = transport_fir_type(transport, self.real_type.clone())?;
                    let value =
                        load_transport(store, transport, self.transport_policies[index].mode, elem);
                    self.cache
                        .transport_loads
                        .insert(transport.transport_id, value);
                    self.trace.transports[index].load = Some(value);
                    value
                };
                (
                    RouteResolution::Value(value),
                    RoutedUseSource::Transport(transport.transport_id),
                )
            }
        };
        let RouteResolution::Value(value) = resolution else {
            unreachable!()
        };
        self.trace.uses.push(RoutedUse {
            signal_id,
            consumer_loop,
            source,
            value,
        });
        Ok(resolution)
    }

    /// Closes the route and independently verifies FIR visibility and all
    /// planned transport declarations/store/load pairs.
    pub fn finish(mut self, store: &FirStore) -> Result<VerifiedRoutedFir, VectorRouteError> {
        self.trace
            .definitions
            .sort_by_key(|definition| (definition.region, definition.signal_id));
        verify_routed_fir_with_policies_after_plan(
            self.plan,
            &self.transport_policies,
            &self.trace,
            &self.real_type,
            store,
        )?;
        Ok(VerifiedRoutedFir {
            plan: self.plan.clone(),
            layout: self.layout,
            trace: self.trace,
        })
    }

    fn signal(
        &self,
        signal_id: u64,
    ) -> Result<&super::vector_verify::SignalRecord, VectorRouteError> {
        self.plan
            .signals
            .iter()
            .find(|signal| signal.signal_id == signal_id)
            .ok_or(VectorRouteError::UnknownSignal { signal_id })
    }

    fn ensure_loop(&self, loop_id: u64) -> Result<(), VectorRouteError> {
        if self.loop_ids.contains(&loop_id) {
            Ok(())
        } else {
            Err(VectorRouteError::UnknownLoop { loop_id })
        }
    }
}

/// Independently checks a complete P5 routed-FIR trace.
pub fn verify_routed_fir(
    plan: &VectorPlan,
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let policies = plan
        .transports
        .iter()
        .map(|transport| ClockTransportPolicy {
            transport_id: transport.transport_id,
            mode: fused_transport_group(plan, transport.transport_id)
                .map_or(ClockTransportMode::OuterChunk, |group_id| {
                    ClockTransportMode::FusedScalar { group_id }
                }),
        })
        .collect::<Vec<_>>();
    verify_routed_fir_with_policies(plan, &policies, trace, real_type, store)
}

/// Independently checks routed FIR against the exact P6.2 transport policy.
pub fn verify_routed_fir_with_clock_plan(
    plan: &VectorPlan,
    clock_plan: &VerifiedVectorClockAdPlan,
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    if clock_plan.vector_plan() != plan {
        return Err(VectorRouteError::ClockPlanMismatch);
    }
    verify_routed_fir_with_policies(plan, &clock_plan.plan().transports, trace, real_type, store)
}

fn verify_routed_fir_with_policies(
    plan: &VectorPlan,
    policies: &[ClockTransportPolicy],
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    verify_vector_plan(plan)?;
    verify_routed_fir_with_policies_after_plan(plan, policies, trace, real_type, store)
}

/// Checks route evidence relative to an already accepted opaque vector plan.
/// Public raw-plan checkers retain the full plan verification above; the
/// production session uses this boundary to avoid rechecking the same global
/// plan at route construction and route closure.
fn verify_routed_fir_with_policies_after_plan(
    plan: &VectorPlan,
    policies: &[ClockTransportPolicy],
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    verify_policy_coverage(plan, policies)?;
    let signal_by_id = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let loop_ids = plan
        .loops
        .iter()
        .map(|record| record.loop_id)
        .collect::<BTreeSet<_>>();

    let mut seen_definitions = BTreeSet::new();
    let mut signals_with_definition = BTreeSet::new();
    for definition in &trace.definitions {
        let signal = signal_by_id.get(&definition.signal_id).copied().ok_or(
            VectorRouteError::UnknownSignal {
                signal_id: definition.signal_id,
            },
        )?;
        if !seen_definitions.insert((definition.region, definition.signal_id)) {
            return Err(VectorRouteError::DuplicateDefinition {
                signal_id: definition.signal_id,
                region: definition.region,
            });
        }
        let legal = match (signal.placement, definition.region) {
            (Placement::Control, VectorRegion::Control) => true,
            (Placement::Inline, VectorRegion::Loop(loop_id)) => loop_ids.contains(&loop_id),
            (Placement::Owned(owner), VectorRegion::Loop(loop_id)) => owner == loop_id,
            _ => false,
        };
        if !legal {
            return Err(VectorRouteError::WrongRegion {
                signal_id: definition.signal_id,
                expected: signal.placement,
                actual: definition.region,
            });
        }
        check_value_type(
            definition.signal_id,
            &signal.value_type,
            real_type,
            definition.value,
            store,
        )?;
        signals_with_definition.insert(definition.signal_id);
    }
    for signal in &plan.signals {
        if !signals_with_definition.contains(&signal.signal_id) {
            // Tuple-valued inline records are structural recursion/group
            // carriers. Their scalar projections carry all executable FIR
            // definitions; the tuple identity itself has no runtime value.
            if signal.structural {
                continue;
            }
            return match signal.placement {
                Placement::Inline => Err(VectorRouteError::MissingInlineDefinition {
                    signal_id: signal.signal_id,
                }),
                Placement::Control | Placement::Owned(_) => {
                    Err(VectorRouteError::DefinitionCoverage {
                        signal_id: signal.signal_id,
                    })
                }
            };
        }
    }

    if trace.transports.len() != plan.transports.len() {
        let transport_id = plan
            .transports
            .get(trace.transports.len())
            .map_or(u64::MAX, |transport| transport.transport_id);
        return Err(VectorRouteError::TransportCoverage { transport_id });
    }
    for ((transport, policy), routed) in plan.transports.iter().zip(policies).zip(&trace.transports)
    {
        if routed.transport_id != transport.transport_id {
            return Err(VectorRouteError::TransportCoverage {
                transport_id: transport.transport_id,
            });
        }
        if routed.mode != policy.mode {
            return Err(VectorRouteError::TransportPolicyCoverage {
                transport_id: transport.transport_id,
            });
        }
        verify_transport(transport, policy.mode, routed, real_type, store)?;
        let producer_value_is_declared = trace.definitions.iter().any(|definition| {
            definition.signal_id == transport.signal_id
                && definition.region == VectorRegion::Loop(transport.producer_loop)
                && Some(definition.value) == routed.producer_value
        });
        if !producer_value_is_declared {
            return Err(VectorRouteError::TransportStore {
                transport_id: transport.transport_id,
            });
        }
        let load_is_consumed = trace.uses.iter().any(|routed_use| {
            routed_use.signal_id == transport.signal_id
                && routed_use.consumer_loop == transport.consumer_loop
                && routed_use.source == RoutedUseSource::Transport(transport.transport_id)
                && Some(routed_use.value) == routed.load
        });
        if !load_is_consumed {
            return Err(VectorRouteError::TransportLoad {
                transport_id: transport.transport_id,
            });
        }
    }

    for routed_use in &trace.uses {
        let signal = signal_by_id.get(&routed_use.signal_id).copied().ok_or(
            VectorRouteError::UnknownSignal {
                signal_id: routed_use.signal_id,
            },
        )?;
        if !loop_ids.contains(&routed_use.consumer_loop) {
            return Err(VectorRouteError::UnknownLoop {
                loop_id: routed_use.consumer_loop,
            });
        }
        let valid = match routed_use.source {
            RoutedUseSource::Direct(VectorRegion::Control) => {
                signal.placement == Placement::Control
                    && trace.definitions.iter().any(|definition| {
                        definition.signal_id == routed_use.signal_id
                            && definition.region == VectorRegion::Control
                            && definition.value == routed_use.value
                    })
            }
            RoutedUseSource::Direct(VectorRegion::Loop(source)) => {
                source == routed_use.consumer_loop
                    && (signal.placement == Placement::Inline
                        || signal.placement == Placement::Owned(source))
                    && trace.definitions.iter().any(|definition| {
                        definition.signal_id == routed_use.signal_id
                            && definition.region == VectorRegion::Loop(source)
                            && definition.value == routed_use.value
                    })
            }
            RoutedUseSource::Transport(transport_id) => plan.transports.iter().any(|transport| {
                transport.transport_id == transport_id
                    && transport.signal_id == routed_use.signal_id
                    && transport.consumer_loop == routed_use.consumer_loop
                    && trace.transports.iter().any(|routed| {
                        routed.transport_id == transport_id && routed.load == Some(routed_use.value)
                    })
            }),
        };
        if !valid {
            return Err(VectorRouteError::InvalidUse {
                signal_id: routed_use.signal_id,
                consumer_loop: routed_use.consumer_loop,
            });
        }
    }
    Ok(())
}

fn verify_transport(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    routed: &RoutedTransport,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let elem = transport_fir_type(transport, real_type.clone())?;
    verify_transport_declaration(transport, mode, routed.declaration, &elem, store)?;
    let Some(producer_value) = routed.producer_value else {
        return Err(VectorRouteError::TransportStore {
            transport_id: transport.transport_id,
        });
    };
    verify_transport_store(transport, mode, routed.store, producer_value, &elem, store)?;
    verify_transport_load(transport, mode, routed.load, &elem, store)
}

fn verify_policy_coverage(
    plan: &VectorPlan,
    policies: &[ClockTransportPolicy],
) -> Result<(), VectorRouteError> {
    if policies.len() != plan.transports.len() {
        let transport_id = plan
            .transports
            .get(policies.len())
            .map_or(u64::MAX, |transport| transport.transport_id);
        return Err(VectorRouteError::TransportPolicyCoverage { transport_id });
    }
    for (transport, policy) in plan.transports.iter().zip(policies) {
        if transport.transport_id != policy.transport_id {
            return Err(VectorRouteError::TransportPolicyCoverage {
                transport_id: transport.transport_id,
            });
        }
        let fused_group = fused_transport_group(plan, transport.transport_id);
        let fused_policy_group = match policy.mode {
            ClockTransportMode::FusedScalar { group_id } => Some(group_id),
            _ => None,
        };
        if fused_group != fused_policy_group {
            return Err(VectorRouteError::TransportPolicyCoverage {
                transport_id: transport.transport_id,
            });
        }
    }
    Ok(())
}

fn fused_transport_group(plan: &VectorPlan, transport_id: u64) -> Option<u64> {
    plan.fused_serial_groups.iter().find_map(|group| {
        group
            .internal_transport_ids
            .binary_search(&transport_id)
            .is_ok()
            .then_some(group.group_id)
    })
}

fn declare_transport(
    store: &mut FirStore,
    transport: &TransportRecord,
    mode: ClockTransportMode,
    elem: FirType,
) -> Result<FirId, VectorRouteError> {
    let (typ, access) = match mode {
        ClockTransportMode::OuterChunk => {
            let length = usize::try_from(transport.length).map_err(|_| {
                VectorRouteError::TransportDeclaration {
                    transport_id: transport.transport_id,
                }
            })?;
            (FirType::Array(Box::new(elem), length), AccessType::Stack)
        }
        ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. } => {
            (elem, AccessType::Stack)
        }
        ClockTransportMode::HeldOutput { .. } => (elem, AccessType::Struct),
    };
    Ok(FirBuilder::new(store).declare_var(transport.stable_name.clone(), typ, access, None))
}

fn store_transport(
    store: &mut FirStore,
    transport: &TransportRecord,
    mode: ClockTransportMode,
    value: FirId,
) -> FirId {
    match mode {
        ClockTransportMode::OuterChunk => {
            let index = chunk_index(store);
            FirBuilder::new(store).store_table(
                transport.stable_name.clone(),
                AccessType::Stack,
                index,
                value,
            )
        }
        ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. } => {
            FirBuilder::new(store).store_var(
                transport.stable_name.clone(),
                AccessType::Stack,
                value,
            )
        }
        ClockTransportMode::HeldOutput { .. } => FirBuilder::new(store).store_var(
            transport.stable_name.clone(),
            AccessType::Struct,
            value,
        ),
    }
}

fn load_transport(
    store: &mut FirStore,
    transport: &TransportRecord,
    mode: ClockTransportMode,
    elem: FirType,
) -> FirId {
    match mode {
        ClockTransportMode::OuterChunk => {
            let index = chunk_index(store);
            FirBuilder::new(store).load_table(
                transport.stable_name.clone(),
                AccessType::Stack,
                index,
                elem,
            )
        }
        ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. } => {
            FirBuilder::new(store).load_var(transport.stable_name.clone(), AccessType::Stack, elem)
        }
        ClockTransportMode::HeldOutput { .. } => {
            FirBuilder::new(store).load_var(transport.stable_name.clone(), AccessType::Struct, elem)
        }
    }
}

fn verify_transport_declaration(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    declaration: FirId,
    elem: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let valid = match (mode, match_fir(store, declaration)) {
        (
            ClockTransportMode::OuterChunk,
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(actual_elem, actual_length),
                access: AccessType::Stack,
                init: None,
            },
        ) => {
            usize::try_from(transport.length) == Ok(actual_length)
                && name == transport.stable_name
                && *actual_elem == *elem
        }
        (
            ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. },
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Stack,
                init: None,
            },
        ) => name == transport.stable_name && typ == *elem,
        (
            ClockTransportMode::HeldOutput { .. },
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Struct,
                init: None,
            },
        ) => name == transport.stable_name && typ == *elem,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(VectorRouteError::TransportDeclaration {
            transport_id: transport.transport_id,
        })
    }
}

fn verify_transport_store(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    statement: Option<FirId>,
    producer_value: FirId,
    elem: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let valid = match (mode, statement.map(|id| match_fir(store, id))) {
        (
            ClockTransportMode::OuterChunk,
            Some(FirMatch::StoreTable {
                name,
                access: AccessType::Stack,
                index,
                value,
            }),
        ) => {
            name == transport.stable_name && value == producer_value && is_chunk_index(store, index)
        }
        (
            ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. },
            Some(FirMatch::StoreVar {
                name,
                access: AccessType::Stack,
                value,
            }),
        )
        | (
            ClockTransportMode::HeldOutput { .. },
            Some(FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            }),
        ) => name == transport.stable_name && value == producer_value,
        _ => false,
    };
    if valid && store.value_type(producer_value) == Some(elem.clone()) {
        Ok(())
    } else {
        Err(VectorRouteError::TransportStore {
            transport_id: transport.transport_id,
        })
    }
}

fn verify_transport_load(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    value: Option<FirId>,
    elem: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let valid = match (mode, value.map(|id| (id, match_fir(store, id)))) {
        (
            ClockTransportMode::OuterChunk,
            Some((
                id,
                FirMatch::LoadTable {
                    name,
                    access: AccessType::Stack,
                    index,
                    typ,
                },
            )),
        ) => {
            name == transport.stable_name
                && typ == *elem
                && is_chunk_index(store, index)
                && store.value_type(id) == Some(elem.clone())
        }
        (
            ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. },
            Some((
                id,
                FirMatch::LoadVar {
                    name,
                    access: AccessType::Stack,
                    typ,
                },
            )),
        )
        | (
            ClockTransportMode::HeldOutput { .. },
            Some((
                id,
                FirMatch::LoadVar {
                    name,
                    access: AccessType::Struct,
                    typ,
                },
            )),
        ) => {
            name == transport.stable_name
                && typ == *elem
                && store.value_type(id) == Some(elem.clone())
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(VectorRouteError::TransportLoad {
            transport_id: transport.transport_id,
        })
    }
}

fn transport_fir_type(
    transport: &TransportRecord,
    real_type: FirType,
) -> Result<FirType, VectorRouteError> {
    if matches!(transport.element_type, ValueType::Tuple(_)) {
        Err(VectorRouteError::UnsupportedTupleTransport {
            signal_id: transport.signal_id,
        })
    } else {
        Ok(value_fir_type(&transport.element_type, real_type))
    }
}

pub(super) fn value_fir_type(value_type: &ValueType, real_type: FirType) -> FirType {
    match value_type {
        ValueType::Int => FirType::Int32,
        ValueType::Real => real_type,
        ValueType::Sound => FirType::Sound,
        ValueType::Tuple(components) => {
            let fields = components
                .iter()
                .map(|component| value_fir_type(component, real_type.clone()))
                .collect::<Vec<_>>();
            FirType::Struct(tuple_type_name(value_type, &real_type), fields)
        }
    }
}

fn tuple_type_name(value_type: &ValueType, real_type: &FirType) -> String {
    fn append_component(name: &mut String, value_type: &ValueType, real_type: &FirType) {
        match value_type {
            ValueType::Int => name.push_str("_i32"),
            ValueType::Sound => name.push_str("_sound"),
            ValueType::Real => match real_type {
                FirType::Float32 => name.push_str("_f32"),
                FirType::Float64 => name.push_str("_f64"),
                _ => name.push_str("_real"),
            },
            ValueType::Tuple(components) => {
                name.push_str("_t");
                name.push_str(&components.len().to_string());
                for component in components {
                    append_component(name, component, real_type);
                }
            }
        }
    }

    let mut name = String::from("frs_vec_tuple");
    append_component(&mut name, value_type, real_type);
    name
}

fn check_value_type(
    signal_id: u64,
    value_type: &ValueType,
    real_type: &FirType,
    value: FirId,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let expected = value_fir_type(value_type, real_type.clone());
    let actual = store.value_type(value);
    if actual != Some(expected.clone()) {
        Err(VectorRouteError::ValueTypeMismatch {
            signal_id,
            expected,
            actual,
        })
    } else if value_shape_matches(value_type, real_type, value, store) {
        Ok(())
    } else {
        Err(VectorRouteError::TupleValueShape { signal_id })
    }
}

fn value_shape_matches(
    value_type: &ValueType,
    real_type: &FirType,
    value: FirId,
    store: &FirStore,
) -> bool {
    let ValueType::Tuple(components) = value_type else {
        return true;
    };
    let FirMatch::ValueArray {
        values,
        typ: FirType::Struct(_, fields),
    } = match_fir(store, value)
    else {
        return false;
    };
    if values.len() != components.len() || fields.len() != components.len() {
        return false;
    }
    components.iter().zip(values).all(|(component, value)| {
        store.value_type(value) == Some(value_fir_type(component, real_type.clone()))
            && value_shape_matches(component, real_type, value, store)
    })
}

fn chunk_index(store: &mut FirStore) -> FirId {
    let mut builder = FirBuilder::new(store);
    let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
    let vindex = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    builder.binop(FirBinOp::Sub, i0, vindex, FirType::Int32)
}

fn is_chunk_index(store: &FirStore, value: FirId) -> bool {
    let FirMatch::BinOp {
        op: FirBinOp::Sub,
        lhs,
        rhs,
        typ: FirType::Int32,
    } = match_fir(store, value)
    else {
        return false;
    };
    matches!(
        match_fir(store, lhs),
        FirMatch::LoadVar {
            name,
            access: AccessType::Loop,
            typ: FirType::Int32,
        } if name == "i0"
    ) && matches!(
        match_fir(store, rhs),
        FirMatch::LoadVar {
            name,
            access: AccessType::Loop,
            typ: FirType::Int32,
        } if name == "vindex"
    )
}

#[cfg(test)]
mod tests {
    use propagate::ClockDomainTable;
    use signals::SigBuilder;
    use tlib::TreeArena;

    use super::*;
    use crate::clk_env::annotate;
    use crate::signal_fir::decoration_verify::certify_decorations;
    use crate::signal_fir::vector_clock_ad::{
        ForwardAdPolicy, VECTOR_CLOCK_AD_PLAN_VERSION, VectorClockAdPlan,
        verified_vector_clock_ad_plan_for_test,
    };
    use crate::signal_fir::vector_plan::{build_vector_plan, verified_vector_plan_for_test};
    use crate::signal_fir::vector_verify::{
        EpochRecord, LoopEdge, LoopRecord, Rate, SignalRecord, VecSafeWitness, Vectorability,
        WitnessKind,
    };
    use crate::signal_prepare::prepare_signals_for_fir_verified;

    fn pure_shared_plan() -> VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 16,
            signals: vec![
                SignalRecord {
                    signal_id: 8,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Block,
                    vectorability: Vectorability::Vect,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Control,
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 9,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Vect,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Inline,
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 10,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Vect,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(0),
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 11,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Vect,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(1),
                    duplicable: true,
                },
            ],
            loops: vec![
                LoopRecord {
                    loop_id: 0,
                    stable_name: "loop_owns_shared".to_owned(),
                    kind: LoopKind::Vectorizable,
                    roots: vec![10],
                    epoch_id: 0,
                },
                LoopRecord {
                    loop_id: 1,
                    stable_name: "loop_consumes_shared".to_owned(),
                    kind: LoopKind::Vectorizable,
                    roots: vec![11],
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
                stable_name: "transport_s10_l0_l1".to_owned(),
                signal_id: 10,
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
        })
    }

    fn tuple_transport_plan() -> VerifiedVectorPlan {
        let mut plan = pure_shared_plan().into_plan();
        let tuple = nested_tuple_type();
        plan.signals
            .iter_mut()
            .find(|signal| signal.signal_id == 10)
            .unwrap()
            .value_type = tuple.clone();
        plan.transports[0].element_type = tuple;
        verified_vector_plan_for_test(plan)
    }

    fn tuple_definition_plan() -> VerifiedVectorPlan {
        let mut plan = pure_shared_plan().into_plan();
        plan.signals
            .iter_mut()
            .find(|signal| signal.signal_id == 8)
            .unwrap()
            .value_type = nested_tuple_type();
        verified_vector_plan_for_test(plan)
    }

    fn nested_tuple_type() -> ValueType {
        ValueType::Tuple(vec![
            ValueType::Real,
            ValueType::Tuple(vec![ValueType::Int, ValueType::Real]),
        ])
    }

    fn recursive_multi_projection_plan() -> VerifiedVectorPlan {
        let mut arena = TreeArena::new();
        let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
        let (body0, body1) = {
            let mut builder = SigBuilder::new(&mut arena);
            let feedback0 = builder.proj(0, self_ref);
            let feedback1 = builder.proj(1, self_ref);
            (builder.delay1(feedback0), builder.delay1(feedback1))
        };
        let nil = arena.nil();
        let tail = arena.cons(body1, nil);
        let bodies = arena.cons(body0, tail);
        let group = tlib::de_bruijn_rec(&mut arena, bodies);
        let (out0, out1) = {
            let mut builder = SigBuilder::new(&mut arena);
            (builder.proj(0, group), builder.proj(1, group))
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[out0, out1], &ui::UiProgram::empty())
                .unwrap();
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        build_vector_plan(&decorations, 8).unwrap()
    }

    fn clock_plan_for_mode(
        vector_plan: &VerifiedVectorPlan,
        mode: ClockTransportMode,
    ) -> VerifiedVectorClockAdPlan {
        verified_vector_clock_ad_plan_for_test(
            VectorClockAdPlan {
                schema_version: VECTOR_CLOCK_AD_PLAN_VERSION,
                vec_size: vector_plan.plan().vec_size,
                clock_islands: vec![],
                transports: vec![ClockTransportPolicy {
                    transport_id: 0,
                    mode,
                }],
                forward_ad: ForwardAdPolicy::ExpandedSignalGraph,
                reverse_ad_fallbacks: vec![],
            },
            vector_plan,
        )
    }

    fn value_for_type(value_type: &ValueType, store: &mut FirStore) -> FirId {
        match value_type {
            ValueType::Int => FirBuilder::new(store).int32(0),
            ValueType::Real => FirBuilder::new(store).float32(0.0),
            ValueType::Sound => panic!("route fixture does not synthesize soundfile handles"),
            ValueType::Tuple(components) => {
                let values = components
                    .iter()
                    .map(|component| value_for_type(component, store))
                    .collect::<Vec<_>>();
                let typ = value_fir_type(value_type, FirType::Float32);
                FirBuilder::new(store).value_array(&values, typ)
            }
        }
    }

    fn value_for(
        signal: &super::super::vector_verify::SignalRecord,
        store: &mut FirStore,
    ) -> FirId {
        value_for_type(&signal.value_type, store)
    }

    fn define_complete<'a>(
        session: &mut VectorRouteSession<'a>,
        store: &mut FirStore,
    ) -> Vec<FirId> {
        let signals = session.plan.signals.clone();
        for signal in &signals {
            if signal.placement == Placement::Control {
                let value = value_for(signal, store);
                session
                    .define_control(signal.signal_id, value, store)
                    .unwrap();
            }
        }
        let loop_order = session
            .layout()
            .loops()
            .iter()
            .map(|region| region.loop_id)
            .collect::<Vec<_>>();
        let mut stores = Vec::new();
        let mut inline_defined = BTreeSet::new();
        for loop_id in loop_order {
            for signal in &signals {
                let should_define = match signal.placement {
                    Placement::Owned(owner) => owner == loop_id,
                    Placement::Inline => inline_defined.insert(signal.signal_id),
                    Placement::Control => false,
                };
                if should_define {
                    let value = value_for(signal, store);
                    stores.extend(
                        session
                            .define_in_loop(loop_id, signal.signal_id, value, store)
                            .unwrap(),
                    );
                }
            }
        }
        stores
    }

    #[test]
    fn pure_plan_routes_all_preplanned_transports_and_verifies() {
        let verified_plan = pure_shared_plan();
        let routes = verified_plan.plan().transports.clone();
        let mut store = FirStore::new();
        let (mut session, declarations) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        assert_eq!(declarations.len(), routes.len());
        let stores = define_complete(&mut session, &mut store);
        assert_eq!(stores.len(), routes.len());
        for transport in &routes {
            assert!(matches!(
                session
                    .resolve_in_loop(transport.consumer_loop, transport.signal_id, &mut store)
                    .unwrap(),
                RouteResolution::Value(_)
            ));
        }
        let routed = session.finish(&store).unwrap();
        assert_eq!(routed.trace().transports().len(), routes.len());
        assert!(
            routed
                .trace()
                .transports()
                .iter()
                .all(|transport| transport.store.is_some() && transport.load.is_some())
        );
    }

    #[test]
    fn tuple_transport_is_rejected_in_favor_of_scalar_projections() {
        let verified_plan = tuple_transport_plan();
        let mut store = FirStore::new();
        assert!(matches!(
            VectorRouteSession::new(
                &verified_plan,
                SchedulingStrategy::DepthFirst,
                FirType::Float32,
                &mut store,
            ),
            Err(VectorRouteError::UnsupportedTupleTransport { signal_id: 10 })
        ));
    }

    #[test]
    fn p6_clock_policy_rematerializes_scalar_and_held_transports() {
        for (mode, expected_access) in [
            (
                ClockTransportMode::IslandScalar { domain_id: 3 },
                AccessType::Stack,
            ),
            (
                ClockTransportMode::HeldOutput { domain_id: 3 },
                AccessType::Struct,
            ),
        ] {
            let verified_plan = pure_shared_plan();
            let clock_plan = clock_plan_for_mode(&verified_plan, mode);
            let mut store = FirStore::new();
            let (mut session, declarations) = VectorRouteSession::new_with_clock_plan(
                &verified_plan,
                &clock_plan,
                SchedulingStrategy::DepthFirst,
                FirType::Float32,
                &mut store,
            )
            .unwrap();
            assert!(matches!(
                match_fir(&store, declarations[0]),
                FirMatch::DeclareVar {
                    typ: FirType::Float32,
                    access,
                    init: None,
                    ..
                } if access == expected_access
            ));
            define_complete(&mut session, &mut store);
            session.resolve_in_loop(1, 10, &mut store).unwrap();
            let routed = session.finish(&store).unwrap();
            let transport = &routed.trace().transports()[0];
            assert_eq!(transport.mode, mode);
            assert!(matches!(
                transport.store.map(|id| match_fir(&store, id)),
                Some(FirMatch::StoreVar { access, .. }) if access == expected_access
            ));
            assert!(matches!(
                transport.load.map(|id| match_fir(&store, id)),
                Some(FirMatch::LoadVar { access, .. }) if access == expected_access
            ));
            verify_routed_fir_with_clock_plan(
                verified_plan.plan(),
                &clock_plan,
                routed.trace(),
                &FirType::Float32,
                &store,
            )
            .unwrap();
        }
    }

    #[test]
    fn tuple_definition_rejects_forged_outer_type_with_wrong_components() {
        let verified_plan = tuple_definition_plan();
        let tuple = verified_plan.plan().signals[0].value_type.clone();
        let tuple_fir = value_fir_type(&tuple, FirType::Float32);
        let mut store = FirStore::new();
        let (mut session, _) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        let only_component = FirBuilder::new(&mut store).float32(0.0);
        let forged = FirBuilder::new(&mut store).value_array(&[only_component], tuple_fir);
        assert_eq!(
            session.define_control(8, forged, &store),
            Err(VectorRouteError::TupleValueShape { signal_id: 8 })
        );
    }

    #[test]
    fn production_recursive_tuple_plan_closes_routed_fir() {
        let verified_plan = recursive_multi_projection_plan();
        assert!(
            verified_plan
                .plan()
                .signals
                .iter()
                .any(|signal| matches!(signal.value_type, ValueType::Tuple(_)))
        );

        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let mut store = FirStore::new();
            let (mut session, _) =
                VectorRouteSession::new(&verified_plan, strategy, FirType::Float32, &mut store)
                    .unwrap();
            define_complete(&mut session, &mut store);
            for transport in &verified_plan.plan().transports {
                session
                    .resolve_in_loop(transport.consumer_loop, transport.signal_id, &mut store)
                    .unwrap();
            }
            session.finish(&store).unwrap();
        }
    }

    #[test]
    fn inline_cache_is_loop_local_and_control_cache_is_ancestor_visible() {
        let verified_plan = pure_shared_plan();
        let mut store = FirStore::new();
        let (mut session, _) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        let loops = session
            .layout()
            .loops()
            .iter()
            .map(|region| region.loop_id)
            .collect::<Vec<_>>();
        let inline = session
            .plan
            .signals
            .iter()
            .find(|signal| signal.placement == Placement::Inline)
            .unwrap()
            .clone();
        let value = value_for(&inline, &mut store);
        session
            .define_in_loop(loops[0], inline.signal_id, value, &mut store)
            .unwrap();
        assert_eq!(
            session
                .resolve_in_loop(loops[0], inline.signal_id, &mut store)
                .unwrap(),
            RouteResolution::Value(value)
        );
        assert_eq!(
            session
                .resolve_in_loop(loops[1], inline.signal_id, &mut store)
                .unwrap(),
            RouteResolution::NeedsInlineLowering
        );

        let control = session
            .plan
            .signals
            .iter()
            .find(|signal| signal.placement == Placement::Control)
            .unwrap()
            .clone();
        let control_value = value_for(&control, &mut store);
        session
            .define_control(control.signal_id, control_value, &store)
            .unwrap();
        for loop_id in loops {
            assert_eq!(
                session
                    .resolve_in_loop(loop_id, control.signal_id, &mut store)
                    .unwrap(),
                RouteResolution::Value(control_value)
            );
        }
    }

    #[test]
    fn owned_definition_in_a_sibling_region_is_rejected() {
        let verified_plan = pure_shared_plan();
        let mut store = FirStore::new();
        let (mut session, _) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        let owned = session
            .plan
            .signals
            .iter()
            .find_map(|signal| match signal.placement {
                Placement::Owned(owner) => Some((signal.clone(), owner)),
                _ => None,
            })
            .unwrap();
        let sibling = session
            .layout()
            .loops()
            .iter()
            .map(|region| region.loop_id)
            .find(|loop_id| *loop_id != owned.1)
            .unwrap();
        let value = value_for(&owned.0, &mut store);
        assert!(matches!(
            session.define_in_loop(sibling, owned.0.signal_id, value, &mut store),
            Err(VectorRouteError::WrongRegion { .. })
        ));
    }

    #[test]
    fn finish_fails_closed_when_a_planned_transport_has_no_load() {
        let verified_plan = pure_shared_plan();
        let expected = verified_plan.plan().transports[0].transport_id;
        let mut store = FirStore::new();
        let (mut session, _) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        define_complete(&mut session, &mut store);
        assert_eq!(
            session.finish(&store),
            Err(VectorRouteError::TransportLoad {
                transport_id: expected
            })
        );
    }

    #[test]
    fn independent_verifier_rejects_a_mutated_transport_load() {
        let verified_plan = pure_shared_plan();
        let expected = verified_plan.plan().transports[0].transport_id;
        let mut store = FirStore::new();
        let (mut session, _) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        define_complete(&mut session, &mut store);
        for transport in &verified_plan.plan().transports {
            session
                .resolve_in_loop(transport.consumer_loop, transport.signal_id, &mut store)
                .unwrap();
        }
        let mut trace = session.finish(&store).unwrap().into_trace();
        trace.transports[0].load = Some(FirBuilder::new(&mut store).int32(0));
        assert_eq!(
            verify_routed_fir(verified_plan.plan(), &trace, &FirType::Float32, &store),
            Err(VectorRouteError::TransportLoad {
                transport_id: expected
            })
        );
    }

    #[test]
    fn independent_verifier_rejects_a_mutated_direct_use() {
        let verified_plan = pure_shared_plan();
        let mut store = FirStore::new();
        let (mut session, _) = VectorRouteSession::new(
            &verified_plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .unwrap();
        define_complete(&mut session, &mut store);
        assert!(matches!(
            session.resolve_in_loop(0, 10, &mut store).unwrap(),
            RouteResolution::Value(_)
        ));
        session.resolve_in_loop(1, 10, &mut store).unwrap();
        let mut trace = session.finish(&store).unwrap().into_trace();
        trace.uses[0].value = FirBuilder::new(&mut store).float32(1.0);
        assert_eq!(
            verify_routed_fir(verified_plan.plan(), &trace, &FirType::Float32, &store),
            Err(VectorRouteError::InvalidUse {
                signal_id: 10,
                consumer_loop: 0
            })
        );
    }

    #[test]
    fn all_strategies_change_only_region_order_not_transport_identity() {
        let verified_plan = pure_shared_plan();
        let expected = verified_plan
            .plan()
            .transports
            .iter()
            .map(|transport| {
                (
                    transport.transport_id,
                    transport.stable_name.clone(),
                    transport.signal_id,
                )
            })
            .collect::<Vec<_>>();
        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let mut store = FirStore::new();
            let (session, declarations) =
                VectorRouteSession::new(&verified_plan, strategy, FirType::Float32, &mut store)
                    .unwrap();
            assert_eq!(session.layout().strategy(), strategy);
            assert_eq!(declarations.len(), expected.len());
            assert_eq!(
                session
                    .plan
                    .transports
                    .iter()
                    .map(|transport| (
                        transport.transport_id,
                        transport.stable_name.clone(),
                        transport.signal_id
                    ))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}
