//! Producer construction of event-order certificates. The terminal step
//! calls the shared verify path in `check.rs`, so every admission guard
//! there also binds the producer (plan provenance: §4.8). Producer-side derivations
//! (event_templates, build_order, build_dependencies,
//! producer_effect_dependencies, producer_checked_sample_count) must NOT
//! be merged with their independent checker counterparts in `check.rs`
//! (independently_*, checker_required_effect_dependencies,
//! independent_checked_sample_count) — the duplication IS the assurance
//! boundary (plan provenance: §3.2).

use super::check::{
    append_state_event_keys, canonical_scalar_loops, context_events, effect_is_managed,
    event_ids_by_transport, managed_state_dependencies, positions, recursion_step_events,
    sorted_epochs, validate_layout, verify_event_order_certificate,
    verify_state_event_order_certificate,
};
use super::model::*;
use crate::signal_fir::vector::analysis::{EffectAtom, ForeignPurity, StateResource};
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::route::VerifiedRoutedFir;
use crate::signal_fir::vector::state::VerifiedVectorStatePlan;
use crate::signal_fir::vector::verify::{LoopEdge, VectorPlan, verify_vector_plan};
use std::collections::{BTreeMap, BTreeSet};

/// Rejects plans whose route-independent event lower bound already exceeds
/// the applicable complete-expansion or compact-basis limit.
///
/// This is intentionally one-sided: it counts only non-tuple loop roots,
/// their unmanaged effects, planned transport store/load pairs, epoch
/// barriers, and exact state actions. Routed inline definitions and uses are
/// omitted, so passing this precheck is not certification. Rejection is safe
/// because the complete event table must contain every counted event.
pub fn precheck_state_event_bound(
    verified_plan: &VerifiedVectorPlan,
    state: &VerifiedVectorStatePlan,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    let plan = verified_plan.plan();
    if state.vector_plan() != plan {
        return Err(VectorEventError::StatePlanMismatch);
    }
    let managed = state.managed_resources();
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut per_sample = plan
        .transports
        .len()
        .checked_mul(2)
        .ok_or(VectorEventError::EventCountOverflow)?;
    for record in &plan.loops {
        for root in &record.roots {
            let signal = signals
                .get(root)
                .ok_or(VectorEventError::EventCountOverflow)?;
            if matches!(
                signal.value_type,
                crate::signal_fir::vector::verify::ValueType::Tuple(_)
            ) {
                continue;
            }
            per_sample = per_sample
                .checked_add(1)
                .and_then(|count| {
                    // Emission now attributes an effect to its performer, so a
                    // root contributes only its direct set; counting the
                    // transitive union here would exceed actual emission and
                    // turn this lower bound into a spurious rejection.
                    count.checked_add(
                        signal
                            .direct_effects
                            .iter()
                            .filter(|effect| !effect_is_managed(effect, &managed))
                            .count(),
                    )
                })
                .ok_or(VectorEventError::EventCountOverflow)?;
        }
    }
    let mut fixed = plan
        .epochs
        .len()
        .checked_mul(2)
        .ok_or(VectorEventError::EventCountOverflow)?;
    for phases in &state.plan().loops {
        per_sample = per_sample
            .checked_add(phases.exec.len())
            .ok_or(VectorEventError::EventCountOverflow)?;
        fixed = fixed
            .checked_add(phases.pre.len())
            .and_then(|count| count.checked_add(phases.post.len()))
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    let sample_count =
        usize::try_from(plan.vec_size).map_err(|_| VectorEventError::EventCountOverflow)?;
    let minimum = per_sample
        .checked_mul(sample_count)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if minimum <= limits.complete {
        return Ok(());
    }
    let checked_samples = sample_count.min(COMPACT_EVENT_SAMPLE_BASIS);
    let compact_minimum = per_sample
        .checked_mul(checked_samples)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if compact_minimum > limits.compact {
        return Err(VectorEventError::EventLowerBoundExceeded {
            minimum: compact_minimum,
            limit: limits.compact,
        });
    }
    Ok(())
}
/// Produces and independently checks bounded event-order evidence.
///
/// `limits.complete` applies to full `vec_size` expansion and `limits.compact`
/// applies only to the canonical two-sample basis. Neither form changes the
/// logical chunk length.
pub fn build_event_order_certificate(
    verified_plan: &VerifiedVectorPlan,
    routed: &VerifiedRoutedFir,
    limits: EventLimits,
) -> Result<VerifiedEventOrderCertificate, VectorEventError> {
    let plan = verified_plan.plan();
    let certificate = derive_certificate(plan, routed, None, limits)?;
    verify_event_order_certificate(plan, routed, &certificate, limits)?;
    Ok(VerifiedEventOrderCertificate { certificate })
}
/// Produces event-order evidence refined by checked state transitions.
pub fn build_state_event_order_certificate(
    verified_plan: &VerifiedVectorPlan,
    routed: &VerifiedRoutedFir,
    state: &VerifiedVectorStatePlan,
    limits: EventLimits,
) -> Result<VerifiedEventOrderCertificate, VectorEventError> {
    let plan = verified_plan.plan();
    if state.vector_plan() != plan {
        return Err(VectorEventError::StatePlanMismatch);
    }
    let certificate = derive_certificate(plan, routed, Some(state), limits)?;
    verify_state_event_order_certificate(plan, routed, state, &certificate, limits)?;
    Ok(VerifiedEventOrderCertificate { certificate })
}
pub(super) fn derive_certificate(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    limits: EventLimits,
) -> Result<EventOrderCertificate, VectorEventError> {
    verify_vector_plan(plan)?;
    if routed.plan() != plan {
        return Err(VectorEventError::RoutedPlanMismatch);
    }
    validate_layout(plan, routed)?;

    if state.is_some_and(|state| state.vector_plan() != plan) {
        return Err(VectorEventError::StatePlanMismatch);
    }
    let templates = event_templates(plan, routed, state)?;
    let checked_sample_count = producer_checked_sample_count(plan, &templates, state, limits)?;
    let mut checked_plan = plan.clone();
    checked_plan.vec_size = checked_sample_count;
    let needed = expanded_event_count(&checked_plan, &templates, state)?;
    let events = expand_events(&checked_plan, templates, state)?;
    debug_assert_eq!(events.len(), needed);
    if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
        eprintln!(
            "[vector-event-size] logical_samples={} checked_samples={} events={} complete_limit={} compact_limit={}",
            plan.vec_size,
            checked_sample_count,
            events.len(),
            limits.complete,
            limits.compact
        );
    }
    let contexts = context_events(&events);
    let scalar_loops = canonical_scalar_loops(&checked_plan);
    let vector_loops = routed_layout_loops(&checked_plan, routed);
    let scalar_order = build_order(&checked_plan, &contexts, &scalar_loops, true);
    let vector_order = build_order(&checked_plan, &contexts, &vector_loops, false);
    let dependencies = build_dependencies(&checked_plan, state, &events, &contexts, &scalar_order);

    Ok(EventOrderCertificate {
        sample_count: plan.vec_size,
        checked_sample_count,
        events,
        scalar_order,
        vector_order,
        dependencies,
    })
}
pub(super) fn producer_checked_sample_count(
    plan: &VectorPlan,
    templates: &BTreeMap<EventRegion, Vec<VectorEventKind>>,
    state: Option<&VerifiedVectorStatePlan>,
    limits: EventLimits,
) -> Result<u64, VectorEventError> {
    let needed = expanded_event_count(plan, templates, state)?;
    if needed <= limits.complete {
        return Ok(plan.vec_size);
    }
    let basis = plan.vec_size.min(COMPACT_EVENT_SAMPLE_BASIS as u64);
    let mut compact_plan = plan.clone();
    compact_plan.vec_size = basis;
    let compact_needed = expanded_event_count(&compact_plan, templates, state)?;
    if compact_needed > limits.compact {
        return Err(VectorEventError::EventBoundExceeded {
            needed: compact_needed,
            limit: limits.compact,
        });
    }
    Ok(basis)
}
pub(super) fn event_templates(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<BTreeMap<EventRegion, Vec<VectorEventKind>>, VectorEventError> {
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut templates = BTreeMap::<EventRegion, Vec<VectorEventKind>>::new();
    let managed = state.map_or_else(BTreeSet::new, VerifiedVectorStatePlan::managed_resources);
    for definition in routed.trace().definitions() {
        let region = event_region(definition.region);
        templates
            .entry(region)
            .or_default()
            .push(VectorEventKind::Definition {
                signal_id: definition.signal_id,
            });
        for (index, effect) in signals[&definition.signal_id]
            .direct_effects
            .iter()
            .enumerate()
        {
            if effect_is_managed(effect, &managed) {
                continue;
            }
            let effect_index =
                u64::try_from(index).map_err(|_| VectorEventError::EventCountOverflow)?;
            templates
                .entry(region)
                .or_default()
                .push(VectorEventKind::Effect {
                    signal_id: definition.signal_id,
                    effect_index,
                    effect: effect.clone(),
                });
        }
    }
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();
    for routed_transport in routed.trace().transports() {
        let transport = transports[&routed_transport.transport_id];
        templates
            .entry(EventRegion::Loop(transport.producer_loop))
            .or_default()
            .push(VectorEventKind::TransportStore {
                transport_id: transport.transport_id,
            });
        templates
            .entry(EventRegion::Loop(transport.consumer_loop))
            .or_default()
            .push(VectorEventKind::TransportLoad {
                transport_id: transport.transport_id,
            });
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
        templates
            .entry(EventRegion::Loop(consumer_loop))
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
    for events in templates.values_mut() {
        events.sort();
    }
    Ok(templates)
}
pub(super) fn expanded_event_count(
    plan: &VectorPlan,
    templates: &BTreeMap<EventRegion, Vec<VectorEventKind>>,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<usize, VectorEventError> {
    let mut total = plan
        .epochs
        .len()
        .checked_mul(2)
        .ok_or(VectorEventError::EventCountOverflow)?;
    total = total
        .checked_add(templates.get(&EventRegion::Control).map_or(0, Vec::len))
        .ok_or(VectorEventError::EventCountOverflow)?;
    let samples =
        usize::try_from(plan.vec_size).map_err(|_| VectorEventError::EventCountOverflow)?;
    for record in &plan.loops {
        let count = templates
            .get(&EventRegion::Loop(record.loop_id))
            .map_or(0, Vec::len);
        total = total
            .checked_add(
                count
                    .checked_mul(samples)
                    .ok_or(VectorEventError::EventCountOverflow)?,
            )
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    if let Some(state) = state {
        for phases in &state.plan().loops {
            total = total
                .checked_add(phases.pre.len())
                .and_then(|value| value.checked_add(phases.post.len()))
                .and_then(|value| {
                    phases
                        .exec
                        .len()
                        .checked_mul(samples)
                        .and_then(|exec| value.checked_add(exec))
                })
                .ok_or(VectorEventError::EventCountOverflow)?;
        }
    }
    Ok(total)
}
pub(super) fn expand_events(
    plan: &VectorPlan,
    templates: BTreeMap<EventRegion, Vec<VectorEventKind>>,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<Vec<VectorEvent>, VectorEventError> {
    let mut keys = Vec::new();
    if let Some(control) = templates.get(&EventRegion::Control) {
        for kind in control {
            keys.push((EventRegion::Control, None, kind.clone()));
        }
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
        if let Some(loop_templates) = templates.get(&EventRegion::Loop(record.loop_id)) {
            for sample in 0..plan.vec_size {
                for kind in loop_templates {
                    keys.push((
                        EventRegion::Loop(record.loop_id),
                        Some(sample),
                        kind.clone(),
                    ));
                }
            }
        }
    }
    if let Some(state) = state {
        append_state_event_keys(&mut keys, state, plan.vec_size);
    }
    keys.sort();
    keys.into_iter()
        .enumerate()
        .map(|(event_id, (region, sample, kind))| {
            Ok(VectorEvent {
                event_id: u64::try_from(event_id)
                    .map_err(|_| VectorEventError::EventCountOverflow)?,
                region,
                sample,
                kind,
            })
        })
        .collect()
}
pub(super) fn routed_layout_loops(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
) -> BTreeMap<u64, Vec<u64>> {
    let mut result = plan
        .epochs
        .iter()
        .map(|epoch| (epoch.epoch_id, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for region in routed.layout().loops() {
        result
            .get_mut(&region.epoch_id)
            .expect("layout validation checked epoch")
            .push(region.loop_id);
    }
    result
}
pub(super) fn build_order(
    plan: &VectorPlan,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    loops_by_epoch: &BTreeMap<u64, Vec<u64>>,
    sample_major: bool,
) -> Vec<u64> {
    let mut order = contexts
        .get(&(EventRegion::Control, None))
        .cloned()
        .unwrap_or_default();
    for epoch in sorted_epochs(plan) {
        order.extend(
            contexts[&(EventRegion::Epoch(epoch.epoch_id), None)]
                .iter()
                .copied()
                .filter(|event_id| is_epoch_enter(*event_id, contexts, epoch.epoch_id)),
        );
        let loop_order = &loops_by_epoch[&epoch.epoch_id];
        append_epoch_events(plan, contexts, loop_order, &mut order, sample_major);
        order.extend(
            contexts[&(EventRegion::Epoch(epoch.epoch_id), None)]
                .iter()
                .copied()
                .filter(|event_id| !is_epoch_enter(*event_id, contexts, epoch.epoch_id)),
        );
    }
    order
}
pub(super) fn append_epoch_events(
    plan: &VectorPlan,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    loops: &[u64],
    order: &mut Vec<u64>,
    sample_major: bool,
) {
    for &loop_id in loops {
        append_context(contexts, EventRegion::LoopPre(loop_id), None, order);
    }
    if sample_major {
        for sample in 0..plan.vec_size {
            for &loop_id in loops {
                append_context(contexts, EventRegion::Loop(loop_id), Some(sample), order);
            }
        }
    } else {
        let group_by_member = plan
            .fused_serial_groups
            .iter()
            .flat_map(|group| {
                group
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, group.group_id))
            })
            .collect::<BTreeMap<_, _>>();
        let bundle_by_member = plan
            .lockstep_bundles
            .iter()
            .flat_map(|bundle| {
                bundle
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, bundle.bundle_id))
            })
            .collect::<BTreeMap<_, _>>();
        let mut emitted_groups = BTreeSet::new();
        let mut emitted_bundles = BTreeSet::new();
        for &loop_id in loops {
            if let Some(&group_id) = group_by_member.get(&loop_id) {
                if !emitted_groups.insert(group_id) {
                    continue;
                }
                let members = &plan
                    .fused_serial_groups
                    .iter()
                    .find(|group| group.group_id == group_id)
                    .expect("member map came from a verified fused group")
                    .member_loop_ids;
                for sample in 0..plan.vec_size {
                    for &member in loops {
                        if members.binary_search(&member).is_ok() {
                            append_context(
                                contexts,
                                EventRegion::Loop(member),
                                Some(sample),
                                order,
                            );
                        }
                    }
                }
            } else if let Some(&bundle_id) = bundle_by_member.get(&loop_id) {
                if !emitted_bundles.insert(bundle_id) {
                    continue;
                }
                let members = &plan
                    .lockstep_bundles
                    .iter()
                    .find(|bundle| bundle.bundle_id == bundle_id)
                    .expect("member map came from a verified lockstep bundle")
                    .member_loop_ids;
                for sample in 0..plan.vec_size {
                    for &member in loops {
                        if members.binary_search(&member).is_ok() {
                            append_context(
                                contexts,
                                EventRegion::Loop(member),
                                Some(sample),
                                order,
                            );
                        }
                    }
                }
            } else {
                for sample in 0..plan.vec_size {
                    append_context(contexts, EventRegion::Loop(loop_id), Some(sample), order);
                }
            }
        }
    }
    for &loop_id in loops {
        append_context(contexts, EventRegion::LoopPost(loop_id), None, order);
    }
}
pub(super) fn append_context(
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    region: EventRegion,
    sample: Option<u64>,
    order: &mut Vec<u64>,
) {
    order.extend(
        contexts
            .get(&(region, sample))
            .into_iter()
            .flatten()
            .copied(),
    );
}
pub(super) fn is_epoch_enter(
    event_id: u64,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    epoch_id: u64,
) -> bool {
    contexts[&(EventRegion::Epoch(epoch_id), None)]
        .first()
        .is_some_and(|first| *first == event_id)
}
pub(super) fn build_dependencies(
    plan: &VectorPlan,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    scalar_order: &[u64],
) -> Vec<EventDependency> {
    let mut dependencies = BTreeSet::new();
    for event_ids in contexts.values() {
        for pair in event_ids.windows(2) {
            add_dependency(&mut dependencies, pair[0], pair[1]);
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
                add_dependency(&mut dependencies, *last, enter);
            }
        } else {
            let previous = &contexts[&(EventRegion::Epoch(epochs[index - 1].epoch_id), None)];
            add_dependency(&mut dependencies, previous[1], enter);
        }
        for &loop_id in &epoch.loops {
            if let Some(pre) = contexts.get(&(EventRegion::LoopPre(loop_id), None))
                && let (Some(first), Some(last)) = (pre.first(), pre.last())
            {
                add_dependency(&mut dependencies, enter, *first);
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_first) = local.first()
                    {
                        add_dependency(&mut dependencies, *last, *sample_first);
                    }
                }
            }
            for sample in 0..plan.vec_size {
                if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                    && let (Some(first), Some(last)) = (local.first(), local.last())
                {
                    add_dependency(&mut dependencies, enter, *first);
                    add_dependency(&mut dependencies, *last, exit);
                }
            }
            if let Some(post) = contexts.get(&(EventRegion::LoopPost(loop_id), None))
                && let (Some(first), Some(last)) = (post.first(), post.last())
            {
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_last) = local.last()
                    {
                        add_dependency(&mut dependencies, *sample_last, *first);
                    }
                }
                add_dependency(&mut dependencies, *last, exit);
            }
        }
    }

    for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
        add_loop_edge_dependencies(plan, contexts, *edge, &mut dependencies);
    }

    let by_id = events
        .iter()
        .map(|event| (event.event_id, event))
        .collect::<BTreeMap<_, _>>();
    let definitions = events
        .iter()
        .filter_map(|event| match event.kind {
            VectorEventKind::Definition { signal_id } => {
                Some(((event.region, signal_id, event.sample), event.event_id))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let stores = event_ids_by_transport(events, true);
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
                let definition = definitions[&(
                    EventRegion::Loop(transport.producer_loop),
                    transport.signal_id,
                    event.sample,
                )];
                add_dependency(&mut dependencies, definition, event.event_id);
                add_dependency(
                    &mut dependencies,
                    event.event_id,
                    loads[&(transport_id, event.sample)],
                );
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
                add_dependency(&mut dependencies, source_event, event.event_id);
            }
            _ => {}
        }
    }
    for ((transport_id, sample), store) in stores {
        add_dependency(&mut dependencies, store, loads[&(transport_id, sample)]);
    }

    let scalar_positions = positions(scalar_order);
    let effects = scalar_order
        .iter()
        .filter_map(|event_id| match &by_id[event_id].kind {
            VectorEventKind::Effect { effect, .. } => Some((*event_id, effect)),
            _ => None,
        })
        .collect::<Vec<_>>();
    dependencies.extend(producer_effect_dependencies(&effects));
    let recursion_steps = recursion_step_events(events);
    for ((loop_id, sample, group), event_id) in &recursion_steps {
        if *sample + 1 < plan.vec_size
            && let Some(next) = recursion_steps.get(&(*loop_id, *sample + 1, *group))
        {
            add_dependency(&mut dependencies, *event_id, *next);
        }
    }
    dependencies.extend(managed_state_dependencies(
        plan,
        state,
        events,
        &scalar_positions,
    ));
    dependencies.into_iter().collect()
}
/// Groups producer-side effect events by resource before materializing only
/// the conflicting pairs. Input order is the scalar event order.
pub(super) fn producer_effect_dependencies(
    effects: &[(u64, &EffectAtom)],
) -> BTreeSet<EventDependency> {
    let mut all = Vec::with_capacity(effects.len());
    let mut barriers = Vec::new();
    let mut states = BTreeMap::<StateResource, (Vec<u64>, Vec<u64>)>::new();
    let mut tables = BTreeMap::<u32, (Vec<u64>, Vec<u64>)>::new();
    let mut ui_writes = BTreeMap::<u32, Vec<u64>>::new();
    let mut output_writes = BTreeMap::<u32, Vec<u64>>::new();
    let positions = effects
        .iter()
        .enumerate()
        .map(|(position, (event_id, _))| (*event_id, position))
        .collect::<BTreeMap<_, _>>();

    for &(event_id, effect) in effects {
        all.push(event_id);
        match effect {
            EffectAtom::ReadState(resource) => {
                states.entry(resource.clone()).or_default().0.push(event_id);
            }
            EffectAtom::WriteState(resource) => {
                states.entry(resource.clone()).or_default().1.push(event_id);
            }
            EffectAtom::ReadTable(resource) => {
                tables.entry(*resource).or_default().0.push(event_id);
            }
            EffectAtom::WriteTable(resource) => {
                tables.entry(*resource).or_default().1.push(event_id);
            }
            EffectAtom::WriteUi(resource) => {
                ui_writes.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::WriteOutput(resource) => {
                output_writes.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::Foreign {
                purity: ForeignPurity::Impure | ForeignPurity::Unknown,
                ..
            } => barriers.push(event_id),
            EffectAtom::Foreign {
                purity: ForeignPurity::Pure,
                ..
            } => {}
        }
    }

    let mut dependencies = BTreeSet::new();
    let mut insert = |left: u64, right: u64| {
        if left == right {
            return;
        }
        let (before, after) = if positions[&left] < positions[&right] {
            (left, right)
        } else {
            (right, left)
        };
        dependencies.insert(EventDependency { before, after });
    };
    for (reads, writes) in states.values().chain(tables.values()) {
        for &read in reads {
            for &write in writes {
                insert(read, write);
            }
        }
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                insert(left, right);
            }
        }
    }
    for writes in ui_writes.values().chain(output_writes.values()) {
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                insert(left, right);
            }
        }
    }
    for &barrier in &barriers {
        for &other in &all {
            insert(barrier, other);
        }
    }
    dependencies
}
pub(super) fn add_loop_edge_dependencies(
    plan: &VectorPlan,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    edge: LoopEdge,
    dependencies: &mut BTreeSet<EventDependency>,
) {
    for sample in 0..plan.vec_size {
        let producer = contexts
            .get(&(EventRegion::Loop(edge.dependency), Some(sample)))
            .and_then(|events| events.last());
        let consumer = contexts
            .get(&(EventRegion::Loop(edge.consumer), Some(sample)))
            .and_then(|events| events.first());
        if let (Some(producer), Some(consumer)) = (producer, consumer) {
            add_dependency(dependencies, *producer, *consumer);
        }
    }
}
