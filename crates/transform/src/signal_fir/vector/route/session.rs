//! Mutable routing session (producer). Its terminal step calls the
//! shared verify path in `check.rs`, so every admission guard there also
//! binds the producer (plan provenance: §4.8).

use super::check::*;
use super::model::*;
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::vector::clock_ad::{
    ClockTransportMode, ClockTransportPolicy, VerifiedVectorClockAdPlan,
};
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::schedule::schedule_verified_vector_plan;
use crate::signal_fir::vector::verify::{Placement, TransportRecord, VectorPlan};
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirStore, FirType};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct VectorValueCache {
    control: BTreeMap<u64, FirId>,
    owned: BTreeMap<u64, (u64, FirId)>,
    inline: BTreeMap<(u64, u64), FirId>,
    transport_loads: BTreeMap<u64, FirId>,
}
/// Stateful routing surface used by a signal lowerer.
pub struct VectorRouteSession<'a> {
    pub(super) plan: &'a VectorPlan,
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

    /// Creates routes using the exact clock-plan transport lifetimes.
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

    /// Returns the scheduled region layout.
    #[must_use]
    pub fn layout(&self) -> &VectorRegionLayout {
        &self.layout
    }

    /// Returns the immutable vector plan that owns all route identities.
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

    /// Resolves one signal use from `consumer_loop` according to routing visibility.
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
    ) -> Result<&crate::signal_fir::vector::verify::SignalRecord, VectorRouteError> {
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

pub(super) fn declare_transport(
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
pub(super) fn store_transport(
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
pub(super) fn load_transport(
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
pub(super) fn chunk_index(store: &mut FirStore) -> FirId {
    let mut builder = FirBuilder::new(store);
    let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
    let vindex = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    builder.binop(FirBinOp::Sub, i0, vindex, FirType::Int32)
}
