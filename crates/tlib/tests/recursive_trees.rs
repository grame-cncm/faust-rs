//! Integration tests for recursive-tree conversion parity helpers.

use tlib::{
    NodeKind, RecursionError, SymbolicRecursionValidationError, TreeArena,
    check_de_bruijn_coherence, de_bruijn_aperture, de_bruijn_rec, de_bruijn_ref, de_bruijn_to_sym,
    is_de_bruijn_closed, lift_de_bruijn, lift_de_bruijn_n, match_de_bruijn_rec,
    match_de_bruijn_ref, match_sym_rec, match_sym_ref, sym_rec, sym_ref,
    validate_closed_de_bruijn_tree, validate_symbolic_recursion_tree,
};

#[test]
fn de_bruijn_to_sym_converts_simple_rec_group() {
    let mut arena = TreeArena::new();
    let x = arena.symbol("x");
    let r1 = de_bruijn_ref(&mut arena, 1);
    let body = pair(&mut arena, r1, x);
    let root = de_bruijn_rec(&mut arena, body);

    let converted = de_bruijn_to_sym(&mut arena, root).expect("closed de Bruijn tree should map");
    let (var, sym_body) = match_sym_rec(&arena, converted).expect("expected SYMREC(var, body)");
    let (left, right) = pair_children(&arena, sym_body).expect("pair shape should be preserved");

    assert_eq!(right, x);
    assert_eq!(
        match_sym_ref(&arena, left),
        Some(var),
        "DEBRUIJNREF(1) should become SYMREF(var)"
    );
}

#[test]
fn de_bruijn_to_sym_converts_nested_scopes() {
    let mut arena = TreeArena::new();
    let r1 = de_bruijn_ref(&mut arena, 1);
    let r2 = de_bruijn_ref(&mut arena, 2);
    let inner = pair(&mut arena, r1, r2);
    let nested = de_bruijn_rec(&mut arena, inner);
    let root = de_bruijn_rec(&mut arena, nested);

    let converted = de_bruijn_to_sym(&mut arena, root).expect("nested closed de Bruijn tree");

    let (outer_var, outer_body) =
        match_sym_rec(&arena, converted).expect("outer group should be symbolic");
    let (inner_var, inner_body) =
        match_sym_rec(&arena, outer_body).expect("inner group should be symbolic");
    let (lhs, rhs) = pair_children(&arena, inner_body).expect("inner pair should stay stable");

    assert_eq!(match_sym_ref(&arena, lhs), Some(inner_var));
    assert_eq!(match_sym_ref(&arena, rhs), Some(outer_var));
}

#[test]
fn de_bruijn_to_sym_rejects_open_tree() {
    let mut arena = TreeArena::new();
    let root = de_bruijn_ref(&mut arena, 1);

    let err = de_bruijn_to_sym(&mut arena, root).expect_err("open de Bruijn tree should fail");
    assert_eq!(err, RecursionError::OpenDeBruijnTree { aperture: 1 });
}

#[test]
fn aperture_matches_de_bruijn_rules() {
    let mut arena = TreeArena::new();
    let r1 = de_bruijn_ref(&mut arena, 1);
    let nested = de_bruijn_rec(&mut arena, r1);
    let r2 = de_bruijn_ref(&mut arena, 2);
    let root = pair(&mut arena, r2, nested);

    assert_eq!(de_bruijn_aperture(&arena, root), 2);
    assert_eq!(de_bruijn_aperture(&arena, nested), 0);
}

#[test]
fn lift_de_bruijn_only_lifts_free_references() {
    let mut arena = TreeArena::new();
    let r1 = de_bruijn_ref(&mut arena, 1);
    let r2 = de_bruijn_ref(&mut arena, 2);
    let body = pair(&mut arena, r1, r2);
    let group = de_bruijn_rec(&mut arena, body);

    let lifted = lift_de_bruijn(&mut arena, group);
    let lifted_body = match_de_bruijn_rec(&arena, lifted).expect("still de Bruijn rec");
    let (lhs, rhs) = pair_children(&arena, lifted_body).expect("pair shape preserved");
    assert_eq!(
        match_de_bruijn_ref(&arena, lhs),
        Some(1),
        "bound ref stays bound"
    );
    assert_eq!(
        match_de_bruijn_ref(&arena, rhs),
        Some(3),
        "free ref gets lifted"
    );
}

#[test]
fn lift_de_bruijn_n_honors_threshold() {
    let mut arena = TreeArena::new();
    let r1 = de_bruijn_ref(&mut arena, 1);
    let r2 = de_bruijn_ref(&mut arena, 2);
    let root = pair(&mut arena, r1, r2);
    let lifted = lift_de_bruijn_n(&mut arena, root, 2);
    let (lhs, rhs) = pair_children(&arena, lifted).expect("pair shape preserved");

    assert_eq!(match_de_bruijn_ref(&arena, lhs), Some(1));
    assert_eq!(match_de_bruijn_ref(&arena, rhs), Some(3));
}

#[test]
fn explicit_symbolic_helpers_roundtrip() {
    let mut arena = TreeArena::new();
    let var = arena.symbol("v");
    let body = arena.int(9);
    let rec = sym_rec(&mut arena, var, body);
    let rf = sym_ref(&mut arena, var);

    assert_eq!(match_sym_rec(&arena, rec), Some((var, body)));
    assert_eq!(match_sym_ref(&arena, rf), Some(var));
}

#[test]
fn validate_closed_de_bruijn_tree_accepts_closed_group() {
    let mut arena = TreeArena::new();
    let r1 = de_bruijn_ref(&mut arena, 1);
    let root = de_bruijn_rec(&mut arena, r1);

    assert_eq!(validate_closed_de_bruijn_tree(&arena, root), Ok(()));
}

#[test]
fn validate_closed_de_bruijn_tree_rejects_open_group() {
    let mut arena = TreeArena::new();
    let root = de_bruijn_ref(&mut arena, 1);

    assert_eq!(
        validate_closed_de_bruijn_tree(&arena, root),
        Err(RecursionError::OpenDeBruijnTree { aperture: 1 })
    );
}

#[test]
fn coherence_ok_for_closed_tree() {
    let mut arena = TreeArena::new();
    let r1 = de_bruijn_ref(&mut arena, 1);
    let root = de_bruijn_rec(&mut arena, r1);

    assert_eq!(check_de_bruijn_coherence(&arena, root), Ok(()));
}

#[test]
fn coherence_ok_for_nested_closed_tree() {
    let mut arena = TreeArena::new();
    let inner_ref = de_bruijn_ref(&mut arena, 1);
    let outer_ref = de_bruijn_ref(&mut arena, 2);
    let body = pair(&mut arena, inner_ref, outer_ref);
    let inner = de_bruijn_rec(&mut arena, body);
    let root = de_bruijn_rec(&mut arena, inner);

    assert_eq!(check_de_bruijn_coherence(&arena, root), Ok(()));
}

#[test]
fn coherence_err_free_ref_at_root() {
    let mut arena = TreeArena::new();
    let root = de_bruijn_ref(&mut arena, 1);

    assert_eq!(
        check_de_bruijn_coherence(&arena, root),
        Err(RecursionError::IncoherentDeBruijnReference {
            node: root,
            level: 1,
            depth: 0
        })
    );
}

#[test]
fn coherence_err_inner_ref_escapes() {
    let mut arena = TreeArena::new();
    let bound = de_bruijn_ref(&mut arena, 1);
    let escaping = de_bruijn_ref(&mut arena, 2);
    let body = pair(&mut arena, bound, escaping);
    let root = de_bruijn_rec(&mut arena, body);

    assert_eq!(
        check_de_bruijn_coherence(&arena, root),
        Err(RecursionError::IncoherentDeBruijnReference {
            node: escaping,
            level: 2,
            depth: 1
        })
    );
}

#[test]
fn coherence_vs_aperture_distinction() {
    let mut arena = TreeArena::new();
    let zero_level = de_bruijn_ref(&mut arena, 0);
    let root = de_bruijn_rec(&mut arena, zero_level);

    assert!(is_de_bruijn_closed(&arena, root));
    assert_eq!(
        check_de_bruijn_coherence(&arena, root),
        Err(RecursionError::IncoherentDeBruijnReference {
            node: zero_level,
            level: 0,
            depth: 1
        })
    );
}

#[test]
fn validate_symbolic_recursion_tree_accepts_bound_refs() {
    let mut arena = TreeArena::new();
    let var = arena.symbol("v");
    let rf = sym_ref(&mut arena, var);
    let nil = arena.nil();
    let body = arena.cons(rf, nil);
    let root = sym_rec(&mut arena, var, body);

    assert_eq!(validate_symbolic_recursion_tree(&arena, root), Ok(()));
}

#[test]
fn validate_symbolic_recursion_tree_rejects_unbound_refs() {
    let mut arena = TreeArena::new();
    let var = arena.symbol("v");
    let root = sym_ref(&mut arena, var);

    assert_eq!(
        validate_symbolic_recursion_tree(&arena, root),
        Err(SymbolicRecursionValidationError::UnboundReference { node: root, var })
    );
}

fn pair(arena: &mut TreeArena, lhs: tlib::TreeId, rhs: tlib::TreeId) -> tlib::TreeId {
    let tag = arena.intern_tag("PAIR");
    arena.intern(NodeKind::Tag(tag), &[lhs, rhs])
}

fn pair_children(arena: &TreeArena, id: tlib::TreeId) -> Option<(tlib::TreeId, tlib::TreeId)> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    if arena.tag_name(*tag_id)? != "PAIR" {
        return None;
    }
    match node.children.as_slice() {
        [lhs, rhs] => Some((*lhs, *rhs)),
        _ => None,
    }
}

#[test]
fn contains_recursion_marks_recursion_nodes_and_everything_above_them() {
    // The kContainsRec bit of C++ tlib: a recursion node occurs in the tree.
    let mut arena = TreeArena::new();
    let x = arena.symbol("x");
    let y = arena.symbol("y");
    let plain = pair(&mut arena, x, y);
    assert!(!arena.contains_recursion(x));
    assert!(!arena.contains_recursion(plain));
    assert!(!arena.contains_recursion(arena.nil()));

    // De Bruijn form: the reference, the binder, and their ancestors.
    let r1 = de_bruijn_ref(&mut arena, 1);
    let body = pair(&mut arena, r1, x);
    let rec = de_bruijn_rec(&mut arena, body);
    let above = pair(&mut arena, plain, rec);
    for id in [r1, body, rec, above] {
        assert!(arena.contains_recursion(id), "{id:?}");
    }

    // Symbolic form, where a self-reference is SYMREF(var): true inside the
    // definition as outside it.
    let sym = de_bruijn_to_sym(&mut arena, rec).expect("closed tree");
    let (var, sym_body) = match_sym_rec(&arena, sym).expect("SYMREC");
    let self_ref = sym_ref(&mut arena, var);
    assert!(arena.contains_recursion(self_ref));
    assert!(arena.contains_recursion(sym_body));
    assert!(arena.contains_recursion(sym));
    let rebuilt = sym_rec(&mut arena, var, plain);
    assert!(
        arena.contains_recursion(rebuilt),
        "SYMREC is a recursion node itself"
    );

    // Hash-consing returns the same node, with the same bit.
    let again = pair(&mut arena, x, y);
    assert_eq!(again, plain);
    assert!(!arena.contains_recursion(again));
    let triple = arena.intern_tag("TRIPLE");
    let wide = arena.intern(NodeKind::Tag(triple), &[x, y, rec]);
    assert!(arena.contains_recursion(wide), "arity >= 3 propagates too");
    let flat = arena.intern(NodeKind::Tag(triple), &[x, y, plain]);
    assert!(!arena.contains_recursion(flat));
}
