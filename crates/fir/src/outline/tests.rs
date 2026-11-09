//! Tests of the outlining pass (plan
//! `porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §8.1), on a
//! hand-built module shaped like a clocked program: a sample loop holding two
//! `ondemand`-like blocks, the first with a nested "done once" block that
//! stores into a local declared before the loop, the second reading the loop
//! counter and the block size.

use super::check::{OutlineViolation, check_outlining};
use super::{Mutation, OutlinePolicy, outline_large_blocks, outline_with};
use crate::checker::verify_fir_module;
use crate::{
    AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir,
};

const F32: FirType = FirType::Float32;
const I32: FirType = FirType::Int32;

fn ptr(t: FirType) -> FirType {
    FirType::Ptr(Box::new(t))
}

/// What to put at the end of the first block, besides its regular content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Extra {
    #[default]
    None,
    Return,
    Pointer,
    Address,
    Shadow,
}

#[derive(Debug, Clone, Copy)]
struct Shape {
    /// Statements of the nested "done once" block, besides its declaration
    /// and the store of the flag.
    inner: usize,
    /// Statements of the first block after the nested one.
    tail: usize,
    /// Statements of the second block.
    second: usize,
    extra: Extra,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            inner: 20,
            tail: 5,
            second: 12,
            extra: Extra::None,
        }
    }
}

fn arg(name: &str, typ: FirType) -> NamedType {
    NamedType {
        name: name.to_owned(),
        typ,
    }
}

fn module(shape: Shape) -> (FirStore, FirId) {
    let mut s = FirStore::new();
    let mut b = FirBuilder::new(&mut s);
    let ff = FirType::FaustFloat;

    let fields = [
        b.declare_var("fHslider0", F32, AccessType::Struct, None),
        b.declare_var("fRec0", F32, AccessType::Struct, None),
        b.declare_var("fRec1", F32, AccessType::Struct, None),
        b.declare_var(
            "fCoef",
            FirType::Array(Box::new(F32), 4),
            AccessType::Struct,
            None,
        ),
    ];
    let dsp_struct = b.block(&fields);

    let slow = b.load_var("fSlow0", AccessType::Stack, F32);
    let done = b.load_var("iDone0", AccessType::Stack, I32);
    let held = b.load_var("fHeld", AccessType::Stack, F32);
    let temp = b.load_var("iTemp0", AccessType::Stack, I32);
    let rec0 = b.load_var("fRec0", AccessType::Struct, F32);
    let i0 = b.load_var("i0", AccessType::Loop, I32);
    let count = b.load_var("count", AccessType::FunArgs, I32);
    let zero_i = b.int32(0);
    let one_i = b.int32(1);
    let two = b.float32(2.0);
    let one_f = b.float32(1.0);

    // The nested block: computes once, then raises the flag.
    let mut inner = Vec::new();
    let fa_init = b.binop(FirBinOp::Mul, slow, two, F32);
    inner.push(b.declare_var("fA", F32, AccessType::Stack, Some(fa_init)));
    let fa = b.load_var("fA", AccessType::Stack, F32);
    for k in 0..shape.inner {
        let at = b.int32((k % 4) as i32);
        let kf = b.float32(k as f32);
        let v = b.binop(FirBinOp::Add, fa, kf, F32);
        inner.push(b.store_table("fCoef", AccessType::Struct, at, v));
    }
    inner.push(b.store_var("iDone0", AccessType::Stack, one_i));
    let inner = b.block(&inner);
    let not_done = b.binop(FirBinOp::Eq, done, zero_i, I32);

    let mut first = vec![b.if_(not_done, inner, None)];
    let held_v = b.binop(FirBinOp::Add, slow, one_f, F32);
    first.push(b.store_var("fHeld", AccessType::Stack, held_v));
    for k in 0..shape.tail {
        let kf = b.float32(k as f32);
        let v = b.binop(FirBinOp::Mul, held, kf, F32);
        first.push(b.store_var("fRec1", AccessType::Struct, v));
    }
    match shape.extra {
        Extra::None => {}
        Extra::Return => first.push(b.ret(None)),
        Extra::Pointer => {
            let out0 = b.load_table("output0", AccessType::Stack, zero_i, ff.clone());
            let v = b.cast(F32, out0);
            first.push(b.store_var("fRec1", AccessType::Struct, v));
        }
        Extra::Address => {
            let addr = b.load_var_address("fHeld", AccessType::Stack, ptr(F32));
            first.push(b.drop_(addr));
        }
        Extra::Shadow => {
            first.push(b.declare_var("iTemp0", I32, AccessType::Stack, Some(one_i)));
            let t = b.load_var("iTemp0", AccessType::Stack, I32);
            let v = b.cast(F32, t);
            first.push(b.store_var("fRec1", AccessType::Struct, v));
        }
    }
    // The first block reads `iTemp0` before a shadowing declaration only in
    // the `Shadow` shape.
    if shape.extra == Extra::Shadow {
        let v = b.cast(F32, temp);
        first.insert(0, b.store_var("fRec1", AccessType::Struct, v));
    }
    let first = b.block(&first);

    let mut second = Vec::new();
    for k in 0..shape.second {
        let at = b.int32((k % 4) as i32);
        let fi = b.cast(F32, i0);
        let fc = b.cast(F32, count);
        let m = b.binop(FirBinOp::Mul, slow, fi, F32);
        let v = b.binop(FirBinOp::Add, m, fc, F32);
        second.push(b.store_table("fCoef", AccessType::Struct, at, v));
    }
    let second = b.block(&second);

    let fires = b.binop(FirBinOp::Ne, temp, zero_i, I32);
    let mut body = Vec::new();
    let changed = b.binop(FirBinOp::Ne, slow, rec0, I32);
    body.push(b.declare_var("iTemp0", I32, AccessType::Stack, Some(changed)));
    body.push(b.if_(fires, first, None));
    body.push(b.if_(fires, second, None));
    let next = b.binop(FirBinOp::Mul, rec0, held, F32);
    body.push(b.store_var("fRec0", AccessType::Struct, next));
    let out = b.cast(ff.clone(), rec0);
    body.push(b.store_table("output0", AccessType::Stack, i0, out));
    let body = b.block(&body);
    let lp = b.simple_for_loop("i0", count, body, false);

    let hslider = b.load_var("fHslider0", AccessType::Struct, F32);
    let zero_f = b.float32(0.0);
    let outputs = b.load_table("outputs", AccessType::FunArgs, zero_i, ptr(ff.clone()));
    let compute_body = [
        b.declare_var("fSlow0", F32, AccessType::Stack, Some(hslider)),
        b.declare_var("iDone0", I32, AccessType::Stack, Some(zero_i)),
        b.declare_var("fHeld", F32, AccessType::Stack, Some(zero_f)),
        b.declare_var("output0", ptr(ff.clone()), AccessType::Stack, Some(outputs)),
        lp,
    ];
    let compute_body = b.block(&compute_body);
    let streams = ptr(ptr(ff));
    let args = [
        arg("dsp", ptr(FirType::Obj)),
        arg("count", I32),
        arg("inputs", streams.clone()),
        arg("outputs", streams.clone()),
    ];
    let typ = FirType::Fun {
        args: args.iter().map(|a| a.typ.clone()).collect(),
        ret: Box::new(FirType::Void),
    };
    let compute = b.declare_fun("compute", typ, &args, Some(compute_body), false);
    let functions = b.block(&[compute]);
    let globals = b.block(&[]);
    let statics = b.block(&[]);
    let root = b.module(
        1,
        1,
        "outlined",
        dsp_struct,
        globals,
        functions,
        statics,
        &[],
    );
    (s, root)
}

fn small() -> OutlinePolicy {
    OutlinePolicy {
        budget: 10,
        min_block: 2,
    }
}

fn functions(s: &FirStore, root: FirId) -> Vec<(String, Vec<NamedType>, FirId)> {
    let FirMatch::Module { functions, .. } = match_fir(s, root) else {
        panic!("not a module");
    };
    let FirMatch::Block(items) = match_fir(s, functions) else {
        panic!("functions is not a block");
    };
    items
        .into_iter()
        .filter_map(|f| match match_fir(s, f) {
            FirMatch::DeclareFun {
                name,
                args,
                body: Some(body),
                ..
            } => Some((name, args, body)),
            _ => None,
        })
        .collect()
}

fn same(a: (&FirStore, FirId), b: (&FirStore, FirId)) -> bool {
    let mut cs = FirStore::new();
    let x = cs.import_from(a.0, a.1);
    let y = cs.import_from(b.0, b.1);
    x == y
}

#[test]
fn a_module_under_the_budget_comes_back_unchanged() {
    let (src, root) = module(Shape::default());
    let (dst, droot, report) =
        outline_large_blocks(&src, root, OutlinePolicy::default()).expect("outlining");
    assert!(report.outlined.is_empty());
    assert!(same((&src, root), (&dst, droot)));
}

#[test]
fn blocks_are_outlined_largest_first_with_value_and_reference_parameters() {
    let (src, root) = module(Shape::default());
    let (dst, droot, report) = outline_large_blocks(&src, root, small()).expect("outlining");
    let names: Vec<_> = report.outlined.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        ["computeOutline0", "computeOutline1", "computeOutline2"]
    );
    let first = &report.outlined[0];
    assert_eq!(first.parent, "compute");
    assert_eq!(first.by_value, ["fSlow0"]);
    assert_eq!(first.by_reference, ["fHeld", "iDone0"]);
    let second = &report.outlined[1];
    assert_eq!(second.parent, "compute");
    assert_eq!(second.by_value, ["count", "fSlow0", "i0"]);
    assert!(second.by_reference.is_empty());
    // The nested block, outlined from the first function, receives the flag
    // reference passed on.
    let nested = &report.outlined[2];
    assert_eq!(nested.parent, "computeOutline0");
    assert_eq!(nested.by_value, ["fSlow0"]);
    assert_eq!(nested.by_reference, ["iDone0"]);

    let funs = functions(&dst, droot);
    assert_eq!(funs.len(), 4);
    let (_, args, _) = &funs[1];
    let arg_names: Vec<_> = args.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(arg_names, ["dsp", "fHeld", "fSlow0", "iDone0"]);
    assert_eq!(args[1].typ, ptr(F32));
    assert_eq!(args[2].typ, F32);
    check_outlining(&src, root, &dst, droot).expect("checker");
}

#[test]
fn the_output_passes_the_fir_verifier() {
    let (src, root) = module(Shape::default());
    let before = verify_fir_module(&src, root);
    let (dst, droot, _) = outline_large_blocks(&src, root, small()).expect("outlining");
    let after = verify_fir_module(&dst, droot);
    let errors = |r: &crate::checker::FirVerifyReport| {
        r.diagnostics
            .iter()
            .filter(|d| d.severity == crate::checker::Severity::Error)
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect::<Vec<_>>()
    };
    assert!(errors(&before).is_empty(), "{:?}", errors(&before));
    assert!(errors(&after).is_empty(), "{:?}", errors(&after));
}

#[test]
fn the_if_stays_in_the_caller_around_the_call() {
    let (src, root) = module(Shape::default());
    let (dst, droot, _) = outline_large_blocks(&src, root, small()).expect("outlining");
    let (_, _, body) = &functions(&dst, droot)[0];
    let FirMatch::Block(items) = match_fir(&dst, *body) else {
        panic!()
    };
    let FirMatch::SimpleForLoop { body: lp, .. } = match_fir(&dst, items[4]) else {
        panic!("the loop moved")
    };
    let FirMatch::Block(lp) = match_fir(&dst, lp) else {
        panic!()
    };
    for &stmt in &lp[1..3] {
        let FirMatch::If { then_block, .. } = match_fir(&dst, stmt) else {
            panic!("the if moved")
        };
        let FirMatch::Block(then) = match_fir(&dst, then_block) else {
            panic!()
        };
        assert_eq!(then.len(), 1);
        let FirMatch::Drop(call) = match_fir(&dst, then[0]) else {
            panic!("not a call statement")
        };
        assert!(matches!(match_fir(&dst, call), FirMatch::FunCall { .. }));
    }
}

#[test]
fn the_pass_is_deterministic() {
    let (src, root) = module(Shape::default());
    let (a, ra, _) = outline_large_blocks(&src, root, small()).expect("outlining");
    let (b, rb, _) = outline_large_blocks(&src, root, small()).expect("outlining");
    assert!(same((&a, ra), (&b, rb)));
}

#[test]
fn a_block_under_the_minimum_stays_inline() {
    let (src, root) = module(Shape::default());
    let policy = OutlinePolicy {
        budget: 10,
        min_block: 13,
    };
    let (_, _, report) = outline_large_blocks(&src, root, policy).expect("outlining");
    // The second block (12 statements) is under the minimum; the first and
    // its nested block (22 statements) are not.
    assert!(report.outlined.iter().all(|f| f.statements >= 13));
    assert!(
        !report
            .outlined
            .iter()
            .any(|f| f.by_value.contains(&"i0".to_owned()))
    );
}

#[test]
fn blocks_that_cannot_be_outlined_exactly_stay_inline() {
    for (extra, reason) in [
        (Extra::Return, "a return"),
        (
            Extra::Pointer,
            "an address, indexed access, tee or shift of a free variable",
        ),
        (
            Extra::Address,
            "an address, indexed access, tee or shift of a free variable",
        ),
        (
            Extra::Shadow,
            "a declaration of a name the block also reads from outside",
        ),
    ] {
        let (src, root) = module(Shape {
            extra,
            ..Shape::default()
        });
        let (dst, droot, report) =
            outline_large_blocks(&src, root, small()).unwrap_or_else(|e| panic!("{extra:?}: {e}"));
        assert!(
            report
                .refused
                .iter()
                .any(|r| r.function == "compute" && r.reason == reason),
            "{extra:?}: {:?}",
            report.refused
        );
        // The second block is still outlined, and the nested block of the
        // first one too (from `compute`).
        assert!(!report.outlined.is_empty(), "{extra:?}");
        check_outlining(&src, root, &dst, droot).expect("checker");
    }
}

fn rejected(mutation: Mutation) -> OutlineViolation {
    let (src, root) = module(Shape::default());
    let (dst, droot, _) = outline_with(&src, root, small(), mutation).expect("the producer runs");
    check_outlining(&src, root, &dst, droot)
        .map(|()| panic!("{mutation:?} should be rejected"))
        .map_err(|v| {
            eprintln!("{mutation:?} -> {v}");
            v
        })
        .unwrap_err()
}

#[test]
fn a_written_variable_passed_by_value_is_rejected_by_closed() {
    assert!(matches!(
        rejected(Mutation::ByValueWrite),
        OutlineViolation::Closed { .. }
    ));
}

#[test]
fn a_missing_parameter_is_rejected_by_closed() {
    assert!(matches!(
        rejected(Mutation::MissingParam),
        OutlineViolation::Closed { .. }
    ));
}

#[test]
fn swapped_arguments_are_rejected_by_calls() {
    assert!(matches!(
        rejected(Mutation::SwapArgs),
        OutlineViolation::Calls { .. }
    ));
}

#[test]
fn a_dropped_statement_is_rejected_by_inline_back() {
    assert!(matches!(
        rejected(Mutation::DropStatement),
        OutlineViolation::InlineBack { .. }
    ));
}

#[test]
fn a_call_written_twice_is_rejected_by_calls() {
    assert!(matches!(
        rejected(Mutation::CallTwice),
        OutlineViolation::Calls { .. }
    ));
}

#[test]
fn a_shadowed_parameter_is_rejected_by_closed() {
    assert!(matches!(
        rejected(Mutation::ShadowParam),
        OutlineViolation::Closed { .. }
    ));
}

#[test]
fn a_reference_read_at_index_one_is_rejected_by_closed() {
    assert!(matches!(
        rejected(Mutation::WrongIndex),
        OutlineViolation::Closed { .. }
    ));
}

#[test]
fn the_unmutated_producer_is_accepted() {
    let (src, root) = module(Shape::default());
    let (dst, droot, _) =
        outline_with(&src, root, small(), Mutation::None).expect("the producer runs");
    check_outlining(&src, root, &dst, droot).expect("accepted");
}
