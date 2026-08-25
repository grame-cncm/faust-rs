//! Independent checker for event-order certificates.
//!
//! `verify_event_order_certificate_impl` is called by BOTH the producer's
//! terminal verification (`produce.rs`) and the standalone checker entries
//! below, so its admission guards remain on both paths (the shared-guard rule
//! of the producer/checker doctrine in `vector/mod.rs`). The independent
//! re-derivations here (independently_*,
//! checker_required_effect_dependencies, independent_checked_sample_count)
//! must NOT be merged with their producer counterparts in `produce.rs` —
//! the duplication IS the assurance boundary (plan provenance: §3.2).

use super::model::*;
use crate::signal_fir::vector::analysis::{EffectAtom, ForeignPurity, StateResource};
use crate::signal_fir::vector::route::{VectorRegion, VerifiedRoutedFir};
use crate::signal_fir::vector::state::{VectorStateAction, VerifiedVectorStatePlan};
use crate::signal_fir::vector::verify::{VectorPlan, verify_vector_plan};
use std::collections::{BTreeMap, BTreeSet};

/// Independently reconstructs and checks event coverage, both total orders,
/// the dependence relation, and `FissionSafe`.
pub fn verify_event_order_certificate(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    certificate: &EventOrderCertificate,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    verify_event_order_certificate_impl(plan, routed, None, certificate, limits)
}
/// Independently checks a state-refined event-order certificate.
pub fn verify_state_event_order_certificate(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: &VerifiedVectorStatePlan,
    certificate: &EventOrderCertificate,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    if state.vector_plan() != plan {
        return Err(VectorEventError::StatePlanMismatch);
    }
    verify_event_order_certificate_impl(plan, routed, Some(state), certificate, limits)
}
pub(super) fn verify_event_order_certificate_impl(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    certificate: &EventOrderCertificate,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    verify_vector_plan(plan)?;
    if routed.plan() != plan {
        return Err(VectorEventError::RoutedPlanMismatch);
    }
    validate_layout(plan, routed)?;
    if certificate.sample_count != plan.vec_size {
        return Err(VectorEventError::EventTableMismatch);
    }
    let expected_checked = independent_checked_sample_count(plan, routed, state, limits)?;
    if certificate.checked_sample_count != expected_checked {
        return Err(VectorEventError::CompactRepetitionMismatch);
    }
    let mut checked_plan = plan.clone();
    checked_plan.vec_size = expected_checked;
    let finite_limit = if expected_checked == plan.vec_size {
        limits.complete
    } else {
        limits.compact
    };
    verify_event_table_independently(
        &checked_plan,
        routed,
        state,
        &certificate.events,
        finite_limit,
    )?;
    if expected_checked < plan.vec_size {
        verify_compact_repetition_basis(&checked_plan, &certificate.events)?;
    }
    validate_order("scalar", &certificate.events, &certificate.scalar_order)?;
    validate_order("vector", &certificate.events, &certificate.vector_order)?;
    let scalar_order = independently_order_events(&checked_plan, routed, &certificate.events, true);
    if certificate.scalar_order != scalar_order {
        return Err(VectorEventError::ScalarOrderMismatch);
    }
    let vector_order =
        independently_order_events(&checked_plan, routed, &certificate.events, false);
    if certificate.vector_order != vector_order {
        return Err(VectorEventError::VectorOrderMismatch);
    }
    if certificate
        .dependencies
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err(VectorEventError::DependencyMismatch);
    }

    let scalar_positions = positions(&certificate.scalar_order);
    let vector_positions = positions(&certificate.vector_order);
    let event_ids = certificate
        .events
        .iter()
        .map(|event| event.event_id)
        .collect::<BTreeSet<_>>();
    for dependency in &certificate.dependencies {
        if dependency.before == dependency.after
            || !event_ids.contains(&dependency.before)
            || !event_ids.contains(&dependency.after)
        {
            return Err(VectorEventError::DependencyMismatch);
        }
        if scalar_positions[&dependency.before] >= scalar_positions[&dependency.after] {
            return Err(VectorEventError::ScalarDependenceViolation {
                before: dependency.before,
                after: dependency.after,
            });
        }
        if vector_positions[&dependency.before] >= vector_positions[&dependency.after] {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                let by_id = certificate
                    .events
                    .iter()
                    .map(|event| (event.event_id, event))
                    .collect::<BTreeMap<_, _>>();
                eprintln!(
                    "[vector-event-reversal] before={:?} scalar={} vector={} after={:?} scalar={} vector={} fused={:?}",
                    by_id[&dependency.before],
                    scalar_positions[&dependency.before],
                    vector_positions[&dependency.before],
                    by_id[&dependency.after],
                    scalar_positions[&dependency.after],
                    vector_positions[&dependency.after],
                    checked_plan.fused_serial_groups
                );
            }
            return Err(VectorEventError::FissionSafeViolation {
                before: dependency.before,
                after: dependency.after,
            });
        }
    }
    verify_required_dependencies(
        &checked_plan,
        state,
        &certificate.events,
        &certificate.dependencies,
        &scalar_positions,
    )?;
    Ok(())
}
pub(super) fn independent_checked_sample_count(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    limits: EventLimits,
) -> Result<u64, VectorEventError> {
    let mut one_sample_plan = plan.clone();
    one_sample_plan.vec_size = 1;
    let keys = independently_expected_event_keys(&one_sample_plan, routed, state)?;
    let fixed = keys
        .iter()
        .filter(|(_, sample, _)| sample.is_none())
        .count();
    let per_sample = keys
        .iter()
        .filter(|(_, sample, _)| sample.is_some())
        .count();
    let logical_samples =
        usize::try_from(plan.vec_size).map_err(|_| VectorEventError::EventCountOverflow)?;
    let complete = per_sample
        .checked_mul(logical_samples)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if complete <= limits.complete {
        return Ok(plan.vec_size);
    }
    let basis = logical_samples.min(COMPACT_EVENT_SAMPLE_BASIS);
    let compact = per_sample
        .checked_mul(basis)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if compact > limits.compact {
        return Err(VectorEventError::EventBoundExceeded {
            needed: compact,
            limit: limits.compact,
        });
    }
    u64::try_from(basis).map_err(|_| VectorEventError::EventCountOverflow)
}
pub(super) fn verify_compact_repetition_basis(
    checked_plan: &VectorPlan,
    events: &[VectorEvent],
) -> Result<(), VectorEventError> {
    if checked_plan.vec_size != COMPACT_EVENT_SAMPLE_BASIS as u64 {
        return Err(VectorEventError::CompactRepetitionMismatch);
    }
    for record in &checked_plan.loops {
        let sample_zero = events
            .iter()
            .filter(|event| {
                event.region == EventRegion::Loop(record.loop_id) && event.sample == Some(0)
            })
            .map(|event| &event.kind)
            .collect::<Vec<_>>();
        let sample_one = events
            .iter()
            .filter(|event| {
                event.region == EventRegion::Loop(record.loop_id) && event.sample == Some(1)
            })
            .map(|event| &event.kind)
            .collect::<Vec<_>>();
        if sample_zero != sample_one {
            return Err(VectorEventError::CompactRepetitionMismatch);
        }
    }
    Ok(())
}
pub(super) fn append_state_event_keys(
    keys: &mut Vec<EventKey>,
    state: &VerifiedVectorStatePlan,
    sample_count: u64,
) {
    for phases in &state.plan().loops {
        for action in &phases.pre {
            keys.push((
                EventRegion::LoopPre(phases.loop_id),
                None,
                VectorEventKind::StateTransition {
                    action: action.clone(),
                },
            ));
        }
        for sample in 0..sample_count {
            for action in &phases.exec {
                keys.push((
                    EventRegion::Loop(phases.loop_id),
                    Some(sample),
                    VectorEventKind::StateTransition {
                        action: action.clone(),
                    },
                ));
            }
        }
        for action in &phases.post {
            keys.push((
                EventRegion::LoopPost(phases.loop_id),
                None,
                VectorEventKind::StateTransition {
                    action: action.clone(),
                },
            ));
        }
    }
}
pub(super) fn effect_is_managed(effect: &EffectAtom, managed: &BTreeSet<StateResource>) -> bool {
    match effect {
        EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource) => {
            managed.contains(resource)
        }
        _ => false,
    }
}
pub(super) fn verify_event_table_independently(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    event_limit: usize,
) -> Result<(), VectorEventError> {
    let expected = independently_expected_event_keys(plan, routed, state)?;
    if expected.len() > event_limit {
        return Err(VectorEventError::EventBoundExceeded {
            needed: expected.len(),
            limit: event_limit,
        });
    }
    if events.len() != expected.len() {
        return Err(VectorEventError::EventTableMismatch);
    }
    for (index, (event, key)) in events.iter().zip(expected).enumerate() {
        let event_id = u64::try_from(index).map_err(|_| VectorEventError::EventCountOverflow)?;
        if event.event_id != event_id || (event.region, event.sample, event.kind.clone()) != key {
            return Err(VectorEventError::EventTableMismatch);
        }
    }
    Ok(())
}
pub(super) fn independently_expected_event_keys(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<Vec<EventKey>, VectorEventError> {
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();
    let managed = state.map_or_else(BTreeSet::new, VerifiedVectorStatePlan::managed_resources);
    let mut loop_kinds = BTreeMap::<u64, Vec<VectorEventKind>>::new();
    let mut keys = Vec::new();

    for definition in routed.trace().definitions() {
        let kinds = match definition.region {
            VectorRegion::Control => None,
            VectorRegion::Loop(loop_id) => Some(loop_kinds.entry(loop_id).or_default()),
        };
        let definition_kind = VectorEventKind::Definition {
            signal_id: definition.signal_id,
        };
        if let Some(kinds) = kinds {
            kinds.push(definition_kind);
        } else {
            keys.push((EventRegion::Control, None, definition_kind));
        }
        // An effect event models one actual operation, so it belongs to the
        // signal that performs it. Enumerating the transitive closure instead
        // emits one event per carrier and invents conflicts between signals
        // that merely contain the performer in their subtree.
        for (effect_index, effect) in signals[&definition.signal_id]
            .direct_effects
            .iter()
            .enumerate()
        {
            if effect_is_managed(effect, &managed) {
                continue;
            }
            let kind = VectorEventKind::Effect {
                signal_id: definition.signal_id,
                effect_index: u64::try_from(effect_index)
                    .map_err(|_| VectorEventError::EventCountOverflow)?,
                effect: effect.clone(),
            };
            match definition.region {
                VectorRegion::Control => keys.push((EventRegion::Control, None, kind)),
                VectorRegion::Loop(loop_id) => loop_kinds.entry(loop_id).or_default().push(kind),
            }
        }
    }
    for routed_transport in routed.trace().transports() {
        let transport = transports[&routed_transport.transport_id];
        loop_kinds.entry(transport.producer_loop).or_default().push(
            VectorEventKind::TransportStore {
                transport_id: transport.transport_id,
            },
        );
        loop_kinds.entry(transport.consumer_loop).or_default().push(
            VectorEventKind::TransportLoad {
                transport_id: transport.transport_id,
            },
        );
    }
    let mut uses = routed
        .trace()
        .uses()
        .iter()
        .map(|routed_use| {
            (
                routed_use.consumer_loop,
                routed_use.signal_id,
                event_use_source(routed_use.source),
            )
        })
        .collect::<Vec<_>>();
    uses.sort_unstable();
    let mut occurrences = BTreeMap::new();
    for (consumer_loop, signal_id, source) in uses {
        let occurrence = occurrences
            .entry((consumer_loop, signal_id, source))
            .or_insert(0_u64);
        loop_kinds
            .entry(consumer_loop)
            .or_default()
            .push(VectorEventKind::Use {
                signal_id,
                source,
                occurrence: *occurrence,
            });
        *occurrence = occurrence
            .checked_add(1)
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    for epoch in &plan.epochs {
        keys.push((
            EventRegion::Epoch(epoch.epoch_id),
            None,
            VectorEventKind::EpochEnter {
                epoch_id: epoch.epoch_id,
            },
        ));
        keys.push((
            EventRegion::Epoch(epoch.epoch_id),
            None,
            VectorEventKind::EpochExit {
                epoch_id: epoch.epoch_id,
            },
        ));
    }
    for record in &plan.loops {
        let kinds = loop_kinds.remove(&record.loop_id).unwrap_or_default();
        for sample in 0..plan.vec_size {
            for kind in &kinds {
                keys.push((
                    EventRegion::Loop(record.loop_id),
                    Some(sample),
                    kind.clone(),
                ));
            }
        }
    }
    if let Some(state) = state {
        append_state_event_keys(&mut keys, state, plan.vec_size);
    }
    keys.sort();
    Ok(keys)
}
pub(super) fn independently_order_events(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    events: &[VectorEvent],
    sample_major: bool,
) -> Vec<u64> {
    let epochs = sorted_epochs(plan);
    let epoch_position = epochs
        .iter()
        .enumerate()
        .map(|(position, epoch)| (epoch.epoch_id, position + 1))
        .collect::<BTreeMap<_, _>>();
    let loop_epoch = plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record.epoch_id))
        .collect::<BTreeMap<_, _>>();
    let canonical_loops = canonical_scalar_loops(plan);
    let scalar_loop_position = canonical_loops
        .values()
        .flat_map(|loops| loops.iter().enumerate())
        .map(|(position, loop_id)| (*loop_id, position))
        .collect::<BTreeMap<_, _>>();
    let vector_loop_position = routed
        .layout()
        .loops()
        .iter()
        .enumerate()
        .map(|(position, region)| (region.loop_id, position))
        .collect::<BTreeMap<_, _>>();
    let mut sample_interleaved_position = BTreeMap::new();
    for group in &plan.fused_serial_groups {
        let members = routed
            .layout()
            .loops()
            .iter()
            .filter(|region| group.member_loop_ids.binary_search(&region.loop_id).is_ok())
            .map(|region| region.loop_id)
            .collect::<Vec<_>>();
        let unit_position = members
            .iter()
            .map(|loop_id| vector_loop_position[loop_id])
            .min()
            .expect("verified fused groups are non-empty");
        for (member_position, loop_id) in members.into_iter().enumerate() {
            sample_interleaved_position.insert(loop_id, (unit_position, member_position));
        }
    }
    for bundle in &plan.lockstep_bundles {
        let unit_position = bundle
            .member_loop_ids
            .iter()
            .map(|loop_id| vector_loop_position[loop_id])
            .min()
            .expect("verified lockstep bundles are non-empty");
        for (member_position, &loop_id) in bundle.member_loop_ids.iter().enumerate() {
            sample_interleaved_position.insert(loop_id, (unit_position, member_position));
        }
    }

    let mut order = events.iter().collect::<Vec<_>>();
    order.sort_unstable_by_key(|event| match event.region {
        EventRegion::Control => (0, 0, 0, 0, 0, 0, event.event_id),
        EventRegion::Epoch(epoch_id) => {
            let phase = match event.kind {
                VectorEventKind::EpochEnter { .. } => 0,
                VectorEventKind::EpochExit { .. } => 4,
                _ => unreachable!("event-table checker restricts epoch events"),
            };
            (epoch_position[&epoch_id], phase, 0, 0, 0, 0, event.event_id)
        }
        EventRegion::LoopPre(loop_id)
        | EventRegion::Loop(loop_id)
        | EventRegion::LoopPost(loop_id) => {
            let epoch = epoch_position[&loop_epoch[&loop_id]];
            let (phase, sample) = match event.region {
                EventRegion::LoopPre(_) => (1, 0),
                EventRegion::Loop(_) => (
                    2,
                    usize::try_from(event.sample.expect("exec event has a sample"))
                        .expect("event table is bounded by usize"),
                ),
                EventRegion::LoopPost(_) => (3, 0),
                EventRegion::Control | EventRegion::Epoch(_) => unreachable!(),
            };
            if phase != 2 {
                let loop_position = if sample_major {
                    scalar_loop_position[&loop_id]
                } else {
                    vector_loop_position[&loop_id]
                };
                (epoch, phase, loop_position, 0, 0, 0, event.event_id)
            } else if sample_major {
                (
                    epoch,
                    phase,
                    sample,
                    scalar_loop_position[&loop_id],
                    0,
                    0,
                    event.event_id,
                )
            } else {
                let (unit_position, member_position) = sample_interleaved_position
                    .get(&loop_id)
                    .copied()
                    .unwrap_or((vector_loop_position[&loop_id], 0));
                (
                    epoch,
                    phase,
                    unit_position,
                    sample,
                    member_position,
                    0,
                    event.event_id,
                )
            }
        }
    });
    order.into_iter().map(|event| event.event_id).collect()
}
pub(super) fn verify_required_dependencies(
    plan: &VectorPlan,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    dependencies: &[EventDependency],
    scalar_positions: &BTreeMap<u64, usize>,
) -> Result<(), VectorEventError> {
    let present = dependencies.iter().copied().collect::<BTreeSet<_>>();
    let contexts = context_events(events);
    let require = |before, after| {
        if before == after || present.contains(&EventDependency { before, after }) {
            Ok(())
        } else {
            Err(VectorEventError::DependencyMismatch)
        }
    };

    for local in contexts.values() {
        for pair in local.windows(2) {
            require(pair[0], pair[1])?;
        }
    }
    let epochs = sorted_epochs(plan);
    for (index, epoch) in epochs.iter().enumerate() {
        let boundaries = &contexts[&(EventRegion::Epoch(epoch.epoch_id), None)];
        let enter = boundaries[0];
        let exit = boundaries[1];
        if index == 0 {
            if let Some(control) = contexts.get(&(EventRegion::Control, None))
                && let Some(last) = control.last()
            {
                require(*last, enter)?;
            }
        } else {
            let previous = &contexts[&(EventRegion::Epoch(epochs[index - 1].epoch_id), None)];
            require(previous[1], enter)?;
        }
        for &loop_id in &epoch.loops {
            if let Some(pre) = contexts.get(&(EventRegion::LoopPre(loop_id), None))
                && let (Some(first), Some(last)) = (pre.first(), pre.last())
            {
                require(enter, *first)?;
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_first) = local.first()
                    {
                        require(*last, *sample_first)?;
                    }
                }
            }
            for sample in 0..plan.vec_size {
                if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                    && let (Some(first), Some(last)) = (local.first(), local.last())
                {
                    require(enter, *first)?;
                    require(*last, exit)?;
                }
            }
            if let Some(post) = contexts.get(&(EventRegion::LoopPost(loop_id), None))
                && let (Some(first), Some(last)) = (post.first(), post.last())
            {
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_last) = local.last()
                    {
                        require(*sample_last, *first)?;
                    }
                }
                require(*last, exit)?;
            }
        }
    }
    for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
        for sample in 0..plan.vec_size {
            let producer = contexts
                .get(&(EventRegion::Loop(edge.dependency), Some(sample)))
                .and_then(|local| local.last());
            let consumer = contexts
                .get(&(EventRegion::Loop(edge.consumer), Some(sample)))
                .and_then(|local| local.first());
            if let (Some(producer), Some(consumer)) = (producer, consumer) {
                require(*producer, *consumer)?;
            }
        }
    }

    let definitions = events
        .iter()
        .filter_map(|event| match event.kind {
            VectorEventKind::Definition { signal_id } => {
                Some(((event.region, signal_id, event.sample), event.event_id))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let loads = event_ids_by_transport(events, false);
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();
    for event in events {
        match event.kind {
            VectorEventKind::TransportStore { transport_id } => {
                let transport = transports[&transport_id];
                require(
                    definitions[&(
                        EventRegion::Loop(transport.producer_loop),
                        transport.signal_id,
                        event.sample,
                    )],
                    event.event_id,
                )?;
                require(event.event_id, loads[&(transport_id, event.sample)])?;
            }
            VectorEventKind::Use {
                signal_id, source, ..
            } => {
                let source_event = match source {
                    EventUseSource::Control => {
                        definitions[&(EventRegion::Control, signal_id, None)]
                    }
                    EventUseSource::Loop(loop_id) => {
                        definitions[&(EventRegion::Loop(loop_id), signal_id, event.sample)]
                    }
                    EventUseSource::Transport(transport_id) => loads[&(transport_id, event.sample)],
                };
                require(source_event, event.event_id)?;
            }
            _ => {}
        }
    }

    let effects = events
        .iter()
        .filter_map(|event| match &event.kind {
            VectorEventKind::Effect { effect, .. } => Some((event.event_id, effect)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for dependency in checker_required_effect_dependencies(&effects, scalar_positions) {
        require(dependency.before, dependency.after)?;
    }
    let recursion_steps = recursion_step_events(events);
    for ((loop_id, sample, group), event_id) in &recursion_steps {
        if *sample + 1 < plan.vec_size
            && let Some(next) = recursion_steps.get(&(*loop_id, *sample + 1, *group))
        {
            require(*event_id, *next)?;
        }
    }
    for dependency in managed_state_dependencies(plan, state, events, scalar_positions) {
        require(dependency.before, dependency.after)?;
    }
    Ok(())
}
pub(super) fn validate_layout(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
) -> Result<(), VectorEventError> {
    let loop_by_id = plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record))
        .collect::<BTreeMap<_, _>>();
    let epoch_by_id = plan
        .epochs
        .iter()
        .map(|epoch| (epoch.epoch_id, epoch))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut positions = BTreeMap::new();
    let mut previous_rank = None;
    for (index, region) in routed.layout().loops().iter().enumerate() {
        let Some(record) = loop_by_id.get(&region.loop_id) else {
            return Err(VectorEventError::InvalidLayout {
                detail: "unknown loop",
                loop_id: region.loop_id,
            });
        };
        let Some(epoch) = epoch_by_id.get(&record.epoch_id) else {
            return Err(VectorEventError::InvalidLayout {
                detail: "unknown epoch",
                loop_id: region.loop_id,
            });
        };
        if !seen.insert(region.loop_id) {
            return Err(VectorEventError::InvalidLayout {
                detail: "duplicate loop",
                loop_id: region.loop_id,
            });
        }
        if region.epoch_id != record.epoch_id
            || region.epoch_rank != epoch.rank
            || region.kind != record.kind
            || region.roots != record.roots
        {
            return Err(VectorEventError::InvalidLayout {
                detail: "metadata mismatch",
                loop_id: region.loop_id,
            });
        }
        if previous_rank.is_some_and(|rank| rank > region.epoch_rank) {
            return Err(VectorEventError::InvalidLayout {
                detail: "epoch rank reversal",
                loop_id: region.loop_id,
            });
        }
        previous_rank = Some(region.epoch_rank);
        positions.insert(region.loop_id, index);
    }
    if seen.len() != plan.loops.len() {
        let missing = plan
            .loops
            .iter()
            .find(|record| !seen.contains(&record.loop_id))
            .map_or(u64::MAX, |record| record.loop_id);
        return Err(VectorEventError::InvalidLayout {
            detail: "missing loop",
            loop_id: missing,
        });
    }
    for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
        if positions[&edge.dependency] >= positions[&edge.consumer] {
            return Err(VectorEventError::InvalidLayout {
                detail: "dependency reversal",
                loop_id: edge.consumer,
            });
        }
    }
    Ok(())
}
pub(super) fn context_events(
    events: &[VectorEvent],
) -> BTreeMap<(EventRegion, Option<u64>), Vec<u64>> {
    let mut contexts = BTreeMap::<_, Vec<_>>::new();
    for event in events {
        contexts
            .entry((event.region, event.sample))
            .or_default()
            .push(event.event_id);
    }
    contexts
}
pub(super) fn canonical_scalar_loops(plan: &VectorPlan) -> BTreeMap<u64, Vec<u64>> {
    let mut result = BTreeMap::new();
    for epoch in sorted_epochs(plan) {
        let members = epoch.loops.iter().copied().collect::<BTreeSet<_>>();
        let mut remaining = members.clone();
        let mut order = Vec::with_capacity(members.len());
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .copied()
                .find(|candidate| {
                    plan.data_edges
                        .iter()
                        .chain(&plan.effect_edges)
                        .filter(|edge| {
                            edge.consumer == *candidate && members.contains(&edge.dependency)
                        })
                        .all(|edge| !remaining.contains(&edge.dependency))
                })
                .expect("verified epoch DAG has a ready loop");
            remaining.remove(&next);
            order.push(next);
        }
        result.insert(epoch.epoch_id, order);
    }
    result
}
/// Independently reconstructs the required effect dependencies for the
/// checker. No producer grouping or producer result is consumed here.
pub(super) fn checker_required_effect_dependencies(
    effects: &[(u64, &EffectAtom)],
    scalar_positions: &BTreeMap<u64, usize>,
) -> BTreeSet<EventDependency> {
    let mut every_effect = Vec::with_capacity(effects.len());
    let mut global_barriers = Vec::new();
    let mut state_accesses = BTreeMap::<StateResource, (Vec<u64>, Vec<u64>)>::new();
    let mut table_accesses = BTreeMap::<u32, (Vec<u64>, Vec<u64>)>::new();
    let mut ui_updates = BTreeMap::<u32, Vec<u64>>::new();
    let mut output_updates = BTreeMap::<u32, Vec<u64>>::new();

    for &(event_id, effect) in effects {
        every_effect.push(event_id);
        match effect {
            EffectAtom::ReadState(resource) => {
                state_accesses
                    .entry(resource.clone())
                    .or_default()
                    .0
                    .push(event_id);
            }
            EffectAtom::WriteState(resource) => {
                state_accesses
                    .entry(resource.clone())
                    .or_default()
                    .1
                    .push(event_id);
            }
            EffectAtom::ReadTable(resource) => {
                table_accesses
                    .entry(*resource)
                    .or_default()
                    .0
                    .push(event_id);
            }
            EffectAtom::WriteTable(resource) => {
                table_accesses
                    .entry(*resource)
                    .or_default()
                    .1
                    .push(event_id);
            }
            EffectAtom::WriteUi(resource) => {
                ui_updates.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::WriteOutput(resource) => {
                output_updates.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::Foreign {
                purity: ForeignPurity::Impure | ForeignPurity::Unknown,
                ..
            } => global_barriers.push(event_id),
            EffectAtom::Foreign {
                purity: ForeignPurity::Pure,
                ..
            } => {}
        }
    }

    let mut required = BTreeSet::new();
    let mut require_pair = |left: u64, right: u64| {
        if left == right {
            return;
        }
        let (before, after) = if scalar_positions[&left] < scalar_positions[&right] {
            (left, right)
        } else {
            (right, left)
        };
        required.insert(EventDependency { before, after });
    };
    for (reads, writes) in state_accesses.values().chain(table_accesses.values()) {
        for &reader in reads {
            for &writer in writes {
                require_pair(reader, writer);
            }
        }
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                require_pair(left, right);
            }
        }
    }
    for writes in ui_updates.values().chain(output_updates.values()) {
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                require_pair(left, right);
            }
        }
    }
    for &barrier in &global_barriers {
        for &other in &every_effect {
            require_pair(barrier, other);
        }
    }
    required
}
pub(super) fn event_ids_by_transport(
    events: &[VectorEvent],
    stores: bool,
) -> BTreeMap<(u64, Option<u64>), u64> {
    events
        .iter()
        .filter_map(|event| {
            let transport_id = match event.kind {
                VectorEventKind::TransportStore { transport_id } if stores => transport_id,
                VectorEventKind::TransportLoad { transport_id } if !stores => transport_id,
                _ => return None,
            };
            Some(((transport_id, event.sample), event.event_id))
        })
        .collect()
}
pub(super) fn managed_state_dependencies(
    plan: &VectorPlan,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    scalar_positions: &BTreeMap<u64, usize>,
) -> BTreeSet<EventDependency> {
    let Some(state) = state else {
        return BTreeSet::new();
    };
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut reads = BTreeMap::<(StateResource, u64), Vec<u64>>::new();
    let mut writes = BTreeMap::<(StateResource, u64), Vec<u64>>::new();
    for event in events {
        let Some(sample) = event.sample else {
            continue;
        };
        match &event.kind {
            VectorEventKind::Definition { signal_id } => {
                for effect in &signals[signal_id].effects {
                    if let EffectAtom::ReadState(resource) = effect {
                        reads
                            .entry((resource.clone(), sample))
                            .or_default()
                            .push(event.event_id);
                    }
                }
            }
            VectorEventKind::StateTransition { action } => {
                for resource in state_action_resources(state, action) {
                    writes
                        .entry((resource, sample))
                        .or_default()
                        .push(event.event_id);
                }
            }
            _ => {}
        }
    }

    let mut dependencies = BTreeSet::new();
    for ((resource, sample), writer_ids) in &writes {
        for &writer_id in writer_ids {
            if let Some(reader_ids) = reads.get(&(resource.clone(), *sample)) {
                for &reader_id in reader_ids {
                    if scalar_positions[&reader_id] < scalar_positions[&writer_id] {
                        add_dependency(&mut dependencies, reader_id, writer_id);
                    }
                }
            }
            let Some(next_sample) = sample.checked_add(1).filter(|next| *next < plan.vec_size)
            else {
                continue;
            };
            if let Some(reader_ids) = reads.get(&(resource.clone(), next_sample)) {
                for &reader_id in reader_ids {
                    if scalar_positions[&writer_id] < scalar_positions[&reader_id] {
                        add_dependency(&mut dependencies, writer_id, reader_id);
                    }
                }
            }
            if let Some(next_writer_ids) = writes.get(&(resource.clone(), next_sample)) {
                for &next_writer_id in next_writer_ids {
                    if scalar_positions[&writer_id] < scalar_positions[&next_writer_id] {
                        add_dependency(&mut dependencies, writer_id, next_writer_id);
                    }
                }
            }
        }
    }
    dependencies
}
pub(super) fn state_action_resources(
    state: &VerifiedVectorStatePlan,
    action: &VectorStateAction,
) -> Vec<StateResource> {
    match action {
        VectorStateAction::DelayWrite { signal_id } => vec![StateResource::Signal {
            owner: u32::try_from(*signal_id).expect("verified signal id fits u32"),
            cell: crate::signal_fir::vector::analysis::StateCell::Delay,
        }],
        VectorStateAction::RecursionStep { group } => state
            .plan()
            .recursions
            .iter()
            .find(|recursion| recursion.group == *group)
            .into_iter()
            .flat_map(|recursion| {
                recursion
                    .projections
                    .iter()
                    .map(move |projection| StateResource::Recursion {
                        group: u32::try_from(*group).expect("verified recursion group fits u32"),
                        projection: u32::try_from(projection.index)
                            .expect("verified recursion projection fits u32"),
                    })
            })
            .collect(),
        VectorStateAction::PrefixWrite { signal_id } => vec![StateResource::Signal {
            owner: u32::try_from(*signal_id).expect("verified signal id fits u32"),
            cell: crate::signal_fir::vector::analysis::StateCell::Prefix,
        }],
        VectorStateAction::WaveformAdvance { signal_id } => vec![StateResource::Signal {
            owner: u32::try_from(*signal_id).expect("verified signal id fits u32"),
            cell: crate::signal_fir::vector::analysis::StateCell::WaveformIndex,
        }],
        VectorStateAction::DelayRegisterLoad { .. }
        | VectorStateAction::DelayRegisterStore { .. }
        | VectorStateAction::DelayCopyIn { .. }
        | VectorStateAction::DelayCopyOut { .. }
        | VectorStateAction::DelayRingAdvance { .. }
        | VectorStateAction::DelayRingSaveAdvance { .. } => Vec::new(),
    }
}
pub(super) fn recursion_step_events(events: &[VectorEvent]) -> BTreeMap<(u64, u64, u64), u64> {
    events
        .iter()
        .filter_map(|event| {
            let EventRegion::Loop(loop_id) = event.region else {
                return None;
            };
            let VectorEventKind::StateTransition {
                action: VectorStateAction::RecursionStep { group },
            } = &event.kind
            else {
                return None;
            };
            Some(((loop_id, event.sample?, *group), event.event_id))
        })
        .collect()
}
pub(super) fn validate_order(
    which: &'static str,
    events: &[VectorEvent],
    order: &[u64],
) -> Result<(), VectorEventError> {
    let expected = events
        .iter()
        .map(|event| event.event_id)
        .collect::<BTreeSet<_>>();
    let actual = order.iter().copied().collect::<BTreeSet<_>>();
    if order.len() != events.len() || actual != expected {
        Err(VectorEventError::InvalidOrder { which })
    } else {
        Ok(())
    }
}
pub(super) fn positions(order: &[u64]) -> BTreeMap<u64, usize> {
    order
        .iter()
        .enumerate()
        .map(|(position, event_id)| (*event_id, position))
        .collect()
}
pub(super) fn sorted_epochs(
    plan: &VectorPlan,
) -> Vec<&crate::signal_fir::vector::verify::EpochRecord> {
    let mut epochs = plan.epochs.iter().collect::<Vec<_>>();
    epochs.sort_unstable_by_key(|epoch| (epoch.rank, epoch.epoch_id));
    epochs
}
