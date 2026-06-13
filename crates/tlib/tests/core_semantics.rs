//! Integration tests for `core_semantics`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use tlib::{
    ListValidationError, NodeKind, PropertyStore, TreeArena, list_to_vec, validate_faust_list,
    vec_to_list,
};

#[test]
fn interning_reuses_structurally_identical_nodes() {
    let mut arena = TreeArena::new();
    let a = arena.symbol("x");
    let b = arena.symbol("x");
    assert_eq!(a, b);

    let seq_tag = arena.intern_tag("seq");
    let nil = arena.nil();
    let seq1 = arena.intern(NodeKind::Tag(seq_tag), &[a, nil]);
    let seq2 = arena.intern(NodeKind::Tag(seq_tag), &[b, nil]);
    assert_eq!(seq1, seq2);
}

/// Structural non-regression for the arity-`>= 3` intern path.
///
/// This path stores children as a single `Arc<[TreeId]>` shared between the
/// hash-cons key and the stored node. The test guards that hash-consing still
/// deduplicates structurally identical high-arity nodes, that children round
/// trip unchanged, and that differing children still produce distinct nodes.
#[test]
fn interning_high_arity_nodes_shares_and_dedups_children() {
    let mut arena = TreeArena::new();
    let seq_tag = arena.intern_tag("seq");

    // Build two structurally identical arity-4 nodes from independently
    // interned (but equal) children.
    let first: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|s| arena.symbol(*s))
        .collect();
    let second: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|s| arena.symbol(*s))
        .collect();
    assert_eq!(first, second, "equal symbols must intern to equal ids");

    let node1 = arena.intern(NodeKind::Tag(seq_tag), &first);
    let node2 = arena.intern(NodeKind::Tag(seq_tag), &second);
    assert_eq!(
        node1, node2,
        "structurally identical arity-4 nodes must share an id"
    );

    // Children must round trip exactly through the shared storage.
    assert_eq!(arena.children(node1), Some(first.as_slice()));

    // A differing child list must produce a distinct node.
    let e = arena.symbol("e");
    let other = arena.intern(NodeKind::Tag(seq_tag), &[first[0], first[1], first[2], e]);
    assert_ne!(node1, other, "differing children must not be deduplicated");
}

#[test]
fn list_operations_preserve_head_and_tail_order() {
    let mut arena = TreeArena::new();
    let one = arena.int(1);
    let two = arena.int(2);
    let nil = arena.nil();

    let tail = arena.cons(two, nil);
    let list = arena.cons(one, tail);

    assert!(arena.is_list(list));
    assert!(!arena.is_nil(list));
    assert_eq!(arena.hd(list), Some(one));

    let next = arena.tl(list).expect("cons tail should exist");
    assert_eq!(arena.hd(next), Some(two));
    assert_eq!(arena.tl(next), Some(nil));
    assert!(arena.is_nil(nil));
}

#[test]
fn vec_to_list_and_back_preserves_order() {
    let mut arena = TreeArena::new();
    let values = [arena.int(1), arena.int(2), arena.int(3)];
    let list = vec_to_list(&mut arena, &values);

    assert_eq!(list_to_vec(&arena, list), Some(values.to_vec()));
}

#[test]
fn list_to_vec_rejects_malformed_cons_chain() {
    let mut arena = TreeArena::new();
    let head = arena.int(1);
    let tail = arena.int(2);
    let malformed = arena.cons(head, tail);

    assert_eq!(list_to_vec(&arena, malformed), None);
}

#[test]
fn validate_faust_list_accepts_nil_and_cons_lists() {
    let mut arena = TreeArena::new();
    let empty = arena.nil();
    let values = [arena.int(1), arena.int(2), arena.int(3)];
    let list = vec_to_list(&mut arena, &values);

    assert_eq!(validate_faust_list(&arena, empty), Ok(0));
    assert_eq!(validate_faust_list(&arena, list), Ok(3));
}

#[test]
fn validate_faust_list_rejects_non_list_tail() {
    let mut arena = TreeArena::new();
    let head = arena.int(1);
    let tail = arena.int(2);
    let malformed = arena.cons(head, tail);

    assert_eq!(
        validate_faust_list(&arena, malformed),
        Err(ListValidationError::MalformedList { node: tail })
    );
}

#[test]
fn property_store_is_node_keyed() {
    let mut arena = TreeArena::new();
    let node = arena.symbol("gain");
    let other = arena.symbol("freq");

    let mut props = PropertyStore::<i32>::new();
    assert!(props.is_empty());

    assert_eq!(props.set(node, "order", 3), None);
    assert_eq!(props.get(node, "order"), Some(&3));
    assert_eq!(props.get(other, "order"), None);

    if let Some(value) = props.get_mut(node, "order") {
        *value += 1;
    }
    assert_eq!(props.get(node, "order"), Some(&4));

    assert_eq!(props.remove(node, "order"), Some(4));
    assert!(props.get(node, "order").is_none());
}

#[test]
fn property_store_interned_key_api_matches_string_api() {
    let mut arena = TreeArena::new();
    let node = arena.symbol("gain");

    let mut props = PropertyStore::<i32>::new();
    let order = props.key("order");

    assert_eq!(props.set_with_key(node, order, 7), None);
    assert_eq!(props.get_with_key(node, order), Some(&7));
    assert_eq!(props.get(node, "order"), Some(&7));

    if let Some(value) = props.get_mut_with_key(node, order) {
        *value += 2;
    }
    assert_eq!(props.get(node, "order"), Some(&9));
    assert_eq!(props.remove_with_key(node, order), Some(9));
}

#[test]
fn property_store_clear_preserves_key_reuse() {
    let mut arena = TreeArena::new();
    let node = arena.symbol("gain");

    let mut props = PropertyStore::<i32>::new();
    assert_eq!(props.set(node, "order", 5), None);
    assert_eq!(props.get(node, "order"), Some(&5));

    props.clear();
    assert!(props.is_empty());
    assert_eq!(props.get(node, "order"), None);

    assert_eq!(props.set(node, "order", 8), None);
    assert_eq!(props.get(node, "order"), Some(&8));
}

#[test]
fn tree_arena_with_capacities_preserves_interning_semantics() {
    let mut arena = TreeArena::with_capacities(64, 64, 16, 64, 16);
    let x1 = arena.symbol("x");
    let x2 = arena.symbol("x");
    assert_eq!(x1, x2);

    let pair_tag = arena.intern_tag("pair");
    let nil = arena.nil();
    let pair1 = arena.intern(NodeKind::Tag(pair_tag), &[x1, nil]);
    let pair2 = arena.intern(NodeKind::Tag(pair_tag), &[x2, nil]);
    assert_eq!(pair1, pair2);
}

#[test]
fn tree_arena_reserve_preserves_interning_semantics() {
    let mut arena = TreeArena::new();
    arena.reserve(128, 128, 32, 128, 32);
    let a = arena.int(1);
    let b = arena.int(1);
    assert_eq!(a, b);
}

#[test]
fn property_store_reserve_slots_does_not_set_values() {
    let mut arena = TreeArena::new();
    let a = arena.symbol("a");
    let b = arena.symbol("b");

    let mut props = PropertyStore::<i32>::with_key_capacity(1);
    let key = props.key("k");
    props.reserve_slots(key, 128);
    assert_eq!(props.get_with_key(a, key), None);
    assert_eq!(props.get_with_key(b, key), None);
    assert!(props.is_empty());
}

#[test]
fn clone_subtree_from_reinterns_nodes_into_destination_arena() {
    let mut src = TreeArena::new();
    let sym = src.symbol("x");
    let one = src.int(1);
    let pair_tag = src.intern_tag("pair");
    let pair = src.intern(NodeKind::Tag(pair_tag), &[sym, one]);
    let tail = src.cons(pair, src.nil());
    let list = src.cons(pair, tail);

    let mut dst = TreeArena::new();
    let cloned = dst.clone_subtree_from(&src, list);

    assert!(dst.is_list(cloned));
    let head = dst.hd(cloned).expect("cloned list head");
    let tail = dst.tl(cloned).expect("cloned list tail");
    let second = dst.hd(tail).expect("cloned list second");
    assert_eq!(
        head, second,
        "repeated subtrees should remain shared after inter-arena cloning"
    );

    let NodeKind::Tag(dst_tag) = dst.node(head).expect("pair node").kind.clone() else {
        panic!("expected cloned tag node");
    };
    assert_eq!(dst.tag_name(dst_tag), Some("pair"));
}

#[test]
fn clone_forest_from_preserves_cross_root_sharing() {
    let mut src = TreeArena::new();
    let sym = src.symbol("x");
    let one = src.int(1);
    let pair_tag = src.intern_tag("pair");
    let shared = src.intern(NodeKind::Tag(pair_tag), &[sym, one]);
    let left = src.cons(shared, src.nil());
    let right = src.cons(shared, src.nil());

    let mut dst = TreeArena::new();
    let cloned = dst.clone_forest_from(&src, &[left, right]);

    assert_eq!(cloned.len(), 2);
    let left_head = dst.hd(cloned[0]).expect("left head");
    let right_head = dst.hd(cloned[1]).expect("right head");
    assert_eq!(
        left_head, right_head,
        "shared source subtrees should remain shared across cloned roots"
    );
}
