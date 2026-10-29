//! Tests for `vector::state` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use propagate::ClockDomainTable;
use signals::SigBuilder;
use tlib::TreeArena;

use super::build::{build_vector_state_plan_with_resources, delay_requirements};
use super::check::verify_vector_state_plan_with_resources;
use super::*;
use crate::clk_env::annotate;
use crate::signal_fir::decoration_verify::VerifiedDecorationCertificate;
use crate::signal_fir::decoration_verify::certify_decorations;
use crate::signal_fir::pv_slice::build_pv_signals;
use crate::signal_fir::vector::analysis::DepKind;
use crate::signal_fir::vector::clock_ad::build_vector_clock_ad_plan;
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::plan::{build_vector_plan, build_vector_plan_with_lockstep};
use crate::signal_fir::vector::verify::LoopKind;
use crate::signal_prepare::VerifiedPreparedSignals;
use crate::signal_prepare::prepare_signals_for_fir_verified;

fn certify(arena: &TreeArena, roots: &[signals::SigId]) -> VerifiedDecorationCertificate {
    let prepared = prepare_signals_for_fir_verified(arena, roots, &ui::UiProgram::empty()).unwrap();
    let domains = ClockDomainTable::new();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    certify_decorations(&prepared, &clocks).unwrap()
}

fn lockstep_delay_one_fixture() -> (
    VerifiedPreparedSignals,
    ClockDomainTable,
    VerifiedDecorationCertificate,
    VerifiedVectorPlan,
) {
    let mut arena = TreeArena::new();
    let mut roots = Vec::new();
    for channel in 0..2 {
        let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
        let body = {
            let mut builder = SigBuilder::new(&mut arena);
            let feedback = builder.proj(0, self_ref);
            let previous = builder.delay1(feedback);
            let input = builder.input(channel);
            let half = builder.real(0.5);
            let scaled = builder.binop(signals::BinOp::Mul, previous, half);
            builder.binop(signals::BinOp::Add, input, scaled)
        };
        let nil = arena.nil();
        let bodies = arena.cons(body, nil);
        let recursion = tlib::de_bruijn_rec(&mut arena, bodies);
        roots.push(SigBuilder::new(&mut arena).proj(0, recursion));
    }
    let prepared =
        prepare_signals_for_fir_verified(&arena, &roots, &ui::UiProgram::empty()).unwrap();
    let domains = ClockDomainTable::new();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let vector_plan = build_vector_plan_with_lockstep(&prepared, &decorations, 8).unwrap();
    (prepared, domains, decorations, vector_plan)
}

#[test]
fn production_delay_geometry_matches_cpp_threshold_and_rounding() {
    assert!(matches!(
        delay_storage(1, 5, 8, 16, None).unwrap(),
        VectorDelayStorage::Copy {
            history_length: 8,
            temporary_length: 16,
            ..
        }
    ));
    let (arena, y, z) = build_pv_signals(20);
    let decorations = certify(&arena, &[y, z]);
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let ring = build_vector_state_plan(&decorations, &vector_plan, 16).unwrap();
    let delayed = ring
        .plan()
        .delays
        .iter()
        .find(|delay| delay.max_delay == 20)
        .unwrap();
    assert_eq!(
        delayed.storage,
        VectorDelayStorage::Ring {
            buffer_name: format!("vstate_s{}", delayed.signal_id),
            index_name: format!("vstate_s{}_idx", delayed.signal_id),
            index_save_name: format!("vstate_s{}_idx_save", delayed.signal_id),
            capacity: 32,
            mask: 31,
        }
    );

    let boundary = build_vector_state_plan(&decorations, &vector_plan, 20).unwrap();
    assert!(matches!(
        boundary
            .plan()
            .delays
            .iter()
            .find(|delay| delay.max_delay == 20)
            .unwrap()
            .storage,
        VectorDelayStorage::Ring { .. }
    ));

    let copy = build_vector_state_plan(&decorations, &vector_plan, 32).unwrap();
    let delayed = copy
        .plan()
        .delays
        .iter()
        .find(|delay| delay.max_delay == 20)
        .unwrap();
    assert!(matches!(
        delayed.storage,
        VectorDelayStorage::Copy {
            history_length: 20,
            temporary_length: 28,
            ..
        }
    ));
}

#[test]
fn a_delayed_dependency_never_widens_history() {
    let mut arena = TreeArena::new();
    let (from, to) = {
        let mut builder = SigBuilder::new(&mut arena);
        (builder.input(0), builder.input(1))
    };
    let mut source = certify(&arena, &[from, to]).into_certificate();
    let from = from.as_u32();
    let to = to.as_u32();
    assert!(delay_requirements(&source).is_empty());

    // The shape X2b widened: a recursive projection whose scheduling edge
    // says "previous sample" while the delay facts give its selected body no
    // history. History comes from the delay facts alone, in the producer and
    // in the checker: widening it here read alias slots two samples late
    // (issue #24).
    source
        .records
        .iter_mut()
        .find(|record| record.signal_id == from)
        .expect("source record")
        .recursive_projection = Some(
        crate::signal_fir::decoration_verify::RecursiveProjectionFact {
            index: 0,
            group: from,
        },
    );
    source
        .dependencies
        .push(crate::signal_fir::decoration_verify::DependencyFact {
            from,
            to,
            kind: DepKind::Delayed { amount: 1 },
            edge_key: 0,
        });
    assert!(delay_requirements(&source).is_empty());
    assert!(independently_expected_delay_requirements(&source).is_empty());
}

#[test]
fn prefix_and_waveform_have_exact_transition_evidence() {
    let mut arena = TreeArena::new();
    let (prefix, waveform, prefix_signal, waveform_signal) = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let initial = builder.real(0.25);
        let prefix = builder.prefix(initial, input);
        let v0 = builder.real(0.1);
        let v1 = builder.real(0.2);
        let waveform = builder.waveform(&[v0, v1]);
        (prefix, waveform, prefix, waveform)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[prefix, waveform], &ui::UiProgram::empty())
            .unwrap();
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let state = build_vector_state_plan_with_resources(
        Some(&prepared),
        &decorations,
        &vector_plan,
        None,
        16,
    )
    .unwrap();

    assert_eq!(state.plan().prefixes.len(), 1);
    assert_eq!(
        state.plan().prefixes[0].signal_id,
        u64::from(prefix_signal.as_u32())
    );
    assert_eq!(
        state.plan().prefixes[0].initial,
        VectorStateInitialValue::RealBits(0.25_f64.to_bits())
    );
    assert_eq!(state.plan().waveforms.len(), 1);
    assert_eq!(
        state.plan().waveforms[0].signal_id,
        u64::from(waveform_signal.as_u32())
    );
    assert_eq!(state.plan().waveforms[0].length, 2);
    let mut mutated = state.plan().clone();
    mutated.waveforms[0].length = 3;
    assert!(
        verify_vector_state_plan_with_resources(
            Some(&prepared),
            &decorations,
            vector_plan.plan(),
            None,
            &mutated,
        )
        .is_err()
    );
}

#[test]
fn clock_delay_geometry_uses_one_domain_cursor_and_power_of_two_ring() {
    assert_eq!(
        delay_storage(9, 20, 8, 64, Some(3)).unwrap(),
        VectorDelayStorage::ClockRing {
            buffer_name: "vstate_s9".to_owned(),
            cursor_name: "vclock_d3_iota".to_owned(),
            domain_id: 3,
            capacity: 32,
            mask: 31,
        }
    );
}

#[test]
fn recursive_projections_share_one_simultaneous_serial_step() {
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
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let state = build_vector_state_plan(&decorations, &vector_plan, 16).unwrap();
    assert_eq!(state.plan().recursions.len(), 1);
    let recursion = &state.plan().recursions[0];
    assert_eq!(
        recursion
            .projections
            .iter()
            .map(|projection| projection.index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        vector_plan
            .plan()
            .loops
            .iter()
            .find(|record| record.loop_id == recursion.loop_id)
            .unwrap()
            .kind,
        LoopKind::Recursive(recursion.group)
    );
    let phases = state
        .plan()
        .loops
        .iter()
        .find(|phases| phases.loop_id == recursion.loop_id)
        .unwrap();
    assert!(phases.exec.contains(&VectorStateAction::RecursionStep {
        group: recursion.group
    }));
}

#[test]
fn identity_recursion_slots_do_not_create_state_transitions() {
    let mut arena = TreeArena::new();
    let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
    let (body0, body1, passthrough0, passthrough1) = {
        let mut builder = SigBuilder::new(&mut arena);
        let feedback0 = builder.proj(0, self_ref);
        let feedback1 = builder.proj(1, self_ref);
        (
            builder.delay1(feedback0),
            builder.delay1(feedback1),
            builder.input(0),
            builder.input(1),
        )
    };
    let nil = arena.nil();
    let tail3 = arena.cons(passthrough1, nil);
    let tail2 = arena.cons(passthrough0, tail3);
    let tail1 = arena.cons(body1, tail2);
    let bodies = arena.cons(body0, tail1);
    let group = tlib::de_bruijn_rec(&mut arena, bodies);
    let (out0, out1) = {
        let mut builder = SigBuilder::new(&mut arena);
        (builder.proj(0, group), builder.proj(1, group))
    };
    let decorations = certify(&arena, &[out0, out1]);
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let state = build_vector_state_plan(&decorations, &vector_plan, 16).unwrap();
    let recursion = &state.plan().recursions[0];
    assert_eq!(
        recursion
            .projections
            .iter()
            .map(|projection| projection.index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );

    let mut forged = state.into_plan();
    forged.recursions[0]
        .projections
        .push(RecursionProjectionTransition {
            index: 2,
            signal_ids: Vec::new(),
            value_signal_id: u64::from(passthrough0.as_u32()),
        });
    assert_eq!(
        verify_vector_state_plan(&decorations, vector_plan.plan(), &forged),
        Err(VectorStateError::RecursionCoverageMismatch)
    );
}

#[test]
fn independent_checker_rejects_geometry_and_phase_mutations() {
    let (arena, y, z) = build_pv_signals(20);
    let decorations = certify(&arena, &[y, z]);
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let verified = build_vector_state_plan(&decorations, &vector_plan, 16).unwrap();

    let mut geometry = verified.plan().clone();
    let transition = geometry
        .delays
        .iter_mut()
        .find(|delay| delay.max_delay == 20)
        .unwrap();
    if let VectorDelayStorage::Ring { capacity, .. } = &mut transition.storage {
        *capacity = 64;
    }
    assert_eq!(
        verify_vector_state_plan(&decorations, vector_plan.plan(), &geometry),
        Err(VectorStateError::DelayCoverageMismatch)
    );

    let mut phase = verified.into_plan();
    phase.loops[0].exec.clear();
    assert!(matches!(
        verify_vector_state_plan(&decorations, vector_plan.plan(), &phase),
        Err(VectorStateError::LoopPhaseMismatch { .. })
    ));
}

#[test]
fn lockstep_delay_one_register_mapping_is_checked_independently() {
    let (prepared, clocks, decorations, vector_plan) = lockstep_delay_one_fixture();
    let clock_plan =
        build_vector_clock_ad_plan(&prepared, &clocks, &decorations, &vector_plan).unwrap();
    let verified =
        build_vector_state_plan_with_clock(&prepared, &decorations, &vector_plan, &clock_plan, 16)
            .unwrap();
    let [bundle] = verified.plan().lockstep_register_bundles.as_slice() else {
        panic!("one register-carried lockstep bundle");
    };
    assert_eq!(bundle.lanes.len(), 2);
    assert!(
        verified
            .plan()
            .delays
            .iter()
            .all(|delay| matches!(delay.storage, VectorDelayStorage::Register { .. }))
    );

    let mut missing_store = verified.plan().clone();
    missing_store
        .loops
        .iter_mut()
        .find(|phases| phases.loop_id == bundle.lanes[0].loop_id)
        .unwrap()
        .post
        .clear();
    assert!(matches!(
        verify_vector_state_plan_with_clock(
            &prepared,
            &decorations,
            vector_plan.plan(),
            &clock_plan,
            &missing_store,
        ),
        Err(VectorStateError::LoopPhaseMismatch { .. })
    ));

    let mut crossed = verified.into_plan();
    crossed.lockstep_register_bundles[0].lanes.swap(0, 1);
    assert_eq!(
        verify_vector_state_plan_with_clock(
            &prepared,
            &decorations,
            vector_plan.plan(),
            &clock_plan,
            &crossed,
        ),
        Err(VectorStateError::DelayCoverageMismatch)
    );
}

#[test]
fn copy_and_ring_refine_newest_first_history_exhaustively() {
    const CHUNKINGS: [[usize; 4]; 3] = [[4, 4, 0, 0], [1, 3, 2, 2], [3, 4, 1, 0]];
    for max_delay in 1_u64..=5 {
        let vec_size = 4_u64;
        let copy_storage = delay_storage(1, max_delay, vec_size, max_delay + 1, None).unwrap();
        let ring_storage = delay_storage(1, max_delay, vec_size, 0, None).unwrap();
        for input_code in 0..3_usize.pow(8) {
            let values = ternary_values(input_code, 8);
            let delays = (0..=max_delay as usize).collect::<Vec<_>>();
            for chunking in CHUNKINGS {
                let mut abstract_history = vec![0_i32; max_delay as usize];
                let mut copy = CopyDelayState::new(&copy_storage, max_delay as usize, 0).unwrap();
                let mut ring =
                    RingDelayState::new(&ring_storage, max_delay as usize, vec_size as usize, 0)
                        .unwrap();
                let mut start = 0;
                for count in chunking.into_iter().filter(|count| *count > 0) {
                    let chunk = &values[start..start + count];
                    start += count;
                    let expected = abstract_chunk(&mut abstract_history, chunk, &delays);
                    assert_eq!(copy.process_chunk(chunk, &delays).unwrap(), expected);
                    assert_eq!(ring.process_chunk(chunk, &delays).unwrap(), expected);
                    assert_eq!(copy.abstract_history(), abstract_history);
                    assert_eq!(ring.abstract_history(), abstract_history);
                }
                assert_eq!(start, values.len());
            }
        }
    }
}

#[test]
fn recursion_commit_is_simultaneous_and_checks_arity() {
    let mut state = vec![1, 2];
    let next = vec![state[1] + 1, state[0] + 10];
    assert_eq!(commit_recursion_step(&mut state, next).unwrap(), vec![1, 2]);
    assert_eq!(state, vec![3, 11]);
    assert_eq!(
        commit_recursion_step(&mut state, vec![4]),
        Err(VectorStateError::RecursionArityMismatch { state: 2, next: 1 })
    );
}

fn ternary_values(mut code: usize, count: usize) -> Vec<i32> {
    (0..count)
        .map(|_| {
            let value = (code % 3) as i32 - 1;
            code /= 3;
            value
        })
        .collect()
}

fn abstract_chunk(history: &mut Vec<i32>, input: &[i32], delays: &[usize]) -> Vec<Vec<i32>> {
    input
        .iter()
        .map(|value| {
            let output = delays
                .iter()
                .map(|delay| *delay_read(history, value, *delay).unwrap())
                .collect();
            history_step(history, *value);
            output
        })
        .collect()
}
