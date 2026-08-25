//! Fused serial-group checking over an accepted plan.

use super::super::analysis::{DepKind, EffectAtom, effects_conflict};
use super::super::decoration_verify::VerifiedDecorationCertificate;
use super::check::*;
use super::checker_reachability::CheckedReachability;
use super::error::*;
use super::model::*;
use ahash::{AHashMap, AHashSet};
use std::collections::{BTreeMap, BTreeSet};

/// Independently verifies the decoration-backed obligations of every fused
/// serial group.
///
/// JSON Schema and [`verify_vector_plan`] can validate only the finite shape
/// and plan-local identities. This second L2 gate reconstructs recursion,
/// delay, and clock facts from an already accepted decoration certificate; it
/// never calls the vector-plan producer.
pub fn verify_fused_serial_groups(
    plan: &VectorPlan,
    decorations: &VerifiedDecorationCertificate,
) -> Result<(), VectorPlanError> {
    verify_vector_plan(plan)?;
    verify_fused_serial_groups_after_plan(plan, decorations)
}
/// Verifies fused-group obligations after the caller has already accepted the
/// same plan with [`verify_vector_plan`]. This avoids repeating the expensive
/// independent plan check at the production boundary while preserving the
/// standalone public checker's fail-closed contract.
/// Everything the per-group obligations share, derived once from the plan
/// and the accepted decoration certificate.
struct FusedGroupCheckContext<'a> {
    /// The accepted plan under check.
    plan: &'a VectorPlan,
    /// The accepted decoration certificate the facts are reconstructed from.
    certificate: &'a crate::signal_fir::decoration_verify::DecorationCertificate,
    /// Decoration records by signal id.
    records: AHashMap<u64, &'a crate::signal_fir::decoration_verify::DecorationRecord>,
    /// Plan signal records by id.
    signals: AHashMap<u64, &'a SignalRecord>,
    /// Plan loop records by id.
    loops: AHashMap<u64, &'a LoopRecord>,
    /// Loop-to-loop reachability over the plan's edges.
    reachability: CheckedReachability,
    /// `(from, to)` occurrence pairs read with a positive delay.
    delayed_occurrences: AHashSet<(u64, u64)>,
}

impl FusedGroupCheckContext<'_> {
    fn new<'a>(
        plan: &'a VectorPlan,
        decorations: &'a VerifiedDecorationCertificate,
    ) -> FusedGroupCheckContext<'a> {
        let certificate = decorations.certificate();
        let records = certificate
            .records
            .iter()
            .map(|record| (u64::from(record.signal_id), record))
            .collect::<AHashMap<_, _>>();
        let signals = plan
            .signals
            .iter()
            .map(|signal| (signal.signal_id, signal))
            .collect::<AHashMap<_, _>>();
        let loops = plan
            .loops
            .iter()
            .map(|loop_| (loop_.loop_id, loop_))
            .collect::<AHashMap<_, _>>();
        let reachability = CheckedReachability::new(plan);
        let delayed_occurrences = certificate
            .occurrence_dependencies
            .iter()
            .filter_map(|dependency| {
                (dependency.delay > 0)
                    .then_some((u64::from(dependency.from), u64::from(dependency.to)))
            })
            .collect::<AHashSet<_>>();
        FusedGroupCheckContext {
            plan,
            certificate,
            records,
            signals,
            loops,
            reachability,
            delayed_occurrences,
        }
    }

    /// Every listed state carrier is genuinely delayed state owned inside
    /// the group, written by the group, serial when it is a recursive
    /// projection — and, conversely, every delayed in-group state read is
    /// listed as a carrier. The lowest carrier owner is the group owner.
    fn check_carriers_are_delayed_state(
        &self,
        group: &FusedSerialGroupRecord,
    ) -> Result<(), VectorPlanError> {
        let Self {
            certificate,
            records,
            signals,
            loops,
            delayed_occurrences,
            ..
        } = self;
        let mut carrier_owners = BTreeSet::new();
        for &carrier_id in &group.state_carrier_signal_ids {
            let Some(carrier) = records.get(&carrier_id).copied() else {
                return Err(VectorPlanError::FusedGroupCarrierNotDelayedState {
                    group_id: group.group_id,
                    signal_id: carrier_id,
                });
            };
            let Placement::Owned(owner_loop_id) = signals[&carrier_id].placement else {
                return Err(VectorPlanError::FusedGroupCarrierNotDelayedState {
                    group_id: group.group_id,
                    signal_id: carrier_id,
                });
            };
            if carrier.max_delay == 0
                || group.member_loop_ids.binary_search(&owner_loop_id).is_err()
                || group
                    .state_write_signal_ids
                    .binary_search(&carrier_id)
                    .is_err()
            {
                return Err(VectorPlanError::FusedGroupCarrierNotDelayedState {
                    group_id: group.group_id,
                    signal_id: carrier_id,
                });
            }
            if let Some(projection) = carrier.recursive_projection
                && loops[&owner_loop_id].kind != LoopKind::Recursive(u64::from(projection.group))
            {
                return Err(VectorPlanError::FusedGroupCarrierNotDelayedState {
                    group_id: group.group_id,
                    signal_id: carrier_id,
                });
            }
            carrier_owners.insert(owner_loop_id);
        }
        if carrier_owners.first().copied() != Some(group.owner_loop_id) {
            return Err(VectorPlanError::FusedGroupOwnerNotStateCarrier {
                group_id: group.group_id,
                owner_loop_id: group.owner_loop_id,
            });
        }
        for (&signal_id, record) in records.iter() {
            let Some(signal) = signals.get(&signal_id) else {
                continue;
            };
            let Placement::Owned(owner_loop_id) = signal.placement else {
                continue;
            };
            if record.max_delay == 0 || group.member_loop_ids.binary_search(&owner_loop_id).is_err()
            {
                continue;
            }
            let has_owned_group_read =
                certificate.dependencies.iter().any(|dependency| {
                    u64::from(dependency.to) == signal_id
                    && (matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0)
                        || (matches!(dependency.kind, DepKind::Immediate)
                            && delayed_occurrences
                                .contains(&(u64::from(dependency.from), signal_id))))
                    && signals
                        .get(&u64::from(dependency.from))
                        .is_some_and(|read| matches!(
                            read.placement,
                            Placement::Owned(read_loop_id)
                                if group.member_loop_ids.binary_search(&read_loop_id).is_ok()
                        ))
                });
            if has_owned_group_read
                && group
                    .state_carrier_signal_ids
                    .binary_search(&signal_id)
                    .is_err()
            {
                return Err(VectorPlanError::FusedGroupCarrierNotDelayedState {
                    group_id: group.group_id,
                    signal_id,
                });
            }
        }

        Ok(())
    }

    /// Every signal owned by a member loop lives in one clock domain, and
    /// the decoration clock of every listed signal agrees with it.
    fn check_single_clock(&self, group: &FusedSerialGroupRecord) -> Result<(), VectorPlanError> {
        let Self {
            plan,
            records,
            signals,
            ..
        } = self;
        let mut group_clock = None;
        for signal in plan.signals.iter().filter(|signal| {
            matches!(signal.placement, Placement::Owned(owner) if group.member_loop_ids.binary_search(&owner).is_ok())
        }) {
            if group_clock.replace(signal.clock_id).is_some_and(|clock| clock != signal.clock_id) {
                return Err(VectorPlanError::FusedGroupClockMismatch {
                    group_id: group.group_id,
                });
            }
        }
        for &signal_id in group
            .state_carrier_signal_ids
            .iter()
            .chain(&group.delayed_read_signal_ids)
            .chain(&group.state_write_signal_ids)
            .chain(&group.output_or_transport_roots)
        {
            let Some(record) = records.get(&signal_id).copied() else {
                return Err(VectorPlanError::FusedGroupUnknownSignal {
                    group_id: group.group_id,
                    signal_id,
                });
            };
            let decoration_clock = record.clock_domain.map_or(0, |clock| u64::from(clock) + 1);
            if decoration_clock != signals[&signal_id].clock_id
                || group_clock.is_some_and(|clock| clock != decoration_clock)
            {
                return Err(VectorPlanError::FusedGroupClockMismatch {
                    group_id: group.group_id,
                });
            }
        }

        Ok(())
    }

    /// Every delayed read reaches a listed carrier through the certificate's
    /// dependencies, every loop on a read/writer path is a member, the
    /// carrier set is exactly the used set, and loops conflicting with an
    /// immediate state effect are members too.
    fn check_delayed_reads_cover_carriers(
        &self,
        group: &FusedSerialGroupRecord,
    ) -> Result<(), VectorPlanError> {
        let Self {
            plan,
            certificate,
            signals,
            loops,
            reachability,
            delayed_occurrences,
            ..
        } = self;
        let mut used_carriers = BTreeSet::new();
        let mut immediate_state_effects = BTreeMap::<u64, BTreeSet<EffectAtom>>::new();
        for &read_signal_id in &group.delayed_read_signal_ids {
            let delayed_carrier_edges = certificate
                .dependencies
                .iter()
                .filter(|dependency| {
                    u64::from(dependency.from) == read_signal_id
                        && group
                            .state_carrier_signal_ids
                            .binary_search(&u64::from(dependency.to))
                            .is_ok()
                        && (matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0)
                            || (matches!(dependency.kind, DepKind::Immediate)
                                && delayed_occurrences
                                    .contains(&(read_signal_id, u64::from(dependency.to)))))
                })
                .collect::<Vec<_>>();
            if delayed_carrier_edges.is_empty() {
                if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                    eprintln!(
                        "[vector-fused-unmatched-read] group={} read={} carriers={:?} dependencies={:?}",
                        group.group_id,
                        read_signal_id,
                        group.state_carrier_signal_ids,
                        certificate
                            .dependencies
                            .iter()
                            .filter(|dependency| { u64::from(dependency.from) == read_signal_id })
                            .collect::<Vec<_>>()
                    );
                }
                return Err(VectorPlanError::FusedGroupDelayedDependencyMissing {
                    group_id: group.group_id,
                    signal_id: read_signal_id,
                });
            }
            let Placement::Owned(read_loop_id) = signals[&read_signal_id].placement else {
                return Err(VectorPlanError::FusedGroupSignalOutside {
                    group_id: group.group_id,
                    signal_id: read_signal_id,
                });
            };
            for delayed_carrier_edge in delayed_carrier_edges {
                let carrier_signal_id = u64::from(delayed_carrier_edge.to);
                used_carriers.insert(carrier_signal_id);
                if matches!(delayed_carrier_edge.kind, DepKind::Immediate) {
                    immediate_state_effects
                        .entry(carrier_signal_id)
                        .or_default()
                        .extend(
                            signals[&carrier_signal_id]
                                .effects
                                .iter()
                                .filter(|carrier_effect| {
                                    signals[&read_signal_id].effects.iter().any(|read_effect| {
                                        effects_conflict(carrier_effect, read_effect)
                                    })
                                })
                                .cloned(),
                        );
                }
                let Placement::Owned(writer_loop_id) = signals[&carrier_signal_id].placement else {
                    return Err(VectorPlanError::FusedGroupSignalOutside {
                        group_id: group.group_id,
                        signal_id: carrier_signal_id,
                    });
                };
                let (path_start, path_end) =
                    if matches!(delayed_carrier_edge.kind, DepKind::Delayed { .. }) {
                        (read_loop_id, writer_loop_id)
                    } else {
                        (writer_loop_id, read_loop_id)
                    };
                for &loop_id in loops.keys() {
                    let follows_start =
                        loop_id == path_start || reachability.reaches(path_start, loop_id);
                    let precedes_end =
                        loop_id == path_end || reachability.reaches(loop_id, path_end);
                    if follows_start
                        && precedes_end
                        && group.member_loop_ids.binary_search(&loop_id).is_err()
                    {
                        return Err(VectorPlanError::FusedGroupPathIncomplete {
                            group_id: group.group_id,
                            loop_id,
                        });
                    }
                }
            }
        }
        if used_carriers
            != group
                .state_carrier_signal_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        {
            let signal_id = group
                .state_carrier_signal_ids
                .iter()
                .find(|signal_id| !used_carriers.contains(signal_id))
                .copied()
                .unwrap_or(u64::MAX);
            return Err(VectorPlanError::FusedGroupCarrierNotDelayedState {
                group_id: group.group_id,
                signal_id,
            });
        }
        for state_effects in immediate_state_effects.values() {
            for loop_record in plan.loops.iter().filter(|loop_record| {
                loop_record.roots.iter().any(|root| {
                    signals[root].effects.iter().any(|effect| {
                        state_effects
                            .iter()
                            .any(|carrier| effects_conflict(carrier, effect))
                    })
                })
            }) {
                if group
                    .member_loop_ids
                    .binary_search(&loop_record.loop_id)
                    .is_err()
                {
                    return Err(VectorPlanError::FusedGroupPathIncomplete {
                        group_id: group.group_id,
                        loop_id: loop_record.loop_id,
                    });
                }
            }
        }
        Ok(())
    }

    /// No outside loop conflicts with the group's effects, and no outside
    /// loop sits between two members in the dependency order.
    fn check_group_effect_isolation(
        &self,
        group: &FusedSerialGroupRecord,
    ) -> Result<(), VectorPlanError> {
        let Self {
            plan,
            signals,
            loops,
            reachability,
            ..
        } = self;
        let group_effects = group
            .member_loop_ids
            .iter()
            .flat_map(|loop_id| loops[loop_id].roots.iter())
            .flat_map(|root| signals[root].effects.iter())
            .cloned()
            .collect::<BTreeSet<_>>();
        for loop_record in plan.loops.iter().filter(|loop_record| {
            group
                .member_loop_ids
                .binary_search(&loop_record.loop_id)
                .is_err()
        }) {
            let conflicts = loop_record.roots.iter().any(|root| {
                signals[root].effects.iter().any(|effect| {
                    group_effects
                        .iter()
                        .any(|group_effect| effects_conflict(group_effect, effect))
                })
            });
            if conflicts {
                return Err(VectorPlanError::FusedGroupPathIncomplete {
                    group_id: group.group_id,
                    loop_id: loop_record.loop_id,
                });
            }
        }
        for &loop_id in loops.keys() {
            if group.member_loop_ids.binary_search(&loop_id).is_ok() {
                continue;
            }
            let follows_member = group
                .member_loop_ids
                .iter()
                .any(|member| reachability.reaches(*member, loop_id));
            let precedes_member = group
                .member_loop_ids
                .iter()
                .any(|member| reachability.reaches(loop_id, *member));
            if follows_member && precedes_member {
                return Err(VectorPlanError::FusedGroupPathIncomplete {
                    group_id: group.group_id,
                    loop_id,
                });
            }
        }

        Ok(())
    }

    /// Every non-carrier state writer is a recursive projection owned by a
    /// member serial loop, and every recursive member loop has a writer.
    fn check_state_writers(&self, group: &FusedSerialGroupRecord) -> Result<(), VectorPlanError> {
        let Self {
            records,
            signals,
            loops,
            ..
        } = self;
        for &writer_signal_id in &group.state_write_signal_ids {
            if group
                .state_carrier_signal_ids
                .binary_search(&writer_signal_id)
                .is_ok()
            {
                continue;
            }
            let Some(writer_projection) = records
                .get(&writer_signal_id)
                .and_then(|record| record.recursive_projection)
            else {
                return Err(VectorPlanError::FusedGroupStateWriterMismatch {
                    group_id: group.group_id,
                    signal_id: writer_signal_id,
                });
            };
            let Placement::Owned(writer_owner) = signals[&writer_signal_id].placement else {
                return Err(VectorPlanError::FusedGroupStateWriterMismatch {
                    group_id: group.group_id,
                    signal_id: writer_signal_id,
                });
            };
            if group.member_loop_ids.binary_search(&writer_owner).is_err()
                || loops[&writer_owner].kind
                    != LoopKind::Recursive(u64::from(writer_projection.group))
            {
                return Err(VectorPlanError::FusedGroupStateWriterMismatch {
                    group_id: group.group_id,
                    signal_id: writer_signal_id,
                });
            }
        }
        for &loop_id in &group.member_loop_ids {
            let LoopKind::Recursive(recursion_group) = loops[&loop_id].kind else {
                continue;
            };
            if !group.state_write_signal_ids.iter().any(|signal_id| {
                signals[signal_id].placement == Placement::Owned(loop_id)
                    && records[signal_id]
                        .recursive_projection
                        .is_some_and(|projection| u64::from(projection.group) == recursion_group)
            }) {
                if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                    eprintln!(
                        "[vector-fused-missing-writer] group={} loop={} recursion={} writers={:?}",
                        group.group_id, loop_id, recursion_group, group.state_write_signal_ids
                    );
                }
                return Err(VectorPlanError::FusedGroupRecursiveMemberMissingWriter {
                    group_id: group.group_id,
                    loop_id,
                });
            }
        }

        Ok(())
    }

    /// Every transport between two members is listed as internal, so none
    /// can remain a whole-chunk array inside the serial slice.
    fn check_internal_transports(
        &self,
        group: &FusedSerialGroupRecord,
    ) -> Result<(), VectorPlanError> {
        let Self { plan, .. } = self;
        // Internal transports may form an arbitrary pure chain between a
        // delayed read and its recursive writer. Finite-shape verification
        // above proves that every listed transport stays within the fused
        // member set; require every internal transport so none can remain a
        // whole-chunk array inside the serial slice.
        for transport in &plan.transports {
            if group
                .member_loop_ids
                .binary_search(&transport.producer_loop)
                .is_ok()
                && group
                    .member_loop_ids
                    .binary_search(&transport.consumer_loop)
                    .is_ok()
                && group
                    .internal_transport_ids
                    .binary_search(&transport.transport_id)
                    .is_err()
            {
                return Err(VectorPlanError::FusedGroupDangerousTransportPresent {
                    group_id: group.group_id,
                    transport_id: transport.transport_id,
                });
            }
        }
        Ok(())
    }

    /// Reconstructs the owned immediate state-mediated crossings from
    /// decorations and raw placement, independently of producer component
    /// discovery, and requires each to be covered by a fused group.
    fn check_uncovered_dangerous_crossings(&self) -> Result<(), VectorPlanError> {
        let Self {
            plan,
            certificate,
            records,
            signals,
            delayed_occurrences,
            ..
        } = self;
        // Reconstruct the owned subset of immediate state-mediated crossings from
        // decorations and raw plan placement. This is deliberately independent of
        // producer component discovery and prevents removing the producer's final
        // fail-closed edge guard without equivalent certificate coverage.
        for dependency in certificate.dependencies.iter().filter(|dependency| {
            matches!(dependency.kind, DepKind::Immediate)
                && delayed_occurrences
                    .contains(&(u64::from(dependency.from), u64::from(dependency.to)))
                && records
                    .get(&u64::from(dependency.from))
                    .is_some_and(|record| record.is_delay_read)
                && records
                    .get(&u64::from(dependency.to))
                    .is_some_and(|record| record.max_delay > 0)
        }) {
            let read_signal_id = u64::from(dependency.from);
            let carrier_signal_id = u64::from(dependency.to);
            let (Placement::Owned(consumer), Placement::Owned(producer)) = (
                signals[&read_signal_id].placement,
                signals[&carrier_signal_id].placement,
            ) else {
                continue;
            };
            if producer == consumer {
                continue;
            }
            let covered = plan.fused_serial_groups.iter().any(|group| {
                group
                    .state_carrier_signal_ids
                    .binary_search(&carrier_signal_id)
                    .is_ok()
                    && group
                        .delayed_read_signal_ids
                        .binary_search(&read_signal_id)
                        .is_ok()
                    && group.member_loop_ids.binary_search(&producer).is_ok()
                    && group.member_loop_ids.binary_search(&consumer).is_ok()
            });
            if !covered {
                return Err(VectorPlanError::FusedGroupDangerousCrossingMissing {
                    producer,
                    consumer,
                });
            }
        }
        Ok(())
    }
}

pub(crate) fn verify_fused_serial_groups_after_plan(
    plan: &VectorPlan,
    decorations: &VerifiedDecorationCertificate,
) -> Result<(), VectorPlanError> {
    let ctx = FusedGroupCheckContext::new(plan, decorations);
    for group in &plan.fused_serial_groups {
        ctx.check_carriers_are_delayed_state(group)?;
        ctx.check_single_clock(group)?;
        ctx.check_delayed_reads_cover_carriers(group)?;
        ctx.check_group_effect_isolation(group)?;
        ctx.check_state_writers(group)?;
        ctx.check_internal_transports(group)?;
    }
    ctx.check_uncovered_dangerous_crossings()
}
