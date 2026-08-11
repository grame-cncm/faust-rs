//! Tests for `vector::plan` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use propagate::ClockDomainTable;
use signals::SigBuilder;
use tlib::TreeArena;

use super::build::EffectConflictSummary;
use super::*;
use crate::clk_env::annotate;
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::decoration_verify::CanonicalSigType;
use crate::signal_fir::decoration_verify::VerifiedDecorationCertificate;
use crate::signal_fir::decoration_verify::certify_decorations;
use crate::signal_fir::pv_slice::build_pv_signals;
use crate::signal_fir::vector::analysis::{
    DepKind, EffectAtom, ForeignPurity, StateCell, StateResource,
};
use crate::signal_fir::vector::schedule::schedule_vector_plan;
use crate::signal_fir::vector::verify::{LoopEdge, LoopKind, Placement, ValueType};
use crate::signal_prepare::prepare_signals_for_fir_verified;
use std::collections::BTreeSet;

fn certify(arena: &TreeArena, roots: &[signals::SigId]) -> VerifiedDecorationCertificate {
    let prepared = prepare_signals_for_fir_verified(arena, roots, &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .unwrap();
    certify_decorations(&prepared, &clocks).unwrap()
}

#[test]
fn compact_effect_summaries_match_atom_pair_semantics() {
    use crate::signal_fir::vector::analysis::{
        ForeignResource, ForeignTypeCode, effect_sets_conflict,
    };

    let state = StateResource::Signal {
        owner: 7,
        cell: crate::signal_fir::vector::analysis::StateCell::Delay,
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
            assert_eq!(
                EffectConflictSummary::new(left).conflicts(&EffectConflictSummary::new(right)),
                effect_sets_conflict(left, right),
                "summary mismatch for {left:?} vs {right:?}"
            );
        }
    }
}

#[test]
fn production_pv_plan_uses_certified_delay_and_occurrence_facts() {
    let (arena, y, z) = build_pv_signals(20);
    let decorations = certify(&arena, &[y, z]);
    let verified = build_vector_plan(&decorations, 16).unwrap();
    let plan = verified.plan();

    assert_eq!(plan.signals.len(), decorations.certificate().records.len());
    assert!(plan.loops.len() >= 2);
    let delayed_carrier = decorations
        .certificate()
        .records
        .iter()
        .find(|record| record.max_delay == 20)
        .unwrap();
    assert!(matches!(
        plan.signals
            .iter()
            .find(|signal| signal.signal_id == u64::from(delayed_carrier.signal_id))
            .unwrap()
            .placement,
        Placement::Owned(_)
    ));
    assert!(plan.transports.iter().all(|transport| {
        transport.length == 16
            && plan.data_edges.contains(&LoopEdge {
                consumer: transport.consumer_loop,
                dependency: transport.producer_loop,
            })
    }));
}

#[test]
fn delayed_cross_loop_use_orders_chunks_without_an_immediate_transport() {
    let mut arena = TreeArena::new();
    let (input, delayed) = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let ten = builder.int(10);
        (input, builder.delay(input, ten))
    };
    let decorations = certify(&arena, &[delayed]);
    let plan = build_vector_plan(&decorations, 8).unwrap();
    let signal = |id: signals::SigId| {
        plan.plan()
            .signals
            .iter()
            .find(|signal| signal.signal_id == u64::from(id.as_u32()))
            .unwrap()
    };
    let Placement::Owned(producer) = signal(input).placement else {
        panic!("delayed carrier must own its state loop");
    };
    let Placement::Owned(consumer) = signal(delayed).placement else {
        panic!("delay read must own its reader loop");
    };
    assert!(plan.plan().data_edges.contains(&LoopEdge {
        consumer,
        dependency: producer,
    }));
    assert!(!plan.plan().transports.iter().any(|transport| {
        transport.producer_loop == producer && transport.consumer_loop == consumer
    }));
}

#[test]
fn sample_demand_keeps_a_delayed_constant_in_runtime_loops() {
    let mut arena = TreeArena::new();
    let (constant, delayed) = {
        let mut builder = SigBuilder::new(&mut arena);
        let constant = builder.real(2.0);
        (constant, builder.delay1(constant))
    };
    let decorations = certify(&arena, &[delayed]);
    let plan = build_vector_plan(&decorations, 8).unwrap();
    let placement = |signal: signals::SigId| {
        plan.plan()
            .signals
            .iter()
            .find(|record| record.signal_id == u64::from(signal.as_u32()))
            .unwrap()
            .placement
    };

    assert!(matches!(placement(constant), Placement::Owned(_)));
    assert!(matches!(placement(delayed), Placement::Owned(_)));
}

#[test]
fn sample_use_does_not_promote_a_pure_fixed_delay_amount() {
    let mut arena = TreeArena::new();
    let (amount, delayed) = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let amount = builder.int(2);
        (amount, builder.delay(input, amount))
    };
    let decorations = certify(&arena, &[delayed]);
    let plan = build_vector_plan(&decorations, 8).unwrap();
    let placement = |signal: signals::SigId| {
        plan.plan()
            .signals
            .iter()
            .find(|record| record.signal_id == u64::from(signal.as_u32()))
            .unwrap()
            .placement
    };

    assert_eq!(placement(amount), Placement::Control);
    assert!(matches!(placement(delayed), Placement::Owned(_)));
}

#[test]
fn plan_is_deterministic_and_independent_of_all_scheduling_strategies() {
    let (arena, y, z) = build_pv_signals(20);
    let decorations = certify(&arena, &[y, z]);
    let reference = build_vector_plan(&decorations, 16).unwrap().into_plan();
    assert_eq!(
        build_vector_plan(&decorations, 16).unwrap().into_plan(),
        reference
    );
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        schedule_vector_plan(&reference, strategy).unwrap();
        assert_eq!(
            build_vector_plan(&decorations, 16).unwrap().into_plan(),
            reference
        );
    }
}

#[test]
fn recursive_projections_of_one_group_share_one_serial_loop() {
    let mut arena = TreeArena::new();
    let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
    let (body0, body1) = {
        let mut builder = SigBuilder::new(&mut arena);
        let feedback0 = builder.proj(0, self_ref);
        let feedback1 = builder.proj(1, self_ref);
        (builder.delay1(feedback0), builder.delay1(feedback1))
    };
    let nil = arena.nil();
    let tail = arena.cons(body1, nil);
    let bodies = arena.cons(body0, tail);
    let group = tlib::de_bruijn_rec(&mut arena, bodies);
    let (out0, out1) = {
        let mut builder = SigBuilder::new(&mut arena);
        (builder.proj(0, group), builder.proj(1, group))
    };
    let decorations = certify(&arena, &[out0, out1]);
    let plan = build_vector_plan(&decorations, 8).unwrap().into_plan();
    let recursive = plan
        .loops
        .iter()
        .filter(|loop_record| matches!(loop_record.kind, LoopKind::Recursive(_)))
        .collect::<Vec<_>>();
    assert_eq!(recursive.len(), 1);
    let group_id = match recursive[0].kind {
        LoopKind::Recursive(group_id) => group_id,
        _ => unreachable!(),
    };
    assert!(
        decorations
            .certificate()
            .records
            .iter()
            .filter_map(|record| record.recursive_projection)
            .all(|projection| u64::from(projection.group) == group_id)
    );
    let writer_projection_indices = decorations
        .certificate()
        .records
        .iter()
        .filter(|record| {
            plan.signals.iter().any(|signal| {
                signal.signal_id == u64::from(record.signal_id)
                    && signal.placement == Placement::Owned(recursive[0].loop_id)
            })
        })
        .filter_map(|record| {
            record
                .recursive_projection
                .map(|projection| projection.index)
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(writer_projection_indices, BTreeSet::from([0, 1]));
    let delayed_projection_indices = decorations
        .certificate()
        .dependencies
        .iter()
        .filter(|dependency| matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0))
        .filter_map(|dependency| {
            decorations
                .certificate()
                .records
                .iter()
                .find(|record| record.signal_id == dependency.to)
                .and_then(|record| record.recursive_projection)
                .map(|projection| projection.index)
        })
        .collect::<BTreeSet<_>>();
    assert!(!delayed_projection_indices.is_empty());
    assert!(delayed_projection_indices.is_subset(&writer_projection_indices));
    assert!(
        plan.fused_serial_groups.is_empty(),
        "a recursion already colocated in one serial loop needs no fused envelope"
    );
}

#[test]
fn stateful_waveform_values_use_typed_numeric_transports() {
    let mut arena = TreeArena::new();
    let (left, right) = {
        let mut builder = SigBuilder::new(&mut arena);
        let v0 = builder.real(0.1);
        let v1 = builder.real(0.5);
        let waveform = builder.waveform(&[v0, v1]);
        let two = builder.real(2.0);
        let three = builder.real(3.0);
        (
            builder.binop(signals::BinOp::Mul, waveform, two),
            builder.binop(signals::BinOp::Mul, waveform, three),
        )
    };
    let decorations = certify(&arena, &[left, right]);
    let plan = build_vector_plan(&decorations, 8).unwrap().into_plan();
    let waveform_record = decorations
        .certificate()
        .records
        .iter()
        .find(|record| {
            record.direct_effects.iter().any(|effect| {
                matches!(
                    effect,
                    EffectAtom::ReadState(StateResource::Signal {
                        owner,
                        cell: StateCell::WaveformIndex,
                    }) if *owner == record.signal_id
                )
            })
        })
        .expect("the prepared waveform should own its read-position state");
    assert!(matches!(
        waveform_record.sig_type,
        CanonicalSigType::Simple {
            variability: sigtype::Variability::Samp,
            ..
        }
    ));
    let waveform_id = u64::from(waveform_record.signal_id);

    assert!(plan.transports.iter().any(|transport| {
        transport.signal_id == waveform_id
            && transport.element_type == ValueType::Real
            && transport.length == 8
    }));
}

#[test]
fn delayed_constant_sample_requirement_propagates_to_its_parent() {
    let mut arena = TreeArena::new();
    let parent = {
        let mut builder = SigBuilder::new(&mut arena);
        let one = builder.real(1.0);
        let delayed = builder.delay1(one);
        let two = builder.real(2.0);
        builder.binop(signals::BinOp::Add, delayed, two)
    };
    let decorations = certify(&arena, &[parent]);
    let plan = build_vector_plan(&decorations, 8).unwrap();
    let parent = plan
        .plan()
        .signals
        .iter()
        .find(|signal| signal.signal_id == u64::from(parent.as_u32()))
        .expect("prepared parent keeps its stable signal id");
    assert!(matches!(parent.placement, Placement::Owned(_)));
}

#[test]
fn effectful_inline_candidate_is_materialized_exactly_once() {
    let mut arena = TreeArena::new();
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        builder.output(0, input)
    };
    let decorations = certify(&arena, &[root]);
    let plan = build_vector_plan(&decorations, 8).unwrap().into_plan();
    let output = plan
        .signals
        .iter()
        .find(|signal| signal.signal_id == u64::from(decorations.certificate().roots[0]))
        .unwrap();
    assert!(!output.duplicable);
    let Placement::Owned(owner) = output.placement else {
        panic!("effectful output must be materialized");
    };
    assert!(plan.loops[owner as usize].roots.contains(&output.signal_id));
}

#[test]
fn previously_visited_inline_sample_root_is_promoted_without_panicking() {
    let mut arena = TreeArena::new();
    let (left, right) = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let half = builder.real(0.5);
        let shared = builder.binop(signals::BinOp::Mul, input, half);
        let one = builder.real(1.0);
        let two = builder.real(2.0);
        (
            builder.binop(signals::BinOp::Add, shared, one),
            builder.binop(signals::BinOp::Mul, shared, two),
        )
    };
    let decorations = certify(&arena, &[left, right]);
    let plan = build_vector_plan(&decorations, 8).unwrap();
    let right_id = u64::from(decorations.certificate().roots[1]);
    assert!(matches!(
        plan.plan()
            .signals
            .iter()
            .find(|signal| signal.signal_id == right_id)
            .unwrap()
            .placement,
        Placement::Owned(_)
    ));
}

#[test]
fn zero_chunk_size_is_rejected_before_plan_construction() {
    let (arena, y, z) = build_pv_signals(20);
    let decorations = certify(&arena, &[y, z]);
    assert_eq!(
        build_vector_plan(&decorations, 0),
        Err(VectorPlanBuildError::VecSizeZero)
    );
}
