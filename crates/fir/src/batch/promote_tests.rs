//! Tests of the state promotion (plan
//! `porting/batch-state-register-promotion-plan-2026-11-08-en.md` §5.1), on
//! the hand-built module of the layout tests: a control read in `compute`, a
//! recursion buffer of each element type, a delay line at a dynamic index
//! with its counter, and the shared state of a table generator.

use super::promote::{Mutation, PromotedElement, promote_state_with};
use super::promote_check::check_promotion;
use super::tests::{Twist, function, module};
use super::{layout_voices, promote_state};
use crate::{AccessType, FirId, FirMatch, FirStore, match_fir};

fn element(field: &str, index: Option<i32>, written: bool) -> PromotedElement {
    PromotedElement {
        field: field.to_owned(),
        index,
        local: match index {
            Some(k) => format!("{field}_reg{k}"),
            None => format!("{field}_reg"),
        },
        written,
    }
}

fn block(s: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(s, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

#[test]
fn scalars_and_literal_elements_are_promoted_and_the_delay_line_is_not() {
    let (src, root) = module(Twist::default());
    let (_, _, state) = promote_state(&src, root).expect("promotion");
    assert_eq!(
        state.elements,
        vec![
            element("fHslider0", None, false),
            element("fRec0", Some(0), true),
            element("fRec0", Some(1), false),
            element("iRec1", Some(0), true),
            element("iRec1", Some(1), false),
            element("fIOTA", None, true),
        ]
    );
    assert_eq!(state.kept, vec![("fVec0".to_owned(), "a dynamic index")]);
}

#[test]
fn the_promoted_compute_is_entry_body_exit() {
    let (src, root) = module(Twist::default());
    let (dst, droot, state) = promote_state(&src, root).expect("promotion");
    let (_, _, body) = function(&dst, droot, "compute");
    let items = block(&dst, body);
    let entries = state.elements.len();
    let written = state.elements.iter().filter(|e| e.written).count();
    // Six entry declarations, the four source statements, three write-backs.
    assert_eq!(items.len(), entries + 4 + written);
    for &item in &items[..entries] {
        assert!(matches!(
            match_fir(&dst, item),
            FirMatch::DeclareVar {
                access: AccessType::Stack,
                init: Some(_),
                ..
            }
        ));
    }
    for &item in &items[items.len() - written..] {
        assert!(matches!(
            match_fir(&dst, item),
            FirMatch::StoreVar {
                access: AccessType::Struct,
                ..
            } | FirMatch::StoreTable {
                access: AccessType::Struct,
                ..
            }
        ));
    }
    // The other functions are untouched.
    for name in ["staticInit", "instanceConstants", "instanceClear"] {
        let mut cs = FirStore::new();
        let a = cs.import_from(&src, function(&src, root, name).2);
        let b = cs.import_from(&dst, function(&dst, droot, name).2);
        assert_eq!(a, b, "{name} changed");
    }
}

#[test]
fn the_promoted_module_is_laid_out_and_checked_by_the_layout_pass() {
    let (src, root) = module(Twist::default());
    let (dst, droot, _) = promote_state(&src, root).expect("promotion");
    layout_voices(&dst, droot).expect("layout of the promoted module");
}

#[test]
fn an_address_keeps_its_field_in_memory_only() {
    let (src, root) = module(Twist {
        address: true,
        ..Twist::default()
    });
    let (_, _, state) = promote_state(&src, root).expect("promotion");
    assert!(state.elements.iter().all(|e| e.field != "fRec0"));
    assert!(state.elements.iter().any(|e| e.field == "iRec1"));
    assert!(state.kept.contains(&("fRec0".to_owned(), "its address")));
}

#[test]
fn an_early_return_or_a_module_call_promotes_nothing() {
    for twist in [
        Twist {
            early_return: true,
            ..Twist::default()
        },
        Twist {
            module_call: true,
            ..Twist::default()
        },
    ] {
        let (src, root) = module(twist);
        let (dst, droot, state) = promote_state(&src, root).expect("promotion");
        assert!(state.elements.is_empty());
        let mut cs = FirStore::new();
        assert_eq!(cs.import_from(&src, root), cs.import_from(&dst, droot));
    }
}

#[test]
fn every_mutation_is_rejected_by_its_rule() {
    let cases = [
        (Mutation::DropWriteBack, "write-back"),
        (Mutation::SpuriousWriteBack, "write-back"),
        (Mutation::LeaveOneAccess, "memory-access"),
        (Mutation::WrongElement, "entry-exit"),
        (Mutation::PromoteDynamic, "literal-only"),
        (Mutation::ExitBeforeBody, "memory-access"),
    ];
    let (src, root) = module(Twist::default());
    for (mutation, rule) in cases {
        let (dst, droot, _) = promote_state_with(&src, root, mutation).expect("producer");
        let err = check_promotion(&src, root, &dst, droot)
            .expect_err(&format!("{mutation:?} was accepted"));
        assert_eq!(err.rule, rule, "{mutation:?}: {err}");
    }
    let (dst, droot, _) = promote_state_with(&src, root, Mutation::None).expect("producer");
    check_promotion(&src, root, &dst, droot).expect("the unmutated output");
}

#[test]
fn the_checker_accepts_an_unchanged_module_and_rejects_a_changed_one() {
    let (src, root) = module(Twist::default());
    check_promotion(&src, root, &src, root).expect("identity");
    let (other, oroot) = module(Twist {
        address: true,
        ..Twist::default()
    });
    let err = check_promotion(&src, root, &other, oroot).expect_err("a different module");
    assert_eq!(err.rule, "unpromote");
}
