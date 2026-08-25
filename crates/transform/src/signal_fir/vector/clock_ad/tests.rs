//! Tests for `vector::clock_ad` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use std::cell::RefCell;
use std::rc::Rc;

use propagate::{ClockDomain, ClockDomainKind, ClockDomainTable};
use signals::{BlockRevPolicy, SigBuilder};
use tlib::TreeArena;

use super::*;
use crate::clk_env::annotate;
use crate::signal_fir::decoration_verify::VerifiedDecorationCertificate;
use crate::signal_fir::decoration_verify::certify_decorations;
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::plan::build_vector_plan;
use crate::signal_fir::vector::verify::LoopKind;
use crate::signal_prepare::VerifiedPreparedSignals;
use crate::signal_prepare::prepare_signals_for_fir_verified;

fn clock_fixture(
    kind: ClockDomainKind,
) -> (
    VerifiedPreparedSignals,
    ClockDomainTable,
    VerifiedDecorationCertificate,
    VerifiedVectorPlan,
) {
    let mut arena = TreeArena::new();
    let wrapper_box = arena.nil();
    let (clock, input) = {
        let mut builder = SigBuilder::new(&mut arena);
        (builder.int(2), builder.input(0))
    };
    let mut domains = ClockDomainTable::new();
    let domain = domains.alloc(ClockDomain {
        parent: None,
        kind,
        clock,
        wrapper_box,
        inputs: vec![input],
    });
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let token = builder.clock_env_token(domain.as_u32());
        let guarded_clock = builder.clocked(token, clock);
        let guarded_value = builder.clocked(token, input);
        let hold = builder.perm_var(guarded_value);
        let wrapper = match kind {
            ClockDomainKind::OnDemand => builder.on_demand(&[guarded_clock, hold]),
            ClockDomainKind::Upsampling => builder.upsampling(&[guarded_clock, hold]),
            ClockDomainKind::Downsampling => builder.downsampling(&[guarded_clock, hold]),
        };
        builder.seq(wrapper, hold)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    (prepared, domains, decorations, vector_plan)
}

#[test]
fn production_clock_plan_builds_serial_island_and_refines_transports() {
    for (kind, guard) in [
        (ClockDomainKind::OnDemand, ClockGuard::CountedOnDemand),
        (ClockDomainKind::Upsampling, ClockGuard::CountedUpsampling),
        (ClockDomainKind::Downsampling, ClockGuard::DownsampleModulo),
    ] {
        let (prepared, domains, decorations, vector_plan) = clock_fixture(kind);
        let verified =
            build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();
        assert_eq!(verified.plan().clock_islands.len(), 1);
        let island = &verified.plan().clock_islands[0];
        assert_eq!(island.domain_id, 0);
        assert_eq!(island.kind, kind);
        assert_eq!(island.guard, guard);
        assert!(matches!(
            vector_plan.plan().loops[island.boundary_loop_id as usize].kind,
            LoopKind::Island(_)
        ));
        assert!(!island.clock_state_signal_ids.is_empty());
        assert!(verified.plan().transports.iter().all(|policy| {
            matches!(
                policy.mode,
                ClockTransportMode::OuterChunk
                    | ClockTransportMode::IslandScalar { domain_id: 0 }
                    | ClockTransportMode::HeldOutput { domain_id: 0 }
            )
        }));
        assert!(
            verified.plan().transports.iter().any(|policy| matches!(
                policy.mode,
                ClockTransportMode::HeldOutput { domain_id: 0 }
            ))
        );
    }
}

#[test]
fn nested_clock_islands_preserve_domain_parentage() {
    let mut arena = TreeArena::new();
    let wrapper_box = arena.nil();
    let (outer_clock, inner_clock, input) = {
        let mut builder = SigBuilder::new(&mut arena);
        (builder.int(1), builder.int(2), builder.input(0))
    };
    let mut domains = ClockDomainTable::new();
    let outer = domains.alloc(ClockDomain {
        parent: None,
        kind: ClockDomainKind::OnDemand,
        clock: outer_clock,
        wrapper_box,
        inputs: vec![input],
    });
    let inner = domains.alloc(ClockDomain {
        parent: Some(outer),
        kind: ClockDomainKind::Upsampling,
        clock: inner_clock,
        wrapper_box,
        inputs: vec![input],
    });
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let outer_token = builder.clock_env_token(outer.as_u32());
        let inner_token = builder.clock_env_token(inner.as_u32());
        let inner_clock_at_outer = builder.clocked(outer_token, inner_clock);
        let inner_guard = builder.clocked(inner_token, inner_clock_at_outer);
        let inner_value = builder.clocked(inner_token, input);
        let inner_hold = builder.perm_var(inner_value);
        let inner_wrapper = builder.upsampling(&[inner_guard, inner_hold]);
        let inner_result = builder.seq(inner_wrapper, inner_hold);

        let outer_guard = builder.clocked(outer_token, outer_clock);
        let outer_value = builder.clocked(outer_token, inner_result);
        let outer_hold = builder.perm_var(outer_value);
        let outer_wrapper = builder.on_demand(&[outer_guard, outer_hold]);
        builder.seq(outer_wrapper, outer_hold)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let verified =
        build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();

    assert_eq!(verified.plan().clock_islands.len(), 2);
    assert_eq!(verified.plan().clock_islands[0].parent_domain, None);
    assert_eq!(verified.plan().clock_islands[1].parent_domain, Some(0));
    assert_ne!(
        verified.plan().clock_islands[0].wrapper_signal_id,
        verified.plan().clock_islands[1].wrapper_signal_id
    );
}

#[test]
fn checker_rejects_clock_island_and_transport_mutations() {
    let (prepared, domains, decorations, vector_plan) = clock_fixture(ClockDomainKind::Upsampling);
    let verified =
        build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();

    let mut island = verified.plan().clone();
    island.clock_islands[0].guard = ClockGuard::DownsampleModulo;
    assert_eq!(
        verify_vector_clock_ad_plan(
            &prepared,
            &domains,
            &decorations,
            vector_plan.plan(),
            &island,
        ),
        Err(VectorClockAdError::IslandCoverageMismatch)
    );

    if let Some(index) = verified
        .plan()
        .transports
        .iter()
        .position(|policy| !matches!(policy.mode, ClockTransportMode::OuterChunk))
    {
        let mut transport = verified.plan().clone();
        transport.transports[index].mode = ClockTransportMode::OuterChunk;
        assert_eq!(
            verify_vector_clock_ad_plan(
                &prepared,
                &domains,
                &decorations,
                vector_plan.plan(),
                &transport,
            ),
            Err(VectorClockAdError::TransportCoverageMismatch)
        );
    }

    let mut mismatched_domains = ClockDomainTable::new();
    let placeholder = prepared.outputs()[0];
    mismatched_domains.alloc(ClockDomain {
        parent: None,
        kind: ClockDomainKind::Downsampling,
        clock: placeholder,
        wrapper_box: placeholder,
        inputs: Vec::new(),
    });
    assert!(matches!(
        verify_vector_clock_ad_plan(
            &prepared,
            &mismatched_domains,
            &decorations,
            vector_plan.plan(),
            verified.plan(),
        ),
        Err(VectorClockAdError::WrapperKindMismatch { .. })
    ));
}

#[test]
fn block_reverse_ad_is_forced_to_fixed_scalar_epochs() {
    let mut arena = TreeArena::new();
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let body = builder.input(0);
        let seed = builder.input(1);
        let cotangent = builder.real(1.0);
        let carrier =
            builder.block_reverse_ad(&[body], &[seed], &[cotangent], BlockRevPolicy::TapeFull);
        builder.proj(1, carrier)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let domains = ClockDomainTable::new();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    let verified =
        build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();
    assert_eq!(verified.plan().reverse_ad_fallbacks.len(), 1);
    let fallback = &verified.plan().reverse_ad_fallbacks[0];
    assert_eq!(fallback.kind, ReverseAdKind::BlockReverseAd);
    assert_eq!(fallback.epochs, vec![AdEpoch::Forward, AdEpoch::Reverse]);
    assert_eq!(
        fallback.diagnostic,
        ReverseAdDiagnostic::ScalarReverseWindowRequired
    );
    assert_eq!(fallback.diagnostic.code(), "FRS-VEC-RAD-SCALAR");
    assert!(
        fallback
            .diagnostic
            .message()
            .contains("forward/tape/reverse")
    );
}

#[test]
fn clock_step_preserves_state_and_hold_when_domain_does_not_fire() {
    let mut runtime = ClockRuntime {
        state: 7_i32,
        held_output: 11_i32,
        downsample_counter: 0,
    };
    let fires = simulate_clock_step(
        ClockGuard::BooleanOnDemand,
        ClockValue::Boolean(false),
        &mut runtime,
        |state, _| (*state + 1, *state + 10),
    )
    .unwrap();
    assert_eq!(fires, 0);
    assert_eq!(runtime.state, 7);
    assert_eq!(runtime.held_output, 11);
}

#[test]
fn counted_and_downsample_guards_match_scalar_reference_equations() {
    let mut counted = ClockRuntime {
        state: 0_i32,
        held_output: -1_i32,
        downsample_counter: 0,
    };
    assert_eq!(
        simulate_clock_step(
            ClockGuard::CountedUpsampling,
            ClockValue::Integer(3),
            &mut counted,
            |state, _| (*state + 1, *state + 1),
        )
        .unwrap(),
        3
    );
    assert_eq!((counted.state, counted.held_output), (3, 3));
    assert_eq!(
        simulate_clock_step(
            ClockGuard::CountedOnDemand,
            ClockValue::Integer(-2),
            &mut counted,
            |state, _| (*state + 1, *state + 1),
        )
        .unwrap(),
        0
    );
    assert_eq!((counted.state, counted.held_output), (3, 3));

    let mut downsample = ClockRuntime {
        state: 0_i32,
        held_output: -1_i32,
        downsample_counter: 0,
    };
    let mut observed = Vec::new();
    for _ in 0..7 {
        observed.push(
            simulate_clock_step(
                ClockGuard::DownsampleModulo,
                ClockValue::Integer(3),
                &mut downsample,
                |state, _| (*state + 1, *state + 1),
            )
            .unwrap(),
        );
    }
    assert_eq!(observed, vec![1, 0, 0, 1, 0, 0, 1]);
    assert_eq!((downsample.state, downsample.held_output), (3, 3));
}

#[test]
fn reverse_window_cannot_interleave_fixed_epochs() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let forward_trace = Rc::clone(&trace);
    let reverse_trace = Rc::clone(&trace);
    let (state, primal, adjoint) = execute_reverse_ad_window(
        2_i32,
        move |state| {
            forward_trace.borrow_mut().push("forward");
            (state + 3, state * 2, vec![state, state + 1])
        },
        move |state, tape: Vec<i32>| {
            reverse_trace.borrow_mut().push("reverse");
            (state + 5, tape.into_iter().sum::<i32>())
        },
    );
    assert_eq!(&*trace.borrow(), &["forward", "reverse"]);
    assert_eq!((state, primal, adjoint), (10, 4, 5));
}

/// Like `clock_fixture`, but the in-domain value reads a *top-rate stateful*
/// producer (`delay1(input)`) at fire time — the exact shape the shared-guard (plan provenance: §4.8)
/// admission guard `reject_unadopted_stateful_reads` must refuse (corpus
/// analogue: the `downsampling_02_domain_free_counter` fallback).
fn unadopted_stateful_read_fixture() -> (
    VerifiedPreparedSignals,
    ClockDomainTable,
    VerifiedDecorationCertificate,
    VerifiedVectorPlan,
) {
    let mut arena = TreeArena::new();
    let wrapper_box = arena.nil();
    let (clock, delayed) = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        (builder.int(2), builder.delay1(input))
    };
    let mut domains = ClockDomainTable::new();
    let domain = domains.alloc(ClockDomain {
        parent: None,
        kind: ClockDomainKind::Downsampling,
        clock,
        wrapper_box,
        inputs: vec![delayed],
    });
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let token = builder.clock_env_token(domain.as_u32());
        let guarded_clock = builder.clocked(token, clock);
        let guarded_value = builder.clocked(token, delayed);
        let hold = builder.perm_var(guarded_value);
        let wrapper = builder.downsampling(&[guarded_clock, hold]);
        builder.seq(wrapper, hold)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let decorations = certify_decorations(&prepared, &clocks).unwrap();
    let vector_plan = build_vector_plan(&decorations, 8).unwrap();
    (prepared, domains, decorations, vector_plan)
}

#[test]
fn unadopted_stateful_read_rejected_on_producer_path() {
    let (prepared, domains, decorations, vector_plan) = unadopted_stateful_read_fixture();
    let err = build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan)
        .expect_err("producer terminal verification must refuse the unadopted stateful read");
    assert!(
        matches!(err, VectorClockAdError::UnadoptedStatefulRead { .. }),
        "expected UnadoptedStatefulRead, got {err:?}"
    );
}

#[test]
fn unadopted_stateful_read_rejected_through_checker_entry_alone() {
    let (prepared, domains, decorations, vector_plan) = unadopted_stateful_read_fixture();
    let plan = vector_plan.plan();
    // Assemble the candidate plan from the derivations directly, bypassing the
    // producer's terminal verification, so only the standalone checker judges it.
    let clock_ad_plan = VectorClockAdPlan {
        schema_version: VECTOR_CLOCK_AD_PLAN_VERSION,
        vec_size: plan.vec_size,
        clock_islands: super::build::derive_clock_islands(&prepared, &domains, &decorations, plan)
            .unwrap(),
        transports: super::build::derive_transport_policies(&prepared, plan).unwrap(),
        forward_ad: ForwardAdPolicy::ExpandedSignalGraph,
        reverse_ad_fallbacks: super::build::derive_reverse_fallbacks(&prepared, &decorations, plan)
            .unwrap(),
    };
    let err = verify_vector_clock_ad_plan(&prepared, &domains, &decorations, plan, &clock_ad_plan)
        .expect_err("standalone checker must refuse the unadopted stateful read");
    assert!(
        matches!(err, VectorClockAdError::UnadoptedStatefulRead { .. }),
        "expected UnadoptedStatefulRead, got {err:?}"
    );
}
