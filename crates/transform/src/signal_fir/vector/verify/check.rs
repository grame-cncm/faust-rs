//! Independent `VectorPlan` checker: re-derives unique ids, epoch coverage,
//! ownership, duplicability, edges, acyclicity, transports, barriers, and
//! `VecSafe` witnesses from the plan's own fields (never from the producer).

use super::super::analysis::{EffectAtom, ForeignPurity, StateResource};
use super::checker_reachability::CheckedReachability;
use super::error::*;
use super::model::*;
use ahash::{AHashMap, AHashSet};

pub(super) fn strictly_ascending<T: Ord>(items: &[T]) -> Result<(), usize> {
    for i in 1..items.len() {
        if items[i - 1] >= items[i] {
            return Err(i);
        }
    }
    Ok(())
}
/// Concrete Lean `duplicableEffectsB`: only an empty effect set or pure
/// foreign calls can be recomputed in several loop regions.
#[must_use]
pub(crate) fn effects_duplicable(effects: &[EffectAtom]) -> bool {
    effects.iter().all(|effect| {
        matches!(
            effect,
            EffectAtom::Foreign {
                purity: ForeignPurity::Pure,
                ..
            }
        )
    })
}
/// Concrete Lean `sampleReorderableB`: loop-carried state is the local
/// per-sample vectorization blocker. Other effect conflicts are ordered or
/// co-located by the plan's separate effect relation.
#[must_use]
pub(crate) fn effects_sample_reorderable(effects: &[EffectAtom]) -> bool {
    !effects
        .iter()
        .any(|effect| matches!(effect, EffectAtom::ReadState(_) | EffectAtom::WriteState(_)))
}
/// Everything the per-obligation checks share, derived once from the plan.
///
/// Construction validates root identities and edge endpoints first, so the
/// reachability and effect summaries it builds are panic-free on a hostile
/// DTO; every later check may index these maps freely.
struct PlanIndex<'a> {
    /// Canonical loop ids, in plan order.
    loop_ids: Vec<u64>,
    /// Set view of the plan's signal ids.
    signal_set: AHashSet<u64>,
    /// Set view of the plan's loop ids.
    loop_set: AHashSet<u64>,
    /// Signal records by id.
    signal_by_id: AHashMap<u64, &'a SignalRecord>,
    /// Loop records by id.
    loop_by_id: AHashMap<u64, &'a LoopRecord>,
    /// Loop-to-loop reachability over the plan's edges.
    reachability: CheckedReachability,
    /// Per-loop effect summary used by the conflict checks.
    effects_by_loop: AHashMap<u64, CheckedEffectConflictSummary>,
}

impl<'a> PlanIndex<'a> {
    fn new(plan: &'a VectorPlan) -> Result<Self, VectorPlanError> {
        let signal_ids: Vec<u64> = plan.signals.iter().map(|s| s.signal_id).collect();
        let loop_ids: Vec<u64> = plan.loops.iter().map(|l| l.loop_id).collect();
        let signal_set: AHashSet<u64> = signal_ids.iter().copied().collect();
        let loop_set: AHashSet<u64> = loop_ids.iter().copied().collect();
        let signal_by_id: AHashMap<u64, &SignalRecord> =
            plan.signals.iter().map(|s| (s.signal_id, s)).collect();
        let loop_by_id: AHashMap<u64, &LoopRecord> =
            plan.loops.iter().map(|l| (l.loop_id, l)).collect();

        // Root identities and edge endpoints are validated first, so building
        // the reachability and per-loop effect summaries below cannot panic on a
        // hostile DTO.
        for loop_record in &plan.loops {
            if let Some(&signal_id) = loop_record
                .roots
                .iter()
                .find(|signal_id| !signal_set.contains(signal_id))
            {
                return Err(VectorPlanError::RootUnknownSignal {
                    loop_id: loop_record.loop_id,
                    signal_id,
                });
            }
        }
        for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
            if !loop_set.contains(&edge.consumer) {
                return Err(VectorPlanError::EdgeEndpointUnknown {
                    edge: *edge,
                    missing: edge.consumer,
                });
            }
            if !loop_set.contains(&edge.dependency) {
                return Err(VectorPlanError::EdgeEndpointUnknown {
                    edge: *edge,
                    missing: edge.dependency,
                });
            }
        }
        let reachability = CheckedReachability::new(plan);
        let effects_by_loop = plan
            .loops
            .iter()
            .map(|loop_record| {
                (
                    loop_record.loop_id,
                    CheckedEffectConflictSummary::new(&signal_by_id, loop_record),
                )
            })
            .collect::<AHashMap<_, _>>();
        Ok(Self {
            loop_ids,
            signal_set,
            loop_set,
            signal_by_id,
            loop_by_id,
            reachability,
            effects_by_loop,
        })
    }
}

/// Independent verifier for a [`VectorPlan`] (plan provenance: §5.5/§5.10
/// `verify_vector_plan`; Lean `VectorPlanCertificate`). Re-derives every
/// invariant from the plan's own fields; never runs a planner.
///
/// # Errors
/// The first [`VectorPlanError`] found (checks ordered so identity/coverage
/// problems surface before the graph/transport checks that assume them).
pub fn verify_vector_plan(plan: &VectorPlan) -> Result<(), VectorPlanError> {
    check_schema_and_canonical_orders(plan)?;
    let index = PlanIndex::new(plan)?;
    check_lockstep_bundles(plan, &index)?;
    check_fused_group_shape(plan, &index)?;
    let epoch_of_loop = check_epoch_coverage(plan, &index)?;
    check_placement_and_roots(plan, &index)?;
    check_edges_and_barriers(plan, &index, &epoch_of_loop)?;
    check_epoch_acyclicity(plan)?;
    check_effect_conflict_ordering(plan, &index)?;
    check_transports(plan, &index)?;
    check_vec_safe_witnesses(plan, &index)
}

/// Schema version, non-zero `vec_size`, and the canonical (strictly
/// ascending, hence duplicate-free) order of every id-carrying plan array.
fn check_schema_and_canonical_orders(plan: &VectorPlan) -> Result<(), VectorPlanError> {
    if plan.schema_version != VECTOR_PLAN_SCHEMA_VERSION {
        return Err(VectorPlanError::UnsupportedSchema {
            found: plan.schema_version,
        });
    }
    if plan.vec_size == 0 {
        return Err(VectorPlanError::VecSizeZero);
    }

    // ── Canonical set orders (also enforce uniqueness). ──────────────────
    let signal_ids: Vec<u64> = plan.signals.iter().map(|s| s.signal_id).collect();
    strictly_ascending(&signal_ids).map_err(|at| VectorPlanError::NotCanonical {
        what: "signals",
        at,
    })?;
    let loop_ids: Vec<u64> = plan.loops.iter().map(|l| l.loop_id).collect();
    strictly_ascending(&loop_ids)
        .map_err(|at| VectorPlanError::NotCanonical { what: "loops", at })?;
    let epoch_keys: Vec<(u64, u64)> = plan.epochs.iter().map(|e| (e.rank, e.epoch_id)).collect();
    strictly_ascending(&epoch_keys)
        .map_err(|at| VectorPlanError::NotCanonical { what: "epochs", at })?;
    let transport_ids: Vec<u64> = plan.transports.iter().map(|t| t.transport_id).collect();
    strictly_ascending(&transport_ids).map_err(|at| VectorPlanError::NotCanonical {
        what: "transports",
        at,
    })?;
    strictly_ascending(&plan.data_edges).map_err(|at| VectorPlanError::NotCanonical {
        what: "data_edges",
        at,
    })?;
    strictly_ascending(&plan.effect_edges).map_err(|at| VectorPlanError::NotCanonical {
        what: "effect_edges",
        at,
    })?;
    let witness_ids: Vec<u64> = plan
        .vec_safe_witnesses
        .iter()
        .map(|witness| witness.loop_id)
        .collect();
    strictly_ascending(&witness_ids).map_err(|at| VectorPlanError::NotCanonical {
        what: "vec_safe_witnesses",
        at,
    })?;
    let fused_group_ids = plan
        .fused_serial_groups
        .iter()
        .map(|group| group.group_id)
        .collect::<Vec<_>>();
    strictly_ascending(&fused_group_ids).map_err(|at| VectorPlanError::NotCanonical {
        what: "fused_serial_groups",
        at,
    })?;
    let lockstep_bundle_ids = plan
        .lockstep_bundles
        .iter()
        .map(|bundle| bundle.bundle_id)
        .collect::<Vec<_>>();
    strictly_ascending(&lockstep_bundle_ids).map_err(|at| VectorPlanError::NotCanonical {
        what: "lockstep_bundles",
        at,
    })?;

    Ok(())
}

/// Lockstep bundles: finite shape, canonical members, per-lane isomorphism
/// witnesses, lane independence, matching clock domains, and no effect
/// conflict between lanes. Prepared-signal skeletons are re-traversed
/// separately by `verify_lockstep_isomorphism`; this is the plan-local gate.
fn check_lockstep_bundles(plan: &VectorPlan, index: &PlanIndex<'_>) -> Result<(), VectorPlanError> {
    let PlanIndex {
        signal_set,
        loop_by_id,
        signal_by_id,
        reachability,
        effects_by_loop,
        ..
    } = index;
    let mut bundled_loops = AHashSet::new();
    for bundle in &plan.lockstep_bundles {
        if bundle.member_loop_ids.len() < 2 || bundle.lanes.len() != bundle.member_loop_ids.len() {
            return Err(VectorPlanError::LockstepWidthMismatch {
                bundle_id: bundle.bundle_id,
            });
        }
        strictly_ascending(&bundle.member_loop_ids).map_err(|at| {
            VectorPlanError::NotCanonical {
                what: "lockstep.member_loop_ids",
                at,
            }
        })?;
        if bundle.member_loop_ids.first().copied() != Some(bundle.representative_loop_id) {
            return Err(VectorPlanError::LockstepMemberMismatch {
                bundle_id: bundle.bundle_id,
                loop_id: bundle.representative_loop_id,
            });
        }
        let lane_loop_ids = bundle
            .lanes
            .iter()
            .map(|lane| lane.loop_id)
            .collect::<Vec<_>>();
        if lane_loop_ids != bundle.member_loop_ids {
            return Err(VectorPlanError::LockstepLaneMismatch {
                bundle_id: bundle.bundle_id,
                loop_id: lane_loop_ids
                    .iter()
                    .zip(&bundle.member_loop_ids)
                    .find_map(|(actual, expected)| (actual != expected).then_some(*actual))
                    .unwrap_or(bundle.representative_loop_id),
            });
        }
        let width =
            u64::try_from(bundle.member_loop_ids.len()).expect("lockstep member count fits u64");
        let representative = loop_by_id.get(&bundle.representative_loop_id).ok_or(
            VectorPlanError::LockstepMemberMismatch {
                bundle_id: bundle.bundle_id,
                loop_id: bundle.representative_loop_id,
            },
        )?;
        for lane in &bundle.lanes {
            let Some(loop_record) = loop_by_id.get(&lane.loop_id).copied() else {
                return Err(VectorPlanError::LockstepMemberMismatch {
                    bundle_id: bundle.bundle_id,
                    loop_id: lane.loop_id,
                });
            };
            if loop_record.kind != (LoopKind::Lockstep { width }) {
                return Err(VectorPlanError::LockstepWidthMismatch {
                    bundle_id: bundle.bundle_id,
                });
            }
            if !bundled_loops.insert(lane.loop_id)
                || loop_record.epoch_id != representative.epoch_id
                || lane.roots.len() != loop_record.roots.len()
                || lane
                    .roots
                    .iter()
                    .map(|root| root.lane_root)
                    .ne(loop_record.roots.iter().copied())
                || lane
                    .roots
                    .iter()
                    .map(|root| root.representative_root)
                    .ne(representative.roots.iter().copied())
            {
                return Err(VectorPlanError::LockstepLaneMismatch {
                    bundle_id: bundle.bundle_id,
                    loop_id: lane.loop_id,
                });
            }
            for root in &lane.roots {
                if root.shape_hash == 0
                    || !signal_set.contains(&root.representative_root)
                    || !signal_set.contains(&root.lane_root)
                    || strictly_ascending(&root.leaf_mapping).is_err()
                    || root.leaf_mapping.iter().any(|mapping| {
                        !signal_set.contains(&mapping.representative_signal_id)
                            || !signal_set.contains(&mapping.lane_signal_id)
                    })
                {
                    return Err(VectorPlanError::LockstepIsoWitnessMismatch {
                        bundle_id: bundle.bundle_id,
                        loop_id: lane.loop_id,
                    });
                }
            }
        }
        for (index, &left) in bundle.member_loop_ids.iter().enumerate() {
            for &right in &bundle.member_loop_ids[index + 1..] {
                if reachability.reaches(left, right) || reachability.reaches(right, left) {
                    return Err(VectorPlanError::LockstepDependentLanes {
                        bundle_id: bundle.bundle_id,
                        left,
                        right,
                    });
                }
                let left_loop = loop_by_id[&left];
                let right_loop = loop_by_id[&right];
                let left_clocks = left_loop
                    .roots
                    .iter()
                    .map(|root| signal_by_id[root].clock_id)
                    .collect::<AHashSet<_>>();
                let right_clocks = right_loop
                    .roots
                    .iter()
                    .map(|root| signal_by_id[root].clock_id)
                    .collect::<AHashSet<_>>();
                if left_loop.epoch_id != right_loop.epoch_id || left_clocks != right_clocks {
                    return Err(VectorPlanError::LockstepDomainMismatch {
                        bundle_id: bundle.bundle_id,
                        left,
                        right,
                    });
                }
                if effects_by_loop[&left].conflicts(&effects_by_loop[&right]) {
                    return Err(VectorPlanError::LockstepEffectConflict {
                        bundle_id: bundle.bundle_id,
                        left,
                        right,
                    });
                }
            }
        }
    }
    for loop_record in &plan.loops {
        if matches!(loop_record.kind, LoopKind::Lockstep { .. })
            != bundled_loops.contains(&loop_record.loop_id)
        {
            return Err(VectorPlanError::LockstepMemberMismatch {
                bundle_id: plan
                    .lockstep_bundles
                    .iter()
                    .find(|bundle| bundle.member_loop_ids.contains(&loop_record.loop_id))
                    .map_or(u64::MAX, |bundle| bundle.bundle_id),
                loop_id: loop_record.loop_id,
            });
        }
    }

    Ok(())
}

/// Fused serial groups: finite shape, canonical member arrays, ownership of
/// every named signal inside the group, and internal transports staying
/// inside the group. Semantic delay/recursion facts are checked
/// independently by `verify_fused_serial_groups` against decorations.
fn check_fused_group_shape(
    plan: &VectorPlan,
    index: &PlanIndex<'_>,
) -> Result<(), VectorPlanError> {
    let PlanIndex {
        signal_set: _,
        loop_set,
        signal_by_id,
        ..
    } = index;
    // ── Fused-group finite shape. Semantic delay/recursion facts are checked
    // independently by `verify_fused_serial_groups` against decorations.
    let transport_by_id = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<AHashMap<_, _>>();
    let mut fused_loop_owner = AHashSet::new();
    for group in &plan.fused_serial_groups {
        for (what, ids) in [
            ("member_loop_ids", group.member_loop_ids.as_slice()),
            (
                "state_carrier_signal_ids",
                group.state_carrier_signal_ids.as_slice(),
            ),
            (
                "delayed_read_signal_ids",
                group.delayed_read_signal_ids.as_slice(),
            ),
            (
                "state_write_signal_ids",
                group.state_write_signal_ids.as_slice(),
            ),
            (
                "output_or_transport_roots",
                group.output_or_transport_roots.as_slice(),
            ),
        ] {
            if ids.is_empty() {
                return Err(VectorPlanError::FusedGroupEmpty {
                    group_id: group.group_id,
                    what,
                });
            }
            strictly_ascending(ids).map_err(|at| VectorPlanError::NotCanonical { what, at })?;
        }
        strictly_ascending(&group.internal_transport_ids).map_err(|at| {
            VectorPlanError::NotCanonical {
                what: "internal_transport_ids",
                at,
            }
        })?;
        for &loop_id in &group.member_loop_ids {
            if !loop_set.contains(&loop_id) {
                return Err(VectorPlanError::FusedGroupUnknownLoop {
                    group_id: group.group_id,
                    loop_id,
                });
            }
            if !fused_loop_owner.insert(loop_id) {
                return Err(VectorPlanError::FusedGroupLoopOverlap { loop_id });
            }
        }
        if group
            .member_loop_ids
            .binary_search(&group.owner_loop_id)
            .is_err()
        {
            return Err(VectorPlanError::FusedGroupOwnerNotMember {
                group_id: group.group_id,
                owner_loop_id: group.owner_loop_id,
            });
        }
        for &signal_id in group
            .state_carrier_signal_ids
            .iter()
            .chain(&group.delayed_read_signal_ids)
            .chain(&group.state_write_signal_ids)
            .chain(&group.output_or_transport_roots)
        {
            let Some(signal) = signal_by_id.get(&signal_id) else {
                return Err(VectorPlanError::FusedGroupUnknownSignal {
                    group_id: group.group_id,
                    signal_id,
                });
            };
            let Placement::Owned(owner) = signal.placement else {
                return Err(VectorPlanError::FusedGroupSignalOutside {
                    group_id: group.group_id,
                    signal_id,
                });
            };
            if group.member_loop_ids.binary_search(&owner).is_err() {
                return Err(VectorPlanError::FusedGroupSignalOutside {
                    group_id: group.group_id,
                    signal_id,
                });
            }
        }
        if !group.state_carrier_signal_ids.iter().any(|signal_id| {
            signal_by_id
                .get(signal_id)
                .is_some_and(|signal| signal.placement == Placement::Owned(group.owner_loop_id))
        }) {
            return Err(VectorPlanError::FusedGroupOwnerNotStateCarrier {
                group_id: group.group_id,
                owner_loop_id: group.owner_loop_id,
            });
        }
        for &transport_id in &group.internal_transport_ids {
            let Some(transport) = transport_by_id.get(&transport_id) else {
                return Err(VectorPlanError::FusedGroupUnknownTransport {
                    group_id: group.group_id,
                    transport_id,
                });
            };
            if group
                .member_loop_ids
                .binary_search(&transport.producer_loop)
                .is_err()
                || group
                    .member_loop_ids
                    .binary_search(&transport.consumer_loop)
                    .is_err()
            {
                return Err(VectorPlanError::FusedGroupTransportOutside {
                    group_id: group.group_id,
                    transport_id,
                });
            }
        }
    }

    Ok(())
}

/// Epoch coverage: every loop lives in exactly one epoch, every epoch names
/// only known loops, and each loop's declared epoch agrees. Returns the
/// derived loop-to-epoch map for the barrier checks.
fn check_epoch_coverage(
    plan: &VectorPlan,
    index: &PlanIndex<'_>,
) -> Result<AHashMap<u64, u64>, VectorPlanError> {
    let PlanIndex {
        loop_ids, loop_set, ..
    } = index;
    // ── Epoch coverage: every loop in exactly one epoch, epoch loops known.
    let mut epoch_of_loop: AHashMap<u64, u64> = AHashMap::new();
    for epoch in &plan.epochs {
        strictly_ascending(&epoch.loops).map_err(|at| VectorPlanError::NotCanonical {
            what: "epoch.loops",
            at,
        })?;
        for &l in &epoch.loops {
            if !loop_set.contains(&l) {
                return Err(VectorPlanError::EpochLoopUnknown {
                    epoch_id: epoch.epoch_id,
                    loop_id: l,
                });
            }
            if epoch_of_loop.insert(l, epoch.epoch_id).is_some() {
                return Err(VectorPlanError::EpochCoverageMismatch { loop_id: l });
            }
        }
    }

    for &l in loop_ids {
        if !epoch_of_loop.contains_key(&l) {
            return Err(VectorPlanError::EpochCoverageMismatch { loop_id: l });
        }
    }
    for lp in &plan.loops {
        let actual = epoch_of_loop[&lp.loop_id];
        if lp.epoch_id != actual {
            return Err(VectorPlanError::LoopEpochMismatch {
                loop_id: lp.loop_id,
                declared: lp.epoch_id,
                actual,
            });
        }
    }

    Ok(epoch_of_loop)
}

/// Placement and roots agreement: duplicability matches the effect-derived
/// rule, inline placement is justified, and every owned signal is a root of
/// exactly its owning loop.
fn check_placement_and_roots(
    plan: &VectorPlan,
    index: &PlanIndex<'_>,
) -> Result<(), VectorPlanError> {
    let PlanIndex {
        signal_set,
        signal_by_id,
        loop_by_id,
        ..
    } = index;
    // ── Placement / roots agreement (P-Unique, P-Root, P-Duplicate). ─────
    for sig in &plan.signals {
        let derived_duplicable = effects_duplicable(&sig.effects);
        if sig.duplicable != derived_duplicable {
            return Err(VectorPlanError::DuplicabilityMismatch {
                signal_id: sig.signal_id,
            });
        }
        if sig.structural
            && (sig.placement != Placement::Inline
                || !matches!(sig.value_type, ValueType::Tuple(_)))
        {
            return Err(VectorPlanError::InlineNotDuplicable {
                signal_id: sig.signal_id,
            });
        }
        if sig.placement == Placement::Inline && !derived_duplicable && !sig.structural {
            return Err(VectorPlanError::InlineNotDuplicable {
                signal_id: sig.signal_id,
            });
        }
        if let Placement::Owned(l) = sig.placement {
            let owner = loop_by_id
                .get(&l)
                .ok_or(VectorPlanError::OwnedSignalNotRoot {
                    signal_id: sig.signal_id,
                    loop_id: l,
                })?;
            if !owner.roots.contains(&sig.signal_id) {
                return Err(VectorPlanError::OwnedSignalNotRoot {
                    signal_id: sig.signal_id,
                    loop_id: l,
                });
            }
        }
    }
    for lp in &plan.loops {
        // Roots must be unique within a loop (Lean `rootsNodup`) but need not
        // be ascending (deterministic materialization order, not a set-like
        // canonical array). Uniqueness and ownership are checked together.
        let mut seen = AHashSet::new();
        for &r in &lp.roots {
            if !signal_set.contains(&r) {
                return Err(VectorPlanError::RootUnknownSignal {
                    loop_id: lp.loop_id,
                    signal_id: r,
                });
            }
            if !seen.insert(r) {
                return Err(VectorPlanError::RootWithoutOwnership {
                    signal_id: r,
                    loop_id: lp.loop_id,
                });
            }
            match signal_by_id.get(&r).map(|s| s.placement) {
                Some(Placement::Owned(owner)) if owner == lp.loop_id => {}
                _ => {
                    return Err(VectorPlanError::RootWithoutOwnership {
                        signal_id: r,
                        loop_id: lp.loop_id,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Edges: endpoints exist, no self-edges, and every edge respects the epoch
/// barrier (dependency rank never exceeds consumer rank).
fn check_edges_and_barriers(
    plan: &VectorPlan,
    index: &PlanIndex<'_>,
    epoch_of_loop: &AHashMap<u64, u64>,
) -> Result<(), VectorPlanError> {
    let PlanIndex { loop_set, .. } = index;
    // ── Edges: endpoints exist, no self-edge, barriers monotone. ─────────
    for edge in plan.data_edges.iter().chain(plan.effect_edges.iter()) {
        if !loop_set.contains(&edge.consumer) {
            return Err(VectorPlanError::EdgeEndpointUnknown {
                edge: *edge,
                missing: edge.consumer,
            });
        }
        if !loop_set.contains(&edge.dependency) {
            return Err(VectorPlanError::EdgeEndpointUnknown {
                edge: *edge,
                missing: edge.dependency,
            });
        }
        if edge.consumer == edge.dependency {
            return Err(VectorPlanError::LoopSelfEdge {
                loop_id: edge.consumer,
            });
        }
        // Barrier: dependency epoch rank ≤ consumer epoch rank.
        let dep_rank = rank_of(plan, epoch_of_loop[&edge.dependency]);
        let con_rank = rank_of(plan, epoch_of_loop[&edge.consumer]);
        if dep_rank > con_rank {
            return Err(VectorPlanError::BarrierViolation { edge: *edge });
        }
    }

    Ok(())
}

/// Per-epoch induced-graph acyclicity.
fn check_epoch_acyclicity(plan: &VectorPlan) -> Result<(), VectorPlanError> {
    // ── Per-epoch induced-graph acyclicity (L-DAG). ──────────────────────
    for epoch in &plan.epochs {
        let members: AHashSet<u64> = epoch.loops.iter().copied().collect();
        if let Some(remaining) = induced_cycle(plan, &members) {
            return Err(VectorPlanError::EpochNotAcyclic {
                epoch_id: epoch.epoch_id,
                remaining,
            });
        }
    }

    Ok(())
}

/// Effect conflicts: every pair of loops with conflicting effects must be
/// ordered by reachability in one direction.
fn check_effect_conflict_ordering(
    plan: &VectorPlan,
    index: &PlanIndex<'_>,
) -> Result<(), VectorPlanError> {
    let PlanIndex {
        effects_by_loop,
        reachability,
        ..
    } = index;
    // ── Effect conflicts: every conflicting loop pair is comparable. ──
    // Root identities and graph endpoints are known valid at this point, so
    // this check cannot panic when presented with a hostile DTO.
    for (index, left) in plan.loops.iter().enumerate() {
        for right in &plan.loops[index + 1..] {
            if effects_by_loop[&left.loop_id].conflicts(&effects_by_loop[&right.loop_id])
                && !reachability.reaches(left.loop_id, right.loop_id)
                && !reachability.reaches(right.loop_id, left.loop_id)
            {
                return Err(VectorPlanError::UnorderedEffectConflict {
                    left: left.loop_id,
                    right: right.loop_id,
                });
            }
        }
    }

    Ok(())
}

/// Transports: known endpoints, no self-transport, element type and length
/// agree with the plan, and interleaved layouts name a matching lockstep
/// bundle.
fn check_transports(plan: &VectorPlan, index: &PlanIndex<'_>) -> Result<(), VectorPlanError> {
    let PlanIndex {
        signal_set,
        loop_set,
        signal_by_id,
        ..
    } = index;
    // ── Transports well-typed (T-TRANSPORT). ─────────────────────────────
    for t in &plan.transports {
        if !signal_set.contains(&t.signal_id) {
            return Err(VectorPlanError::TransportUnknownRef {
                transport_id: t.transport_id,
                missing: t.signal_id,
            });
        }
        for l in [t.producer_loop, t.consumer_loop] {
            if !loop_set.contains(&l) {
                return Err(VectorPlanError::TransportUnknownRef {
                    transport_id: t.transport_id,
                    missing: l,
                });
            }
        }
        if t.producer_loop == t.consumer_loop {
            return Err(VectorPlanError::TransportSelfLoop {
                transport_id: t.transport_id,
            });
        }
        if signal_by_id[&t.signal_id].value_type != t.element_type {
            return Err(VectorPlanError::TransportTypeMismatch {
                transport_id: t.transport_id,
            });
        }
        if t.length != plan.vec_size {
            return Err(VectorPlanError::TransportLengthMismatch {
                transport_id: t.transport_id,
            });
        }
        if let TransportLayout::Interleaved(width) = t.layout {
            let matching_bundle = plan.lockstep_bundles.iter().any(|bundle| {
                u64::try_from(bundle.member_loop_ids.len()).ok() == Some(width)
                    && (bundle
                        .member_loop_ids
                        .binary_search(&t.producer_loop)
                        .is_ok()
                        || bundle
                            .member_loop_ids
                            .binary_search(&t.consumer_loop)
                            .is_ok())
            });
            if width < 2 || !matching_bundle {
                return Err(VectorPlanError::TransportLayoutMismatch {
                    transport_id: t.transport_id,
                });
            }
        }
    }

    Ok(())
}

/// `VecSafe` witnesses versus loop kinds: every vectorizable loop carries a
/// vectorizing witness and satisfies the derived per-root safety rule;
/// serial loops never claim a pointwise witness.
fn check_vec_safe_witnesses(
    plan: &VectorPlan,
    index: &PlanIndex<'_>,
) -> Result<(), VectorPlanError> {
    let PlanIndex {
        loop_set,
        signal_by_id,
        ..
    } = index;
    // ── VecSafe witnesses vs loop kinds. ─────────────────────────────────
    let mut witness_of: AHashMap<u64, WitnessKind> = AHashMap::new();
    for w in &plan.vec_safe_witnesses {
        if !loop_set.contains(&w.loop_id) {
            return Err(VectorPlanError::WitnessUnknownLoop { loop_id: w.loop_id });
        }
        witness_of.insert(w.loop_id, w.witness_kind);
    }
    for lp in &plan.loops {
        match lp.kind {
            LoopKind::Vectorizable => {
                let vec_safe = lp.roots.iter().all(|root| {
                    let signal = signal_by_id[root];
                    signal.vectorability == Vectorability::Vect
                        && effects_sample_reorderable(&signal.effects)
                });
                if !vec_safe {
                    return Err(VectorPlanError::VectorizableNotSafe {
                        loop_id: lp.loop_id,
                    });
                }
                let Some(kind) = witness_of.get(&lp.loop_id) else {
                    return Err(VectorPlanError::VectorizableWithoutWitness {
                        loop_id: lp.loop_id,
                    });
                };
                // A vectorizable loop's witness must be a *vectorizing* one.
                if !matches!(kind, WitnessKind::Pointwise | WitnessKind::ProvenIntrinsic) {
                    return Err(VectorPlanError::VectorizableWithoutWitness {
                        loop_id: lp.loop_id,
                    });
                }
            }
            LoopKind::Recursive(_) | LoopKind::Island(_) | LoopKind::Lockstep { .. } => {
                // Serial loops must not claim a pointwise (per-lane
                // parallel) witness; only a serial-state witness is
                // consistent with their kind.
                if matches!(witness_of.get(&lp.loop_id), Some(WitnessKind::Pointwise)) {
                    return Err(VectorPlanError::SerialLoopNotSerial {
                        loop_id: lp.loop_id,
                    });
                }
            }
        }
    }

    Ok(())
}
pub(super) fn rank_of(plan: &VectorPlan, epoch_id: u64) -> u64 {
    plan.epochs
        .iter()
        .find(|e| e.epoch_id == epoch_id)
        .map_or(u64::MAX, |e| e.rank)
}
pub(super) struct CheckedEffectConflictSummary {
    any: bool,
    barrier: bool,
    state_reads: AHashSet<StateResource>,
    state_writes: AHashSet<StateResource>,
    table_reads: AHashSet<u32>,
    table_writes: AHashSet<u32>,
    ui_writes: AHashSet<u32>,
    output_writes: AHashSet<u32>,
}
impl Default for CheckedEffectConflictSummary {
    fn default() -> Self {
        Self {
            any: false,
            barrier: false,
            state_reads: AHashSet::new(),
            state_writes: AHashSet::new(),
            table_reads: AHashSet::new(),
            table_writes: AHashSet::new(),
            ui_writes: AHashSet::new(),
            output_writes: AHashSet::new(),
        }
    }
}
impl CheckedEffectConflictSummary {
    pub(super) fn new(
        signal_by_id: &AHashMap<u64, &SignalRecord>,
        loop_record: &LoopRecord,
    ) -> Self {
        let mut summary = Self::default();
        for effect in loop_record
            .roots
            .iter()
            .flat_map(|root| &signal_by_id[root].effects)
        {
            summary.any = true;
            match effect {
                EffectAtom::ReadState(resource) => {
                    summary.state_reads.insert(resource.clone());
                }
                EffectAtom::WriteState(resource) => {
                    summary.state_writes.insert(resource.clone());
                }
                EffectAtom::ReadTable(table) => {
                    summary.table_reads.insert(*table);
                }
                EffectAtom::WriteTable(table) => {
                    summary.table_writes.insert(*table);
                }
                EffectAtom::WriteUi(zone) => {
                    summary.ui_writes.insert(*zone);
                }
                EffectAtom::WriteOutput(output) => {
                    summary.output_writes.insert(*output);
                }
                EffectAtom::Foreign { purity, .. } => {
                    summary.barrier |=
                        matches!(purity, ForeignPurity::Impure | ForeignPurity::Unknown);
                }
            }
        }
        summary
    }

    pub(super) fn conflicts(&self, other: &Self) -> bool {
        (self.barrier && other.any)
            || (other.barrier && self.any)
            || hash_intersects(&self.state_writes, &other.state_reads)
            || hash_intersects(&self.state_writes, &other.state_writes)
            || hash_intersects(&self.state_reads, &other.state_writes)
            || hash_intersects(&self.table_writes, &other.table_reads)
            || hash_intersects(&self.table_writes, &other.table_writes)
            || hash_intersects(&self.table_reads, &other.table_writes)
            || hash_intersects(&self.ui_writes, &other.ui_writes)
            || hash_intersects(&self.output_writes, &other.output_writes)
    }
}
pub(super) fn hash_intersects<T: Eq + std::hash::Hash>(
    left: &AHashSet<T>,
    right: &AHashSet<T>,
) -> bool {
    let (small, large) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    small.iter().any(|item| large.contains(item))
}
/// Kahn peeling on the induced graph of `members` (edges from both edge
/// families whose endpoints are both members). Returns the unschedulable set
/// (stable-sorted) if a cycle remains, else `None`.
pub(super) fn induced_cycle(plan: &VectorPlan, members: &AHashSet<u64>) -> Option<Vec<u64>> {
    let mut pending: AHashMap<u64, AHashSet<u64>> = AHashMap::new();
    let mut successors: AHashMap<u64, Vec<u64>> = AHashMap::new();
    for &m in members {
        pending.insert(m, AHashSet::new());
    }
    for edge in plan.data_edges.iter().chain(plan.effect_edges.iter()) {
        if members.contains(&edge.consumer) && members.contains(&edge.dependency) {
            pending
                .get_mut(&edge.consumer)
                .expect("consumer is a member")
                .insert(edge.dependency);
            successors
                .entry(edge.dependency)
                .or_default()
                .push(edge.consumer);
        }
    }
    let mut ready: Vec<u64> = pending
        .iter()
        .filter(|(_, deps)| deps.is_empty())
        .map(|(&n, _)| n)
        .collect();
    let mut removed: AHashSet<u64> = AHashSet::new();
    while let Some(n) = ready.pop() {
        if !removed.insert(n) {
            continue;
        }
        if let Some(succs) = successors.get(&n) {
            for &s in succs {
                if let Some(set) = pending.get_mut(&s) {
                    set.remove(&n);
                    if set.is_empty() && !removed.contains(&s) {
                        ready.push(s);
                    }
                }
            }
        }
    }
    if removed.len() == members.len() {
        None
    } else {
        let mut remaining: Vec<u64> = members.difference(&removed).copied().collect();
        remaining.sort_unstable();
        Some(remaining)
    }
}
