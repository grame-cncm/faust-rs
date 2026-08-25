//! Independent checker and the shared terminal verify path.
//!
//! `verify_vector_state_plan_after_vector_plan` is called by BOTH the
//! producer's terminal verification (`build.rs`) and the standalone
//! checker entry points below, so its admission guards remain on both
//! paths (the shared-guard rule of the producer/checker doctrine in
//! `vector/mod.rs`).

use super::model::*;
use crate::signal_fir::vector::analysis::{DepKind, EffectAtom, StateCell, StateResource};
use crate::signal_fir::vector::clock_ad::VerifiedVectorClockAdPlan;
use crate::signal_fir::vector::decoration_verify::{
    CanonicalSigType, DecorationCertificate, DecorationRecord, VerifiedDecorationCertificate,
};
use crate::signal_fir::vector::recursion::decode_symbolic_group_bodies;
use crate::signal_fir::vector::verify::{
    LoopKind, Placement, Rate, ValueType, VectorPlan, verify_vector_plan,
};
use crate::signal_prepare::VerifiedPreparedSignals;
use signals::{SigId, SigMatch, match_sig};
use sigtype::{Nature, Variability};
use std::collections::{BTreeMap, BTreeSet};

/// Independently checks source alignment, storage equations, coverage, and phases.
pub fn verify_vector_state_plan(
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VectorPlan,
    state_plan: &VectorStatePlan,
) -> Result<(), VectorStateError> {
    verify_vector_state_plan_with_resources(None, decorations, vector_plan, None, state_plan)
}
/// Checks the state plan while accepting only the clock/hold resources named
/// by the clock/AD plan.
pub fn verify_vector_state_plan_with_clock(
    prepared: &VerifiedPreparedSignals,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VectorPlan,
    clock_plan: &VerifiedVectorClockAdPlan,
    state_plan: &VectorStatePlan,
) -> Result<(), VectorStateError> {
    if clock_plan.vector_plan() != vector_plan {
        return Err(VectorStateError::SignalCoverageMismatch {
            signal_id: u64::MAX,
        });
    }
    verify_vector_state_plan_with_resources(
        Some(prepared),
        decorations,
        vector_plan,
        Some(clock_plan),
        state_plan,
    )
}
pub(super) fn verify_vector_state_plan_with_resources(
    prepared: Option<&VerifiedPreparedSignals>,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VectorPlan,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
    state_plan: &VectorStatePlan,
) -> Result<(), VectorStateError> {
    verify_vector_plan(vector_plan)?;
    verify_vector_state_plan_after_vector_plan(
        prepared,
        decorations,
        vector_plan,
        clock_plan,
        state_plan,
    )
}
/// Checks state-specific obligations after the caller has independently
/// accepted the same vector plan. Production construction uses this boundary
/// to avoid repeating the full graph verification; the public checker above
/// remains independently fail-closed for arbitrary DTOs.
pub(super) fn verify_vector_state_plan_after_vector_plan(
    prepared: Option<&VerifiedPreparedSignals>,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VectorPlan,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
    state_plan: &VectorStatePlan,
) -> Result<(), VectorStateError> {
    if state_plan.schema_version != VECTOR_STATE_PLAN_VERSION {
        return Err(VectorStateError::UnsupportedSchema {
            found: state_plan.schema_version,
        });
    }
    if state_plan.vec_size != vector_plan.vec_size {
        return Err(VectorStateError::VecSizeMismatch {
            declared: state_plan.vec_size,
            actual: vector_plan.vec_size,
        });
    }
    let source = decorations.certificate();
    let records = &source.records;
    verify_source_alignment(records, vector_plan)?;
    verify_supported_state(records, vector_plan, state_plan, clock_plan)?;
    verify_recursions(prepared, records, vector_plan, state_plan, clock_plan)?;
    verify_delays(source, vector_plan, state_plan, clock_plan)?;
    let expected_registers = canonical_lockstep_register_bundles(
        prepared,
        vector_plan,
        &state_plan.delays,
        &state_plan.recursions,
    );
    if state_plan.lockstep_register_bundles != expected_registers {
        return Err(VectorStateError::DelayCoverageMismatch);
    }
    let (expected_prefixes, expected_waveforms) =
        expected_special_transitions(prepared, vector_plan)?;
    if state_plan.prefixes != expected_prefixes || state_plan.waveforms != expected_waveforms {
        return Err(VectorStateError::SignalFactMismatch {
            signal_id: u64::MAX,
        });
    }
    if state_plan.no_op_resources != expected_no_op_resources(records) {
        return Err(VectorStateError::DelayCoverageMismatch);
    }
    verify_phases(state_plan)?;
    Ok(())
}
pub(super) fn verify_source_alignment(
    records: &[DecorationRecord],
    plan: &VectorPlan,
) -> Result<(), VectorStateError> {
    if records.len() != plan.signals.len() {
        return Err(VectorStateError::SignalCoverageMismatch {
            signal_id: u64::MAX,
        });
    }
    for (record, signal) in records.iter().zip(&plan.signals) {
        let signal_id = u64::from(record.signal_id);
        if signal.signal_id != signal_id {
            return Err(VectorStateError::SignalCoverageMismatch { signal_id });
        }
        if signal.value_type != value_type(&record.sig_type)
            || signal.structural != record.is_symbolic_recursion_carrier
            || signal.rate != rate(record.variability)
            || signal.clock_id != record.clock_domain.map_or(0, |clock| u64::from(clock) + 1)
            || signal.effects != record.effects
        {
            return Err(VectorStateError::SignalFactMismatch { signal_id });
        }
    }
    Ok(())
}
pub(super) fn verify_supported_state(
    records: &[DecorationRecord],
    vector_plan: &VectorPlan,
    state_plan: &VectorStatePlan,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), VectorStateError> {
    let resources = managed_resources(state_plan);
    let reachable_recursions = records
        .iter()
        .filter_map(|record| {
            record
                .recursive_projection
                .map(|projection| StateResource::Recursion {
                    group: projection.group,
                    projection: u32::try_from(projection.index).expect("projection index fits u32"),
                })
        })
        .collect::<BTreeSet<_>>();
    let external_resources = clock_plan
        .map(VerifiedVectorClockAdPlan::managed_state_resources)
        .unwrap_or_default();
    let signals = vector_plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    for record in records {
        // Structural recursion carriers aggregate the effects of their
        // executable projection bodies but do not execute in a loop of their
        // own. Resource ownership is therefore checked on those bodies.
        if record.is_symbolic_recursion_carrier {
            continue;
        }
        for effect in &record.effects {
            let Some(resource) = state_resource(effect) else {
                continue;
            };
            // Projection records inherit the enclosing `SYMREC` aggregate
            // effects. An unreferenced identity body slot therefore appears
            // here even though it has no reachable `SIGPROJ` and C++ emits no
            // recursion storage for it. It is descriptive group metadata, not
            // a resource the state-transition plan must materialize.
            if matches!(resource, StateResource::Recursion { .. })
                && !reachable_recursions.contains(resource)
            {
                continue;
            }
            if resources.contains(resource) {
                if let Some(clock_id) = record.clock_domain {
                    let signal_id = u64::from(record.signal_id);
                    let signal = signals
                        .get(&signal_id)
                        .ok_or(VectorStateError::SignalCoverageMismatch { signal_id })?;
                    let Placement::Owned(loop_id) = signal.placement else {
                        return Err(VectorStateError::MissingLoopOwner { signal_id });
                    };
                    verify_clock_loop(signal_id, u64::from(clock_id), loop_id, clock_plan)?;
                }
            } else if !external_resources.contains(resource) {
                if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                    let related = records
                        .iter()
                        .filter(|candidate| match resource {
                            StateResource::Signal { owner, .. } => candidate.signal_id == *owner,
                            StateResource::Recursion { group, .. } => candidate
                                .recursive_projection
                                .is_some_and(|projection| projection.group == *group),
                        })
                        .map(|candidate| {
                            (
                                candidate.signal_id,
                                candidate.variability,
                                candidate.max_delay,
                                candidate.is_delay_read,
                                candidate.recursive_projection,
                                signals
                                    .get(&u64::from(candidate.signal_id))
                                    .map(|signal| signal.placement),
                            )
                        })
                        .collect::<Vec<_>>();
                    eprintln!(
                        "[vector-state-unsupported] resource={resource:?} source_signal={} related={related:?}",
                        record.signal_id
                    );
                }
                return Err(VectorStateError::UnsupportedStateResource {
                    resource: resource.clone(),
                });
            }
        }
    }
    Ok(())
}
/// Re-derives delay coverage for the state-plan checker without calling the
/// producer's projection helper.
pub(super) fn independently_expected_delay_requirements(
    source: &DecorationCertificate,
    placements: &BTreeMap<u64, Placement>,
) -> Vec<(u64, u64)> {
    let mut expected = source
        .records
        .iter()
        .filter(|record| record.max_delay > 0)
        .map(|record| (u64::from(record.signal_id), u64::from(record.max_delay)))
        .collect::<BTreeMap<_, _>>();
    for dependency in &source.dependencies {
        let DepKind::Delayed { amount } = dependency.kind else {
            continue;
        };
        let recursive_projection = source
            .records
            .binary_search_by_key(&dependency.from, |record| record.signal_id)
            .ok()
            .is_some_and(|index| source.records[index].recursive_projection.is_some());
        let distinct_owners = match (
            placements.get(&u64::from(dependency.from)),
            placements.get(&u64::from(dependency.to)),
        ) {
            (Some(Placement::Owned(source_loop)), Some(Placement::Owned(target_loop))) => {
                source_loop != target_loop
            }
            _ => false,
        };
        if recursive_projection && distinct_owners {
            expected
                .entry(u64::from(dependency.to))
                .and_modify(|maximum| *maximum = (*maximum).max(u64::from(amount)))
                .or_insert_with(|| u64::from(amount));
        }
    }
    expected.into_iter().collect()
}
pub(super) fn verify_delays(
    source: &DecorationCertificate,
    vector_plan: &VectorPlan,
    state_plan: &VectorStatePlan,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), VectorStateError> {
    check_strict_by(&state_plan.delays, "delay transitions", |delay| {
        delay.signal_id
    })?;
    let signals = vector_plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let placements = signals
        .iter()
        .map(|(signal_id, signal)| (*signal_id, signal.placement))
        .collect::<BTreeMap<_, _>>();
    let requirements = independently_expected_delay_requirements(source, &placements);
    let expected = requirements
        .iter()
        .map(|(signal_id, _)| *signal_id)
        .collect::<Vec<_>>();
    if state_plan
        .delays
        .iter()
        .map(|delay| delay.signal_id)
        .collect::<Vec<_>>()
        != expected
    {
        return Err(VectorStateError::DelayCoverageMismatch);
    }
    let loops = vector_plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record))
        .collect::<BTreeMap<_, _>>();
    for (transition, (signal_id, max_delay)) in state_plan.delays.iter().zip(requirements) {
        let record = source
            .records
            .binary_search_by_key(&u32::try_from(signal_id).unwrap_or(u32::MAX), |record| {
                record.signal_id
            })
            .ok()
            .map(|index| &source.records[index])
            .ok_or(VectorStateError::SignalCoverageMismatch { signal_id })?;
        let signal = signals[&transition.signal_id];
        let Placement::Owned(loop_id) = signal.placement else {
            return Err(VectorStateError::MissingLoopOwner {
                signal_id: transition.signal_id,
            });
        };
        if transition.loop_id != loop_id
            || transition.value_type != signal.value_type
            || transition.max_delay != max_delay
            || transition.clock_domain != record.clock_domain.map(u64::from)
        {
            return Err(VectorStateError::DelayCoverageMismatch);
        }
        verify_delay_owner(
            transition.signal_id,
            loop_id,
            loops[&loop_id].kind,
            transition.clock_domain,
            clock_plan,
        )?;
        let mut expected_storage = delay_storage(
            transition.signal_id,
            transition.max_delay,
            state_plan.vec_size,
            state_plan.max_copy_delay,
            transition.clock_domain,
        )?;
        if let Some((bundle, lane)) =
            state_plan
                .lockstep_register_bundles
                .iter()
                .find_map(|bundle| {
                    bundle
                        .lanes
                        .iter()
                        .find(|lane| lane.signal_id == transition.signal_id)
                        .map(|lane| (bundle, lane))
                })
        {
            expected_storage = VectorDelayStorage::Register {
                local_name: lane.local_name.clone(),
                persistent_name: lane.persistent_name.clone(),
                bundle_id: bundle.bundle_id,
                lane: lane.lane,
            };
        }
        if transition.storage != expected_storage {
            return Err(VectorStateError::DelayCoverageMismatch);
        }
    }
    Ok(())
}
pub(super) fn verify_recursions(
    prepared: Option<&VerifiedPreparedSignals>,
    records: &[DecorationRecord],
    vector_plan: &VectorPlan,
    state_plan: &VectorStatePlan,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), VectorStateError> {
    check_strict_by(&state_plan.recursions, "recursion transitions", |rec| {
        rec.group
    })?;
    let mut expected = BTreeMap::<u64, BTreeMap<u64, Vec<u64>>>::new();
    for record in records {
        if let Some(projection) = record.recursive_projection {
            expected
                .entry(u64::from(projection.group))
                .or_default()
                .entry(projection.index)
                .or_default()
                .push(u64::from(record.signal_id));
        }
    }
    // Mirror the producer's reachable-projection rule independently. Effects
    // on a structural `SYMREC` aggregate every body slot, including identity
    // slots with no reachable `SIGPROJ`; treating those as state would forge
    // phantom recursion words that C++ does not emit.
    let prepared_ids = prepared.map(collect_prepared_ids);
    let expected = expected
        .into_iter()
        .map(|(group, projections)| -> Result<_, VectorStateError> {
            Ok((
                group,
                projections
                    .into_iter()
                    .map(|(index, signal_ids)| {
                        Ok(RecursionProjectionTransition {
                            index,
                            value_signal_id: recursion_value_signal(
                                prepared,
                                prepared_ids.as_ref(),
                                group,
                                index,
                                &signal_ids,
                            )?,
                            signal_ids,
                        })
                    })
                    .collect::<Result<Vec<_>, VectorStateError>>()?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, VectorStateError>>()?;
    if state_plan.recursions.len() != expected.len() {
        return Err(VectorStateError::RecursionCoverageMismatch);
    }
    for transition in &state_plan.recursions {
        if expected.get(&transition.group) != Some(&transition.projections) {
            return Err(VectorStateError::RecursionCoverageMismatch);
        }
        let loop_id = recursion_loop(vector_plan, transition.group)?;
        if transition.loop_id != loop_id {
            return Err(VectorStateError::RecursionLoopMismatch {
                group: transition.group,
                loop_id: transition.loop_id,
            });
        }
        for projection in &transition.projections {
            check_strict_by(
                &projection.signal_ids,
                "recursion projection aliases",
                |id| *id,
            )?;
            for &signal_id in &projection.signal_ids {
                let signal = vector_plan
                    .signals
                    .iter()
                    .find(|signal| signal.signal_id == signal_id)
                    .ok_or(VectorStateError::SignalCoverageMismatch { signal_id })?;
                if signal.placement != Placement::Owned(loop_id) {
                    return Err(VectorStateError::RecursionLoopMismatch {
                        group: transition.group,
                        loop_id,
                    });
                }
                if let Some(domain_id) = signal.clock_id.checked_sub(1) {
                    verify_clock_loop(signal_id, domain_id, loop_id, clock_plan)?;
                }
            }
            let value = vector_plan
                .signals
                .iter()
                .find(|signal| signal.signal_id == projection.value_signal_id)
                .ok_or(VectorStateError::SignalCoverageMismatch {
                    signal_id: projection.value_signal_id,
                })?;
            if value.placement == Placement::Control {
                return Err(VectorStateError::RecursionLoopMismatch {
                    group: transition.group,
                    loop_id,
                });
            }
        }
    }
    Ok(())
}
pub(super) fn verify_phases(state_plan: &VectorStatePlan) -> Result<(), VectorStateError> {
    check_strict_by(&state_plan.loops, "stateful loops", |phases| phases.loop_id)?;
    let mut expected = BTreeMap::<u64, LoopStatePhases>::new();
    for delay in &state_plan.delays {
        let phases = expected
            .entry(delay.loop_id)
            .or_insert_with(|| empty_phases(delay.loop_id));
        match delay.storage {
            VectorDelayStorage::Register { .. } => {
                phases.pre.push(VectorStateAction::DelayRegisterLoad {
                    signal_id: delay.signal_id,
                });
                phases.post.push(VectorStateAction::DelayRegisterStore {
                    signal_id: delay.signal_id,
                });
            }
            VectorDelayStorage::Copy { .. } => {
                phases.pre.push(VectorStateAction::DelayCopyIn {
                    signal_id: delay.signal_id,
                });
                phases.post.push(VectorStateAction::DelayCopyOut {
                    signal_id: delay.signal_id,
                });
            }
            VectorDelayStorage::Ring { .. } => {
                phases.pre.push(VectorStateAction::DelayRingAdvance {
                    signal_id: delay.signal_id,
                });
                phases.post.push(VectorStateAction::DelayRingSaveAdvance {
                    signal_id: delay.signal_id,
                });
            }
            VectorDelayStorage::ClockRing { .. } => {}
        }
        phases.exec.push(VectorStateAction::DelayWrite {
            signal_id: delay.signal_id,
        });
    }
    for recursion in &state_plan.recursions {
        expected
            .entry(recursion.loop_id)
            .or_insert_with(|| empty_phases(recursion.loop_id))
            .exec
            .push(VectorStateAction::RecursionStep {
                group: recursion.group,
            });
    }
    for prefix in &state_plan.prefixes {
        expected
            .entry(prefix.loop_id)
            .or_insert_with(|| empty_phases(prefix.loop_id))
            .exec
            .push(VectorStateAction::PrefixWrite {
                signal_id: prefix.signal_id,
            });
    }
    for waveform in &state_plan.waveforms {
        expected
            .entry(waveform.loop_id)
            .or_insert_with(|| empty_phases(waveform.loop_id))
            .exec
            .push(VectorStateAction::WaveformAdvance {
                signal_id: waveform.signal_id,
            });
    }
    for phases in expected.values_mut() {
        phases.pre.sort();
        phases.exec.sort();
        phases.post.sort();
    }
    let expected = expected.into_values().collect::<Vec<_>>();
    if state_plan.loops != expected {
        let loop_id = state_plan
            .loops
            .iter()
            .zip(&expected)
            .find(|(left, right)| left != right)
            .map_or(u64::MAX, |(left, _)| left.loop_id);
        return Err(VectorStateError::LoopPhaseMismatch { loop_id });
    }
    for phases in &state_plan.loops {
        if phases
            .pre
            .iter()
            .any(|action| action.phase() != VectorStatePhase::Pre)
            || phases
                .exec
                .iter()
                .any(|action| action.phase() != VectorStatePhase::Exec)
            || phases
                .post
                .iter()
                .any(|action| action.phase() != VectorStatePhase::Post)
        {
            return Err(VectorStateError::LoopPhaseMismatch {
                loop_id: phases.loop_id,
            });
        }
    }
    Ok(())
}
pub(super) fn delay_storage(
    signal_id: u64,
    max_delay: u64,
    vec_size: u64,
    max_copy_delay: u64,
    clock_domain: Option<u64>,
) -> Result<VectorDelayStorage, VectorStateError> {
    let base = format!("vstate_s{signal_id}");
    if let Some(domain_id) = clock_domain {
        let required = max_delay
            .checked_add(1)
            .ok_or(VectorStateError::ArithmeticOverflow { signal_id })?;
        let capacity = required
            .checked_next_power_of_two()
            .ok_or(VectorStateError::ArithmeticOverflow { signal_id })?;
        return Ok(VectorDelayStorage::ClockRing {
            buffer_name: base,
            cursor_name: format!("vclock_d{domain_id}_iota"),
            domain_id,
            capacity,
            mask: capacity - 1,
        });
    }
    if max_delay < max_copy_delay {
        let history_length = max_delay
            .checked_add(3)
            .map(|value| value & !3)
            .ok_or(VectorStateError::ArithmeticOverflow { signal_id })?;
        let temporary_length = history_length
            .checked_add(vec_size)
            .ok_or(VectorStateError::ArithmeticOverflow { signal_id })?;
        Ok(VectorDelayStorage::Copy {
            temporary_name: format!("{base}_tmp"),
            permanent_name: format!("{base}_perm"),
            history_length,
            temporary_length,
        })
    } else {
        let required = max_delay
            .checked_add(vec_size)
            .ok_or(VectorStateError::ArithmeticOverflow { signal_id })?;
        let capacity = required
            .checked_next_power_of_two()
            .ok_or(VectorStateError::ArithmeticOverflow { signal_id })?;
        Ok(VectorDelayStorage::Ring {
            buffer_name: base.clone(),
            index_name: format!("{base}_idx"),
            index_save_name: format!("{base}_idx_save"),
            capacity,
            mask: capacity - 1,
        })
    }
}
/// Reconstructs the only initially supported register-carry shape from
/// independently checked vector-plan and state facts: every lane in the
/// bundle owns one scalar recursion projection and one matching top-rate
/// delay-one carrier. A partially eligible bundle remains array-backed.
pub(super) fn canonical_lockstep_register_bundles(
    prepared: Option<&VerifiedPreparedSignals>,
    plan: &VectorPlan,
    delays: &[DelayTransition],
    recursions: &[RecursionTransition],
) -> Vec<LockstepRegisterBundle> {
    let mut result = Vec::new();
    for bundle in &plan.lockstep_bundles {
        let lanes = bundle
            .lanes
            .iter()
            .enumerate()
            .map(|(lane_index, lane)| {
                let recursion = recursions
                    .iter()
                    .find(|recursion| recursion.group == lane.recursion_group)?;
                let [_projection] = recursion.projections.as_slice() else {
                    return None;
                };
                let matching = delays
                    .iter()
                    .filter(|delay| {
                        delay.loop_id == lane.loop_id
                            && delay.max_delay == 1
                            && delay.clock_domain.is_none()
                    })
                    .collect::<Vec<_>>();
                let [delay] = matching.as_slice() else {
                    return None;
                };
                if !has_only_fixed_delay_one_reads(prepared?, delay.signal_id) {
                    return None;
                }
                let lane = u64::try_from(lane_index).ok()?;
                let base = format!("vlock_b{}_l{lane}_s{}", bundle.bundle_id, delay.signal_id);
                Some(LockstepRegisterLane {
                    lane,
                    loop_id: delay.loop_id,
                    recursion_group: recursion.group,
                    signal_id: delay.signal_id,
                    local_name: format!("{base}_local"),
                    persistent_name: format!("{base}_state"),
                })
            })
            .collect::<Option<Vec<_>>>();
        if let Some(lanes) = lanes {
            result.push(LockstepRegisterBundle {
                bundle_id: bundle.bundle_id,
                lanes,
            });
        }
    }
    result
}
pub(super) fn has_only_fixed_delay_one_reads(
    prepared: &VerifiedPreparedSignals,
    signal_id: u64,
) -> bool {
    let ids = collect_prepared_ids(prepared);
    let Some(&carrier) = ids.get(&signal_id) else {
        return false;
    };
    let mut found = false;
    for signal in ids.values().copied() {
        match match_sig(prepared.arena(), signal) {
            SigMatch::Delay1(value) if value == carrier => found = true,
            SigMatch::Delay(value, amount) if value == carrier => {
                if !matches!(match_sig(prepared.arena(), amount), SigMatch::Int(1)) {
                    return false;
                }
                found = true;
            }
            _ => {}
        }
    }
    found
}
pub(super) fn verify_delay_owner(
    signal_id: u64,
    loop_id: u64,
    kind: LoopKind,
    clock_domain: Option<u64>,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), VectorStateError> {
    if let Some(domain_id) = clock_domain {
        return verify_clock_loop(signal_id, domain_id, loop_id, clock_plan);
    }
    // Every top-rate plan loop is emitted as an inner sample loop. `Island`
    // denotes a conservative serial loop when state/effects prevent sample
    // reordering; it is therefore a valid delay owner. Clock-domain islands
    // take the separate checked branch above.
    if matches!(
        kind,
        LoopKind::Vectorizable
            | LoopKind::Recursive(_)
            | LoopKind::Island(_)
            | LoopKind::Lockstep { .. }
    ) {
        Ok(())
    } else {
        Err(VectorStateError::DelayOwnerNotVectorLoop { signal_id, loop_id })
    }
}
pub(super) fn verify_clock_loop(
    signal_id: u64,
    domain_id: u64,
    loop_id: u64,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), VectorStateError> {
    let Some(clock_plan) = clock_plan else {
        return Err(VectorStateError::ClockPlanRequired {
            signal_id,
            clock_id: u32::try_from(domain_id).unwrap_or(u32::MAX),
        });
    };
    if clock_plan
        .plan()
        .clock_islands
        .iter()
        .any(|island| island.domain_id == domain_id && island.nested_loop_ids.contains(&loop_id))
    {
        Ok(())
    } else {
        Err(VectorStateError::ClockLoopMismatch {
            signal_id,
            clock_id: domain_id,
            loop_id,
        })
    }
}
pub(super) fn recursion_loop(plan: &VectorPlan, group: u64) -> Result<u64, VectorStateError> {
    let mut matches = plan
        .loops
        .iter()
        .filter(|record| record.kind == LoopKind::Recursive(group))
        .map(|record| record.loop_id)
        .collect::<Vec<_>>();
    matches.extend(plan.lockstep_bundles.iter().flat_map(|bundle| {
        bundle
            .lanes
            .iter()
            .filter_map(|lane| (lane.recursion_group == group).then_some(lane.loop_id))
    }));
    matches.sort_unstable();
    matches.dedup();
    if matches.len() != 1 {
        return Err(VectorStateError::RecursionLoopMismatch {
            group,
            loop_id: matches.first().copied().unwrap_or(u64::MAX),
        });
    }
    Ok(matches[0])
}
pub(super) fn empty_phases(loop_id: u64) -> LoopStatePhases {
    LoopStatePhases {
        loop_id,
        pre: Vec::new(),
        exec: Vec::new(),
        post: Vec::new(),
    }
}
pub(super) fn managed_resources(plan: &VectorStatePlan) -> BTreeSet<StateResource> {
    let mut result = plan
        .delays
        .iter()
        .map(|delay| StateResource::Signal {
            owner: u32::try_from(delay.signal_id).expect("decorated signal id fits u32"),
            cell: StateCell::Delay,
        })
        .collect::<BTreeSet<_>>();
    for recursion in &plan.recursions {
        for projection in &recursion.projections {
            result.insert(StateResource::Recursion {
                group: u32::try_from(recursion.group).expect("decorated recursion group fits u32"),
                projection: u32::try_from(projection.index)
                    .expect("decorated recursion projection fits u32"),
            });
        }
    }
    result.extend(
        plan.prefixes
            .iter()
            .map(|transition| StateResource::Signal {
                owner: u32::try_from(transition.signal_id).expect("decorated signal id fits u32"),
                cell: StateCell::Prefix,
            }),
    );
    result.extend(
        plan.waveforms
            .iter()
            .map(|transition| StateResource::Signal {
                owner: u32::try_from(transition.signal_id).expect("decorated signal id fits u32"),
                cell: StateCell::WaveformIndex,
            }),
    );
    result.extend(plan.no_op_resources.iter().cloned());
    result
}
pub(super) fn expected_no_op_resources(records: &[DecorationRecord]) -> Vec<StateResource> {
    let max_delay_by_signal = records
        .iter()
        .map(|record| (record.signal_id, record.max_delay))
        .collect::<BTreeMap<_, _>>();
    records
        .iter()
        .flat_map(|record| record.effects.iter())
        .filter_map(state_resource)
        .filter(|resource| {
            matches!(
                resource,
                StateResource::Signal {
                    owner,
                    cell: StateCell::Delay
                } if max_delay_by_signal.get(owner) == Some(&0)
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub(super) fn expected_special_transitions(
    prepared: Option<&VerifiedPreparedSignals>,
    plan: &VectorPlan,
) -> Result<(Vec<PrefixTransition>, Vec<WaveformTransition>), VectorStateError> {
    let resources = plan
        .signals
        .iter()
        .flat_map(|signal| signal.effects.iter())
        .filter_map(state_resource)
        .filter(|resource| {
            matches!(
                resource,
                StateResource::Signal {
                    cell: StateCell::Prefix | StateCell::WaveformIndex,
                    ..
                }
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if resources.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let prepared = prepared.ok_or_else(|| VectorStateError::UnsupportedStateResource {
        resource: resources.first().expect("non-empty resource set").clone(),
    })?;
    let ids = collect_prepared_ids(prepared);
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut prefixes = Vec::new();
    let mut waveforms = Vec::new();
    for resource in resources {
        let StateResource::Signal { owner, cell } = resource else {
            unreachable!("resource filter keeps signal-owned state");
        };
        let signal_id = u64::from(owner);
        let signal = signals
            .get(&signal_id)
            .copied()
            .ok_or(VectorStateError::SignalCoverageMismatch { signal_id })?;
        let Placement::Owned(loop_id) = signal.placement else {
            return Err(VectorStateError::MissingLoopOwner { signal_id });
        };
        let sig = ids
            .get(&signal_id)
            .copied()
            .ok_or(VectorStateError::SignalCoverageMismatch { signal_id })?;
        match (cell, match_sig(prepared.arena(), sig)) {
            (StateCell::Prefix, SigMatch::Prefix(initial, value)) => {
                let initial = match match_sig(prepared.arena(), initial) {
                    SigMatch::Int(value) => VectorStateInitialValue::Int(value),
                    SigMatch::Real(value) => VectorStateInitialValue::RealBits(value.to_bits()),
                    _ => VectorStateInitialValue::Zero,
                };
                prefixes.push(PrefixTransition {
                    signal_id,
                    loop_id,
                    value_signal_id: u64::from(value.as_u32()),
                    state_name: format!("vprefix_s{signal_id}"),
                    value_type: signal.value_type.clone(),
                    initial,
                });
            }
            (StateCell::WaveformIndex, SigMatch::Waveform(values)) if !values.is_empty() => {
                waveforms.push(WaveformTransition {
                    signal_id,
                    loop_id,
                    index_name: format!("vwave_s{signal_id}_index"),
                    length: u64::try_from(values.len())
                        .map_err(|_| VectorStateError::ArithmeticOverflow { signal_id })?,
                    value_type: signal.value_type.clone(),
                });
            }
            _ => {
                return Err(VectorStateError::UnsupportedStateResource {
                    resource: StateResource::Signal { owner, cell },
                });
            }
        }
    }
    Ok((prefixes, waveforms))
}
pub(super) fn state_resource(effect: &EffectAtom) -> Option<&StateResource> {
    match effect {
        EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource) => Some(resource),
        _ => None,
    }
}
pub(super) fn collect_prepared_ids(prepared: &VerifiedPreparedSignals) -> BTreeMap<u64, SigId> {
    let mut ids = BTreeMap::new();
    let mut stack = prepared.outputs().to_vec();
    while let Some(signal) = stack.pop() {
        if ids.insert(u64::from(signal.as_u32()), signal).is_some() {
            continue;
        }
        if let Some(children) = prepared.arena().children(signal) {
            stack.extend(children.iter().copied());
        }
    }
    ids
}
pub(super) fn recursion_value_signal(
    prepared: Option<&VerifiedPreparedSignals>,
    prepared_ids: Option<&BTreeMap<u64, SigId>>,
    group: u64,
    index: u64,
    aliases: &[u64],
) -> Result<u64, VectorStateError> {
    if let Some(alias) = aliases.first().copied() {
        return Ok(alias);
    }
    let resource = || StateResource::Recursion {
        group: u32::try_from(group).unwrap_or(u32::MAX),
        projection: u32::try_from(index).unwrap_or(u32::MAX),
    };
    let prepared = prepared.ok_or_else(|| VectorStateError::UnsupportedStateResource {
        resource: resource(),
    })?;
    let group_signal = prepared_ids
        .and_then(|ids| ids.get(&group))
        .copied()
        .ok_or(VectorStateError::SignalCoverageMismatch { signal_id: group })?;
    let (_, bodies) =
        decode_symbolic_group_bodies(prepared.arena(), group_signal).ok_or_else(|| {
            VectorStateError::UnsupportedStateResource {
                resource: resource(),
            }
        })?;
    let index = usize::try_from(index)
        .map_err(|_| VectorStateError::ArithmeticOverflow { signal_id: group })?;
    bodies
        .get(if bodies.len() == 1 { 0 } else { index })
        .map(|signal| u64::from(signal.as_u32()))
        .ok_or(VectorStateError::RecursionArityMismatch {
            state: bodies.len(),
            next: index + 1,
        })
}
fn check_strict_by<T, K: Ord>(
    values: &[T],
    what: &'static str,
    key: impl Fn(&T) -> K,
) -> Result<(), VectorStateError> {
    if let Some(at) = values
        .windows(2)
        .position(|pair| key(&pair[0]) >= key(&pair[1]))
    {
        return Err(VectorStateError::NotCanonical { what, at: at + 1 });
    }
    Ok(())
}
pub(super) fn rate(variability: Variability) -> Rate {
    match variability {
        Variability::Konst => Rate::Konst,
        Variability::Block => Rate::Block,
        Variability::Samp => Rate::Samp,
    }
}
pub(super) fn value_type(sig_type: &CanonicalSigType) -> ValueType {
    match sig_type {
        CanonicalSigType::Sound => ValueType::Sound,
        CanonicalSigType::Simple { nature, .. } => scalar_value_type(*nature),
        CanonicalSigType::Table { nature, .. } => scalar_value_type(*nature),
        CanonicalSigType::Tuplet { components, .. } => {
            ValueType::Tuple(components.iter().map(value_type).collect())
        }
    }
}
pub(super) fn scalar_value_type(nature: Nature) -> ValueType {
    match nature {
        Nature::Int => ValueType::Int,
        Nature::Real | Nature::Any => ValueType::Real,
    }
}
