//! `reverse_ad` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_fir::SignalFirOptions;
use fir::{FirMatch, match_fir};
use signals::{BinOp, BlockRevPolicy, SigBuilder};
use tlib::{TreeArena, de_bruijn_rec, de_bruijn_ref};

// ── BlockReverseAD golden FIR tests (Phase B3–B6) ────────────────────────────

/// A BRA carrier lowering must produce exactly two sample loops in `compute`:
/// one forward loop for the primal output and one **reverse** loop for the
/// gradient output.
///
/// Circuit: `process = rad(2 * x, x)` where x is a constant seed (real=2.0).
/// The primal output is `2 * 2.0 = 4.0` and the gradient w.r.t. x is `2.0`.
///
/// This test checks the structural FIR property: the compute function contains
/// two `SimpleForLoop` nodes, the second of which has `is_reverse = true`.
#[test]
fn bra_simple_linear_produces_forward_and_reverse_loops() {
    let mut arena = TreeArena::new();

    // body = 2.0 * x  where x = Real(2.0) (constant seed)
    let x = SigBuilder::new(&mut arena).real(2.0);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body = SigBuilder::new(&mut arena).binop(BinOp::Mul, two, x);
    let cot = SigBuilder::new(&mut arena).real(1.0);

    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body],
        &[x],
        &[cot],
        BlockRevPolicy::TapeFull,
    );

    // Proj(0, carrier) = primal output (forward loop)
    // Proj(1, carrier) = gradient output (reverse loop)
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("simple BRA should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };

    // Collect all SimpleForLoop nodes in compute.
    let compute_body = find_decl_fun_body(&out.store, functions, "compute");
    let FirMatch::Block(stmts) = match_fir(&out.store, compute_body) else {
        panic!("compute body block expected");
    };
    let loops: Vec<bool> = stmts
        .iter()
        .filter_map(|id| match match_fir(&out.store, *id) {
            FirMatch::SimpleForLoop { is_reverse, .. } => Some(is_reverse),
            _ => None,
        })
        .collect();

    assert_eq!(
        loops.len(),
        2,
        "BRA should produce exactly two sample loops (forward + reverse), got {loops:?}"
    );
    assert!(
        !loops[0],
        "first loop should be forward (is_reverse = false)"
    );
    assert!(
        loops[1],
        "second loop should be reverse (is_reverse = true)"
    );
}
/// A BRA carrier with a `Delay1` in the body must declare a `fBraCarry*`
/// struct field for the anti-causal adjoint carry.
///
/// Circuit: `body = delay1(x)` where x is a constant seed.
/// The BRA backward rule for `Delay1(x)` allocates a carry variable
/// (`fBraCarry0`) to propagate `adj[x][n-1] += adj[Delay1(x)][n]`
/// across reverse-loop iterations.
#[test]
fn bra_delay1_seed_produces_carry_struct_field() {
    let mut arena = TreeArena::new();

    let x = SigBuilder::new(&mut arena).real(0.5);
    let body = SigBuilder::new(&mut arena).delay1(x);
    let cot = SigBuilder::new(&mut arena).real(1.0);

    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body],
        &[x],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("Delay1 BRA should compile");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };

    let has_bra_carry = struct_items.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name.starts_with("fBraCarry")
        )
    });
    assert!(
        has_bra_carry,
        "Delay1 in BRA body should produce a fBraCarry* struct field for the anti-causal carry"
    );
}
/// BRA adjoint carry variables must appear in `instanceClear` but NOT in
/// `instanceResetUserInterface`.
///
/// BRA carry variables (`fBraCarry*`) are internal DSP state, not UI-controlled
/// parameters.  They must be zeroed during `instanceClear` but must NOT appear
/// in `instanceResetUserInterface` (which is reserved for sliders, buttons, etc.).
/// They are also reset by `emit_bra_compute_resets` in the compute preamble
/// (TBPTT(BS,BS) truncation: adjoint carry = 0 at start of each block).
///
/// Before this fix, `ensure_bra_delay1_carry` passed `Some(zero)` to
/// `ensure_named_struct_var`, causing the carry to be registered as a reset-time
/// UI init.
#[test]
fn bra_carry_absent_from_instance_reset_user_interface() {
    let mut arena = TreeArena::new();

    // Build: Proj(0, Rec([p * Delay1(Proj(0, self_ref)) + 2]))
    let p = SigBuilder::new(&mut arena).real(0.5);
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let feedback = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, p, delayed);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body_expr = SigBuilder::new(&mut arena).binop(signals::BinOp::Add, two, feedback);
    let body_list = arena.cons(body_expr, arena.nil());
    let rec_group = de_bruijn_rec(&mut arena, body_list);
    let rec_out = SigBuilder::new(&mut arena).proj(0, rec_group);

    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[rec_out],
        &[p],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("recursive BRA (one-pole, no UI) should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };

    // `instanceResetUserInterface` must NOT contain any `fBraCarry*` store.
    let reset_body = find_decl_fun_body(&out.store, functions, "instanceResetUserInterface");
    let FirMatch::Block(reset_stmts) = match_fir(&out.store, reset_body) else {
        panic!("instanceResetUserInterface body block expected");
    };
    let has_bra_carry_in_reset = reset_stmts.iter().any(|id| {
        if let FirMatch::StoreVar { name, .. } = match_fir(&out.store, *id) {
            return name.starts_with("fBraCarry");
        }
        false
    });
    assert!(
        !has_bra_carry_in_reset,
        "fBraCarry* must NOT appear in instanceResetUserInterface; \
         BRA carries are DSP state, not UI state"
    );

    // `instanceClear` MUST contain the `fBraCarry*` zero-init.
    let clear_body = find_decl_fun_body(&out.store, functions, "instanceClear");
    let FirMatch::Block(clear_stmts) = match_fir(&out.store, clear_body) else {
        panic!("instanceClear body block expected");
    };
    let has_bra_carry_in_clear = clear_stmts.iter().any(|id| {
        if let FirMatch::StoreVar { name, .. } = match_fir(&out.store, *id) {
            return name.starts_with("fBraCarry");
        }
        false
    });
    assert!(
        has_bra_carry_in_clear,
        "fBraCarry* must appear in instanceClear for proper DSP reset"
    );
}
/// BRA primal SYMREC carrier must NOT be reset in the `compute()` preamble.
///
/// The SYMREC primal state (e.g. `fRec<N>`) is persistent DSP filter memory.
/// It must only be cleared in `instanceClear()` and must never be reset in
/// `compute()`, which would sabotage the filter continuity across host blocks.
///
/// Before this fix, `emit_reverse_time_rec_compute_resets` reset ALL entries in
/// `rec_array_by_group_index`, including SYMREC primal carriers.  The fix adds
/// `reverse_time_rec_group_ids` to `RecursionState` so that only `ReverseTimeRec`
/// adjoint carriers are zeroed in the compute preamble.
#[test]
fn bra_symrec_primal_carrier_absent_from_compute_preamble_resets() {
    let mut arena = TreeArena::new();

    // Same one-pole recursive circuit as above.
    let p = SigBuilder::new(&mut arena).real(0.5);
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let feedback = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, p, delayed);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body_expr = SigBuilder::new(&mut arena).binop(signals::BinOp::Add, two, feedback);
    let body_list = arena.cons(body_expr, arena.nil());
    let rec_group = de_bruijn_rec(&mut arena, body_list);
    let rec_out = SigBuilder::new(&mut arena).proj(0, rec_group);

    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[rec_out],
        &[p],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
            .expect("recursive BRA (one-pole) should compile");

    let FirMatch::Module { functions, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };

    // The compute() body starts with a preamble of StoreVar (resets) followed by
    // the forward/reverse sample loops.  Extract all top-level StoreVar names.
    let compute_body = find_decl_fun_body(&out.store, functions, "compute");
    let FirMatch::Block(compute_stmts) = match_fir(&out.store, compute_body) else {
        panic!("compute body block expected");
    };
    // Collect all `fRec*` variable names reset in the compute preamble
    // (StoreVar statements before any loop).
    let mut rec_resets_in_compute: Vec<String> = Vec::new();
    for &id in compute_stmts.iter() {
        match match_fir(&out.store, id) {
            FirMatch::StoreVar { name, .. } if name.starts_with("fRec") => {
                rec_resets_in_compute.push(name.clone());
            }
            _ => {}
        }
    }
    assert!(
        rec_resets_in_compute.is_empty(),
        "SYMREC primal carrier(s) {:?} must NOT be reset in compute() preamble; \
         they are persistent DSP state that must survive across host compute() calls",
        rec_resets_in_compute
    );

    // BRA adjoint carry MUST appear in the compute preamble (TBPTT(BS,BS) reset).
    let has_bra_carry_reset_in_compute = compute_stmts.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::StoreVar { ref name, .. } if name.starts_with("fBraCarry")
        )
    });
    assert!(
        has_bra_carry_reset_in_compute,
        "fBraCarry* adjoint carry must be reset in compute() preamble for TBPTT(BS,BS)"
    );
}
/// A BRA body containing a `FConst` node (e.g. `fSampleRate` pulled in via
/// `stdfaust.lib` → `ma.SR`) must compile without error.
///
/// Before the fix, `FConst` was not listed as a leaf in:
/// - `is_trivially_reverse_evaluable` — causing it to be mistakenly taped,
/// - `collect_bra_postorder` — reaching the postorder as an unknown node,
/// - `propagate_bra_adj` — triggering `[FRS-SFIR-0004] signal FConst(…) not
///   supported in BlockReverseAD backward pass (B6)`.
///
/// The BRA backward pass treats `FConst` (and `FVar`) as zero-gradient leaves:
/// they are external scalars with no differentiable children.
#[test]
fn bra_body_with_fconst_leaf_compiles() {
    let mut arena = TreeArena::new();

    // Build a body: seed * FConst("fSampleRate")
    // This represents e.g. `p * ma.SR` inside the BRA expression.
    let p = SigBuilder::new(&mut arena).real(0.5);
    let ty_node = SigBuilder::new(&mut arena).int(0); // type tag (int=0)
    // Use the recognized alias "fSamplingFreq" — lower_fconst maps it to fSampleRate.
    let name_node = arena.string_lit("fSamplingFreq");
    let file_node = arena.string_lit("<math.h>");
    let fc = SigBuilder::new(&mut arena).fconst(ty_node, name_node, file_node);
    let body_expr = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, p, fc);

    // BRA: differentiate body w.r.t. p
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body_expr],
        &[p],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    // Must not crash with "signal FConst(…) not supported in BlockReverseAD backward pass".
    compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
        .expect("BRA body with FConst leaf must compile successfully");
}
/// A BRA carrier wrapping a recursive circuit must declare a `fBraCarry*`
/// struct field for the TBPTT feedback carry.
///
/// Circuit: `process = rad(c : +~*(c), c)` — the Faust one-pole feedback
/// filter where `c` is both input and feedback coefficient.  In Signal IR
/// the recursive circuit becomes `Proj(0, SYMREC(var, Add(Input, Mul(c,
/// Delay1(Proj(0, SYMREF(var)))))))`.  The pre-scan in
/// `ensure_bra_backward_sweep` allocates a `fBraCarry*` struct field for the
/// `Delay1(Proj(0, SYMREF))` feedback node.
#[test]
fn bra_recursive_one_pole_produces_feedback_carry_struct_field() {
    let mut arena = TreeArena::new();

    // Build: Proj(0, Rec([Input(0) + c * Delay1(Proj(0, self_ref))]))
    let c = SigBuilder::new(&mut arena).real(0.5); // feedback coefficient seed
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let feedback = SigBuilder::new(&mut arena).binop(BinOp::Mul, c, delayed);
    let input0 = SigBuilder::new(&mut arena).input(0);
    let body_expr = SigBuilder::new(&mut arena).binop(BinOp::Add, input0, feedback);
    let body_list = arena.cons(body_expr, arena.nil());
    let rec_group = de_bruijn_rec(&mut arena, body_list);
    let rec_out = SigBuilder::new(&mut arena).proj(0, rec_group);

    // BRA: differentiate rec_out w.r.t. c
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[rec_out],
        &[c],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let out =
        compile_fastlane_without_ui(&arena, &[primal, grad], 1, 2, &SignalFirOptions::default())
            .expect("recursive BRA (one-pole) should compile");

    let FirMatch::Module { dsp_struct, .. } = match_fir(&out.store, out.module) else {
        panic!("module root expected");
    };
    let FirMatch::Block(struct_items) = match_fir(&out.store, dsp_struct) else {
        panic!("dsp_struct block expected");
    };

    // A carry struct field for the recursive feedback (Delay1(Proj(0, SYMREF)))
    // must be present after Phase B6 support.
    let has_bra_carry = struct_items.iter().any(|id| {
        matches!(
            match_fir(&out.store, *id),
            FirMatch::DeclareVar { ref name, .. } if name.starts_with("fBraCarry")
        )
    });
    assert!(
        has_bra_carry,
        "recursive BRA (one-pole) should produce a fBraCarry* struct field \
         for the TBPTT feedback carry"
    );
}
/// Regression test: BRA body containing a `FloatCast(int_rec)` node (e.g.
/// `no.noise`'s integer LCG accumulator cast to float) must compile without
/// emitting invalid `BinOp(Float32, Int32)` FIR nodes in the backward sweep.
///
/// Before this fix, `propagate_bra_adj` propagated the Float32 adjoint into
/// integer-typed subtrees via the `FloatCast(x)` identity rule, causing the
/// FIR checker to flag `[FRS-FIR-0001] BinOp operands have incompatible types:
/// Float32 vs Int32` in `compute`.
///
/// The fix: stop gradient propagation at `FloatCast(x)` when `x` is
/// integer-typed (non-differentiable integer-to-float cast).
///
/// Circuit: `bra(FloatCast(int_rec) * seed, seed)` — the integer LCG
/// recursion `Proj(0, SYMREC(v, Add(Mul(Delay1(Proj(0, SYMREF(v))), 1103515245), 12345)))`
/// cast to float, then multiplied by a float seed.
#[test]
fn bra_body_with_integer_float_cast_compiles() {
    let mut arena = TreeArena::new();

    // Build an integer LCG recursion: iRec = 1103515245 * iRec + 12345
    // Proj(0, SYMREC(v, Add(Mul(Delay1(Proj(0, SYMREF(v))), Int(1103515245)), Int(12345))))
    let self_ref = de_bruijn_ref(&mut arena, 1);
    let proj_ref = SigBuilder::new(&mut arena).proj(0, self_ref);
    let delayed = SigBuilder::new(&mut arena).delay1(proj_ref);
    let mult_const = SigBuilder::new(&mut arena).int(1103515245);
    let add_const = SigBuilder::new(&mut arena).int(12345);
    let lcg_mul = SigBuilder::new(&mut arena).binop(BinOp::Mul, delayed, mult_const);
    let lcg_body = SigBuilder::new(&mut arena).binop(BinOp::Add, lcg_mul, add_const);
    let lcg_body_list = arena.cons(lcg_body, arena.nil());
    let lcg_rec = de_bruijn_rec(&mut arena, lcg_body_list);
    let lcg_proj = SigBuilder::new(&mut arena).proj(0, lcg_rec); // Int32 output

    // Cast the integer LCG to float (mimics `no.noise`'s `float` primitive)
    let lcg_float = SigBuilder::new(&mut arena).float_cast(lcg_proj); // Float32

    // Multiply by a float seed: body = FloatCast(int_rec) * seed
    let seed = SigBuilder::new(&mut arena).real(0.5);
    let body_expr = SigBuilder::new(&mut arena).binop(BinOp::Mul, lcg_float, seed);

    // BRA: differentiate body w.r.t. seed
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body_expr],
        &[seed],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    // Before the fix this crashed with:
    //   [FRS-FIR-0001] BinOp operands have incompatible types: Float32 vs Int32
    // because the backward sweep propagated Float32 gradient into the Int32 LCG body.
    compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
        .expect("BRA body with FloatCast(int_rec) must compile without BinOp type mismatch");
}

#[test]
fn block_reverse_ad_program_is_rejected_under_one_sample() {
    // Execution-options port D2: `-os` has no meaning for block-scoped
    // reverse-AD carriers; the pipeline rejects them with FRS-SFIR-0010
    // before lowering (plan §3.5).
    use crate::signal_fir::ProcessingApi;

    let mut arena = TreeArena::new();
    let x = SigBuilder::new(&mut arena).real(2.0);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body = SigBuilder::new(&mut arena).binop(BinOp::Mul, two, x);
    let cot = SigBuilder::new(&mut arena).real(1.0);
    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body],
        &[x],
        &[cot],
        BlockRevPolicy::TapeFull,
    );
    let primal = SigBuilder::new(&mut arena).proj(0, carrier);
    let grad = SigBuilder::new(&mut arena).proj(1, carrier);

    let options = SignalFirOptions {
        processing_api: ProcessingApi::OneSample,
        ..SignalFirOptions::default()
    };
    let err = compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &options)
        .expect_err("BRA program must be rejected under -os");
    assert_eq!(err.code().as_str(), "FRS-SFIR-0010");

    // The same program stays accepted with the default block API.
    compile_fastlane_without_ui(&arena, &[primal, grad], 0, 2, &SignalFirOptions::default())
        .expect("BRA program still compiles in block mode");
}
