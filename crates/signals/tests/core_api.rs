//! Integration tests for `core_api`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use signals::{
    BinOp, BlockRevPolicy, SigBuilder, SigMatch, add_sig_fir, add_sig_iir, concerned_iir,
    convert_fir_to_sig, delay_sig_fir, delay_sig_iir, div_sig_iir, dump_sig, dump_sig_annotated,
    dump_sig_readable, embedded_iir, make_sig_fir, match_sig, mul_sig_iir, neg_sig_fir,
    proj_to_sig_iir, simplify_fir, sub_sig_fir, sub_sig_iir,
};
use tlib::{TreeArena, list_to_vec};

#[test]
fn builder_and_match_cover_core_signal_shapes() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);

    let i0 = b.int(0);
    let i1 = b.int(1);
    let r = b.real(0.25);
    let inp = b.input(3);
    let out = b.output(2, inp);
    let d1 = b.delay1(inp);
    let d = b.delay(inp, i1);
    let p = b.prefix(i0, inp);
    let add = b.add(inp, i1);
    let pow = b.pow(inp, i1);
    let min = b.min(inp, i1);
    let max = b.max(inp, i1);
    let acos = b.acos(inp);
    let asin = b.asin(inp);
    let atan = b.atan(inp);
    let atan2 = b.atan2(inp, i1);
    let cos = b.cos(inp);
    let sin = b.sin(inp);
    let tan = b.tan(inp);
    let exp = b.exp(inp);
    let exp10 = b.exp10(inp);
    let log = b.log(inp);
    let log10 = b.log10(inp);
    let sqrt = b.sqrt(inp);
    let abs = b.abs(inp);
    let fmod = b.fmod(inp, i1);
    let remainder = b.remainder(inp, i1);
    let floor = b.floor(inp);
    let ceil = b.ceil(inp);
    let rint = b.rint(inp);
    let round = b.round(inp);
    let cast_i = b.int_cast(r);
    let cast_f = b.float_cast(i1);

    assert_eq!(match_sig(&arena, i1), SigMatch::Int(1));
    assert!(matches!(match_sig(&arena, r), SigMatch::Real(_)));
    assert_eq!(match_sig(&arena, inp), SigMatch::Input(3));
    assert_eq!(match_sig(&arena, out), SigMatch::Output(2, inp));
    assert_eq!(match_sig(&arena, d1), SigMatch::Delay1(inp));
    assert_eq!(match_sig(&arena, d), SigMatch::Delay(inp, i1));
    assert_eq!(match_sig(&arena, p), SigMatch::Prefix(i0, inp));
    assert_eq!(match_sig(&arena, add), SigMatch::BinOp(BinOp::Add, inp, i1));
    assert_eq!(match_sig(&arena, pow), SigMatch::Pow(inp, i1));
    assert_eq!(match_sig(&arena, min), SigMatch::Min(inp, i1));
    assert_eq!(match_sig(&arena, max), SigMatch::Max(inp, i1));
    assert_eq!(match_sig(&arena, acos), SigMatch::Acos(inp));
    assert_eq!(match_sig(&arena, asin), SigMatch::Asin(inp));
    assert_eq!(match_sig(&arena, atan), SigMatch::Atan(inp));
    assert_eq!(match_sig(&arena, atan2), SigMatch::Atan2(inp, i1));
    assert_eq!(match_sig(&arena, cos), SigMatch::Cos(inp));
    assert_eq!(match_sig(&arena, sin), SigMatch::Sin(inp));
    assert_eq!(match_sig(&arena, tan), SigMatch::Tan(inp));
    assert_eq!(match_sig(&arena, exp), SigMatch::Exp(inp));
    assert_eq!(match_sig(&arena, exp10), SigMatch::Exp10(inp));
    assert_eq!(match_sig(&arena, log), SigMatch::Log(inp));
    assert_eq!(match_sig(&arena, log10), SigMatch::Log10(inp));
    assert_eq!(match_sig(&arena, sqrt), SigMatch::Sqrt(inp));
    assert_eq!(match_sig(&arena, abs), SigMatch::Abs(inp));
    assert_eq!(match_sig(&arena, fmod), SigMatch::Fmod(inp, i1));
    assert_eq!(match_sig(&arena, remainder), SigMatch::Remainder(inp, i1));
    assert_eq!(match_sig(&arena, floor), SigMatch::Floor(inp));
    assert_eq!(match_sig(&arena, ceil), SigMatch::Ceil(inp));
    assert_eq!(match_sig(&arena, rint), SigMatch::Rint(inp));
    assert_eq!(match_sig(&arena, round), SigMatch::Round(inp));
    assert_eq!(match_sig(&arena, cast_i), SigMatch::IntCast(r));
    assert!(matches!(match_sig(&arena, cast_f), SigMatch::Real(_)));
}

#[test]
fn table_wrappers_and_select3_follow_cpp_shape() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let size = b.int(64);
    let init = b.int(7);
    let widx = b.int(4);
    let wsig = b.int(9);
    let ridx = b.int(5);

    let gen_init = b.generate(init);
    let wr = b.wrtbl(size, gen_init, widx, wsig);
    let ro = b.read_only_table(size, init, ridx);
    let rw = b.write_read_table(size, init, widx, wsig, ridx);
    let sidx = b.int(0);
    let s1 = b.int(10);
    let s2 = b.int(11);
    let s3 = b.int(12);
    let sel3 = b.select3(sidx, s1, s2, s3);

    assert!(matches!(match_sig(&arena, wr), SigMatch::WrTbl(_, _, _, _)));
    assert!(matches!(match_sig(&arena, ro), SigMatch::RdTbl(_, _)));
    assert!(matches!(match_sig(&arena, rw), SigMatch::RdTbl(_, _)));
    assert!(matches!(
        match_sig(&arena, sel3),
        SigMatch::Select2(_, _, _)
    ));
}

#[test]
fn ui_shapes_decode_with_control_ids() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let sig = b.input(0);
    let slider = 7;
    let bargraph = 8;
    let soundfile = 9;

    let vs = b.vslider(slider);
    let hs = b.hslider(slider);
    let ne = b.numentry(slider);
    let vb = b.vbargraph(bargraph, sig);
    let hb = b.hbargraph(bargraph, sig);
    let sf = b.soundfile(soundfile);
    let sf_part = b.int(2);
    let sf_chan = b.int(1);
    let sf_ridx = b.input(3);
    let sf_len = b.soundfile_length(sf, sf_part);
    let sf_rate = b.soundfile_rate(sf, sf_part);
    let sf_buf = b.soundfile_buffer(sf, sf_chan, sf_part, sf_ridx);

    assert_eq!(match_sig(&arena, vs), SigMatch::VSlider(slider));
    assert_eq!(match_sig(&arena, hs), SigMatch::HSlider(slider));
    assert_eq!(match_sig(&arena, ne), SigMatch::NumEntry(slider));
    assert_eq!(match_sig(&arena, vb), SigMatch::VBargraph(bargraph, sig));
    assert_eq!(match_sig(&arena, hb), SigMatch::HBargraph(bargraph, sig));
    assert_eq!(match_sig(&arena, sf), SigMatch::Soundfile(soundfile));
    assert_eq!(
        match_sig(&arena, sf_len),
        SigMatch::SoundfileLength(sf, sf_part)
    );
    assert_eq!(
        match_sig(&arena, sf_rate),
        SigMatch::SoundfileRate(sf, sf_part)
    );
    assert_eq!(
        match_sig(&arena, sf_buf),
        SigMatch::SoundfileBuffer(sf, sf_chan, sf_part, sf_ridx)
    );
}

#[test]
fn stream_wrappers_and_recursion_shapes_are_stable() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let y = b.input(1);

    let od = b.on_demand(&[x, y]);
    let us = b.upsampling(&[x, y]);
    let ds = b.downsampling(&[x, y]);
    let tv = b.temp_var(x);
    let pv = b.perm_var(y);
    let clk = b.int(7);
    let clocked = b.clocked(clk, x);
    let double_clocked = b.double_clocked(clk, y, x);
    let rec = b.rec(x);
    let reverse_rec = b.reverse_time_rec(y);
    let proj = b.proj(2, rec);
    let c0 = b.real(0.5);
    let c1 = b.real(-0.25);
    let fir = b.fir(&[x, c0, c1]);
    let iir = b.iir(&[proj, x, c0, c1]);
    let seq = b.seq(x, y);
    let zp = b.zero_pad(x, y);

    assert!(matches!(match_sig(&arena, od), SigMatch::OnDemand(v) if v == [x, y]));
    assert!(matches!(match_sig(&arena, us), SigMatch::Upsampling(v) if v == [x, y]));
    assert!(matches!(match_sig(&arena, ds), SigMatch::Downsampling(v) if v == [x, y]));
    assert_eq!(match_sig(&arena, tv), SigMatch::TempVar(x));
    assert_eq!(match_sig(&arena, pv), SigMatch::PermVar(y));
    assert_eq!(match_sig(&arena, clocked), SigMatch::Clocked(clk, x));
    let SigMatch::Clocked(inner_clk, nested) = match_sig(&arena, double_clocked) else {
        panic!("double_clocked should decode as outer clocked");
    };
    assert_eq!(inner_clk, clk);
    assert_eq!(match_sig(&arena, nested), SigMatch::Clocked(y, x));
    assert_eq!(match_sig(&arena, rec), SigMatch::Rec(x));
    assert_eq!(match_sig(&arena, reverse_rec), SigMatch::ReverseTimeRec(y));
    assert_eq!(match_sig(&arena, proj), SigMatch::Proj(2, rec));
    assert!(matches!(match_sig(&arena, fir), SigMatch::Fir(v) if v == [x, c0, c1]));
    assert!(matches!(match_sig(&arena, iir), SigMatch::Iir(v) if v == [proj, x, c0, c1]));
    assert_eq!(match_sig(&arena, seq), SigMatch::Seq(x, y));
    assert_eq!(match_sig(&arena, zp), SigMatch::ZeroPad(x, y));
}

#[test]
fn block_reverse_ad_carrier_round_trips() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);

    let x = b.input(0);
    let y = b.input(1);
    let z = b.input(2);
    let primal0 = b.add(x, y);
    let primal1 = b.mul(x, z);
    let one_a = b.real(1.0);
    let one_b = b.real(1.0);

    let carrier = b.block_reverse_ad(
        &[primal0, primal1],
        &[x, y],
        &[one_a, one_b],
        BlockRevPolicy::TapeFull,
    );

    let SigMatch::BlockReverseAD {
        body,
        primal_count,
        seeds,
        cotangents,
        policy,
    } = match_sig(&arena, carrier)
    else {
        panic!("BlockReverseAD carrier should decode through SigMatch");
    };
    assert_eq!(primal_count, 2);
    assert_eq!(policy, BlockRevPolicy::TapeFull);

    let body_vec = list_to_vec(&arena, body).expect("body must be a well-formed cons list");
    assert_eq!(body_vec, vec![primal0, primal1]);
    let seed_vec = list_to_vec(&arena, seeds).expect("seed list must be well-formed");
    assert_eq!(seed_vec, vec![x, y]);
    let cot_vec = list_to_vec(&arena, cotangents).expect("cotangent list must be well-formed");
    assert_eq!(cot_vec, vec![one_a, one_b]);

    // Same children at the SigBuilder boundary deduplicate to the same SigId.
    let again = SigBuilder::new(&mut arena).block_reverse_ad(
        &[primal0, primal1],
        &[x, y],
        &[one_a, one_b],
        BlockRevPolicy::TapeFull,
    );
    assert_eq!(carrier, again);
}

#[test]
fn block_reverse_ad_policy_round_trip() {
    for policy in [
        BlockRevPolicy::TapeFull,
        BlockRevPolicy::Checkpointed,
        BlockRevPolicy::Recompute,
    ] {
        assert_eq!(BlockRevPolicy::from_raw(policy.to_raw()), Some(policy));
    }
    assert_eq!(BlockRevPolicy::from_raw(-1), None);
    assert_eq!(BlockRevPolicy::from_raw(99), None);
}

#[test]
fn block_reverse_ad_distinct_policies_intern_distinct_carriers() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let one = b.real(1.0);

    let tape = b.block_reverse_ad(&[x], &[x], &[one], BlockRevPolicy::TapeFull);
    let chk = b.block_reverse_ad(&[x], &[x], &[one], BlockRevPolicy::Checkpointed);
    assert_ne!(
        tape, chk,
        "different policies must hash to different interned carriers"
    );
}

#[test]
fn fir_iir_carriers_preserve_cpp_branch_layout() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let c0 = b.int(1);
    let c1 = b.real(0.5);
    let c2 = b.real(-0.25);
    let rec = b.rec(x);
    let state = b.proj(0, rec);

    let fir = b.fir(&[x, c0, c1, c2]);
    let iir = b.iir(&[state, x, c1, c2]);

    let SigMatch::Fir(fir_coefs) = match_sig(&arena, fir) else {
        panic!("SIGFIR should decode as Fir");
    };
    assert_eq!(fir_coefs, [x, c0, c1, c2]);

    let SigMatch::Iir(iir_coefs) = match_sig(&arena, iir) else {
        panic!("SIGIIR should decode as Iir");
    };
    assert_eq!(iir_coefs, [state, x, c1, c2]);

    assert_eq!(
        dump_sig(&arena, fir),
        "SIGFIR(SIGINPUT(int(0)), int(1), float_bits(0x3fe0000000000000), float_bits(0xbfd0000000000000))"
    );
    assert!(dump_sig(&arena, iir).starts_with("SIGIIR(SIGPROJ(int(0), SIGREC("));
}

#[test]
fn fir_helpers_make_and_delay_constant_taps() {
    let mut arena = TreeArena::new();
    let x = SigBuilder::new(&mut arena).input(0);

    let fir = make_sig_fir(&mut arena, x, 2);
    let SigMatch::Fir(coefs) = match_sig(&arena, fir) else {
        panic!("make_sig_fir should create a FIR carrier");
    };
    assert_eq!(coefs.len(), 4);
    assert_eq!(coefs[0], x);
    assert_eq!(match_sig(&arena, coefs[1]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, coefs[2]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, coefs[3]), SigMatch::Int(1));

    let one = SigBuilder::new(&mut arena).int(1);
    let delayed = delay_sig_fir(&mut arena, fir, one);
    let SigMatch::Fir(delayed_coefs) = match_sig(&arena, delayed) else {
        panic!("delay_sig_fir should preserve FIR carrier");
    };
    assert_eq!(delayed_coefs.len(), 5);
    assert_eq!(match_sig(&arena, delayed_coefs[1]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, delayed_coefs[4]), SigMatch::Int(1));
}

#[test]
fn fir_helpers_add_sub_neg_and_simplify_same_base_filters() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let one = b.int(1);
    let half = b.real(0.5);
    let quarter = b.real(0.25);
    let zero = b.int(0);
    let fir_a = b.fir(&[x, one, half, zero]);
    let fir_b = b.fir(&[x, quarter, one]);

    let simplified = simplify_fir(&mut arena, fir_a);
    let SigMatch::Fir(simplified_coefs) = match_sig(&arena, simplified) else {
        panic!("simplify_fir should keep a real FIR");
    };
    assert_eq!(simplified_coefs.len(), 3);

    let neg = neg_sig_fir(&mut arena, fir_b);
    let SigMatch::Fir(neg_coefs) = match_sig(&arena, neg) else {
        panic!("neg_sig_fir should keep FIR shape");
    };
    assert_eq!(neg_coefs[0], x);
    assert!(matches!(match_sig(&arena, neg_coefs[1]), SigMatch::Real(v) if v == -0.25));

    let added = add_sig_fir(&mut arena, simplified, fir_b);
    let SigMatch::Fir(added_coefs) = match_sig(&arena, added) else {
        panic!("add_sig_fir should combine same-base FIRs");
    };
    assert_eq!(added_coefs[0], x);
    assert!(matches!(
        match_sig(&arena, added_coefs[1]),
        SigMatch::BinOp(BinOp::Add, _, _)
    ));
    assert!(matches!(
        match_sig(&arena, added_coefs[2]),
        SigMatch::BinOp(BinOp::Add, _, _)
    ));

    let subbed = sub_sig_fir(&mut arena, added, fir_b);
    let SigMatch::Fir(subbed_coefs) = match_sig(&arena, subbed) else {
        panic!("sub_sig_fir should keep same-base FIRs");
    };
    assert_eq!(subbed_coefs[0], x);
}

#[test]
fn fir_helper_expands_back_to_delay_terms() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let one = b.int(1);
    let half = b.real(0.5);
    let fir = b.fir(&[x, one, half]);

    let expanded = convert_fir_to_sig(&mut arena, fir);
    let SigMatch::BinOp(BinOp::Add, direct, delayed_term) = match_sig(&arena, expanded) else {
        panic!("expanded FIR should be an add tree");
    };
    assert_eq!(direct, x);
    let SigMatch::BinOp(BinOp::Mul, coef, delayed) = match_sig(&arena, delayed_term) else {
        panic!("second term should multiply coefficient and delayed input");
    };
    assert_eq!(coef, half);
    assert!(matches!(match_sig(&arena, delayed), SigMatch::Delay(_, amount) if amount == one));
}

#[test]
fn iir_helpers_create_identity_and_shift_constant_delays() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let other = b.input(1);
    let rec_body = b.add(x, other);
    let rec = b.rec(rec_body);
    let rt = b.proj(0, rec);
    let other_proj = b.proj(1, rec);

    let identity = proj_to_sig_iir(&mut arena, rt, rt);
    let Some(identity_coefs) = concerned_iir(&arena, rt, identity) else {
        panic!("identity should be concerned by rt");
    };
    assert_eq!(identity_coefs[0], rt);
    assert_eq!(match_sig(&arena, identity_coefs[1]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, identity_coefs[2]), SigMatch::Int(1));

    let nil = arena.nil();
    assert_eq!(proj_to_sig_iir(&mut arena, rt, other_proj), nil);

    let two = SigBuilder::new(&mut arena).int(2);
    let delayed = delay_sig_iir(&mut arena, rt, identity, two);
    let Some(delayed_coefs) = concerned_iir(&arena, rt, delayed) else {
        panic!("delayed IIR should stay concerned");
    };
    assert_eq!(delayed_coefs[0], rt);
    assert_eq!(match_sig(&arena, delayed_coefs[1]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, delayed_coefs[2]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, delayed_coefs[3]), SigMatch::Int(0));
    assert_eq!(match_sig(&arena, delayed_coefs[4]), SigMatch::Int(1));
}

#[test]
fn iir_helpers_add_sub_scale_and_reject_nonlinear_products() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let rec = b.rec(x);
    let rt = b.proj(0, rec);
    let p = b.real(0.5);
    let q = b.real(0.25);
    let iir_a = b.iir(&[rt, x, p]);
    let iir_b = b.iir(&[rt, q, q]);
    let factor = b.int(3);

    let added = add_sig_iir(&mut arena, rt, iir_a, iir_b);
    let Some(added_coefs) = concerned_iir(&arena, rt, added) else {
        panic!("IIR addition should stay concerned");
    };
    assert!(matches!(
        match_sig(&arena, added_coefs[1]),
        SigMatch::BinOp(BinOp::Add, _, _)
    ));
    assert!(matches!(
        match_sig(&arena, added_coefs[2]),
        SigMatch::BinOp(BinOp::Add, _, _)
    ));

    let subbed = sub_sig_iir(&mut arena, rt, added, iir_b);
    let Some(subbed_coefs) = concerned_iir(&arena, rt, subbed) else {
        panic!("IIR subtraction should stay concerned");
    };
    assert_eq!(subbed_coefs[0], rt);

    let scaled = mul_sig_iir(&mut arena, rt, iir_a, factor);
    let Some(scaled_coefs) = concerned_iir(&arena, rt, scaled) else {
        panic!("IIR scaling should stay concerned");
    };
    assert!(matches!(
        match_sig(&arena, scaled_coefs[1]),
        SigMatch::BinOp(BinOp::Mul, _, _)
    ));
    assert!(matches!(
        match_sig(&arena, scaled_coefs[2]),
        SigMatch::BinOp(BinOp::Mul, _, _)
    ));

    let divided = div_sig_iir(&mut arena, rt, scaled, factor);
    let Some(divided_coefs) = concerned_iir(&arena, rt, divided) else {
        panic!("IIR division by independent term should stay concerned");
    };
    assert!(matches!(
        match_sig(&arena, divided_coefs[1]),
        SigMatch::BinOp(BinOp::Div, _, _)
    ));

    let nil = arena.nil();
    assert_eq!(mul_sig_iir(&mut arena, rt, iir_a, iir_b), nil);
    assert_eq!(div_sig_iir(&mut arena, rt, x, iir_b), nil);
}

#[test]
fn iir_helper_embeds_fir_applied_to_concerned_iir() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let rec = b.rec(x);
    let rt = b.proj(0, rec);
    let p = b.real(0.5);
    let one = b.int(1);
    let iir = b.iir(&[rt, x, p]);
    let fir_on_iir = b.fir(&[iir, one, p]);

    let embedded = embedded_iir(&mut arena, rt, fir_on_iir);
    let Some(embedded_coefs) = concerned_iir(&arena, rt, embedded) else {
        panic!("embedded IIR should stay concerned");
    };
    assert_eq!(embedded_coefs[0], rt);
    assert!(embedded_coefs.len() >= 3);
}

#[test]
fn dump_is_deterministic() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let btn = b.button(1);
    let inp = b.input(0);
    let d1 = b.delay1(inp);
    let sig = b.attach(btn, d1);
    let a = dump_sig(&arena, sig);
    let c = dump_sig(&arena, sig);
    assert_eq!(a, c);
}

#[test]
fn dump_readable_prints_binop_opcode_name() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let x = b.input(0);
    let y = b.input(1);
    let add = b.add(x, y);
    let got = dump_sig_readable(&arena, add);
    assert!(got.contains("SIGBINOP(op=add (+),"));
}

#[test]
fn dump_readable_handles_deep_left_spines_without_recursing() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let mut sig = b.input(0);
    for _ in 0..20_000 {
        let zero = b.int(0);
        sig = b.add(sig, zero);
    }

    let got = dump_sig_readable(&arena, sig);
    assert!(got.starts_with("SIGBINOP(op=add (+),"));
    assert!(got.contains("SIGINPUT(int(0))"));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "SIGINPUT index must be non-negative")]
fn builder_rejects_negative_input_index_in_debug() {
    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let _ = b.input(-1);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "SIGOUTPUT index must be non-negative")]
fn builder_rejects_negative_output_index_in_debug() {
    let mut arena = TreeArena::new();
    let input = {
        let mut b = SigBuilder::new(&mut arena);
        b.input(0)
    };
    let mut b = SigBuilder::new(&mut arena);
    let _ = b.output(-1, input);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "SIGPROJ index must be non-negative")]
fn builder_rejects_negative_proj_index_in_debug() {
    let mut arena = TreeArena::new();
    let input = {
        let mut b = SigBuilder::new(&mut arena);
        b.input(0)
    };
    let rec = {
        let mut b = SigBuilder::new(&mut arena);
        b.rec(input)
    };
    let mut b = SigBuilder::new(&mut arena);
    let _ = b.proj(-1, rec);
}

/// `dump_sig_annotated` resolves the declared range of slider-like controls,
/// which the plain dumps cannot: a signal node stores only a `ControlId`.
#[test]
fn annotated_dump_resolves_control_ranges() {
    use ui::{ControlKind, ControlRange, ControlSpec, UiProgram};

    let mut program = UiProgram::empty();
    program.controls.push(ControlSpec::synthetic(
        0,
        ControlKind::HSlider,
        "cutoff".to_owned(),
        Vec::new(),
        Some(ControlRange {
            init: 440.0,
            min: 20.0,
            max: 20000.0,
            step: 0.5,
        }),
    ));
    // A control without a range must be left exactly as the plain dump has it.
    program.controls.push(ControlSpec::synthetic(
        1,
        ControlKind::Button,
        "gate".to_owned(),
        Vec::new(),
        None,
    ));

    let mut arena = TreeArena::new();
    let mut b = SigBuilder::new(&mut arena);
    let slider = b.hslider(0);
    let button = b.button(1);
    let sig = b.add(slider, button);

    assert_eq!(
        dump_sig_annotated(&arena, sig, &program),
        "SIGBINOP(op=add (+), \
         SIGHSLIDER(int(0), init=440.0, min=20.0, max=20000.0, step=0.5), \
         SIGBUTTON(int(1)))"
    );
    // The unannotated dumps stay byte-identical: they are the reference for
    // structural differential checks and are called from crates that own no
    // `UiProgram`.
    assert_eq!(
        dump_sig_readable(&arena, sig),
        "SIGBINOP(op=add (+), SIGHSLIDER(int(0)), SIGBUTTON(int(1)))"
    );
}
