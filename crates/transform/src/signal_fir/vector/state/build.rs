//! Producer construction of the vector state plan. The terminal step
//! calls the shared verify path in `check.rs`, so every admission guard
//! there also binds the producer (plan provenance: §4.8).

use super::check::*;
use super::model::*;
use crate::signal_fir::vector::analysis::DepKind;
use crate::signal_fir::vector::clock_ad::VerifiedVectorClockAdPlan;
use crate::signal_fir::vector::decoration_verify::{
    DecorationCertificate, DecorationRecord, VerifiedDecorationCertificate,
};
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::verify::Placement;
use crate::signal_prepare::VerifiedPreparedSignals;
use std::collections::BTreeMap;

/// Builds and independently checks the state-transition plan.
pub fn build_vector_state_plan(
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VerifiedVectorPlan,
    max_copy_delay: u64,
) -> Result<VerifiedVectorStatePlan, VectorStateError> {
    build_vector_state_plan_with_resources(None, decorations, vector_plan, None, max_copy_delay)
}
/// Builds state transitions while delegating clock/hold resources to an
/// independently accepted clock/AD-plan artifact.
pub fn build_vector_state_plan_with_clock(
    prepared: &VerifiedPreparedSignals,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VerifiedVectorPlan,
    clock_plan: &VerifiedVectorClockAdPlan,
    max_copy_delay: u64,
) -> Result<VerifiedVectorStatePlan, VectorStateError> {
    if clock_plan.vector_plan() != vector_plan.plan() {
        return Err(VectorStateError::SignalCoverageMismatch {
            signal_id: u64::MAX,
        });
    }
    build_vector_state_plan_with_resources(
        Some(prepared),
        decorations,
        vector_plan,
        Some(clock_plan),
        max_copy_delay,
    )
}
pub(super) fn build_vector_state_plan_with_resources(
    prepared: Option<&VerifiedPreparedSignals>,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VerifiedVectorPlan,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
    max_copy_delay: u64,
) -> Result<VerifiedVectorStatePlan, VectorStateError> {
    let source = decorations.certificate();
    let plan = vector_plan.plan();
    verify_source_alignment(source.records.as_slice(), plan)?;

    let loops_by_id = plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record))
        .collect::<BTreeMap<_, _>>();
    let signals_by_id = plan
        .signals
        .iter()
        .map(|record| (record.signal_id, record))
        .collect::<BTreeMap<_, _>>();
    let mut phases = BTreeMap::<u64, LoopStatePhases>::new();
    let mut delays = Vec::new();
    let placements = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal.placement))
        .collect::<BTreeMap<_, _>>();

    for (record, max_delay) in effective_delay_requirements(source, &placements) {
        let signal_id = u64::from(record.signal_id);
        let signal = signals_by_id[&signal_id];
        let Placement::Owned(loop_id) = signal.placement else {
            return Err(VectorStateError::MissingLoopOwner { signal_id });
        };
        let loop_record = loops_by_id[&loop_id];
        let clock_domain = record.clock_domain.map(u64::from);
        verify_delay_owner(
            signal_id,
            loop_id,
            loop_record.kind,
            clock_domain,
            clock_plan,
        )?;
        let storage = delay_storage(
            signal_id,
            max_delay,
            plan.vec_size,
            max_copy_delay,
            clock_domain,
        )?;
        let loop_phases = phases
            .entry(loop_id)
            .or_insert_with(|| empty_phases(loop_id));
        match storage {
            VectorDelayStorage::Register { .. } => {
                unreachable!("generic delay selection does not produce register storage")
            }
            VectorDelayStorage::Copy { .. } => {
                loop_phases
                    .pre
                    .push(VectorStateAction::DelayCopyIn { signal_id });
                loop_phases
                    .post
                    .push(VectorStateAction::DelayCopyOut { signal_id });
            }
            VectorDelayStorage::Ring { .. } => {
                loop_phases
                    .pre
                    .push(VectorStateAction::DelayRingAdvance { signal_id });
                loop_phases
                    .post
                    .push(VectorStateAction::DelayRingSaveAdvance { signal_id });
            }
            VectorDelayStorage::ClockRing { .. } => {}
        }
        loop_phases
            .exec
            .push(VectorStateAction::DelayWrite { signal_id });
        delays.push(DelayTransition {
            signal_id,
            loop_id,
            value_type: signal.value_type.clone(),
            max_delay,
            clock_domain,
            storage,
        });
    }

    let mut recursion_groups = BTreeMap::<u64, BTreeMap<u64, Vec<u64>>>::new();
    for record in source.records.iter().filter_map(|record| {
        record
            .recursive_projection
            .map(|projection| (record, projection))
    }) {
        recursion_groups
            .entry(u64::from(record.1.group))
            .or_default()
            .entry(record.1.index)
            .or_default()
            .push(u64::from(record.0.signal_id));
    }
    // A symbolic-recursion carrier aggregates a read/write effect for every
    // body slot, including identity/pass-through slots which have no reachable
    // `SIGPROJ`. Those aggregate effects describe the group, not physical
    // state. Only a reachable projection needs a recursion transition;
    // otherwise C++ emits the body as an ordinary direct value (for example
    // `(_*0.5,_*0.5,_,_)~(_,_)` emits only two `fRec` lines).
    let prepared_ids = prepared.map(collect_prepared_ids);
    let mut recursions = Vec::new();
    for (group, projections) in recursion_groups {
        let projections = projections
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
            .collect::<Result<Vec<_>, VectorStateError>>()?;
        let loop_id = recursion_loop(plan, group)?;
        phases
            .entry(loop_id)
            .or_insert_with(|| empty_phases(loop_id))
            .exec
            .push(VectorStateAction::RecursionStep { group });
        recursions.push(RecursionTransition {
            group,
            loop_id,
            projections,
        });
    }

    let lockstep_register_bundles =
        canonical_lockstep_register_bundles(prepared, plan, &delays, &recursions);
    promote_lockstep_register_delays(&lockstep_register_bundles, &mut delays, &mut phases);

    let (prefixes, waveforms) = expected_special_transitions(prepared, plan)?;
    for transition in &prefixes {
        phases
            .entry(transition.loop_id)
            .or_insert_with(|| empty_phases(transition.loop_id))
            .exec
            .push(VectorStateAction::PrefixWrite {
                signal_id: transition.signal_id,
            });
    }
    for transition in &waveforms {
        phases
            .entry(transition.loop_id)
            .or_insert_with(|| empty_phases(transition.loop_id))
            .exec
            .push(VectorStateAction::WaveformAdvance {
                signal_id: transition.signal_id,
            });
    }

    for loop_phases in phases.values_mut() {
        loop_phases.pre.sort();
        loop_phases.exec.sort();
        loop_phases.post.sort();
    }
    let state_plan = VectorStatePlan {
        schema_version: VECTOR_STATE_PLAN_VERSION,
        vec_size: plan.vec_size,
        max_copy_delay,
        loops: phases.into_values().collect(),
        delays,
        recursions,
        lockstep_register_bundles,
        prefixes,
        waveforms,
        no_op_resources: expected_no_op_resources(&source.records),
    };
    verify_vector_state_plan_after_vector_plan(
        prepared,
        decorations,
        plan,
        clock_plan,
        &state_plan,
    )?;
    let delegated_resources = clock_plan
        .map(VerifiedVectorClockAdPlan::managed_state_resources)
        .unwrap_or_default();
    Ok(VerifiedVectorStatePlan {
        plan: state_plan,
        vector_plan: plan.clone(),
        delegated_resources,
    })
}
/// Returns the effective history obligation for every carried signal.
///
/// C++ `getSignalDependencies` marks `sigProj(..., sigRef(...))` as a
/// one-sample dependency on the selected recursive body, while `OccMarkup`
/// marks the structural recursion carrier and can therefore leave that body
/// at `max_delay == 0`. The state plan closes that intentional projection gap locally
/// when the projection and selected body have distinct loop owners (the
/// cross-loop pass-through alias case): the delayed scheduling edge then
/// requires storage for the selected producer. Same-loop and explicitly
/// delayed projections retain their existing carrier storage. This preserves
/// the previous-sample back-edge without rewriting the prepared scalar tree.
pub(super) fn effective_delay_requirements<'a>(
    source: &'a DecorationCertificate,
    placements: &BTreeMap<u64, Placement>,
) -> Vec<(&'a DecorationRecord, u64)> {
    let mut maxima = source
        .records
        .iter()
        .map(|record| (record.signal_id, u64::from(record.max_delay)))
        .collect::<BTreeMap<_, _>>();
    let records = source
        .records
        .iter()
        .map(|record| (record.signal_id, record))
        .collect::<BTreeMap<_, _>>();
    for dependency in &source.dependencies {
        if let DepKind::Delayed { amount } = dependency.kind {
            // An explicit `sigDelay` occurrence already allocates storage for
            // the projection itself. X2b concerns the distinct cross-loop
            // pass-through case, where lowering the alias also needs history
            // for its selected body rather than a current-value transport.
            let pass_through_projection = records
                .get(&dependency.from)
                .is_some_and(|record| record.recursive_projection.is_some())
                && matches!(
                    (
                        placements.get(&u64::from(dependency.from)),
                        placements.get(&u64::from(dependency.to)),
                    ),
                    (Some(Placement::Owned(from)), Some(Placement::Owned(to))) if from != to
                );
            if !pass_through_projection {
                continue;
            }
            maxima
                .entry(dependency.to)
                .and_modify(|maximum| *maximum = (*maximum).max(u64::from(amount)))
                .or_insert_with(|| u64::from(amount));
        }
    }
    source
        .records
        .iter()
        .filter_map(|record| {
            maxima
                .get(&record.signal_id)
                .copied()
                .filter(|maximum| *maximum > 0)
                .map(|maximum| (record, maximum))
        })
        .collect()
}
pub(super) fn promote_lockstep_register_delays(
    bundles: &[LockstepRegisterBundle],
    delays: &mut [DelayTransition],
    phases: &mut BTreeMap<u64, LoopStatePhases>,
) {
    for bundle in bundles {
        for lane in &bundle.lanes {
            let delay = delays
                .iter_mut()
                .find(|delay| delay.signal_id == lane.signal_id)
                .expect("canonical register lane names one delay transition");
            delay.storage = VectorDelayStorage::Register {
                local_name: lane.local_name.clone(),
                persistent_name: lane.persistent_name.clone(),
                bundle_id: bundle.bundle_id,
                lane: lane.lane,
            };
            let loop_phases = phases
                .get_mut(&lane.loop_id)
                .expect("delay construction created owner phases");
            loop_phases.pre.retain(|action| {
                *action
                    != VectorStateAction::DelayCopyIn {
                        signal_id: lane.signal_id,
                    }
            });
            loop_phases.post.retain(|action| {
                *action
                    != VectorStateAction::DelayCopyOut {
                        signal_id: lane.signal_id,
                    }
            });
            loop_phases.pre.push(VectorStateAction::DelayRegisterLoad {
                signal_id: lane.signal_id,
            });
            loop_phases
                .post
                .push(VectorStateAction::DelayRegisterStore {
                    signal_id: lane.signal_id,
                });
        }
    }
}
