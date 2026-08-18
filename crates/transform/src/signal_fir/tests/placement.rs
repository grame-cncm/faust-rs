//! `placement` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_fir::{SignalFirOptions, SignalFirRequest, compile_signals_to_fir_fastlane};
use fir::{AccessType, FirMatch, match_fir};
use signals::{BinOp, SigBuilder};
use tlib::{TreeArena, de_bruijn_rec, de_bruijn_ref};
use ui::{ControlKind, ControlRange};

#[test]
fn slider_cast_hoisted_to_control_as_fslow() {
    // Faust: process = hslider("gain", 0.5, 0, 1, 0.01) * _;
    // Expected: fSlow0 = float(fHslider0) in compute preamble,
    //           fSlow0 * input used in sample loop.
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HSlider,
        "gain",
        Some(ControlRange {
            init: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }),
        false,
        false,
    );
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let slider = b.hslider(0);
        let in0 = b.input(0);
        b.binop(BinOp::Mul, slider, in0)
    };
    let out = compile_signals_to_fir_fastlane(&SignalFirRequest::new(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    ))
    .expect("slider*input should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let compute_stmts = find_compute_body_stmts(&out.store, functions);

    // There should be a DeclareVar(fSlow*, Stack) in the compute preamble.
    let has_fslow = compute_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("fSlow")
        )
    });
    assert!(
        has_fslow,
        "block-rate slider cast should be hoisted to compute preamble as fSlow*"
    );

    // Inside the sample loop, the slider should appear as a LoadVar(fSlow*),
    // not as an inline Cast(Float32, LoadVar(fHslider*)).
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(loop_stmts) = match_fir(&out.store, loop_body) else {
        panic!("loop body block expected");
    };
    let output_store = loop_stmts
        .iter()
        .find_map(|id| match match_fir(&out.store, *id) {
            FirMatch::StoreTable { name, value, .. } if name == "output0" => Some(value),
            _ => None,
        })
        .expect("output store expected");
    let inner = unwrap_output_cast(&out.store, output_store);

    // The Mul should have a LoadVar(fSlow*) as one operand.
    if let FirMatch::BinOp { lhs, rhs, .. } = match_fir(&out.store, inner) {
        let lhs_is_fslow = matches!(
            match_fir(&out.store, lhs),
            FirMatch::LoadVar { ref name, .. } if name.starts_with("fSlow")
        );
        let rhs_is_fslow = matches!(
            match_fir(&out.store, rhs),
            FirMatch::LoadVar { ref name, .. } if name.starts_with("fSlow")
        );
        assert!(
            lhs_is_fslow || rhs_is_fslow,
            "sample-loop Mul should reference a hoisted fSlow* variable"
        );
    }
}
#[test]
fn sample_rate_const_hoisted_to_instance_constants_as_fconst_struct() {
    // Faust: process = 2.0 * float(SR);
    // The `FloatCast(fSamplingFreq)` sub-expression should be hoisted to
    // instanceConstants as a fConst*.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0); // type tag for integer fconst
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let two = b.real(2.0);
        let sr = b.fconst(ty, name, file);
        let sr_float = b.float_cast(sr);
        b.binop(BinOp::Mul, two, sr_float)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("const SR expression should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    // instanceConstants should contain a StoreVar(fConst*, Struct, ...).
    let has_fconst = ic_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fConst")
        )
    });
    assert!(
        has_fconst,
        "init-time constant expression should be hoisted to instanceConstants as fConst*"
    );
}
#[test]
fn recursive_feedback_stays_in_sample_loop() {
    // Faust: process = + ~ _;   (recursive feedback: Samp variability)
    // The recursion load/store must remain in the sample loop body.
    let mut arena = TreeArena::new();
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut b = SigBuilder::new(&mut arena);
        let in0 = b.input(0);
        let feedback = b.proj(0, self_ref);
        b.binop(BinOp::Add, in0, feedback)
    };
    let body_list = arena.cons(body, arena.nil());
    let group = de_bruijn_rec(&mut arena, body_list);
    let sig0 = SigBuilder::new(&mut arena).proj(0, group);
    let out = compile_fastlane_without_ui(&arena, &[sig0], 1, 1, &SignalFirOptions::default())
        .expect("recursive feedback should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let loop_body = find_compute_loop_body(&out.store, functions);
    let FirMatch::Block(loop_stmts) = match_fir(&out.store, loop_body) else {
        panic!("loop body block expected");
    };

    // The sample loop should still contain the output store (not empty).
    assert!(
        loop_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable { ref name, .. } if name == "output0"
        )),
        "recursive feedback output should remain in the sample loop"
    );

    // The recursive state update should be in the sample loop (not hoisted).
    let has_rec_update = loop_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreTable { ref name, .. }
                if name.starts_with("fRec") || name.starts_with("iRec")
        ) || matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. }
                if name.starts_with("fRec") || name.starts_with("iRec")
        )
    });
    assert!(
        has_rec_update,
        "recursive state update should remain in the sample loop, not be hoisted"
    );
}
#[test]
fn shared_konst_subexpr_stays_stack_local_inside_instance_constants() {
    // Faust-like shape: let s = 2.0 * float(SR); process = s + sin(s);
    // The shared `s` subtree is Konst and referenced twice, but only within
    // `instanceConstants()`, so it should materialize as a stack-local fConst*.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let sr = b.fconst(ty, name, file);
        let s = {
            let sr_float = b.float_cast(sr);
            let two = b.real(2.0);
            b.binop(BinOp::Mul, two, sr_float)
        };
        let sin_s = b.sin(s);
        b.binop(BinOp::Add, s, sin_s)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("shared Konst subtree should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    assert!(
        ic_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("fConst")
        )),
        "shared Konst subtree used only during init should hoist to a stack-local fConst*"
    );
    assert!(
        ic_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fConst")
        )),
        "the escaping outer Konst result should still materialize as a persistent fConst*"
    );
}
#[test]
fn integer_konst_uses_iconst_prefix() {
    // Faust-like shape: process = SR + 1;
    // The root integer Konst expression escapes to compute via the output path,
    // so it should materialize as an iConst* struct field.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let sr = b.fconst(ty, name, file);
        let one = b.int(1);
        b.binop(BinOp::Add, sr, one)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("integer Konst expression should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    assert!(
        ic_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("iConst")
        )),
        "escaping integer Konst expression should hoist as an iConst* struct field"
    );
}
#[test]
fn trivial_constant_not_hoisted() {
    // Bare literal constants (e.g. `process = 0.5;`) are trivial and
    // should NOT be materialized into fConst* variables.
    let mut arena = TreeArena::new();
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        b.real(0.5)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("bare constant should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    let has_fconst = ic_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                ..
            } if name.starts_with("fConst")
        )
    });
    assert!(
        !has_fconst,
        "trivial literal constant should NOT be hoisted to a fConst* variable"
    );
}
#[test]
fn integer_block_subexpr_uses_islow_prefix() {
    // Faust-like shape: process = int(slider) + 1;
    // The root integer block-rate expression sits at the Block->Samp boundary,
    // so it should hoist as iSlow*.
    let mut arena = TreeArena::new();
    let ui = one_control_ui(
        ControlKind::HSlider,
        "gain",
        Some(ControlRange {
            init: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }),
        false,
        false,
    );
    let sig0 = {
        let mut b = SigBuilder::new(&mut arena);
        let slider = b.hslider(0);
        let int_slider = b.int_cast(slider);
        let one = b.int(1);
        b.binop(BinOp::Add, int_slider, one)
    };
    let out = compile_signals_to_fir_fastlane(&SignalFirRequest::new(
        &arena,
        &[sig0],
        1,
        1,
        &ui,
        &SignalFirOptions::default(),
    ))
    .expect("integer block-rate subtree should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let compute_stmts = find_compute_body_stmts(&out.store, functions);

    assert!(
        compute_stmts.iter().any(|id| matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar {
                ref name,
                access: AccessType::Stack,
                ..
            } if name.starts_with("iSlow")
        )),
        "shared integer block-rate subtree should hoist to an iSlow* local"
    );
}
#[test]
fn konst_sr_based_mul_hoisted_to_instance_constants() {
    // Faust: process = 2.0 * float(SR) * float(SR);
    // The multiplication of two non-trivial init-time expressions should
    // produce fConst* variables in instanceConstants.
    let mut arena = TreeArena::new();
    let sig0 = {
        let ty = arena.int(0);
        let name = arena.symbol("fSamplingFreq");
        let file = arena.symbol("<math.h>");
        let mut b = SigBuilder::new(&mut arena);
        let sr = b.fconst(ty, name, file);
        let sr_float = b.float_cast(sr);
        let two = b.real(2.0);
        let product = b.binop(BinOp::Mul, two, sr_float);
        // Multiply again so the outer Mul is non-trivial and Konst.
        b.binop(BinOp::Mul, product, sr_float)
    };
    let out = compile_fastlane_without_ui(&arena, &[sig0], 0, 1, &SignalFirOptions::default())
        .expect("SR-based expression should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let ic_stmts = find_instance_constants_stmts(&out.store, functions);

    // instanceConstants should contain at least one fConst* store.
    let has_fconst = ic_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar {
                ref name,
                access: AccessType::Struct,
                ..
            } if name.starts_with("fConst")
        )
    });
    assert!(
        has_fconst,
        "non-trivial init-time SR expression should be hoisted as fConst*"
    );
}
