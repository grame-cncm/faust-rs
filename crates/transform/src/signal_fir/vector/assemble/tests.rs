//! Tests for `vector::assemble` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use fir::{AccessType, FirBuilder, FirMatch, FirStore, FirType, match_fir};

use super::*;
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::vector::clock_ad::{
    ClockGuard, ClockIsland, ClockTransportMode, ForwardAdPolicy, VECTOR_CLOCK_AD_PLAN_VERSION,
    VectorClockAdPlan, verified_vector_clock_ad_plan_for_test,
};
use crate::signal_fir::vector::plan::verified_vector_plan_for_test;
use crate::signal_fir::vector::route::{RouteResolution, VectorRouteSession};
use crate::signal_fir::vector::state::{
    DelayTransition, LoopStatePhases, RecursionProjectionTransition, RecursionTransition,
    VECTOR_STATE_PLAN_VERSION, VectorDelayStorage, VectorStateAction, VectorStatePlan,
    VerifiedVectorStatePlan, verified_vector_state_plan_for_test,
};
use crate::signal_fir::vector::verify::ValueType;
use crate::signal_fir::vector::verify::{
    EpochRecord, FusedSerialGroupRecord, IsoLeafMapping, IsoRootWitness, LockstepBundleRecord,
    LockstepLaneRecord, LoopEdge, LoopKind, LoopRecord, Placement, Rate, SignalRecord,
    TransportRecord, VecSafeWitness, VectorPlan, Vectorability, WitnessKind,
};

fn lockstep_vector_plan() -> super::super::plan::VerifiedVectorPlan {
    verified_vector_plan_for_test(VectorPlan {
        schema_version: crate::signal_fir::vector::verify::VECTOR_PLAN_SCHEMA_VERSION,
        vec_size: 8,
        signals: (0..2)
            .map(|lane| SignalRecord {
                signal_id: 10 + lane,
                value_type: ValueType::Real,
                structural: false,
                rate: Rate::Samp,
                vectorability: Vectorability::Scal,
                clock_id: 0,
                effects: vec![],
                direct_effects: vec![],
                placement: Placement::Owned(lane),
                duplicable: true,
            })
            .collect(),
        loops: (0..2)
            .map(|lane| LoopRecord {
                loop_id: lane,
                stable_name: format!("lockstep_lane_{lane}"),
                kind: LoopKind::Lockstep { width: 2 },
                roots: vec![10 + lane],
                epoch_id: 0,
            })
            .collect(),
        epochs: vec![EpochRecord {
            epoch_id: 0,
            rank: 0,
            loops: vec![0, 1],
        }],
        transports: vec![],
        data_edges: vec![],
        effect_edges: vec![],
        vec_safe_witnesses: (0..2)
            .map(|loop_id| VecSafeWitness {
                loop_id,
                witness_kind: WitnessKind::SerialStateInternal,
            })
            .collect(),
        fused_serial_groups: vec![],
        lockstep_bundles: vec![LockstepBundleRecord {
            bundle_id: 0,
            representative_loop_id: 0,
            member_loop_ids: vec![0, 1],
            lanes: (0..2)
                .map(|lane| LockstepLaneRecord {
                    loop_id: lane,
                    recursion_group: 20 + lane,
                    roots: vec![IsoRootWitness {
                        representative_root: 10,
                        lane_root: 10 + lane,
                        shape_hash: 0x55,
                        leaf_mapping: vec![IsoLeafMapping {
                            representative_signal_id: 10,
                            lane_signal_id: 10 + lane,
                        }],
                    }],
                })
                .collect(),
        }],
    })
}

#[test]
fn lockstep_lanes_share_one_physical_sample_loop() {
    let vector = lockstep_vector_plan();
    let mut store = FirStore::new();
    let values = [
        FirBuilder::new(&mut store).float32(0.25),
        FirBuilder::new(&mut store).float32(0.5),
    ];
    let (mut route, _) = VectorRouteSession::new(
        &vector,
        SchedulingStrategy::DepthFirst,
        FirType::Float32,
        &mut store,
    )
    .expect("lockstep route");
    for (loop_id, value) in values.into_iter().enumerate() {
        route
            .define_in_loop(loop_id as u64, 10 + loop_id as u64, value, &mut store)
            .expect("lane definition");
    }
    let routed = route.finish(&store).expect("finish lockstep route");
    let inputs = values
        .into_iter()
        .enumerate()
        .map(|(loop_id, value)| VectorLoopFirInput {
            loop_id: loop_id as u64,
            statements: vec![FirBuilder::new(&mut store).drop_(value)],
        })
        .collect::<Vec<_>>();
    let verified = assemble_vector_fir(
        &routed,
        None,
        None,
        &inputs,
        &[],
        FirType::Float32,
        &mut store,
    )
    .expect("assemble lockstep lanes");
    let assembly = verified.assembly();
    let FirMatch::Block(top_level) = match_fir(&store, assembly.top_level_statement) else {
        panic!("top-level block");
    };
    let [physical_loop] = top_level.as_slice() else {
        panic!("exactly one physical loop");
    };
    let FirMatch::ForLoop { body, .. } = match_fir(&store, *physical_loop) else {
        panic!("physical sample loop");
    };
    assert_eq!(
        match_fir(&store, body),
        FirMatch::Block(
            assembly
                .loops
                .iter()
                .map(|loop_| loop_.iteration_statement)
                .collect()
        )
    );

    let mut forged = assembly.clone();
    let separate = assembly
        .loops
        .iter()
        .map(|loop_| sample_loop(&mut FirBuilder::new(&mut store), loop_.iteration_statement))
        .collect::<Vec<_>>();
    forged.top_level_statement = FirBuilder::new(&mut store).block(&separate);
    assert_eq!(
        verify_vector_fir_assembly(&routed, None, None, &forged, &store),
        Err(VectorFirAssemblyError::LockstepBundleShape { bundle_id: 0 })
    );
}

fn state_vector_plan() -> super::super::plan::VerifiedVectorPlan {
    verified_vector_plan_for_test(VectorPlan {
        schema_version: crate::signal_fir::vector::verify::VECTOR_PLAN_SCHEMA_VERSION,
        lockstep_bundles: Vec::new(),
        vec_size: 8,
        signals: vec![
            SignalRecord {
                signal_id: 11,
                value_type: ValueType::Real,
                structural: false,
                rate: Rate::Samp,
                vectorability: Vectorability::Scal,
                clock_id: 0,
                effects: vec![],
                direct_effects: vec![],
                placement: Placement::Owned(0),
                duplicable: true,
            },
            SignalRecord {
                signal_id: 12,
                value_type: ValueType::Real,
                structural: false,
                rate: Rate::Samp,
                vectorability: Vectorability::Scal,
                clock_id: 0,
                effects: vec![],
                direct_effects: vec![],
                placement: Placement::Owned(0),
                duplicable: true,
            },
        ],
        loops: vec![LoopRecord {
            loop_id: 0,
            stable_name: "recursive_1".to_owned(),
            kind: LoopKind::Recursive(1),
            roots: vec![11, 12],
            epoch_id: 0,
        }],
        epochs: vec![EpochRecord {
            epoch_id: 0,
            rank: 0,
            loops: vec![0],
        }],
        transports: vec![],
        data_edges: vec![],
        effect_edges: vec![],
        vec_safe_witnesses: vec![],
        fused_serial_groups: vec![],
    })
}

fn state_plan(vector: &super::super::plan::VerifiedVectorPlan) -> VerifiedVectorStatePlan {
    verified_vector_state_plan_for_test(
        VectorStatePlan {
            schema_version: VECTOR_STATE_PLAN_VERSION,
            vec_size: 8,
            max_copy_delay: 4,
            loops: vec![LoopStatePhases {
                loop_id: 0,
                pre: vec![
                    VectorStateAction::DelayCopyIn { signal_id: 11 },
                    VectorStateAction::DelayRingAdvance { signal_id: 12 },
                ],
                exec: vec![
                    VectorStateAction::RecursionStep { group: 1 },
                    VectorStateAction::DelayWrite { signal_id: 11 },
                    VectorStateAction::DelayWrite { signal_id: 12 },
                ],
                post: vec![
                    VectorStateAction::DelayCopyOut { signal_id: 11 },
                    VectorStateAction::DelayRingSaveAdvance { signal_id: 12 },
                ],
            }],
            delays: vec![
                DelayTransition {
                    signal_id: 11,
                    loop_id: 0,
                    clock_domain: None,
                    value_type: ValueType::Real,
                    max_delay: 3,
                    storage: VectorDelayStorage::Copy {
                        temporary_name: "fVec11_tmp".to_owned(),
                        permanent_name: "fVec11_perm".to_owned(),
                        history_length: 4,
                        temporary_length: 12,
                    },
                },
                DelayTransition {
                    signal_id: 12,
                    loop_id: 0,
                    clock_domain: None,
                    value_type: ValueType::Real,
                    max_delay: 5,
                    storage: VectorDelayStorage::Ring {
                        buffer_name: "fVec12".to_owned(),
                        index_name: "fVec12_idx".to_owned(),
                        index_save_name: "fVec12_idx_save".to_owned(),
                        capacity: 16,
                        mask: 15,
                    },
                },
            ],
            recursions: vec![RecursionTransition {
                group: 1,
                loop_id: 0,
                projections: vec![RecursionProjectionTransition {
                    index: 0,
                    signal_ids: vec![11],
                    value_signal_id: 11,
                }],
            }],
            lockstep_register_bundles: vec![],
            prefixes: vec![],
            waveforms: vec![],
            no_op_resources: vec![],
        },
        vector,
    )
}

#[test]
fn materializes_copy_ring_and_simultaneous_recursion_words() {
    let vector = state_vector_plan();
    let state = state_plan(&vector);
    let mut store = FirStore::new();
    let value11 = FirBuilder::new(&mut store).float32(0.25);
    let value12 = FirBuilder::new(&mut store).float32(0.5);
    let (mut route, _) = VectorRouteSession::new(
        &vector,
        SchedulingStrategy::DepthFirst,
        FirType::Float32,
        &mut store,
    )
    .expect("route");
    route
        .define_in_loop(0, 11, value11, &mut store)
        .expect("define 11");
    route
        .define_in_loop(0, 12, value12, &mut store)
        .expect("define 12");
    let routed = route.finish(&store).expect("finish route");
    let input = VectorLoopFirInput {
        loop_id: 0,
        statements: vec![],
    };
    let verified = assemble_vector_fir(
        &routed,
        Some(&state),
        None,
        &[input],
        &[],
        FirType::Float32,
        &mut store,
    )
    .expect("assemble state");
    let assembled = verified.assembly();
    assert_eq!(assembled.loops[0].pre.len(), 2);
    assert_eq!(assembled.loops[0].exec_actions.len(), 3);
    assert_eq!(assembled.loops[0].post.len(), 2);
    assert!(assembled.state_declarations.len() >= 4);
    assert!(matches!(
        match_fir(&store, assembled.loops[0].exec_actions[0].statement),
        FirMatch::Block(body) if body.len() == 1
    ));
    for action in [&assembled.loops[0].pre[0], &assembled.loops[0].post[0]] {
        let FirMatch::SimpleForLoop { body, .. } = match_fir(&store, action.statement) else {
            panic!("copy transition must materialize as a simple loop");
        };
        assert!(matches!(match_fir(&store, body), FirMatch::Block(words) if words.len() == 1));
    }
    assert!(assembled.clear_statements.iter().all(|statement| {
        let FirMatch::SimpleForLoop { body, .. } = match_fir(&store, *statement) else {
            return true;
        };
        matches!(match_fir(&store, body), FirMatch::Block(words) if words.len() == 1)
    }));

    let mut forged = assembled.clone();
    let FirMatch::SimpleForLoop {
        var,
        upper,
        body,
        is_reverse,
    } = match_fir(&store, forged.loops[0].pre[0].statement)
    else {
        panic!("copy-in loop");
    };
    let FirMatch::Block(words) = match_fir(&store, body) else {
        panic!("canonical copy-in body");
    };
    forged.loops[0].pre[0].statement =
        FirBuilder::new(&mut store).simple_for_loop(var, upper, words[0], is_reverse);
    assert!(matches!(
        verify_vector_fir_assembly(&routed, Some(&state), None, &forged, &store),
        Err(VectorFirAssemblyError::ActionShape {
            action: VectorStateAction::DelayCopyIn { signal_id: 11 },
            ..
        })
    ));

    let mut forged = assembled.clone();
    forged.loops[0].exec_actions[1].statement = FirBuilder::new(&mut store).int32(0);
    assert!(matches!(
        verify_vector_fir_assembly(&routed, Some(&state), None, &forged, &store),
        Err(VectorFirAssemblyError::ActionShape {
            action: VectorStateAction::DelayWrite { signal_id: 11 },
            ..
        })
    ));
}

fn clock_vector_plan() -> super::super::plan::VerifiedVectorPlan {
    verified_vector_plan_for_test(VectorPlan {
        schema_version: crate::signal_fir::vector::verify::VECTOR_PLAN_SCHEMA_VERSION,
        lockstep_bundles: Vec::new(),
        vec_size: 8,
        signals: vec![
            SignalRecord {
                signal_id: 1,
                value_type: ValueType::Int,
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
                signal_id: 10,
                value_type: ValueType::Real,
                structural: false,
                rate: Rate::Samp,
                vectorability: Vectorability::Scal,
                clock_id: 8,
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
            SignalRecord {
                signal_id: 12,
                value_type: ValueType::Real,
                structural: false,
                rate: Rate::Samp,
                vectorability: Vectorability::Scal,
                clock_id: 8,
                effects: vec![],
                direct_effects: vec![],
                placement: Placement::Owned(2),
                duplicable: true,
            },
        ],
        loops: vec![
            LoopRecord {
                loop_id: 0,
                stable_name: "island_7".to_owned(),
                kind: LoopKind::Island(7),
                roots: vec![10],
                epoch_id: 0,
            },
            LoopRecord {
                loop_id: 1,
                stable_name: "outer".to_owned(),
                kind: LoopKind::Vectorizable,
                roots: vec![11],
                epoch_id: 0,
            },
            LoopRecord {
                loop_id: 2,
                stable_name: "island_7_consumer".to_owned(),
                kind: LoopKind::Island(7),
                roots: vec![12],
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
                stable_name: "island_s10".to_owned(),
                signal_id: 10,
                producer_loop: 0,
                consumer_loop: 2,
                element_type: ValueType::Real,
                length: 8,
                layout: crate::signal_fir::vector::verify::TransportLayout::Planar,
            },
            TransportRecord {
                transport_id: 1,
                stable_name: "held_s12".to_owned(),
                signal_id: 12,
                producer_loop: 2,
                consumer_loop: 1,
                element_type: ValueType::Real,
                length: 8,
                layout: crate::signal_fir::vector::verify::TransportLayout::Planar,
            },
        ],
        data_edges: vec![
            LoopEdge {
                consumer: 1,
                dependency: 2,
            },
            LoopEdge {
                consumer: 2,
                dependency: 0,
            },
        ],
        effect_edges: vec![],
        vec_safe_witnesses: vec![VecSafeWitness {
            loop_id: 1,
            witness_kind: WitnessKind::Pointwise,
        }],
        fused_serial_groups: vec![FusedSerialGroupRecord {
            group_id: 0,
            owner_loop_id: 2,
            member_loop_ids: vec![0, 2],
            state_carrier_signal_ids: vec![12],
            delayed_read_signal_ids: vec![10],
            state_write_signal_ids: vec![12],
            internal_transport_ids: vec![0],
            output_or_transport_roots: vec![10, 12],
        }],
    })
}

#[test]
fn nests_clock_loop_and_materializes_held_transport_lifetime() {
    let vector = clock_vector_plan();
    let clock = verified_vector_clock_ad_plan_for_test(
        VectorClockAdPlan {
            schema_version: VECTOR_CLOCK_AD_PLAN_VERSION,
            vec_size: 8,
            clock_islands: vec![ClockIsland {
                domain_id: 7,
                parent_domain: None,
                kind: propagate::ClockDomainKind::OnDemand,
                clock_signal_id: 1,
                wrapper_signal_id: 10,
                boundary_loop_id: 0,
                guard: ClockGuard::CountedOnDemand,
                signal_ids: vec![10],
                clock_state_signal_ids: vec![],
                nested_loop_ids: vec![0, 2],
            }],
            transports: vec![
                super::super::clock_ad::ClockTransportPolicy {
                    transport_id: 0,
                    mode: ClockTransportMode::FusedScalar { group_id: 0 },
                },
                super::super::clock_ad::ClockTransportPolicy {
                    transport_id: 1,
                    mode: ClockTransportMode::HeldOutput { domain_id: 7 },
                },
            ],
            forward_ad: ForwardAdPolicy::ExpandedSignalGraph,
            reverse_ad_fallbacks: vec![],
        },
        &vector,
    );
    let state = verified_vector_state_plan_for_test(
        VectorStatePlan {
            schema_version: VECTOR_STATE_PLAN_VERSION,
            vec_size: 8,
            max_copy_delay: 16,
            loops: vec![LoopStatePhases {
                loop_id: 2,
                pre: vec![],
                exec: vec![VectorStateAction::DelayWrite { signal_id: 12 }],
                post: vec![],
            }],
            delays: vec![DelayTransition {
                signal_id: 12,
                loop_id: 2,
                value_type: ValueType::Real,
                max_delay: 3,
                clock_domain: Some(7),
                storage: VectorDelayStorage::ClockRing {
                    buffer_name: "vstate_s12".to_owned(),
                    cursor_name: "vclock_d7_iota".to_owned(),
                    domain_id: 7,
                    capacity: 4,
                    mask: 3,
                },
            }],
            recursions: vec![],
            lockstep_register_bundles: vec![],
            prefixes: vec![],
            waveforms: vec![],
            no_op_resources: vec![],
        },
        &vector,
    );
    let mut store = FirStore::new();
    let clock_value = FirBuilder::new(&mut store).int32(2);
    let value = FirBuilder::new(&mut store).float32(0.5);
    let (mut route, _) = VectorRouteSession::new_with_clock_plan(
        &vector,
        &clock,
        SchedulingStrategy::DepthFirst,
        FirType::Float32,
        &mut store,
    )
    .expect("clock route");
    route
        .define_control(1, clock_value, &store)
        .expect("clock definition");
    let island_stores = route
        .define_in_loop(0, 10, value, &mut store)
        .expect("island definition");
    let island_value = match route
        .resolve_in_loop(2, 10, &mut store)
        .expect("island scalar load")
    {
        RouteResolution::Value(value) => value,
        RouteResolution::NeedsInlineLowering => panic!("unexpected inline"),
    };
    let held_stores = route
        .define_in_loop(2, 12, island_value, &mut store)
        .expect("held definition");
    let loaded = match route.resolve_in_loop(1, 12, &mut store).expect("held load") {
        RouteResolution::Value(value) => value,
        RouteResolution::NeedsInlineLowering => panic!("unexpected inline"),
    };
    route
        .define_in_loop(1, 11, loaded, &mut store)
        .expect("outer definition");
    let routed = route.finish(&store).expect("finish route");
    let inputs = vec![
        VectorLoopFirInput {
            loop_id: 0,
            statements: island_stores,
        },
        VectorLoopFirInput {
            loop_id: 1,
            statements: vec![FirBuilder::new(&mut store).drop_(loaded)],
        },
        VectorLoopFirInput {
            loop_id: 2,
            statements: held_stores,
        },
    ];
    let clock_output = {
        let mut builder = FirBuilder::new(&mut store);
        let index = builder.load_var("i0", AccessType::Loop, FirType::Int32);
        VectorClockOutputStore {
            owner_loop_id: 2,
            statement: builder.store_table("output0", AccessType::Stack, index, loaded),
        }
    };
    let verified = assemble_vector_fir(
        &routed,
        Some(&state),
        Some(&clock),
        &inputs,
        std::slice::from_ref(&clock_output),
        FirType::Float32,
        &mut store,
    )
    .expect("assemble clock");
    let assembly = verified.assembly();
    assert_eq!(assembly.islands.len(), 1);
    assert!(assembly.islands[0].state_cursor_advance.is_some());
    assert!(assembly.islands[0].local_declarations.is_empty());
    assert_eq!(assembly.local_declarations.len(), 1);
    assert!(matches!(
        match_fir(&store, assembly.islands[0].statement),
        FirMatch::SimpleForLoop { .. }
    ));
    assert_eq!(assembly.state_declarations.len(), 3);
    assert_eq!(assembly.clear_statements.len(), 3);
    assert_eq!(assembly.clock_output_stores, [clock_output]);
    assert!(matches!(
        match_fir(&store, assembly.state_declarations[0]),
        FirMatch::DeclareVar {
            access: AccessType::Struct,
            ..
        }
    ));

    let mut forged = assembly.clone();
    forged.islands[0].statement = FirBuilder::new(&mut store).int32(0);
    assert!(matches!(
        verify_vector_fir_assembly(&routed, Some(&state), Some(&clock), &forged, &store),
        Err(VectorFirAssemblyError::IslandShape { domain_id: 7 })
    ));

    let mut forged = assembly.clone();
    let mut second_island = forged.islands[0].clone();
    second_island.nested_loop_ids = vec![2];
    forged.islands.push(second_island);
    assert!(matches!(
        verify_assembled_fused_serial_groups(&routed, Some(&state), &forged, &store),
        Err(VectorFirAssemblyError::FusedGroupShape { group_id: 0 })
    ));

    // Tolerating an eliminated delayed read must not tolerate a relocated one:
    // the read stays reachable from the assembled top level while the physical
    // body no longer holds it, which is an escape and not dead-code removal.
    let mut forged = assembly.clone();
    forged.islands[0].statement = FirBuilder::new(&mut store).int32(0);
    assert!(matches!(
        verify_assembled_fused_serial_groups(&routed, Some(&state), &forged, &store),
        Err(VectorFirAssemblyError::FusedGroupShape { group_id: 0 })
    ));

    let mut forged = assembly.clone();
    forged.clock_output_stores[0].owner_loop_id = 99;
    assert!(matches!(
        verify_vector_fir_assembly(&routed, Some(&state), Some(&clock), &forged, &store),
        Err(VectorFirAssemblyError::ClockLoopOwnership { loop_id: 99 })
    ));

    let mut forged = assembly.clone();
    forged.islands[0].state_cursor_advance = None;
    assert_eq!(
        verify_vector_fir_assembly(&routed, Some(&state), Some(&clock), &forged, &store),
        Err(VectorFirAssemblyError::IslandShape { domain_id: 7 })
    );
}
