//! Unit tests for the extracted propagation modules.
//!
//! Keeping tests in their own module lets `lib.rs` stay a small facade while
//! still exercising arity inference, route lowering, grouped UI collection,
//! memoization, and AD/recursion behavior across module boundaries.

use super::*;
use crate::engine::{debruijn_ref, liftn};
use boxes::BoxBuilder;
use signals::SigBuilder;

#[test]
fn liftn_and_aperture_memoize_shared_debruijn_subtrees() {
    let mut arena = TreeArena::new();
    let shared = {
        let rec_ref = debruijn_ref(&mut arena, 1);
        let mut b = SigBuilder::new(&mut arena);
        let proj = b.proj(0, rec_ref);
        b.add(proj, proj)
    };

    let mut memo = PropagateMemo::default();
    let lifted_once = liftn(&mut arena, shared, 1, &mut memo);
    let liftn_cache_len = memo.liftn.len();
    assert!(liftn_cache_len > 0, "liftn should populate its memo table");

    let lifted_twice = liftn(&mut arena, shared, 1, &mut memo);
    assert_eq!(
        lifted_once, lifted_twice,
        "memoized liftn must preserve structural output"
    );
    assert_eq!(
        memo.liftn.len(),
        liftn_cache_len,
        "repeating liftn on the same subtree should hit the memo table"
    );

    let aperture_once = de_bruijn_aperture_with_memo(&arena, lifted_once, &mut memo.aperture);
    let aperture_cache_len = memo.aperture.len();
    assert!(
        aperture_cache_len > 0,
        "aperture should populate its memo table"
    );

    let aperture_twice = de_bruijn_aperture_with_memo(&arena, lifted_once, &mut memo.aperture);
    assert_eq!(aperture_once, aperture_twice);
    assert_eq!(
        memo.aperture.len(),
        aperture_cache_len,
        "repeating aperture on the same subtree should hit the memo table"
    );
}

#[test]
fn try_build_flat_box_accepts_deep_shared_box_dag() {
    let mut arena = TreeArena::new();
    let shared = {
        let mut b = BoxBuilder::new(&mut arena);
        let left = b.wire();
        let right = b.wire();
        let pair = b.par(left, right);
        let add = b.add();
        b.seq(pair, add)
    };

    let mut root = shared;
    for _ in 0..14 {
        root = {
            let mut b = BoxBuilder::new(&mut arena);
            b.par(root, shared)
        };
    }

    let flat = try_build_flat_box(&arena, root).expect("shared DAG should validate once");
    let arity = box_arity_typed(&arena, flat, &mut ArityCache::new())
        .expect("validated shared DAG should infer arity");
    assert_eq!(arity.inputs, 30);
    assert_eq!(arity.outputs, 15);
}

#[test]
fn propagate_route_identity_preserves_all_inputs() {
    let mut arena = TreeArena::new();
    let route_spec = {
        let one = BoxBuilder::new(&mut arena).int(1);
        let one_b = BoxBuilder::new(&mut arena).int(1);
        let two = BoxBuilder::new(&mut arena).int(2);
        let two_b = BoxBuilder::new(&mut arena).int(2);
        let three = BoxBuilder::new(&mut arena).int(3);
        let three_b = BoxBuilder::new(&mut arena).int(3);
        let four = BoxBuilder::new(&mut arena).int(4);
        let four_b = BoxBuilder::new(&mut arena).int(4);
        let p1 = BoxBuilder::new(&mut arena).par(one, one_b);
        let p2 = BoxBuilder::new(&mut arena).par(two, two_b);
        let p3 = BoxBuilder::new(&mut arena).par(three, three_b);
        let p4 = BoxBuilder::new(&mut arena).par(four, four_b);
        let left = BoxBuilder::new(&mut arena).par(p1, p2);
        let right = BoxBuilder::new(&mut arena).par(p3, p4);
        BoxBuilder::new(&mut arena).par(left, right)
    };
    let route = {
        let ins = BoxBuilder::new(&mut arena).int(4);
        let outs = BoxBuilder::new(&mut arena).int(4);
        BoxBuilder::new(&mut arena).route(ins, outs, route_spec)
    };
    let inputs = {
        let w0 = BoxBuilder::new(&mut arena).wire();
        let w1 = BoxBuilder::new(&mut arena).wire();
        let w2 = BoxBuilder::new(&mut arena).wire();
        let w3 = BoxBuilder::new(&mut arena).wire();
        let left = BoxBuilder::new(&mut arena).par(w0, w1);
        let right = BoxBuilder::new(&mut arena).par(w2, w3);
        BoxBuilder::new(&mut arena).par(left, right)
    };
    let expr = BoxBuilder::new(&mut arena).seq(inputs, route);

    let flat = try_build_flat_box(&arena, expr).expect("flat route box");
    let provided_inputs = {
        let mut b = SigBuilder::new(&mut arena);
        vec![b.input(0), b.input(1), b.input(2), b.input(3)]
    };
    let outputs = propagate_typed(&mut arena, flat, &provided_inputs, &mut ArityCache::new())
        .expect("route propagate");

    assert_eq!(outputs.len(), 4);
    assert!(matches!(match_sig(&arena, outputs[0]), SigMatch::Input(0)));
    assert!(matches!(match_sig(&arena, outputs[1]), SigMatch::Input(1)));
    assert!(matches!(match_sig(&arena, outputs[2]), SigMatch::Input(2)));
    assert!(matches!(match_sig(&arena, outputs[3]), SigMatch::Input(3)));
}

#[test]
fn propagation_retains_all_box_origins_for_a_hash_consed_signal() {
    let mut arena = TreeArena::new();
    let constant = BoxBuilder::new(&mut arena).int(7);
    let duplicated = BoxBuilder::new(&mut arena).par(constant, constant);
    let flat = try_build_flat_box(&arena, duplicated).expect("flat constant pair");

    let output = propagate_typed_with_ui(
        &mut arena,
        flat,
        &[],
        &mut ArityCache::new(),
        &crate::PropagateUiOptions::default(),
    )
    .expect("constant pair should propagate");

    assert_eq!(output.signals.len(), 2);
    assert_eq!(output.signals[0], output.signals[1]);
    let origins = output.signal_origins.origins_for(output.signals[0]);
    assert!(origins.contains(&constant));
    assert!(origins.contains(&duplicated));
    assert_eq!(
        origins.iter().filter(|&&origin| origin == constant).count(),
        1,
        "origin sets must remain deduplicated under hash-consing"
    );
}

/// Reference implementation of the pre-pruning walk, kept only for this test.
///
/// It descends past already-attributed nodes, which is exactly the redundant
/// traversal `record_derived_forest` now prunes.
fn record_derived_forest_unpruned(
    origins: &mut SignalOrigins,
    arena: &TreeArena,
    signals: &[SigId],
    box_node: BoxId,
) {
    let mut stack = signals.to_vec();
    let mut visited = AHashSet::new();
    while let Some(signal) = stack.pop() {
        if !visited.insert(signal) {
            continue;
        }
        if origins.origins_for(signal).is_empty() {
            origins.record(signal, box_node);
        }
        if let Some(children) = arena.children(signal) {
            stack.extend(children.iter().copied());
        }
    }
    origins.record_outputs(signals, box_node);
}

#[test]
fn pruned_derived_forest_walk_records_exactly_what_the_full_walk_records() {
    // Nested shared structure: the inner box attributes `shared` and its
    // subtree, then two enclosing boxes re-reach it. That is the shape where
    // pruning skips traversal, so it is the shape that must stay equivalent.
    let mut arena = TreeArena::new();
    let (inner, mid, outer) = {
        let mut b = BoxBuilder::new(&mut arena);
        (b.int(1), b.int(2), b.int(3))
    };
    let (shared, mid_root, outer_root) = {
        let mut b = SigBuilder::new(&mut arena);
        let leaf = b.int(7);
        let shared = b.add(leaf, leaf);
        let mid_root = b.mul(shared, shared);
        let outer_root = b.add(mid_root, shared);
        (shared, mid_root, outer_root)
    };

    let steps = [
        (vec![shared], inner),
        (vec![mid_root], mid),
        (vec![outer_root], outer),
    ];

    let mut pruned = SignalOrigins::default();
    let mut reference = SignalOrigins::default();
    for (signals, box_node) in &steps {
        pruned.record_derived_forest(&arena, signals, *box_node);
        record_derived_forest_unpruned(&mut reference, &arena, signals, *box_node);
    }

    for signal in [shared, mid_root, outer_root] {
        assert_eq!(
            pruned.origins_for(signal),
            reference.origins_for(signal),
            "pruning must not change recorded origins for {signal:?}"
        );
    }
    assert_eq!(pruned.len(), reference.len());
}

#[test]
fn derived_forest_walk_attributes_every_reachable_node() {
    // The pruning is only sound because a call leaves no reachable node
    // unattributed; assert that closure property directly.
    let mut arena = TreeArena::new();
    let box_node = {
        let mut b = BoxBuilder::new(&mut arena);
        b.int(1)
    };
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let leaf = b.int(7);
        let inner = b.add(leaf, leaf);
        b.mul(inner, leaf)
    };

    let mut origins = SignalOrigins::default();
    origins.record_derived_forest(&arena, &[root], box_node);

    let mut stack = vec![root];
    let mut visited = AHashSet::new();
    while let Some(signal) = stack.pop() {
        if !visited.insert(signal) {
            continue;
        }
        assert!(
            !origins.origins_for(signal).is_empty(),
            "{signal:?} reachable from the walked root must carry an origin"
        );
        if let Some(children) = arena.children(signal) {
            stack.extend(children.iter().copied());
        }
    }
}

#[test]
fn disabled_origins_record_nothing() {
    let mut arena = TreeArena::new();
    let box_node = {
        let mut b = BoxBuilder::new(&mut arena);
        b.int(1)
    };
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let leaf = b.int(7);
        b.add(leaf, leaf)
    };

    let mut origins = SignalOrigins::disabled();
    origins.record_derived_forest(&arena, &[root], box_node);
    origins.record(root, box_node);
    origins.inherit_forest(&arena, &[root]);

    assert!(!origins.is_recording());
    assert!(origins.is_empty());
    assert!(origins.origins_for(root).is_empty());
}

#[test]
fn remap_is_independent_of_node_map_hash_order() {
    // A many-to-one clone mapping combined with MAX_ORIGINS_PER_SIGNAL makes
    // iteration order decide which candidates survive. Build one that
    // overflows the cap and check the result is a function of the inputs.
    let mut arena = TreeArena::new();
    let boxes = {
        let mut b = BoxBuilder::new(&mut arena);
        (0..12).map(|i| b.int(i)).collect::<Vec<_>>()
    };
    let (sources, destination) = {
        let mut b = SigBuilder::new(&mut arena);
        let sources = (0..12).map(|i| b.int(100 + i)).collect::<Vec<_>>();
        let destination = b.int(999);
        (sources, destination)
    };

    let mut table = SignalOrigins::default();
    for (signal, box_node) in sources.iter().zip(&boxes) {
        table.record(*signal, *box_node);
    }

    // A fresh HashMap is built on every round: its iteration order is what
    // varies, so agreement across rounds is the property under test.
    let mut rounds = (0..16).map(|_| {
        let node_map = sources
            .iter()
            .map(|source| (*source, destination))
            .collect::<std::collections::HashMap<_, _>>();
        table.remap(&node_map).origins_for(destination).to_vec()
    });

    let first = rounds.next().expect("at least one round");
    assert_eq!(
        first.len(),
        SignalOrigins::MAX_ORIGINS_PER_SIGNAL,
        "the fixture must actually overflow the cap, otherwise it proves nothing"
    );
    for round in rounds {
        assert_eq!(
            round, first,
            "remap must not depend on HashMap iteration order"
        );
    }
}

/// Propagates `flat` from the top level the way `api.rs` does, with the
/// profiler on and the result memo forced on or off, and returns the output
/// bus with the memo's `(probes, hits)`.
fn propagate_counting_memo(
    arena: &mut TreeArena,
    flat: FlatBoxId,
    memo_enabled: bool,
) -> (Vec<SigId>, (u64, u64)) {
    use crate::clock_domain::ClockDomainTable;
    use crate::context_id::{SlotEnv, UiPathContext};
    use crate::engine::{PropagateContext, PropagateMemo, propagate_in_slot_env};
    use crate::profile::PropagateProfile;
    use crate::result_memo::result_memo_is_safe_root;

    let ui = build_ui_program(arena, flat, &PropagateUiOptions::default());
    let mut cache = ArityCache::new();
    let mut slot_env = SlotEnv::new();
    let mut memo = PropagateMemo {
        profile: PropagateProfile::enabled_for_test(),
        ..Default::default()
    };
    let safe = result_memo_is_safe_root(arena, flat).expect("root analysis");
    memo.results.set_enabled(memo_enabled && safe);
    let mut clock_domains = ClockDomainTable::new();
    let mut signal_origins = SignalOrigins::default();
    let mut ctx = PropagateContext {
        cache: &mut cache,
        control_ids: &ui.control_ids,
        slot_env: &mut slot_env,
        memo: &mut memo,
        clock_domains: &mut clock_domains,
        clock_env: arena.nil(),
        clock_domain: None,
        suppress_fad: false,
        pending_fad_seeds: Vec::new(),
        ui_path: UiPathContext::new(),
        signal_origins: &mut signal_origins,
    };
    let outputs = propagate_in_slot_env(arena, flat, &[], &mut ctx).expect("propagation");
    (outputs, memo.profile.result_memo_counts())
}

/// Propagates `flat` from the top level with the result memo on, and returns
/// the output bus with the number of clock domains the traversal allocated.
fn propagate_counting_domains(arena: &mut TreeArena, flat: FlatBoxId) -> (Vec<SigId>, usize) {
    use crate::clock_domain::ClockDomainTable;
    use crate::context_id::{SlotEnv, UiPathContext};
    use crate::engine::{PropagateContext, PropagateMemo, propagate_in_slot_env};
    use crate::result_memo::result_memo_is_safe_root;

    let ui = build_ui_program(arena, flat, &PropagateUiOptions::default());
    let mut cache = ArityCache::new();
    let mut slot_env = SlotEnv::new();
    let mut memo = PropagateMemo::default();
    let safe = result_memo_is_safe_root(arena, flat).expect("root analysis");
    memo.results.set_enabled(safe);
    let mut clock_domains = ClockDomainTable::new();
    let mut signal_origins = SignalOrigins::default();
    let mut ctx = PropagateContext {
        cache: &mut cache,
        control_ids: &ui.control_ids,
        slot_env: &mut slot_env,
        memo: &mut memo,
        clock_domains: &mut clock_domains,
        clock_env: arena.nil(),
        clock_domain: None,
        suppress_fad: false,
        pending_fad_seeds: Vec::new(),
        ui_path: UiPathContext::new(),
        signal_origins: &mut signal_origins,
    };
    let outputs = propagate_in_slot_env(arena, flat, &[], &mut ctx).expect("propagation");
    (outputs, clock_domains.len())
}

/// A closed definition holding a clocked wrapper, referenced outside and
/// inside a `boxSymbolic` body (the `f ~ g` of an unapplied `f`), is one
/// block: the result memo keys a box that mentions no slot on the empty slot
/// environment, so the reference inside the body hits the propagation made
/// outside and replays its domain. Before 2026-09-21 the key carried the
/// body's environment, the box was propagated again, and the wrapper got a
/// second domain: `t = (1, 2.0) : ondemand(exp); process = t + ((\(r).(t +
/// 0.5 * r)) ~ _)` allocated two.
#[test]
fn closed_clocked_box_is_one_block_across_slot_environments() {
    let mut arena = TreeArena::new();
    let root = {
        let mut b = BoxBuilder::new(&mut arena);
        // `counter = +(1) ~ _`; the clock `(counter % 4) == 3` is a signal
        // (a constant clock of 1 makes the wrapper transparent, and of 0 a
        // zero), and `x = counter * 0.001` a stateful input, so the block is
        // not a constant the propagation folds away.
        let counter = {
            let wire = b.wire();
            let one = b.int(1);
            let pair = b.par(wire, one);
            let add = b.add();
            let inc = b.seq(pair, add);
            let wire_back = b.wire();
            b.rec(inc, wire_back)
        };
        let clock = {
            let four = b.int(4);
            let p = b.par(counter, four);
            let rem = b.rem();
            let modulo = b.seq(p, rem);
            let three = b.int(3);
            let q = b.par(modulo, three);
            let eq = b.eq();
            b.seq(q, eq)
        };
        let x = {
            let scale = b.real(0.001);
            let p = b.par(counter, scale);
            let m = b.mul();
            b.seq(p, m)
        };
        let payload = b.par(clock, x);
        let exp = b.exp();
        let od = b.ondemand(exp);
        let t = b.seq(payload, od);
        let slot = b.slot(1);
        let half = b.real(0.5);
        let scaled = {
            let p = b.par(slot, half);
            let m = b.mul();
            b.seq(p, m)
        };
        let body = {
            let p = b.par(t, scaled);
            let a = b.add();
            b.seq(p, a)
        };
        let sym = b.symbolic(slot, body);
        let wire = b.wire();
        let rec = b.rec(sym, wire);
        let pair = b.par(t, rec);
        let add = b.add();
        b.seq(pair, add)
    };
    let flat = try_build_flat_box(&arena, root).expect("flat root");

    let (outputs, domains) = propagate_counting_domains(&mut arena, flat);

    assert_eq!(outputs.len(), 1);
    assert_eq!(
        domains, 1,
        "one clocked definition read in two slot environments must be one block"
    );
}

/// The analysis behind that key: a `Slot` below a box that no `Symbolic`
/// below it binds makes it open; a `Symbolic` binding the only slot its body
/// mentions is closed, as is a box with no slot at all; and the memo caches
/// the answer per box.
#[test]
fn free_slots_stop_at_the_binder() {
    let mut arena = TreeArena::new();
    let (closed, open_body, own_binder, other_binder, wrapped) = {
        let mut b = BoxBuilder::new(&mut arena);
        let one = b.int(1);
        let x = b.real(2.0);
        let payload = b.par(one, x);
        let exp = b.exp();
        let od = b.ondemand(exp);
        let closed = b.seq(payload, od);
        let slot = b.slot(7);
        let add = b.add();
        let pair = b.par(closed, slot);
        let open_body = b.seq(pair, add);
        let own_binder = b.symbolic(slot, open_body);
        let other_slot = b.slot(8);
        let other_binder = b.symbolic(other_slot, open_body);
        let wrapped = b.ondemand(open_body);
        (closed, open_body, own_binder, other_binder, wrapped)
    };
    let mut memo = AHashMap::new();
    let flat = |b| try_build_flat_box(&arena, b).expect("flat box");
    let mut open = |b, what: &str| {
        !free_slots(&arena, flat(b), &mut memo)
            .expect(what)
            .is_empty()
    };
    assert!(!open(closed, "closed"));
    assert!(open(open_body, "open body"));
    assert!(
        !open(own_binder, "own binder"),
        "a symbolic binding the only slot its body mentions is closed"
    );
    assert!(
        open(other_binder, "other binder"),
        "a symbolic binding another slot leaves the body's free"
    );
    assert!(open(wrapped, "wrapped"));
    assert!(memo.contains_key(&flat(closed)) && memo.contains_key(&flat(open_body)));
}

/// Runs `f` on a worker thread with a stack sized for the deep chains below:
/// several hundred nested `seq` levels overflow a debug test thread.
fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("propagate-memo-test".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .expect("spawn big-stack worker")
        .join()
        .expect("big-stack worker must not panic")
}

/// A zero-input chain deep enough for one propagation of it to pass the
/// result memo's warm-up: `levels` times `(x, 1) : +` over an integer.
fn deep_constant_chain(arena: &mut TreeArena, levels: usize) -> BoxId {
    let mut b = BoxBuilder::new(arena);
    let mut chain = b.int(1);
    for _ in 0..levels {
        let one = b.int(1);
        let pair = b.par(chain, one);
        let add = b.add();
        chain = b.seq(pair, add);
    }
    chain
}

/// Regression for the 2026-09-16 `fad` compile-time blow-up: a root that
/// contains a forward-AD node is eligible for the exact result memo, so the
/// second reference to a shared subtree inside the `fad` body is a hit and
/// the outputs are the ones the memo-less traversal builds.
#[test]
fn fad_root_reuses_the_result_memo_for_shared_subtrees() {
    on_big_stack(fad_root_reuses_the_result_memo_for_shared_subtrees_body);
}

fn fad_root_reuses_the_result_memo_for_shared_subtrees_body() {
    let mut arena = TreeArena::new();
    let root = {
        let shared = deep_constant_chain(&mut arena, 400);
        let mut b = BoxBuilder::new(&mut arena);
        let seed = b.real(0.25);
        let pair = b.par(shared, shared);
        let add = b.add();
        let body = b.seq(pair, add);
        b.forward_ad(body, seed)
    };
    let flat = try_build_flat_box(&arena, root).expect("flat fad root");

    let (with_memo, (probes, hits)) = propagate_counting_memo(&mut arena, flat, true);
    let (without_memo, (_, hits_off)) = propagate_counting_memo(&mut arena, flat, false);

    assert_eq!(with_memo.len(), 2, "primal + one tangent lane");
    assert_eq!(with_memo, without_memo, "replay must be exact");
    assert!(probes > 0, "a fad root must probe the result memo");
    assert!(
        hits >= 1,
        "the second reference to the shared chain must hit the memo (probes={probes}, hits={hits})"
    );
    assert_eq!(hits_off, 0);
}

/// The one propagation-time side effect of a `ForwardAD` box is the seeds it
/// appends under `suppress_fad` (the `ExpandAfterRec` recursion mode). Two
/// distinct recursions whose left branch is the same `fad` box fed by the
/// same signals share that box's memo entry; the hit must replay the seeds,
/// or the second recursion expands with none and loses its tangent lane.
/// (Mutation checked 2026-09-16: without the replay the output bus has four
/// signals instead of five.)
#[test]
fn result_memo_hit_replays_pending_fad_seeds_across_recursions() {
    on_big_stack(result_memo_hit_replays_pending_fad_seeds_across_recursions_body);
}

fn result_memo_hit_replays_pending_fad_seeds_across_recursions_body() {
    let mut arena = TreeArena::new();
    let root = {
        // Propagated first, so the memo is past its warm-up when the first
        // recursion reaches the `fad` box and that call is recorded.
        let warm_up = deep_constant_chain(&mut arena, 400);
        let mut b = BoxBuilder::new(&mut arena);
        let gain = b.real(0.5);
        let wire = b.wire();
        let pair = b.par(wire, gain);
        let mul = b.mul();
        let body = b.seq(pair, mul);
        let seed = b.real(0.25);
        let fad = b.forward_ad(body, seed);
        // `fad ~ _` and `fad ~ (_ : _)`: different Rec boxes, same feedback
        // signal, hence the same memo key for the shared `fad` box.
        let rec_a = b.rec(fad, wire);
        let wire_again = b.wire();
        let wires = b.seq(wire, wire_again);
        let rec_b = b.rec(fad, wires);
        let recs = b.par(rec_a, rec_b);
        b.par(warm_up, recs)
    };
    let flat = try_build_flat_box(&arena, root).expect("flat recursive fad root");

    let (with_memo, (_, hits)) = propagate_counting_memo(&mut arena, flat, true);
    let (without_memo, _) = propagate_counting_memo(&mut arena, flat, false);

    assert_eq!(
        with_memo.len(),
        5,
        "the chain, then primal + tangent per recursion: {with_memo:?}"
    );
    assert_eq!(with_memo, without_memo, "replay must be exact");
    assert_eq!(with_memo[1], with_memo[3]);
    assert_eq!(with_memo[2], with_memo[4]);
    assert!(
        hits >= 1,
        "the second recursion's fad box must hit the memo"
    );
}

/// A zero-input chain whose DAG has `3 * levels` nodes and whose tree
/// unfolding has `2^levels` paths: `levels` times `(x, x) : +` over an
/// integer. Unlike a chain that composes `x` with itself, its signal graph
/// stays linear, so only the box walks see the exponential.
fn shared_constant_dag(arena: &mut TreeArena, levels: usize) -> BoxId {
    let mut b = BoxBuilder::new(arena);
    let mut x = b.int(1);
    for _ in 0..levels {
        let pair = b.par(x, x);
        let add = b.add();
        x = b.seq(pair, add);
    }
    x
}

/// Regression for the 2026-09-16 compile-time blow-up on recursions: the
/// walks a `Rec` node triggers over its branches (`rec_fad_mode`'s two
/// forward-AD reachability scans and `box_arity_wiring`) are memoized in the
/// `ArityCache`, so they cost the nodes of a shared box DAG, not its paths.
/// The recursion is `(_, x) : + ~ _` with `x` a 40-level shared DAG: an
/// unmemoized walk would take `2^40` steps; the budget is generous for a
/// debug build and the memoized cost is milliseconds.
#[test]
fn rec_arity_walks_are_linear_on_a_shared_dag() {
    use std::sync::mpsc;
    use std::time::Duration;

    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("rec-arity-walks-shared-dag".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let mut arena = TreeArena::new();
            let shared = shared_constant_dag(&mut arena, 40);
            let rec = {
                let mut b = BoxBuilder::new(&mut arena);
                let wire = b.wire();
                let pair = b.par(wire, shared);
                let add = b.add();
                let left = b.seq(pair, add);
                b.rec(left, wire)
            };
            let flat = try_build_flat_box(&arena, rec).expect("flat recursive box");
            let mut cache = ArityCache::new();
            let arity = box_arity_typed(&arena, flat, &mut cache).expect("rec arity");
            let outputs = propagate_typed(&mut arena, flat, &[], &mut cache).expect("propagation");
            let _ = sender.send((
                arity,
                outputs.len(),
                cache.wiring.len(),
                cache.forward_ad.len(),
            ));
        })
        .expect("spawn rec-arity worker");
    let (arity, outputs, wiring_entries, fad_entries) = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("arity and propagation of a 40-level shared DAG must finish within the budget");
    assert_eq!((arity.inputs, arity.outputs), (0, 1));
    assert_eq!(outputs, 1);
    assert!(
        wiring_entries >= 40 && fad_entries >= 40,
        "the walks must keep one verdict per distinct node, got wiring {wiring_entries}, fad {fad_entries}"
    );
}
