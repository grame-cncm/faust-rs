//! Unit tests for clock-environment inference (roadmap P1.1).
//!
//! One test per rule (`R_PROJ`, `R_CLOCKED`, `R_CD`, `R_SEQ`,
//! `R_COMPOSITE`), a fixpoint-convergence test (recursion spanning a
//! boundary), and the incomparable-domain diagnostic test. Fixtures are
//! hand-built signal graphs plus a hand-built [`ClockDomainTable`], mirroring
//! the shapes `propagate_clocked_wrapper` emits.

use propagate::{ClockDomain, ClockDomainId, ClockDomainKind, ClockDomainTable};
use signals::{BinOp, SigBuilder, SigId};
use tlib::{TreeArena, sym_rec, sym_ref, vec_to_list};

use super::{ClkEnvError, annotate, is_ancestor_clk_env, max_clk_env};

/// Builds a domain table with the requested parent links, plus placeholder
/// clock/box payloads (inference only reads `parent`).
fn make_domains(parents: &[Option<usize>], arena: &mut TreeArena) -> ClockDomainTable {
    let placeholder_clock = SigBuilder::new(arena).int(1);
    let placeholder_box = arena.nil();
    let mut table = ClockDomainTable::new();
    for &parent in parents {
        let parent = parent
            .map(|index| ClockDomainId::from_u32(u32::try_from(index).expect("small test index")));
        table.alloc(ClockDomain {
            parent,
            kind: ClockDomainKind::OnDemand,
            clock: placeholder_clock,
            wrapper_box: placeholder_box,
            inputs: Vec::new(),
        });
    }
    table
}

fn domain(index: usize) -> ClockDomainId {
    ClockDomainId::from_u32(u32::try_from(index).expect("small test index"))
}

fn token(arena: &mut TreeArena, index: usize) -> SigId {
    SigBuilder::new(arena).clock_env_token(u32::try_from(index).expect("small test index"))
}

/// Builds the wrapper payload shape emitted by `propagate_clocked_wrapper`:
/// `OnDemand([Clocked(env, clock), PermVar(Clocked(env, out))...])`.
fn make_ondemand(
    arena: &mut TreeArena,
    env_index: usize,
    clock: SigId,
    body_outputs: &[SigId],
) -> SigId {
    let env = token(arena, env_index);
    let mut b = SigBuilder::new(arena);
    let clocked_clock = b.clocked(env, clock);
    let mut payload = vec![clocked_clock];
    for &out in body_outputs {
        let clocked = b.clocked(env, out);
        payload.push(b.perm_var(clocked));
    }
    b.on_demand(&payload)
}

// ── Order and join primitives ────────────────────────────────────────────────

#[test]
fn ancestor_order_walks_parent_chain() {
    let mut arena = TreeArena::new();
    // d0 (top) ← d1 ← d2, plus sibling d3 under d0.
    let domains = make_domains(&[None, Some(0), Some(1), Some(0)], &mut arena);

    // nil is the bottom element.
    assert!(is_ancestor_clk_env(&domains, None, None));
    assert!(is_ancestor_clk_env(&domains, None, Some(domain(2))));
    assert!(!is_ancestor_clk_env(&domains, Some(domain(0)), None));

    // Reflexive + transitive along the chain.
    assert!(is_ancestor_clk_env(
        &domains,
        Some(domain(0)),
        Some(domain(0))
    ));
    assert!(is_ancestor_clk_env(
        &domains,
        Some(domain(0)),
        Some(domain(2))
    ));
    assert!(is_ancestor_clk_env(
        &domains,
        Some(domain(1)),
        Some(domain(2))
    ));
    assert!(!is_ancestor_clk_env(
        &domains,
        Some(domain(2)),
        Some(domain(1))
    ));

    // Siblings are unrelated.
    assert!(!is_ancestor_clk_env(
        &domains,
        Some(domain(1)),
        Some(domain(3))
    ));
    assert!(!is_ancestor_clk_env(
        &domains,
        Some(domain(3)),
        Some(domain(1))
    ));
}

#[test]
fn max_clk_env_joins_chains_and_rejects_siblings() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None, Some(0), Some(0)], &mut arena);
    let sig = SigBuilder::new(&mut arena).int(0);

    assert_eq!(
        max_clk_env(&domains, sig, None, Some(domain(1))).expect("comparable"),
        Some(domain(1))
    );
    assert_eq!(
        max_clk_env(&domains, sig, Some(domain(1)), Some(domain(0))).expect("comparable"),
        Some(domain(1))
    );
    let err = max_clk_env(&domains, sig, Some(domain(1)), Some(domain(2)))
        .expect_err("siblings are incomparable");
    assert!(matches!(err, ClkEnvError::Incomparable { .. }));
    // The diagnostic names both domains and the signal.
    let text = err.to_string();
    assert!(
        text.contains("domain #1") && text.contains("domain #2"),
        "{text}"
    );
}

// ── R_CLOCKED ────────────────────────────────────────────────────────────────

#[test]
fn r_clocked_lifts_and_checks_visibility() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None, Some(0)], &mut arena);

    // Lift: an audio-rate signal annotated into d1 lives in d1.
    let env1 = token(&mut arena, 1);
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let clocked = b.clocked(env1, x);
    let map = annotate(&arena, &domains, &[clocked]).expect("well-clocked");
    assert_eq!(map.env(clocked), Some(Some(domain(1))));
    assert_eq!(map.env(x), Some(None));

    // Violation: a d1 signal re-annotated to shallower d0 is rejected.
    let env0 = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let deep = b.clocked(env1, x);
    let shallow = b.clocked(env0, deep);
    let err =
        annotate(&arena, &domains, &[shallow]).expect_err("re-clocking may deepen, never shallow");
    assert!(matches!(err, ClkEnvError::ClockedViolation { .. }));
}

// ── R_CD ─────────────────────────────────────────────────────────────────────

#[test]
fn r_cd_wrapper_belongs_to_outer_domain() {
    let mut arena = TreeArena::new();
    // One top-level domain d0 (parent nil).
    let domains = make_domains(&[None], &mut arena);

    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let inner_body = b.input(0); // will be lifted into d0 by the Clocked wrap
    let od = make_ondemand(&mut arena, 0, clock, &[inner_body]);

    let map = annotate(&arena, &domains, &[od]).expect("well-formed wrapper");
    // The block as a whole belongs to the outer (nil) domain.
    assert_eq!(map.env(od), Some(None));
    // The clock stays outside.
    assert_eq!(map.env(clock), Some(None));
}

#[test]
fn r_cd_rejects_clock_computed_inside() {
    let mut arena = TreeArena::new();
    // d0 (top) ← d1 ← d2.
    let domains = make_domains(&[None, Some(0), Some(1)], &mut arena);

    // The d1 wrapper's clock is annotated into the *deeper* d2: it is not
    // computed in parent(d1) = d0 — must be rejected.
    let env1 = token(&mut arena, 1);
    let env2 = token(&mut arena, 2);
    let mut b = SigBuilder::new(&mut arena);
    let raw_clock = b.button(0);
    let deep_clock = b.clocked(env2, raw_clock);
    let x = b.input(0);
    let body = b.clocked(env1, x);
    let od = make_ondemand(&mut arena, 1, deep_clock, &[body]);

    let err = annotate(&arena, &domains, &[od]).expect_err("clock must be computed outside");
    assert!(
        matches!(err, ClkEnvError::ClockComputedInside { .. }),
        "{err}"
    );
}

#[test]
fn r_cd_rejects_output_child_outside_domain_but_accepts_literal_zero() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None], &mut arena);

    // Output child left at audio rate (no Clocked wrap): rejected.
    let env0 = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let clocked_clock = b.clocked(env0, clock);
    let stray = b.input(0);
    let od_bad = b.on_demand(&[clocked_clock, stray]);
    let err = annotate(&arena, &domains, &[od_bad])
        .expect_err("wrapper outputs must live exactly in the inner domain");
    assert!(
        matches!(err, ClkEnvError::WrapperChildOutsideDomain { .. }),
        "{err}"
    );

    // Literal 0 is the documented exception.
    let mut b = SigBuilder::new(&mut arena);
    let zero = b.int(0);
    let od_zero = b.on_demand(&[clocked_clock, zero]);
    annotate(&arena, &domains, &[od_zero]).expect("literal 0 output child is legal");
}

// ── R_SEQ ────────────────────────────────────────────────────────────────────

#[test]
fn r_seq_result_is_left_env_and_checks_domination() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None], &mut arena);

    // Well-formed `Seq(od, permvar)` as emitted by propagation.
    let env0 = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let body = b.input(0);
    let od = make_ondemand(&mut arena, 0, clock, &[body]);
    let mut b = SigBuilder::new(&mut arena);
    let clocked_out = b.clocked(env0, body);
    let held = b.perm_var(clocked_out);
    let seq = b.seq(od, held);

    let map = annotate(&arena, &domains, &[seq]).expect("well-formed seq");
    // Result = C⟦od⟧ = nil, even though the held value lives in d0.
    assert_eq!(map.env(seq), Some(None));
    assert_eq!(map.env(held), Some(Some(domain(0))));

    // Violation: left deeper than right.
    let mut b = SigBuilder::new(&mut arena);
    let deep = b.clocked(env0, body);
    let shallow = b.input(1);
    let bad_seq = b.seq(deep, shallow);
    let err = annotate(&arena, &domains, &[bad_seq]).expect_err("seq order violated");
    assert!(matches!(err, ClkEnvError::SeqViolation { .. }), "{err}");
}

// ── R_COMPOSITE + incomparable diagnostic ────────────────────────────────────

#[test]
fn r_composite_takes_deepest_comparable_child() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None, Some(0)], &mut arena);

    let env1 = token(&mut arena, 1);
    let mut b = SigBuilder::new(&mut arena);
    let shallow = b.input(0);
    let deep = b.clocked(env1, shallow);
    let sum = b.binop(BinOp::Add, shallow, deep);

    let map = annotate(&arena, &domains, &[sum]).expect("comparable children");
    assert_eq!(map.env(sum), Some(Some(domain(1))));
}

#[test]
fn waveform_elements_receive_total_clock_annotations() {
    let mut arena = TreeArena::new();
    let domains = ClockDomainTable::new();
    let (first, second, waveform) = {
        let mut b = SigBuilder::new(&mut arena);
        let first = b.int(1);
        let second = b.real(2.0);
        let waveform = b.waveform(&[first, second]);
        (first, second, waveform)
    };

    let map = annotate(&arena, &domains, &[waveform]).expect("waveform must be annotatable");
    assert_eq!(map.env(waveform), Some(None));
    assert_eq!(map.env(first), Some(None));
    assert_eq!(map.env(second), Some(None));
}

#[test]
fn sibling_domains_cannot_mix_without_annotation() {
    let mut arena = TreeArena::new();
    // d0 and d1 are both top-level: siblings.
    let domains = make_domains(&[None, None], &mut arena);

    let env0 = token(&mut arena, 0);
    let env1 = token(&mut arena, 1);
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let in_d0 = b.clocked(env0, x);
    let in_d1 = b.clocked(env1, x);
    let mix = b.binop(BinOp::Mul, in_d0, in_d1);

    let err = annotate(&arena, &domains, &[mix]).expect_err("sibling mix is a scoping error");
    let ClkEnvError::Incomparable { left, right, .. } = err else {
        panic!("expected Incomparable, got {err}");
    };
    assert_ne!(left, right);
}

// ── R_PROJ + fixpoint ────────────────────────────────────────────────────────

#[test]
fn r_proj_fixpoint_pulls_group_into_clocked_domain() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None], &mut arena);

    // W = symrec(v, [ delay1(proj(0, ref(v))) + clocked(d0, input0) ])
    // The definition reads a d0-clocked input → the group converges to d0.
    let var = arena.int(1000);
    let env0 = token(&mut arena, 0);
    let reference = sym_ref(&mut arena, var);
    let mut b = SigBuilder::new(&mut arena);
    let back_edge = b.proj(0, reference);
    let delayed = b.delay1(back_edge);
    let x = b.input(0);
    let clocked_x = b.clocked(env0, x);
    let def = b.binop(BinOp::Add, delayed, clocked_x);
    let body = vec_to_list(&mut arena, &[def]);
    let group = sym_rec(&mut arena, var, body);
    let out = SigBuilder::new(&mut arena).proj(0, group);

    let map = annotate(&arena, &domains, &[out]).expect("fixpoint must converge");
    assert_eq!(map.group_env(var), Some(Some(domain(0))));
    assert_eq!(map.env(out), Some(Some(domain(0))));
}

#[test]
fn fixpoint_least_solution_keeps_untouched_group_at_audio_rate() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None], &mut arena);

    // A plain accumulator touching no clocked signal stays at nil.
    let var = arena.int(1001);
    let reference = sym_ref(&mut arena, var);
    let mut b = SigBuilder::new(&mut arena);
    let back_edge = b.proj(0, reference);
    let delayed = b.delay1(back_edge);
    let x = b.input(0);
    let def = b.binop(BinOp::Add, delayed, x);
    let body = vec_to_list(&mut arena, &[def]);
    let group = sym_rec(&mut arena, var, body);
    let out = SigBuilder::new(&mut arena).proj(0, group);

    let map = annotate(&arena, &domains, &[out]).expect("fixpoint must converge");
    assert_eq!(map.group_env(var), Some(None));
    assert_eq!(map.env(out), Some(None));
}
