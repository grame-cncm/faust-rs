//! Tests for `vector::lower` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use propagate::ClockDomainTable;
use signals::SigBuilder;
use sigtype::{Nature, Variability, Vectorability as SigVectorability};
use tlib::TreeArena;

use super::*;
use crate::clk_env::annotate;
use crate::signal_fir::decoration_verify::{CanonicalSigType, certify_decorations};
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::plan::verified_vector_plan_for_test;
use crate::signal_fir::vector::route::VectorRegion;
use crate::signal_fir::vector::verify::{
    EpochRecord, LoopEdge, LoopKind, LoopRecord, Rate, TransportRecord, VecSafeWitness, VectorPlan,
    Vectorability, WitnessKind,
};
use crate::signal_prepare::VerifiedPreparedSignals;
use crate::signal_prepare::prepare_signals_for_fir_verified;
use signals::BinOp;
use std::collections::BTreeSet;

use crate::schedule::SchedulingStrategy;
use crate::signal_fir::vector::verify::{Placement, SignalRecord, ValueType};
use fir::{AccessType, FirBuilder, FirMatch, FirStore, FirType, match_fir};
use signals::dump_sig_readable;
use signals::{SigId, SigMatch, match_sig};

fn pure_fixture() -> (VerifiedPreparedSignals, VerifiedVectorPlan) {
    let mut arena = TreeArena::new();
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let input0 = builder.input(0);
        let input1 = builder.input(1);
        let sum = builder.binop(BinOp::Add, input0, input1);
        let producer = builder.atan2(sum, sum);
        builder.binop(BinOp::Add, producer, input0)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let producer =
        find_repeated_atan2(prepared.arena(), prepared.outputs()[0]).unwrap_or_else(|| {
            panic!(
                "prepared pure fixture changed shape: {}",
                dump_sig_readable(prepared.arena(), prepared.outputs()[0])
            )
        });
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let consumer = prepared.outputs()[0];
    let signals = decorations
        .certificate()
        .records
        .iter()
        .map(|record| SignalRecord {
            signal_id: u64::from(record.signal_id),
            value_type: canonical_value_type(&record.sig_type),
            structural: record.is_symbolic_recursion_carrier,
            rate: match record.variability {
                Variability::Konst => Rate::Konst,
                Variability::Block => Rate::Block,
                Variability::Samp => Rate::Samp,
            },
            vectorability: match record.vectorability {
                SigVectorability::Vect => Vectorability::Vect,
                SigVectorability::Scal => Vectorability::Scal,
                SigVectorability::TrueScal => Vectorability::TrueScal,
            },
            clock_id: 0,
            effects: record.effects.clone(),
            direct_effects: record.direct_effects.clone(),
            placement: if record.signal_id == producer.as_u32() {
                Placement::Owned(0)
            } else if record.signal_id == consumer.as_u32() {
                Placement::Owned(1)
            } else if record.variability == Variability::Samp {
                Placement::Inline
            } else {
                Placement::Control
            },
            duplicable: true,
        })
        .collect();
    let plan = verified_vector_plan_for_test(VectorPlan {
        schema_version: crate::signal_fir::vector::verify::VECTOR_PLAN_SCHEMA_VERSION,
        lockstep_bundles: Vec::new(),
        vec_size: 16,
        signals,
        loops: vec![
            LoopRecord {
                loop_id: 0,
                stable_name: "loop_pure_producer".to_owned(),
                kind: LoopKind::Vectorizable,
                roots: vec![u64::from(producer.as_u32())],
                epoch_id: 0,
            },
            LoopRecord {
                loop_id: 1,
                stable_name: "loop_pure_consumer".to_owned(),
                kind: LoopKind::Vectorizable,
                roots: vec![u64::from(consumer.as_u32())],
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
            stable_name: "transport_pure_producer".to_owned(),
            signal_id: u64::from(producer.as_u32()),
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
    });
    (prepared, plan)
}

fn find_repeated_atan2(arena: &TreeArena, root: SigId) -> Option<SigId> {
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(signal) = stack.pop() {
        if !seen.insert(signal) {
            continue;
        }
        if matches!(
            match_sig(arena, signal),
            SigMatch::Atan2(lhs, rhs) if lhs == rhs
        ) {
            return Some(signal);
        }
        if let Some(children) = arena.children(signal) {
            stack.extend(children.iter().copied());
        }
    }
    None
}

fn canonical_value_type(sig_type: &CanonicalSigType) -> ValueType {
    match sig_type {
        CanonicalSigType::Sound => ValueType::Sound,
        CanonicalSigType::Simple { nature, .. } => match nature {
            Nature::Int => ValueType::Int,
            Nature::Real | Nature::Any => ValueType::Real,
        },
        CanonicalSigType::Table { content, .. } => canonical_value_type(content),
        CanonicalSigType::Tuplet { components, .. } => {
            ValueType::Tuple(components.iter().map(canonical_value_type).collect())
        }
    }
}

#[test]
fn lowers_actual_pure_closures_with_local_cse_and_transport() {
    let (prepared, plan) = pure_fixture();
    let program = lower_pure_vector_program(
        &prepared,
        &plan,
        SchedulingStrategy::DepthFirst,
        FirType::Float32,
        2,
    )
    .unwrap();
    assert_eq!(program.regions().len(), 2);
    assert!(program.regions()[0].statements().iter().any(|statement| {
        matches!(
            match_fir(program.store(), *statement),
            FirMatch::DeclareVar { name, .. } if name == "fVecL0Temp0"
        )
    }));
    assert!(!program.regions()[1].statements().iter().any(|statement| {
        matches!(
            match_fir(program.store(), *statement),
            FirMatch::DeclareVar { name, .. } if name.starts_with("fVecL0Temp")
        )
    }));
    assert!(body_contains(
        program.store(),
        program.regions()[1].statements(),
        program.routed().trace().transports()[0].load.unwrap()
    ));
    assert_eq!(program.routed().trace().transports().len(), 1);
}

#[test]
fn all_scheduling_strategies_keep_region_local_storage_names() {
    let (prepared, plan) = pure_fixture();
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        let program =
            lower_pure_vector_program(&prepared, &plan, strategy, FirType::Float64, 2).unwrap();
        assert_eq!(program.routed().trace().transports()[0].transport_id, 0);
        assert!(program.regions().iter().any(|region| {
            region.statements().iter().any(|statement| {
                matches!(
                    match_fir(program.store(), *statement),
                    FirMatch::DeclareVar { name, .. } if name == "fVecL0Temp0"
                )
            })
        }));
    }
}

#[test]
fn final_body_verifier_rejects_a_missing_consumer_body() {
    let (prepared, plan) = pure_fixture();
    let program = lower_pure_vector_program(
        &prepared,
        &plan,
        SchedulingStrategy::DepthFirst,
        FirType::Float32,
        2,
    )
    .unwrap();
    let mut regions = program.regions().to_vec();
    regions[1].statements.clear();
    assert!(matches!(
        verify_pure_vector_bodies(
            plan.plan(),
            program.routed(),
            program.transport_declarations(),
            program.control_statements(),
            &regions,
            None,
            program.store()
        ),
        Err(PureVectorLowerError::BodyEvidence { .. })
    ));
}

#[test]
fn body_transport_load_evidence_survives_cse_index_rewriting() {
    let mut store = FirStore::new();
    let expected = {
        let mut builder = FirBuilder::new(&mut store);
        let i0 = builder.load_var("i0", AccessType::Loop, FirType::Int32);
        builder.load_table(
            "transport_s1_l0_l1",
            AccessType::Stack,
            i0,
            FirType::Float32,
        )
    };
    let actual = {
        let mut builder = FirBuilder::new(&mut store);
        let index = builder.load_var("iTemp0", AccessType::Stack, FirType::Int32);
        let load = builder.load_table(
            "transport_s1_l0_l1",
            AccessType::Stack,
            index,
            FirType::Float32,
        );
        builder.drop_(load)
    };
    assert!(body_contains_equivalent_table_load(
        &store,
        &[actual],
        expected
    ));

    let wrong = {
        let mut builder = FirBuilder::new(&mut store);
        let index = builder.load_var("iTemp0", AccessType::Stack, FirType::Int32);
        let load = builder.load_table(
            "transport_s2_l0_l1",
            AccessType::Stack,
            index,
            FirType::Float32,
        );
        builder.drop_(load)
    };
    assert!(!body_contains_equivalent_table_load(
        &store,
        &[wrong],
        expected
    ));
}

#[test]
fn fused_scalar_transport_load_evidence_survives_cse_rebuilding() {
    let mut store = FirStore::new();
    let expected = FirBuilder::new(&mut store).load_var(
        "transport_s1_l0_l1",
        AccessType::Stack,
        FirType::Float32,
    );
    let actual_load = FirBuilder::new(&mut store).load_var(
        "transport_s1_l0_l1",
        AccessType::Stack,
        FirType::Float32,
    );
    let actual = FirBuilder::new(&mut store).drop_(actual_load);
    assert!(body_contains_equivalent_scalar_load(
        &store,
        &[actual],
        expected
    ));

    let wrong_load = FirBuilder::new(&mut store).load_var(
        "transport_s2_l0_l1",
        AccessType::Stack,
        FirType::Float32,
    );
    let wrong = FirBuilder::new(&mut store).drop_(wrong_load);
    assert!(!body_contains_equivalent_scalar_load(
        &store,
        &[wrong],
        expected
    ));
}

#[test]
fn external_control_promotes_a_trivial_root_before_final_drop_cleanup() {
    let mut store = FirStore::new();
    let value = FirBuilder::new(&mut store).float32(0.5);

    let (statements, rewritten, fields) =
        materialize_region_roots_promoted(&mut store, &[value], VectorRegion::Control).unwrap();

    assert_eq!(
        fields,
        vec![("fVecControlTemp0".to_owned(), FirType::Float32)]
    );
    assert_eq!(statements.len(), 2);
    assert!(matches!(
        match_fir(&store, statements[0]),
        FirMatch::StoreVar {
            ref name,
            access: AccessType::Struct,
            value: stored,
        } if name == "fVecControlTemp0" && stored == value
    ));
    assert!(matches!(
        match_fir(&store, rewritten[0]),
        FirMatch::LoadVar {
            ref name,
            access: AccessType::Struct,
            typ: FirType::Float32,
        } if name == "fVecControlTemp0"
    ));
    assert!(matches!(
        match_fir(&store, statements[1]),
        FirMatch::Drop(value) if value == rewritten[0]
    ));
}

#[test]
fn stateful_signal_fails_closed_before_region_lowering() {
    let mut arena = TreeArena::new();
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        builder.delay1(input)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let plan = crate::signal_fir::vector::plan::build_vector_plan(&decorations, 16).unwrap();
    assert!(matches!(
        lower_pure_vector_program(
            &prepared,
            &plan,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            1
        ),
        Err(PureVectorLowerError::EffectfulSignal { .. })
            | Err(PureVectorLowerError::UnsupportedSignal { .. })
    ));
}
