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
