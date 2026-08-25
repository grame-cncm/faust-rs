//! Tests for `vector::verify` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use propagate::ClockDomainTable;
use signals::SigBuilder;
use tlib::TreeArena;

use super::super::super::decoration_verify::VerifiedDecorationCertificate;
use super::super::analysis::{DepKind, ForeignPurity};
use super::*;
use crate::clk_env::annotate;
use crate::signal_fir::decoration_verify::certify_decorations;
use crate::signal_fir::vector::analysis::{
    ForeignResource, ForeignTypeCode, StateCell, StateResource, effect_sets_conflict,
};
use crate::signal_prepare::prepare_signals_for_fir_verified;
use ahash::AHashMap;

#[test]
fn checked_effect_summaries_match_atom_pair_semantics() {
    let state = StateResource::Signal {
        owner: 7,
        cell: StateCell::Delay,
    };
    let atoms = vec![
        EffectAtom::ReadState(state.clone()),
        EffectAtom::WriteState(state),
        EffectAtom::ReadTable(3),
        EffectAtom::WriteTable(3),
        EffectAtom::WriteUi(4),
        EffectAtom::WriteOutput(5),
        EffectAtom::Foreign {
            resource: ForeignResource::Variable {
                name: "unknown".to_owned(),
                value_type: ForeignTypeCode(1),
            },
            purity: ForeignPurity::Unknown,
        },
        EffectAtom::Foreign {
            resource: ForeignResource::Variable {
                name: "pure".to_owned(),
                value_type: ForeignTypeCode(1),
            },
            purity: ForeignPurity::Pure,
        },
    ];
    let mut sets = vec![Vec::new(), atoms.clone()];
    sets.extend(atoms.into_iter().map(|atom| vec![atom]));
    for left in &sets {
        for right in &sets {
            let left_signal = test_effect_signal(0, left.clone());
            let right_signal = test_effect_signal(1, right.clone());
            let signals = AHashMap::from([(0, &left_signal), (1, &right_signal)]);
            let left_loop = test_effect_loop(0, 0);
            let right_loop = test_effect_loop(1, 1);
            assert_eq!(
                CheckedEffectConflictSummary::new(&signals, &left_loop)
                    .conflicts(&CheckedEffectConflictSummary::new(&signals, &right_loop)),
                effect_sets_conflict(left, right),
                "summary mismatch for {left:?} vs {right:?}"
            );
        }
    }
}

fn test_effect_signal(signal_id: u64, effects: Vec<EffectAtom>) -> SignalRecord {
    SignalRecord {
        signal_id,
        value_type: ValueType::Real,
        structural: false,
        rate: Rate::Samp,
        vectorability: Vectorability::Scal,
        clock_id: 0,
        direct_effects: effects.clone(),
        effects,
        placement: Placement::Owned(signal_id),
        duplicable: false,
    }
}

fn test_effect_loop(loop_id: u64, root: u64) -> LoopRecord {
    LoopRecord {
        loop_id,
        stable_name: format!("effect_loop_{loop_id}"),
        kind: LoopKind::Island(loop_id),
        roots: vec![root],
        epoch_id: 0,
    }
}

/// A minimal valid two-loop plan mirroring the PV DSP shape: loop 0 owns
/// `x` (a vectorizable producer), loop 1 consumes it (vectorizable), one
/// typed transport, both in the single forward epoch.
fn valid_plan() -> VectorPlan {
    VectorPlan {
        schema_version: crate::signal_fir::vector::verify::VECTOR_PLAN_SCHEMA_VERSION,
        lockstep_bundles: Vec::new(),
        vec_size: 16,
        signals: vec![
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
                stable_name: "loop_owns_x".to_owned(),
                kind: LoopKind::Vectorizable,
                roots: vec![10],
                epoch_id: 0,
            },
            LoopRecord {
                loop_id: 1,
                stable_name: "loop_consumes".to_owned(),
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
            stable_name: "transportX".to_owned(),
            signal_id: 10,
            producer_loop: 0,
            consumer_loop: 1,
            element_type: ValueType::Real,
            length: 16,
            layout: crate::signal_fir::vector::verify::TransportLayout::Planar,
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
    }
}

fn structural_fused_plan() -> VectorPlan {
    let mut plan = valid_plan();
    plan.loops[0].kind = LoopKind::Recursive(7);
    plan.vec_safe_witnesses[0].witness_kind = WitnessKind::SerialStateInternal;
    plan.transports[0].signal_id = 11;
    plan.transports[0].producer_loop = 1;
    plan.transports[0].consumer_loop = 0;
    plan.fused_serial_groups = vec![FusedSerialGroupRecord {
        group_id: 0,
        owner_loop_id: 0,
        member_loop_ids: vec![0, 1],
        state_carrier_signal_ids: vec![10],
        delayed_read_signal_ids: vec![11],
        state_write_signal_ids: vec![10],
        internal_transport_ids: vec![0],
        output_or_transport_roots: vec![10],
    }];
    plan
}

fn structural_lockstep_plan() -> VectorPlan {
    let mut plan = valid_plan();
    plan.transports.clear();
    plan.data_edges.clear();
    for (loop_record, group) in plan.loops.iter_mut().zip([7_u64, 8]) {
        loop_record.kind = LoopKind::Lockstep { width: 2 };
        let witness = plan
            .vec_safe_witnesses
            .iter_mut()
            .find(|witness| witness.loop_id == loop_record.loop_id)
            .expect("reference loop has a witness");
        witness.witness_kind = WitnessKind::SerialStateInternal;
        assert!(group > 0);
    }
    plan.lockstep_bundles = vec![LockstepBundleRecord {
        bundle_id: 0,
        representative_loop_id: 0,
        member_loop_ids: vec![0, 1],
        lanes: vec![
            LockstepLaneRecord {
                loop_id: 0,
                recursion_group: 7,
                roots: vec![IsoRootWitness {
                    representative_root: 10,
                    lane_root: 10,
                    shape_hash: 0x10,
                    leaf_mapping: vec![IsoLeafMapping {
                        representative_signal_id: 10,
                        lane_signal_id: 10,
                    }],
                }],
            },
            LockstepLaneRecord {
                loop_id: 1,
                recursion_group: 8,
                roots: vec![IsoRootWitness {
                    representative_root: 10,
                    lane_root: 11,
                    shape_hash: 0x10,
                    leaf_mapping: vec![IsoLeafMapping {
                        representative_signal_id: 10,
                        lane_signal_id: 11,
                    }],
                }],
            },
        ],
    }];
    plan
}

fn fused_decoration_fixture() -> (VectorPlan, VerifiedDecorationCertificate) {
    let mut arena = TreeArena::new();
    let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let feedback = builder.proj(0, self_ref);
        let previous = builder.delay1(feedback);
        builder.binop(signals::BinOp::Add, input, previous)
    };
    let nil = arena.nil();
    let bodies = arena.cons(body, nil);
    let recursion = tlib::de_bruijn_rec(&mut arena, bodies);
    let output = {
        let mut builder = SigBuilder::new(&mut arena);
        let projection = builder.proj(0, recursion);
        builder.output(0, projection)
    };
    let prepared = prepare_signals_for_fir_verified(&arena, &[output], &ui::UiProgram::empty())
        .expect("prepare recursive fixture");
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .expect("clock recursive fixture");
    let decorations = certify_decorations(&prepared, &clocks).expect("decorate fixture");
    let certificate = decorations.certificate();
    let dependency = certificate
        .dependencies
        .iter()
        .find(|dependency| {
            matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0)
                && certificate.records.iter().any(|record| {
                    record.signal_id == dependency.to
                        && record.max_delay > 0
                        && record.recursive_projection.is_some()
                })
        })
        .expect("recursive delayed dependency");
    let read_id = u64::from(dependency.from);
    let carrier_id = u64::from(dependency.to);
    let record = |signal_id: u64| {
        certificate
            .records
            .iter()
            .find(|record| u64::from(record.signal_id) == signal_id)
            .expect("referenced decoration record")
    };
    let carrier = record(carrier_id);
    let read = record(read_id);
    let middle_id = certificate
        .records
        .iter()
        .map(|record| u64::from(record.signal_id))
        .find(|signal_id| *signal_id != carrier_id && *signal_id != read_id)
        .expect("recursive fixture has an intermediate signal");
    let recursion_group = u64::from(carrier.recursive_projection.unwrap().group);
    let signal = |signal_id: u64, owner: u64| {
        let decoration = record(signal_id);
        SignalRecord {
            signal_id,
            value_type: ValueType::Real,
            structural: false,
            rate: Rate::Samp,
            vectorability: Vectorability::Scal,
            clock_id: decoration
                .clock_domain
                .map_or(0, |clock| u64::from(clock) + 1),
            effects: decoration.effects.clone(),
            direct_effects: decoration.direct_effects.clone(),
            placement: Placement::Owned(owner),
            duplicable: effects_duplicable(&decoration.effects),
        }
    };
    let mut signals = vec![
        signal(carrier_id, 0),
        signal(read_id, 1),
        signal(middle_id, 2),
    ];
    signals.sort_by_key(|signal| signal.signal_id);
    let plan = VectorPlan {
        schema_version: crate::signal_fir::vector::verify::VECTOR_PLAN_SCHEMA_VERSION,
        lockstep_bundles: Vec::new(),
        vec_size: 16,
        signals,
        loops: vec![
            LoopRecord {
                loop_id: 0,
                stable_name: "fused_owner".to_owned(),
                kind: LoopKind::Recursive(recursion_group),
                roots: vec![carrier_id],
                epoch_id: 0,
            },
            LoopRecord {
                loop_id: 1,
                stable_name: "fused_reader".to_owned(),
                kind: LoopKind::Island(0),
                roots: vec![read_id],
                epoch_id: 0,
            },
            LoopRecord {
                loop_id: 2,
                stable_name: "fused_middle".to_owned(),
                kind: LoopKind::Island(1),
                roots: vec![middle_id],
                epoch_id: 0,
            },
        ],
        epochs: vec![EpochRecord {
            epoch_id: 0,
            rank: 0,
            loops: vec![0, 1, 2],
        }],
        transports: vec![
            TransportRecord {
                transport_id: 0,
                stable_name: "fused_delayed_read".to_owned(),
                signal_id: read_id,
                producer_loop: 1,
                consumer_loop: 2,
                element_type: ValueType::Real,
                length: 16,
                layout: crate::signal_fir::vector::verify::TransportLayout::Planar,
            },
            TransportRecord {
                transport_id: 1,
                stable_name: "fused_middle_value".to_owned(),
                signal_id: middle_id,
                producer_loop: 2,
                consumer_loop: 0,
                element_type: ValueType::Real,
                length: 16,
                layout: crate::signal_fir::vector::verify::TransportLayout::Planar,
            },
        ],
        data_edges: vec![
            LoopEdge {
                consumer: 0,
                dependency: 2,
            },
            LoopEdge {
                consumer: 2,
                dependency: 1,
            },
        ],
        effect_edges: vec![],
        vec_safe_witnesses: vec![
            VecSafeWitness {
                loop_id: 0,
                witness_kind: WitnessKind::SerialStateInternal,
            },
            VecSafeWitness {
                loop_id: 1,
                witness_kind: WitnessKind::SerialStateInternal,
            },
            VecSafeWitness {
                loop_id: 2,
                witness_kind: WitnessKind::SerialStateInternal,
            },
        ],
        fused_serial_groups: vec![FusedSerialGroupRecord {
            group_id: 0,
            owner_loop_id: 0,
            member_loop_ids: vec![0, 1, 2],
            state_carrier_signal_ids: vec![carrier_id],
            delayed_read_signal_ids: vec![read_id],
            state_write_signal_ids: vec![carrier_id],
            internal_transport_ids: vec![0, 1],
            output_or_transport_roots: {
                let mut roots = vec![carrier_id, middle_id];
                roots.sort_unstable();
                roots
            },
        }],
    };
    assert_eq!(carrier.clock_domain, read.clock_domain);
    (plan, decorations)
}

#[test]
fn the_reference_plan_verifies() {
    verify_vector_plan(&valid_plan()).expect("reference plan is valid");
}

#[test]
fn structural_lockstep_bundle_verifies() {
    verify_vector_plan(&structural_lockstep_plan()).expect("lockstep shape is valid");
}

#[test]
fn rejects_lockstep_width_mutation() {
    let mut plan = structural_lockstep_plan();
    plan.loops[1].kind = LoopKind::Lockstep { width: 3 };
    assert_eq!(
        verify_vector_plan(&plan),
        Err(VectorPlanError::LockstepWidthMismatch { bundle_id: 0 })
    );
}

#[test]
fn rejects_dependency_connected_lockstep_lanes() {
    let mut plan = structural_lockstep_plan();
    plan.data_edges.push(LoopEdge {
        consumer: 1,
        dependency: 0,
    });
    assert_eq!(
        verify_vector_plan(&plan),
        Err(VectorPlanError::LockstepDependentLanes {
            bundle_id: 0,
            left: 0,
            right: 1,
        })
    );
}

#[test]
fn rejects_corrupted_lockstep_leaf_mapping() {
    let mut plan = structural_lockstep_plan();
    plan.lockstep_bundles[0].lanes[1].roots[0].leaf_mapping[0].lane_signal_id = 99;
    assert_eq!(
        verify_vector_plan(&plan),
        Err(VectorPlanError::LockstepIsoWitnessMismatch {
            bundle_id: 0,
            loop_id: 1,
        })
    );
}

#[test]
fn rejects_v2_plan_at_the_v3_boundary() {
    let mut plan = valid_plan();
    plan.schema_version = 2;
    assert_eq!(
        verify_vector_plan(&plan),
        Err(VectorPlanError::UnsupportedSchema { found: 2 })
    );
}

#[test]
fn structural_fused_group_verifies() {
    verify_vector_plan(&structural_fused_plan()).expect("fused group shape is valid");
}

#[test]
fn structural_multi_carrier_group_is_canonical() {
    let mut plan = structural_fused_plan();
    let group = &mut plan.fused_serial_groups[0];
    group.state_carrier_signal_ids = vec![10, 11];
    group.state_write_signal_ids = vec![10, 11];
    group.output_or_transport_roots = vec![10, 11];
    verify_vector_plan(&plan).expect("ascending multi-carrier set is finite-shape valid");

    plan.fused_serial_groups[0]
        .state_carrier_signal_ids
        .reverse();
    assert!(matches!(
        verify_vector_plan(&plan),
        Err(VectorPlanError::NotCanonical {
            what: "state_carrier_signal_ids",
            ..
        })
    ));
}

#[test]
fn rejects_missing_or_duplicated_state_carrier() {
    let mut missing = structural_fused_plan();
    missing.fused_serial_groups[0]
        .state_carrier_signal_ids
        .clear();
    assert!(matches!(
        verify_vector_plan(&missing),
        Err(VectorPlanError::FusedGroupEmpty {
            what: "state_carrier_signal_ids",
            ..
        })
    ));

    let mut duplicated = structural_fused_plan();
    duplicated.fused_serial_groups[0].state_carrier_signal_ids = vec![10, 10];
    assert!(matches!(
        verify_vector_plan(&duplicated),
        Err(VectorPlanError::NotCanonical {
            what: "state_carrier_signal_ids",
            ..
        })
    ));
}

#[test]
fn rejects_empty_fused_group() {
    let mut plan = structural_fused_plan();
    plan.fused_serial_groups[0].member_loop_ids.clear();
    assert!(matches!(
        verify_vector_plan(&plan),
        Err(VectorPlanError::FusedGroupEmpty {
            what: "member_loop_ids",
            ..
        })
    ));
}

#[test]
fn rejects_unknown_and_duplicated_fused_member_loops() {
    let mut unknown = structural_fused_plan();
    unknown.fused_serial_groups[0].member_loop_ids.push(99);
    assert!(matches!(
        verify_vector_plan(&unknown),
        Err(VectorPlanError::FusedGroupUnknownLoop { loop_id: 99, .. })
    ));

    let mut duplicate = structural_fused_plan();
    duplicate.fused_serial_groups[0].member_loop_ids = vec![0, 0, 1];
    assert!(matches!(
        verify_vector_plan(&duplicate),
        Err(VectorPlanError::NotCanonical {
            what: "member_loop_ids",
            ..
        })
    ));
}

#[test]
fn decoration_backed_fused_group_verifies() {
    let (plan, decorations) = fused_decoration_fixture();
    verify_fused_serial_groups(&plan, &decorations)
        .expect("real delayed recursion facts certify the synthetic fused group");
}

#[test]
fn fused_checker_rejects_extra_non_delayed_carrier() {
    let (mut plan, decorations) = fused_decoration_fixture();
    let read = plan.fused_serial_groups[0].delayed_read_signal_ids[0];
    plan.fused_serial_groups[0]
        .state_carrier_signal_ids
        .push(read);
    plan.fused_serial_groups[0]
        .state_carrier_signal_ids
        .sort_unstable();
    assert!(matches!(
        verify_fused_serial_groups(&plan, &decorations),
        Err(VectorPlanError::FusedGroupCarrierNotDelayedState { .. })
    ));
}

#[test]
fn fused_checker_rejects_read_without_delayed_dependency() {
    let (mut plan, decorations) = fused_decoration_fixture();
    let carrier = plan.fused_serial_groups[0].state_carrier_signal_ids[0];
    plan.fused_serial_groups[0].delayed_read_signal_ids = vec![carrier];
    assert!(matches!(
        verify_fused_serial_groups(&plan, &decorations),
        Err(VectorPlanError::FusedGroupDelayedDependencyMissing { .. })
    ));
}

#[test]
fn fused_checker_rejects_unlisted_dangerous_transport() {
    let (mut plan, decorations) = fused_decoration_fixture();
    plan.fused_serial_groups[0].internal_transport_ids.clear();
    assert!(matches!(
        verify_fused_serial_groups(&plan, &decorations),
        Err(VectorPlanError::FusedGroupDangerousTransportPresent {
            transport_id: 0,
            ..
        })
    ));
}

#[test]
fn fused_checker_rejects_a_missing_intermediate_path_loop() {
    let (mut plan, decorations) = fused_decoration_fixture();
    let group = &mut plan.fused_serial_groups[0];
    group.member_loop_ids = vec![0, 1];
    group.internal_transport_ids.clear();
    group.output_or_transport_roots = vec![group.state_carrier_signal_ids[0]];
    assert!(matches!(
        verify_fused_serial_groups(&plan, &decorations),
        Err(VectorPlanError::FusedGroupPathIncomplete { loop_id: 2, .. })
    ));
}

#[test]
fn fused_checker_rejects_recursive_member_without_state_writer() {
    let (mut plan, decorations) = fused_decoration_fixture();
    plan.loops[1].kind = LoopKind::Recursive(999);
    assert!(matches!(
        verify_fused_serial_groups(&plan, &decorations),
        Err(VectorPlanError::FusedGroupRecursiveMemberMissingWriter { loop_id: 1, .. })
    ));
}

#[test]
fn fused_checker_rejects_incompatible_clock_islands() {
    let (mut plan, decorations) = fused_decoration_fixture();
    let read = plan.fused_serial_groups[0].delayed_read_signal_ids[0];
    plan.signals
        .iter_mut()
        .find(|signal| signal.signal_id == read)
        .unwrap()
        .clock_id += 1;
    assert!(matches!(
        verify_fused_serial_groups(&plan, &decorations),
        Err(VectorPlanError::FusedGroupClockMismatch { .. })
    ));
}

// ── one rejecting mutation per obligation (plan provenance: §8) ──────────────────

#[test]
fn rejects_zero_vec_size() {
    let mut p = valid_plan();
    p.vec_size = 0;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::VecSizeZero)
    ));
}

#[test]
fn rejects_noncanonical_loops() {
    let mut p = valid_plan();
    p.loops.reverse();
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::NotCanonical { what: "loops", .. })
    ));
}

#[test]
fn rejects_a_loop_in_two_epochs() {
    let mut p = valid_plan();
    p.epochs = vec![
        EpochRecord {
            epoch_id: 0,
            rank: 0,
            loops: vec![0, 1],
        },
        EpochRecord {
            epoch_id: 1,
            rank: 1,
            loops: vec![1],
        },
    ];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::EpochCoverageMismatch { loop_id: 1 })
    ));
}

#[test]
fn rejects_a_loop_covered_by_no_epoch() {
    let mut p = valid_plan();
    p.epochs[0].loops = vec![0];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::EpochCoverageMismatch { loop_id: 1 })
    ));
}

#[test]
fn rejects_owned_signal_absent_from_roots() {
    let mut p = valid_plan();
    p.loops[0].roots.clear();
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::OwnedSignalNotRoot {
            signal_id: 10,
            loop_id: 0
        })
    ));
}

#[test]
fn rejects_unknown_root_before_inspecting_its_effects() {
    let mut p = valid_plan();
    p.loops[1].roots.push(99);
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::RootUnknownSignal {
            loop_id: 1,
            signal_id: 99
        })
    ));
}

#[test]
fn rejects_inline_signal_not_duplicable() {
    let mut p = valid_plan();
    // Detach signal 10 from loop 0 ownership and make it a non-duplicable
    // inline signal.
    p.loops[0].roots = vec![];
    p.loops[0].kind = LoopKind::Vectorizable;
    p.signals[0].placement = Placement::Inline;
    p.signals[0].duplicable = false;
    p.signals[0].effects = vec![EffectAtom::WriteState(StateResource::Signal {
        owner: 10,
        cell: StateCell::Delay,
    })];
    // Give loop 0 a different owned root so it stays valid otherwise.
    p.signals.insert(
        0,
        SignalRecord {
            signal_id: 5,
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
    );
    p.loops[0].roots = vec![5];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::InlineNotDuplicable { signal_id: 10 })
    ));

    p.signals
        .iter_mut()
        .find(|signal| signal.signal_id == 10)
        .unwrap()
        .structural = true;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::InlineNotDuplicable { signal_id: 10 })
    ));
}

#[test]
fn rejects_a_duplicability_bit_not_derived_from_effects() {
    let mut p = valid_plan();
    p.signals[0].duplicable = false;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::DuplicabilityMismatch { signal_id: 10 })
    ));
}

#[test]
fn rejects_a_loop_epoch_field_not_matching_membership() {
    let mut p = valid_plan();
    p.loops[1].epoch_id = 7;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::LoopEpochMismatch {
            loop_id: 1,
            declared: 7,
            actual: 0
        })
    ));
}

#[test]
fn rejects_noncanonical_vec_safe_witnesses() {
    let mut p = valid_plan();
    p.vec_safe_witnesses.reverse();
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::NotCanonical {
            what: "vec_safe_witnesses",
            ..
        })
    ));
}

#[test]
fn rejects_vectorizable_loop_whose_root_is_not_vec_safe() {
    let mut p = valid_plan();
    p.signals[1].vectorability = Vectorability::Scal;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::VectorizableNotSafe { loop_id: 1 })
    ));
}

#[test]
fn rejects_unordered_conflicting_effects() {
    let mut p = valid_plan();
    let effect = EffectAtom::WriteOutput(0);
    for signal in &mut p.signals {
        signal.effects = vec![effect.clone()];
        signal.duplicable = false;
    }
    p.data_edges.clear();
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::UnorderedEffectConflict { left: 0, right: 1 })
    ));
}

#[test]
fn rejects_edge_to_unknown_loop() {
    let mut p = valid_plan();
    p.data_edges = vec![LoopEdge {
        consumer: 1,
        dependency: 99,
    }];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::EdgeEndpointUnknown { missing: 99, .. })
    ));
}

#[test]
fn rejects_loop_self_edge() {
    let mut p = valid_plan();
    p.data_edges = vec![LoopEdge {
        consumer: 1,
        dependency: 1,
    }];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::LoopSelfEdge { loop_id: 1 })
    ));
}

#[test]
fn rejects_a_cyclic_epoch() {
    let mut p = valid_plan();
    // Canonical ascending order by (consumer, dependency), so the cycle
    // 0 -> 1 -> 0 reaches the acyclicity check rather than tripping the
    // canonical-order check first.
    p.data_edges = vec![
        LoopEdge {
            consumer: 0,
            dependency: 1,
        },
        LoopEdge {
            consumer: 1,
            dependency: 0,
        },
    ];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::EpochNotAcyclic { epoch_id: 0, .. })
    ));
}

#[test]
fn rejects_transport_self_loop() {
    let mut p = valid_plan();
    p.transports[0].consumer_loop = 0;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::TransportSelfLoop { transport_id: 0 })
    ));
}

#[test]
fn rejects_transport_type_mismatch() {
    let mut p = valid_plan();
    p.transports[0].element_type = ValueType::Int;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::TransportTypeMismatch { transport_id: 0 })
    ));
}

#[test]
fn rejects_transport_length_mismatch() {
    let mut p = valid_plan();
    p.transports[0].length = 8;
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::TransportLengthMismatch { transport_id: 0 })
    ));
}

#[test]
fn rejects_a_backwards_barrier() {
    let mut p = valid_plan();
    // Two epochs: rank 0 = {0}, rank 1 = {1}. An edge 0 -> 1 makes the
    // rank-0 consumer depend on a rank-1 dependency: a backwards barrier.
    p.epochs = vec![
        EpochRecord {
            epoch_id: 0,
            rank: 0,
            loops: vec![0],
        },
        EpochRecord {
            epoch_id: 1,
            rank: 1,
            loops: vec![1],
        },
    ];
    p.loops[0].epoch_id = 0;
    p.loops[1].epoch_id = 1;
    p.data_edges = vec![LoopEdge {
        consumer: 0,
        dependency: 1,
    }];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::BarrierViolation { .. })
    ));
}

#[test]
fn rejects_vectorizable_loop_without_witness() {
    let mut p = valid_plan();
    p.vec_safe_witnesses.retain(|w| w.loop_id != 1);
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::VectorizableWithoutWitness { loop_id: 1 })
    ));
}

#[test]
fn rejects_serial_loop_claiming_pointwise_witness() {
    let mut p = valid_plan();
    p.loops[1].kind = LoopKind::Recursive(0);
    // loop 1's witness is still Pointwise from the reference plan.
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::SerialLoopNotSerial { loop_id: 1 })
    ));
}

#[test]
fn recursive_loop_with_serial_witness_is_accepted() {
    let mut p = valid_plan();
    p.loops[1].kind = LoopKind::Recursive(0);
    for w in &mut p.vec_safe_witnesses {
        if w.loop_id == 1 {
            w.witness_kind = WitnessKind::SerialStateInternal;
        }
    }
    verify_vector_plan(&p).expect("a serial loop with a serial witness is valid");
}

#[test]
fn changing_only_edge_order_is_rejected_as_noncanonical_not_accepted_silently() {
    // The verifier rejects a noncanonical *equivalent* set, so a plan is
    // identified by canonical bytes (P-Strategy support): reordering edges
    // is not silently accepted.
    let mut p = valid_plan();
    p.effect_edges = vec![
        LoopEdge {
            consumer: 1,
            dependency: 0,
        },
        LoopEdge {
            consumer: 1,
            dependency: 0,
        },
    ];
    assert!(matches!(
        verify_vector_plan(&p),
        Err(VectorPlanError::NotCanonical {
            what: "effect_edges",
            ..
        })
    ));
}
