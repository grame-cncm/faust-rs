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

    let output = propagate_typed_with_ui(&mut arena, flat, &[], &mut ArityCache::new())
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
