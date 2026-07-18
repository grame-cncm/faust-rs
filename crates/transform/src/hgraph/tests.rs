//! Unit tests for the hierarchical dependency graph and schedule (P1.2).
//!
//! Fixtures mirror the shapes `propagate_clocked_wrapper` emits:
//! `Seq(OnDemand([Clocked(env, clock), PermVar(Clocked(env, body))...]), permvar_i)`.

use std::collections::HashMap;

use propagate::{ClockDomain, ClockDomainId, ClockDomainKind, ClockDomainTable};
use signals::{BinOp, SigBuilder, SigId};
use tlib::{NodeKind, TreeArena, vec_to_list};

use super::{
    Digraph, GraphKey, Hgraph, HgraphError, audit_control_variability, audit_hgraph, build_hgraph,
    needs_subgraph, orient_effect_conflicts, schedule,
};
use crate::clk_env::annotate;
use crate::schedule::SchedulingStrategy;

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

fn token(arena: &mut TreeArena, index: usize) -> SigId {
    SigBuilder::new(arena).clock_env_token(u32::try_from(index).expect("small test index"))
}

/// Builds the full propagation shape for a one-output ondemand block:
/// returns `(seq_output, wrapper, clocked_clock, held_output, body)`.
fn make_od_program(
    arena: &mut TreeArena,
    env_index: usize,
    clock: SigId,
    body: SigId,
) -> (SigId, SigId, SigId, SigId) {
    let env = token(arena, env_index);
    let mut b = SigBuilder::new(arena);
    let clocked_clock = b.clocked(env, clock);
    let clocked_body = b.clocked(env, body);
    let held = b.perm_var(clocked_body);
    let od = b.on_demand(&[clocked_clock, held]);
    let seq = b.seq(od, held);
    (seq, od, clocked_clock, held)
}

#[test]
fn ondemand_partitions_top_and_subgraph() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let domains = make_domains(&[None], &mut arena);

    // The body reads the outer input through the boundary glue:
    // Clocked(d0, Clocked(nil, TempVar(x))) * 2.
    let env = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let x = b.input(0);
    let two = b.int(2);
    let temp = b.temp_var(x);
    let outer_clocked = b.clocked(nil, temp);
    let inner_clocked = b.clocked(env, outer_clocked);
    let body_in_domain = b.binop(BinOp::Mul, inner_clocked, two);

    let (seq, od, _clocked_clock, held) = make_od_program(&mut arena, 0, clock, body_in_domain);

    let envs = annotate(&arena, &domains, &[seq]).expect("well-clocked fixture");
    let hgraph =
        build_hgraph(&arena, &domains, &envs, &[seq], &HashMap::new()).expect("hgraph builds");

    // Partition property holds.
    audit_hgraph(&hgraph).expect("every signal owned by exactly one graph");

    // The wrapper is a subgraph key.
    assert!(needs_subgraph(&arena, od));
    let top = hgraph.graph(GraphKey::Top).expect("top graph exists");
    let sub = hgraph
        .graph(GraphKey::Wrapper(od))
        .expect("wrapper subgraph exists");

    // Top owns the seq output, the wrapper node, and the raw clock (the
    // clock stays outside the block).
    assert!(top.contains(seq));
    assert!(top.contains(od));
    assert!(
        top.contains(clock),
        "raw clock must stay in the outer graph"
    );

    // The subgraph owns the held output and the domain-internal body.
    assert!(sub.contains(held));
    assert!(sub.contains(body_in_domain));
    assert!(!top.contains(body_in_domain));

    // `Seq(od, y)` depends only on `od`.
    let seq_edges = top.edges(seq);
    assert_eq!(seq_edges.len(), 1);
    assert_eq!(seq_edges[0].to, od);
    assert!(!seq_edges[0].delayed);
}

#[test]
fn schedule_orders_clock_before_wrapper_before_seq() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[None], &mut arena);

    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let x = b.input(0);
    let env = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let lifted = b.clocked(env, x);
    let (seq, od, _clocked_clock, _held) = make_od_program(&mut arena, 0, clock, lifted);

    let envs = annotate(&arena, &domains, &[seq]).expect("well-clocked fixture");
    let hgraph =
        build_hgraph(&arena, &domains, &envs, &[seq], &HashMap::new()).expect("hgraph builds");
    let sched =
        schedule(&hgraph, SchedulingStrategy::DepthFirst).expect("acyclic per-domain graphs");

    let top_order = sched.schedule(GraphKey::Top).expect("top schedule");
    let pos = |sig: SigId| {
        top_order
            .iter()
            .position(|&s| s == sig)
            .unwrap_or_else(|| panic!("signal {} missing from top schedule", sig.as_u32()))
    };
    assert!(pos(clock) < pos(od), "clock is a precondition of the block");
    assert!(
        pos(od) < pos(seq),
        "the block runs before its output is read"
    );

    // Determinism: rebuilding gives the identical schedule.
    let hgraph2 =
        build_hgraph(&arena, &domains, &envs, &[seq], &HashMap::new()).expect("hgraph rebuilds");
    let sched2 = schedule(&hgraph2, SchedulingStrategy::DepthFirst).expect("still acyclic");
    assert_eq!(sched.schedules.len(), sched2.schedules.len());
    for ((k1, s1), (k2, s2)) in sched.schedules.iter().zip(sched2.schedules.iter()) {
        assert_eq!(k1, k2);
        assert_eq!(s1, s2);
    }
}

#[test]
fn external_dependency_becomes_wrapper_precondition() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let domains = make_domains(&[None], &mut arena);

    // The block body reads an outer-domain computation `g = input0 * 0.5`
    // through the boundary glue: Clocked(d0, Clocked(nil, TempVar(g))).
    let env = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let x = b.input(0);
    let half = b.real(0.5);
    let g = b.binop(BinOp::Mul, x, half);
    let temp = b.temp_var(g);
    let outer_clocked = b.clocked(nil, temp);
    let boundary = b.clocked(env, outer_clocked);
    let (seq, od, _cc, _held) = make_od_program(&mut arena, 0, clock, boundary);

    let envs = annotate(&arena, &domains, &[seq]).expect("well-clocked fixture");
    let hgraph =
        build_hgraph(&arena, &domains, &envs, &[seq], &HashMap::new()).expect("hgraph builds");
    audit_hgraph(&hgraph).expect("partition holds");

    // `g` and its temp-var snapshot are owned by the top graph.
    let top = hgraph.graph(GraphKey::Top).expect("top graph");
    assert!(top.contains(g), "external computation lands at top");

    // The wrapper carries a precondition edge to the external chain.
    let od_edges = top.edges(od);
    assert!(
        od_edges.len() >= 2,
        "wrapper must depend on its clock and on the external input chain, got {od_edges:?}"
    );
}

#[test]
fn instantaneous_cycle_is_reported_as_causality_error() {
    // Build an artificial immediate cycle: a + (a * 1) where the graph edges
    // are forced circular by aliasing — simplest is a direct self-edge via a
    // hand-built graph. We go through the public API with a degenerate
    // fixture: BinOp whose operand is itself is impossible under hash-consing
    // from builders, so this test drives `schedule` directly.
    use super::{Digraph, Hgraph};

    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let a = b.input(0);
    let one = b.int(1);
    let prod = b.binop(BinOp::Mul, a, one);

    let mut graph = Digraph::default();
    graph.add_edge(prod, a, false);
    graph.add_edge(a, prod, false); // artificial back-edge: instantaneous cycle
    let mut hgraph = Hgraph::default();
    let slot = hgraph.graph_mut(GraphKey::Top);
    *slot = graph;

    let err = schedule(&hgraph, SchedulingStrategy::DepthFirst)
        .expect_err("cycle must be a causality error");
    assert!(
        matches!(err, HgraphError::InstantaneousCycle { .. }),
        "{err}"
    );
}

#[test]
fn delayed_edges_do_not_order_the_tick() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[], &mut arena);

    // y = x + y' : a same-tick read of `x` plus a delayed self-read. The
    // recursion is expressed through SYMREC as in the prepared forest.
    let var = arena.int(1000);
    let reference = tlib::sym_ref(&mut arena, var);
    let mut b = SigBuilder::new(&mut arena);
    let back = b.proj(0, reference);
    let delayed = b.delay1(back);
    let x = b.input(0);
    let def = b.binop(BinOp::Add, x, delayed);
    let body = tlib::vec_to_list(&mut arena, &[def]);
    let group = tlib::sym_rec(&mut arena, var, body);
    let out = SigBuilder::new(&mut arena).proj(0, group);

    let envs = annotate(&arena, &domains, &[out]).expect("well-clocked fixture");
    let hgraph =
        build_hgraph(&arena, &domains, &envs, &[out], &HashMap::new()).expect("hgraph builds");
    // The recursion is broken by the delayed edge: scheduling must succeed.
    schedule(&hgraph, SchedulingStrategy::DepthFirst)
        .expect("state-breaking recursion is acyclic on immediate edges");
}

#[test]
fn conflicting_unknown_effects_receive_a_strategy_independent_order() {
    let mut arena = TreeArena::new();
    let (call_a, call_b) = {
        let int_type = arena.int(0);
        let real_type = arena.int(1);
        let name_f32 = arena.symbol("p3_probe_f");
        let name_f64 = arena.symbol("p3_probe");
        let names = vec_to_list(&mut arena, &[name_f32, name_f64]);
        let signature = vec_to_list(&mut arena, &[int_type, names, real_type]);
        let include = arena.symbol("<p3_probe.h>");
        let library = arena.symbol("");
        let tag = arena.intern_tag("FFUN");
        let descriptor = arena.intern(NodeKind::Tag(tag), &[signature, include, library]);
        let mut b = SigBuilder::new(&mut arena);
        let input_a = b.input(0);
        let input_b = b.input(1);
        let args_a = vec_to_list(&mut arena, &[input_a]);
        let args_b = vec_to_list(&mut arena, &[input_b]);
        let mut b = SigBuilder::new(&mut arena);
        (b.ffun(descriptor, args_a), b.ffun(descriptor, args_b))
    };
    let prepared = crate::signal_prepare::prepare_signals_for_fir_verified(
        &arena,
        &[call_a, call_b],
        &ui::UiProgram::empty(),
    )
    .expect("foreign-call fixture prepares");
    let domains = ClockDomainTable::new();
    let envs = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let analysis = crate::signal_fir::vector_analysis::analyze_scalar_scheduling_effects(&prepared)
        .expect("scalar effect analysis succeeds");
    let mut hgraph = build_hgraph(
        prepared.arena(),
        &domains,
        &envs,
        prepared.outputs(),
        prepared.sig_types_map(),
    )
    .unwrap();
    let [left, right] = prepared.outputs() else {
        panic!("fixture must keep two roots");
    };
    let top = hgraph.graph(GraphKey::Top).unwrap();
    assert!(
        !top.edges(*left).iter().any(|edge| edge.to == *right)
            && !top.edges(*right).iter().any(|edge| edge.to == *left),
        "the fixture starts without a data-order relation"
    );

    orient_effect_conflicts(&mut hgraph, &analysis).unwrap();
    let baseline = schedule(&hgraph, SchedulingStrategy::DepthFirst).unwrap();
    let baseline = baseline.schedule(GraphKey::Top).unwrap();
    let left_before_right =
        baseline.iter().position(|sig| sig == left) < baseline.iter().position(|sig| sig == right);
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        let order = schedule(&hgraph, strategy).unwrap();
        let order = order.schedule(GraphKey::Top).unwrap();
        assert_eq!(
            order.iter().position(|sig| sig == left) < order.iter().position(|sig| sig == right),
            left_before_right,
            "{strategy:?} reordered conflicting foreign effects"
        );
    }
}

// ── Control graph (plan §4.6) ────────────────────────────────────────────────

fn sig_type(variability: sigtype::Variability) -> sigtype::SigType {
    sigtype::SigType::Simple(sigtype::SimpleType {
        nature: sigtype::Nature::Real,
        variability,
        computability: sigtype::Computability::Comp,
        vectorability: sigtype::Vectorability::Vect,
        boolean: sigtype::Boolean::Num,
        interval: interval::Interval::new_default(),
        res: sigtype::Res::default(),
    })
}

#[test]
fn control_owns_top_level_konst_signal_and_precondition_edge_is_implicit() {
    let mut arena = TreeArena::new();
    let domains = make_domains(&[], &mut arena);

    // s = input(0) + k, a flat (non-clocked) program. `k` is declared Konst
    // in sig_types even though nothing here enforces that at the type-system
    // level: build_hgraph's redirect only consumes the supplied map, which
    // lets this test exercise the mechanism directly.
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let k = b.real(0.5);
    let s = b.binop(BinOp::Add, x, k);

    let mut sig_types = HashMap::new();
    sig_types.insert(k, sig_type(sigtype::Variability::Konst));
    sig_types.insert(x, sig_type(sigtype::Variability::Samp));
    sig_types.insert(s, sig_type(sigtype::Variability::Samp));

    let envs = annotate(&arena, &domains, &[s]).expect("well-clocked fixture");
    let hgraph = build_hgraph(&arena, &domains, &envs, &[s], &sig_types).expect("hgraph builds");
    audit_hgraph(&hgraph).expect("partition property");
    audit_control_variability(&hgraph, &sig_types).expect("Control owns no Samp signal");

    let control = hgraph
        .graph(GraphKey::Control)
        .expect("Control was created");
    assert!(
        control.contains(k),
        "the Konst signal must be redirected to Control"
    );
    let top = hgraph.graph(GraphKey::Top).expect("top graph exists");
    assert!(
        !top.contains(k),
        "Control ownership must not also appear in Top"
    );
    assert!(top.contains(s), "the Samp signal stays in Top");
    assert!(top.contains(x));

    // The precondition on `k` is implicit: Top's edge list for `s` only
    // names `x`, never `k` (module docs, "Control graph").
    let s_edges = top.edges(s);
    assert_eq!(s_edges.len(), 1, "got {s_edges:?}");
    assert_eq!(s_edges[0].to, x);

    // Every strategy still schedules Top and Control independently and
    // validly (P1 integration, not just the literal DFS this module used to
    // hand-roll).
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        let sched = schedule(&hgraph, strategy)
            .unwrap_or_else(|e| panic!("{strategy:?} must schedule an acyclic hgraph: {e}"));
        let control_order = sched.schedule(GraphKey::Control).expect("control schedule");
        assert_eq!(control_order, &[k]);
        let top_order = sched.schedule(GraphKey::Top).expect("top schedule");
        let pos = |sig: SigId| top_order.iter().position(|&n| n == sig).unwrap();
        assert!(pos(x) < pos(s), "{strategy:?}: x must precede s");
    }
}

#[test]
fn missing_sig_types_entry_conservatively_stays_out_of_control() {
    // A signal absent from sig_types must never be silently redirected: the
    // conservative default is Samp (module docs on `Builder::effective_key`).
    let mut arena = TreeArena::new();
    let domains = make_domains(&[], &mut arena);
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let k = b.real(0.5);
    let s = b.binop(BinOp::Add, x, k);

    let envs = annotate(&arena, &domains, &[s]).expect("well-clocked fixture");
    let hgraph =
        build_hgraph(&arena, &domains, &envs, &[s], &HashMap::new()).expect("hgraph builds");
    assert!(
        hgraph.graph(GraphKey::Control).is_none(),
        "Control must not be created when sig_types has no non-Samp entry"
    );
    let top = hgraph.graph(GraphKey::Top).expect("top graph exists");
    assert!(top.contains(k), "k stays in Top without a sig_types entry");
}

#[test]
fn audit_control_variability_rejects_a_samp_signal_in_control() {
    // Hand-build a malformed Hgraph the same way
    // `instantaneous_cycle_is_reported_as_causality_error` does, bypassing
    // the builder's own invariant to prove the audit is a genuine, separate
    // trust boundary — not just "the builder never produces this."
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let a = b.input(0);

    let mut graph = Digraph::default();
    graph.add_node(a);
    let mut hgraph = Hgraph::default();
    let slot = hgraph.graph_mut(GraphKey::Control);
    *slot = graph;

    let mut sig_types = HashMap::new();
    sig_types.insert(a, sig_type(sigtype::Variability::Samp));

    let err = audit_control_variability(&hgraph, &sig_types)
        .expect_err("a Samp signal must never validate inside Control");
    assert!(
        matches!(err, HgraphError::ControlVariabilityViolated { sig } if sig == a),
        "{err}"
    );
}

// ── contains_wrapper (P3 unconditional-gate guard) ───────────────────────────

#[test]
fn contains_wrapper_is_false_for_a_flat_program() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let half = b.real(0.5);
    let s = b.binop(BinOp::Mul, x, half);

    assert!(!super::contains_wrapper(&arena, &[s]).expect("pure structural scan cannot fail here"));
}

#[test]
fn contains_wrapper_is_true_when_an_ondemand_node_is_reachable() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let clock = b.button(0);
    let x = b.input(0);
    let env = token(&mut arena, 0);
    let mut b = SigBuilder::new(&mut arena);
    let lifted = b.clocked(env, x);
    let (seq, _od, _cc, _held) = make_od_program(&mut arena, 0, clock, lifted);

    assert!(
        super::contains_wrapper(&arena, &[seq]).expect("pure structural scan cannot fail here")
    );
}
