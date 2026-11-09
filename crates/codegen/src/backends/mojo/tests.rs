//! Unit tests of the Mojo emitter.
//!
//! The contract checked here is the one of
//! `porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md` §2; the
//! numeric behaviour is checked by the opt-in `make mojo` impulse lane.

use super::{
    CodegenErrorCode, EmitMode, Emitter, MojoOptions, MojoRealType, Renamer, generate_mojo_module,
    snake_case, wrap_compile_options, write_method,
};
use crate::fixtures::{
    build_control_flow_test_module, build_gain_bias_ui_meta_test_module,
    build_heavy_bench_test_module, build_ir_coverage_test_module,
    build_math_intrinsics_test_module, build_passthrough_test_module,
    build_sine_phasor_test_module, build_table_state_delay_test_module,
};
use fir::{AccessType, FirBinOp, FirBuilder, FirStore, FirType};

fn emit(store: &FirStore, id: fir::FirId) -> String {
    let options = MojoOptions::default();
    let mut emitter = Emitter::new(store, &options, None);
    emitter.scopes.push(std::collections::HashMap::new());
    emitter.emit_value(id).expect("value should emit")
}

#[test]
fn snake_case_follows_the_cpp_rename() {
    let cases = [
        ("fRec0", "rec0"),
        ("iRec1", "i_rec1"),
        ("fSampleRate", "sample_rate"),
        ("fHslider0", "hslider0"),
        ("fRecCur0", "rec_cur0"),
        ("ftbl0mydspSIG0", "ftbl0_mydsp_sig0"),
        ("mydspSIG0_iRec0", "mydsp_sig0_i_rec0"),
        ("__fir_inl0_i0", "__fir_inl0_i0"),
        ("fbargraph0", "fbargraph0"),
        ("i0", "i0"),
        ("count", "count"),
    ];
    for (fir, mojo) in cases {
        assert_eq!(snake_case(fir), mojo, "{fir}");
    }
}

#[test]
fn a_rename_collision_is_rejected_instead_of_merging_two_variables() {
    let mut names = Renamer::new(true);
    assert_eq!(names.get("fA_b").expect("first name"), "a_b");
    let err = names.get("fA_B").expect_err("same identifier");
    assert_eq!(err.code(), CodegenErrorCode::NameCollision);
    assert!(err.to_string().contains("FRS-CGEN-MOJO-0004"), "{err}");

    let mut locals = Renamer::new(true);
    locals.reserve("count", "count");
    let err = locals.get("fCount").expect_err("a local onto a parameter");
    assert_eq!(err.code(), CodegenErrorCode::NameCollision);
}

#[test]
fn keywords_and_called_names_are_escaped() {
    let mut fields = Renamer::new(false);
    assert_eq!(fields.get("fIn").expect("keyword field"), "in_");
    assert_eq!(
        fields.get("fMin").expect("a field may be called min"),
        "min"
    );
    let mut locals = Renamer::new(true);
    assert_eq!(locals.get("fMin").expect("local"), "min_");
    assert_eq!(locals.get("fPass").expect("local"), "pass_");
}

#[test]
fn module_has_the_architecture_contract_shape() {
    let (store, module) = build_sine_phasor_test_module();
    let out = generate_mojo_module(&store, module, &MojoOptions::default())
        .expect("mojo module generation should succeed");

    assert!(out.starts_with("# ======"), "{out}");
    assert!(
        !out.contains("import "),
        "the architecture supplies every import"
    );
    assert!(
        out.contains("@fieldwise_init\nstruct mydsp(FaustDsp):\n    var null_val: FaustFloat\n")
    );
    assert!(out.contains("    def __init__(out dsp):\n        dsp.null_val = FaustFloat(0.0)\n"));
    for signature in [
        "def get_sample_rate(imm dsp) -> S32:",
        "def get_num_outputs(imm dsp) -> S32:",
        "def get_num_inputs(imm dsp) -> S32:",
        "def class_init(mut dsp, imm sample_rate: S32) -> None:",
        "def instance_constants(mut dsp, imm sample_rate: S32) -> None:",
        "def instance_reset_user_interface(mut dsp) -> None:",
        "def instance_clear(mut dsp) -> None:",
        "def instance_init(mut dsp, imm sample_rate: S32) -> None:",
        "def init(mut dsp, imm sample_rate: S32) -> None:",
        "def get_json(imm dsp) -> String:",
        "def metadata(imm dsp, mut meta: Some[FaustMeta]) -> None:",
        "def build_user_interface(mut dsp, mut ui: Some[FaustGui]) -> None:",
        "def compute(mut dsp, var count: S32, var inputs: ImmStreams, var outputs: MutStreams) -> None:",
    ] {
        assert!(
            out.contains(&format!("    @always_inline\n    {signature}\n"))
                || out.contains(&format!(
                    "    @always_inline\n    {}\n        ref dsp = self\n",
                    aliased(signature)
                )),
            "missing {signature}\n{out}"
        );
    }
    assert!(out.contains(
        "        dsp.instance_constants(sample_rate)\n        dsp.instance_reset_user_interface()\n        dsp.instance_clear()\n"
    ));
    assert!(
        out.contains(
            "        dsp.class_init(sample_rate)\n        dsp.instance_init(sample_rate)\n"
        )
    );
    assert!(out.contains("for var i0 in range(S32(0), count):"), "{out}");
    assert!(out.contains("output0[unsafe_offset = i0] = "), "{out}");
    assert!(
        out.contains("ui.add_horizontal_slider(\"freq\", dsp.freq, FaustFloat("),
        "{out}"
    );
}

#[test]
fn every_fixture_emits() {
    for (name, build) in [
        (
            "sine_phasor",
            build_sine_phasor_test_module as fn() -> (FirStore, fir::FirId),
        ),
        ("passthrough", build_passthrough_test_module),
        ("gain_bias_ui_meta", build_gain_bias_ui_meta_test_module),
        ("table_state_delay", build_table_state_delay_test_module),
        ("control_flow", build_control_flow_test_module),
        ("math_intrinsics", build_math_intrinsics_test_module),
        ("heavy_bench", build_heavy_bench_test_module),
    ] {
        let (store, module) = build();
        let out = generate_mojo_module(&store, module, &MojoOptions::default())
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        assert!(!out.contains('\t'), "{name}: Mojo indents with spaces");
        // A compound statement is never left without a body.
        let lines: Vec<&str> = out.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_end().ends_with(':') && !line.trim_start().starts_with('#') {
                let indent = line.len() - line.trim_start().len();
                let next = lines.get(i + 1).copied().unwrap_or("");
                let next_indent = next.len() - next.trim_start().len();
                assert!(
                    next_indent > indent,
                    "{name}: empty body after `{line}`\n{out}"
                );
            }
        }
    }
}

#[test]
fn literals_are_typed_in_both_precisions() {
    let mut store = FirStore::new();
    let (f32_lit, f64_lit, int_lit, nan, neg_inf) = {
        let mut b = FirBuilder::new(&mut store);
        (
            b.float32(0.3),
            b.float64(0.5),
            b.int32(-7),
            b.float64(f64::NAN),
            b.float32(f32::NEG_INFINITY),
        )
    };
    assert_eq!(emit(&store, f32_lit), "F32(0.3)");
    assert_eq!(emit(&store, f64_lit), "F64(0.5)");
    assert_eq!(emit(&store, int_lit), "S32(-7)");
    assert_eq!(emit(&store, nan), "nan[f64]()");
    assert_eq!(emit(&store, neg_inf), "-inf[f32]()");
}

#[test]
fn a_mixed_operand_is_converted_explicitly() {
    let mut store = FirStore::new();
    let sum = {
        let mut b = FirBuilder::new(&mut store);
        let i = b.load_var("iTemp0", AccessType::Stack, FirType::Int32);
        let x = b.load_var("fTemp1", AccessType::Stack, FirType::Float64);
        b.binop(FirBinOp::Add, i, x, FirType::Float64)
    };
    assert_eq!(emit(&store, sum), "(F64(i_temp0) + temp1)");
}

#[test]
fn an_integer_comparison_is_an_integer() {
    let mut store = FirStore::new();
    let (cmp, sel) = {
        let mut b = FirBuilder::new(&mut store);
        let i = b.load_var("iTemp0", AccessType::Stack, FirType::Int32);
        let zero = b.int32(0);
        let cmp = b.binop(FirBinOp::Lt, i, zero, FirType::Int32);
        let one = b.float64(1.0);
        let two = b.float64(2.0);
        let sel = b.select2(cmp, one, two, FirType::Float64);
        (cmp, sel)
    };
    assert_eq!(emit(&store, cmp), "S32((i_temp0 < S32(0)))");
    // As a condition, the comparison stays a `Bool`.
    assert_eq!(
        emit(&store, sel),
        "(F64(1.0) if (i_temp0 < S32(0)) else F64(2.0))"
    );
}

#[test]
fn c_semantics_go_through_helpers_written_only_when_used() {
    let mut store = FirStore::new();
    let (div, rem, fmod, round, rint) = {
        let mut b = FirBuilder::new(&mut store);
        let a = b.load_var("iTemp0", AccessType::Stack, FirType::Int32);
        let c = b.int32(3);
        let x = b.load_var("fTemp1", AccessType::Stack, FirType::Float64);
        let y = b.float64(2.0);
        (
            b.binop(FirBinOp::Div, a, c, FirType::Int32),
            b.binop(FirBinOp::Rem, a, c, FirType::Int32),
            b.fun_call("fmod", &[x, y], FirType::Float64),
            b.fun_call("roundf", &[x], FirType::Float64),
            b.fun_call("rint", &[x], FirType::Float64),
        )
    };
    let options = MojoOptions::default();
    let mut emitter = Emitter::new(&store, &options, None);
    emitter.scopes.push(std::collections::HashMap::new());
    assert_eq!(
        emitter.emit_value(div).unwrap(),
        "faust_idiv(i_temp0, S32(3))"
    );
    assert_eq!(
        emitter.emit_value(rem).unwrap(),
        "faust_irem(i_temp0, S32(3))"
    );
    assert_eq!(
        emitter.emit_value(fmod).unwrap(),
        "faust_fmod(temp1, F64(2.0))"
    );
    assert_eq!(emitter.emit_value(round).unwrap(), "faust_round(temp1)");
    // `std.math` has no `rint`; its `round` is C `rint`.
    assert_eq!(emitter.emit_value(rint).unwrap(), "round(temp1)");
    assert_eq!(emitter.helpers.len(), 4);

    let (store, module) = build_sine_phasor_test_module();
    let out = generate_mojo_module(&store, module, &MojoOptions::default()).unwrap();
    assert!(
        !out.contains("def faust_"),
        "no helper without a use\n{out}"
    );
}

#[test]
fn soundfiles_are_rejected() {
    let mut store = FirStore::new();
    let stmt = {
        let mut b = FirBuilder::new(&mut store);
        b.add_soundfile("snd", "fSoundfile0")
    };
    let options = MojoOptions::default();
    let mut emitter = Emitter::new(&store, &options, None);
    let mut out = String::new();
    let mut mode = EmitMode::Ui;
    let err = emitter
        .emit_stmt(&mut out, stmt, 2, &mut mode)
        .expect_err("soundfiles are not supported");
    assert_eq!(err.code(), CodegenErrorCode::UnsupportedNode);
    assert!(err.to_string().contains("soundfiles"), "{err}");
}

#[test]
fn double_precision_option_line_and_banner() {
    let (store, module) = build_sine_phasor_test_module();
    let options = MojoOptions {
        real_type: MojoRealType::Float64,
        metadata_name: Some("osc".to_owned()),
        metadata_entries: vec![("author".to_owned(), "Grame".to_owned())],
        ..MojoOptions::default()
    };
    let out = generate_mojo_module(&store, module, &options).unwrap();
    assert!(
        out.contains("# author: \"Grame\"\n# name: \"osc\"\n"),
        "{out}"
    );
    assert!(
        out.contains("# Compilation options: \n#   -lang mojo -double\n"),
        "{out}"
    );
    assert!(
        out.contains("meta.declare(\"compile_options\", \"-lang mojo -double\")"),
        "{out}"
    );
}

#[test]
fn compile_options_wrap_before_81_columns_keeping_option_values() {
    let wrapped = wrap_compile_options(
        "-lang mojo -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -double -ftz 0",
    );
    // As in C++, each unit is followed by a space, so a wrapped line keeps
    // its trailing one; only the last is removed.
    assert_eq!(
        wrapped,
        "#   -lang mojo -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -double \n#   -ftz 0"
    );
    assert!(wrapped.lines().all(|line| line.len() <= 81));
}

#[test]
fn buffer_iterators_are_refused_as_cpp_refuses_them() {
    // `DeclareBufferIterators` belongs to the Rust backend's iterator loops;
    // the signal lowering never produces it for a textual C-like target.
    let (store, module) = build_ir_coverage_test_module();
    let err = generate_mojo_module(&store, module, &MojoOptions::default())
        .expect_err("iterator loops are not a Mojo shape");
    assert_eq!(err.code(), CodegenErrorCode::UnsupportedNode);
}

#[test]
fn rejects_non_module_root() {
    let mut store = FirStore::new();
    let root = {
        let mut b = FirBuilder::new(&mut store);
        b.int32(1)
    };
    let err = generate_mojo_module(&store, root, &MojoOptions::default())
        .expect_err("non-module roots must fail");
    assert_eq!(err.code(), CodegenErrorCode::RootNotModule);
}

/// `signature` as written for a method whose body has a loop: the receiver
/// is `self`, aliased as `dsp` (see `write_method`).
fn aliased(signature: &str) -> String {
    signature
        .replacen("(mut dsp", "(mut self", 1)
        .replacen("(imm dsp", "(imm self", 1)
}

/// The body of one generated method, from its signature to the next method,
/// whichever receiver name the method takes.
fn method_body<'a>(out: &'a str, signature: &str) -> &'a str {
    let alias = aliased(signature);
    let (start, len) = out
        .find(signature)
        .map(|start| (start, signature.len()))
        .or_else(|| out.find(&alias).map(|start| (start, alias.len())))
        .unwrap_or_else(|| panic!("missing {signature}"));
    let rest = &out[start + len..];
    let end = rest.find("\n    @always_inline").unwrap_or(rest.len());
    &rest[..end]
}

/// `porting/backend-lifecycle-contract-en.md`, required before a backend
/// joins the impulse, golden or parity gates.
#[test]
fn lifecycle_conformance_matches_the_cpp_contract() {
    for (name, build) in [
        (
            "sine_phasor",
            build_sine_phasor_test_module as fn() -> (FirStore, fir::FirId),
        ),
        ("table_state_delay", build_table_state_delay_test_module),
        ("gain_bias_ui_meta", build_gain_bias_ui_meta_test_module),
    ] {
        let (store, module) = build();
        let out = generate_mojo_module(&store, module, &MojoOptions::default()).unwrap();

        // init = class_init -> instance_init.
        let init = method_body(&out, "def init(mut dsp, imm sample_rate: S32) -> None:");
        assert_eq!(
            init.trim(),
            "dsp.class_init(sample_rate)\n        dsp.instance_init(sample_rate)",
            "{name}"
        );

        // instance_init = instance_constants -> instance_reset_user_interface
        // -> instance_clear, and never class_init.
        let instance_init = method_body(
            &out,
            "def instance_init(mut dsp, imm sample_rate: S32) -> None:",
        );
        assert_eq!(
            instance_init.trim(),
            "dsp.instance_constants(sample_rate)\n        dsp.instance_reset_user_interface()\n        dsp.instance_clear()",
            "{name}"
        );
        assert!(!instance_init.contains("class_init"), "{name}");

        // compute does not repeat instance_clear's field clearing.
        let clear = method_body(&out, "def instance_clear(mut dsp) -> None:");
        let compute = method_body(
            &out,
            "def compute(mut dsp, var count: S32, var inputs: ImmStreams, var outputs: MutStreams) -> None:",
        );
        for line in clear
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("dsp."))
        {
            assert!(
                !compute.lines().any(|c| c.trim() == line),
                "{name}: compute repeats `{line}` of instance_clear"
            );
        }

        // The compiled instance_constants is the one emitted, not a stub.
        let constants = method_body(
            &out,
            "def instance_constants(mut dsp, imm sample_rate: S32) -> None:",
        );
        assert!(
            constants.contains("dsp.sample_rate = sample_rate"),
            "{name}: {constants}"
        );
    }
}

/// Nested counted loops keep the C++ `for ... in range` form, and a method
/// whose body has a loop reaches the fields through `ref dsp = self`: Mojo
/// 1.1.0.dev2026090705 never finishes compiling field accesses through the
/// receiver inside nested loops, and compiles them at once through a local
/// reference.
#[test]
fn methods_with_loops_alias_their_receiver() {
    let mut store = FirStore::new();
    let outer = {
        let mut b = FirBuilder::new(&mut store);
        let load = b.load_var("fRec0", AccessType::Struct, FirType::Float64);
        let store_rec = b.store_var("fRec0", AccessType::Struct, load);
        let inner_body = b.block(&[store_rec]);
        let two = b.int32(2);
        let inner = b.simple_for_loop("l0", two, inner_body, false);
        let reverse_body = b.block(&[]);
        let four = b.int32(4);
        let reverse = b.simple_for_loop("l1", four, reverse_body, true);
        let outer_body = b.block(&[inner, reverse]);
        let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        b.simple_for_loop("i0", count, outer_body, false)
    };
    let options = MojoOptions::default();
    let mut emitter = Emitter::new(&store, &options, None);
    emitter
        .field_types
        .insert("fRec0".to_owned(), FirType::Float64);
    emitter.scopes.push(std::collections::HashMap::new());
    let mut body = String::new();
    let mut mode = EmitMode::Default;
    emitter.emit_stmt(&mut body, outer, 2, &mut mode).unwrap();
    assert_eq!(
        body,
        "        for var i0 in range(S32(0), count):\n\
         \x20           for var l0 in range(S32(0), S32(2)):\n\
         \x20               dsp.rec0 = dsp.rec0\n\
         \x20           for var l1 in range((S32(4)) - S32(1), S32(-1), S32(-1)):\n\
         \x20               pass\n"
    );

    let signature = "def compute(mut dsp, var count: S32, var inputs: ImmStreams, var outputs: MutStreams) -> None:";
    let mut out = String::new();
    write_method(&mut out, signature, &body);
    assert_eq!(
        out,
        format!(
            "    @always_inline\n    {}\n        ref dsp = self\n{body}\n",
            aliased(signature)
        )
    );

    // A loop-free body keeps the receiver `dsp`, as in the C++ backend.
    let mut out = String::new();
    write_method(&mut out, signature, "        pass\n");
    assert_eq!(
        out,
        format!("    @always_inline\n    {signature}\n        pass\n\n")
    );

    // So does every generated method of a loop-free program, and a program
    // with a sample loop aliases `compute` only.
    let (store, module) = build_sine_phasor_test_module();
    let out = generate_mojo_module(&store, module, &MojoOptions::default()).unwrap();
    assert_eq!(out.matches("ref dsp = self").count(), 1, "{out}");
    assert!(out.contains(&format!(
        "    {}\n        ref dsp = self\n",
        aliased(signature)
    )));
}

// ------------------------------------------------------------------ batch

fn batch_options() -> MojoOptions {
    MojoOptions {
        batch: true,
        ..MojoOptions::default()
    }
}

/// `MojoOptions::batch`: the struct is built over one state buffer, sizes
/// its arenas, reaches its controls through the control table, and the
/// instance lifecycle takes the voice (design
/// `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md` §3.5).
/// A module shaped like the transform lane's output (fields declared without
/// initialiser, set by the lifecycle): a slider `fFreq`, a recursion `fRec0`,
/// a delay line `fVec0` with its counter `fIOTA`. The shared fixtures declare
/// initialised fields, which the voice layout refuses.
fn build_batch_test_module() -> (FirStore, fir::FirId) {
    use fir::{NamedType, SliderRange, SliderType};
    let ff = FirType::FaustFloat;
    let i32t = FirType::Int32;
    let arr = |t: FirType, n: usize| FirType::Array(Box::new(t), n);
    let dsp = NamedType {
        name: "dsp".to_owned(),
        typ: FirType::Ptr(Box::new(FirType::Obj)),
    };
    let sr = NamedType {
        name: "sample_rate".to_owned(),
        typ: i32t.clone(),
    };
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let fun = |b: &mut FirBuilder<'_>, name: &str, extra: &[NamedType], body| {
        let mut args = vec![dsp.clone()];
        args.extend_from_slice(extra);
        let typ = FirType::Fun {
            args: args.iter().map(|a| a.typ.clone()).collect(),
            ret: Box::new(FirType::Void),
        };
        b.declare_fun(name, typ, &args, Some(body), false)
    };
    let fields = [
        b.declare_var("fSampleRate", i32t.clone(), AccessType::Struct, None),
        b.declare_var("fFreq", ff.clone(), AccessType::Struct, None),
        b.declare_var("fRec0", arr(ff.clone(), 2), AccessType::Struct, None),
        b.declare_var("fVec0", arr(ff.clone(), 8), AccessType::Struct, None),
        b.declare_var("fIOTA", i32t.clone(), AccessType::Struct, None),
    ];
    let srv = b.load_var("sample_rate", AccessType::FunArgs, i32t.clone());
    let set_sr = b.store_var("fSampleRate", AccessType::Struct, srv);
    let constants_body = b.block(&[set_sr]);
    let constants = fun(
        &mut b,
        "instanceConstants",
        std::slice::from_ref(&sr),
        constants_body,
    );
    let init = b.float32(440.0);
    let init = b.cast(ff.clone(), init);
    let set_freq = b.store_var("fFreq", AccessType::Struct, init);
    let reset_body = b.block(&[set_freq]);
    let reset = fun(&mut b, "instanceResetUserInterface", &[], reset_body);
    let zero = b.int32(0);
    let one = b.int32(1);
    let fzero = b.float32(0.0);
    let fzero = b.cast(ff.clone(), fzero);
    let l0 = b.load_var("l0", AccessType::Loop, i32t.clone());
    let clear_vec = b.store_table("fVec0", AccessType::Struct, l0, fzero);
    let loop_body = b.block(&[clear_vec]);
    let eight = b.int32(8);
    let clear_loop = b.simple_for_loop("l0", eight, loop_body, false);
    let clear_iota = b.store_var("fIOTA", AccessType::Struct, zero);
    let clear_body = b.block(&[clear_loop, clear_iota]);
    let clear = fun(&mut b, "instanceClear", &[], clear_body);
    let slider = b.add_slider(
        SliderType::Horizontal,
        "freq",
        "fFreq",
        SliderRange {
            init: 440.0,
            lo: 20.0,
            hi: 2000.0,
            step: 1.0,
        },
    );
    let ui_body = b.block(&[slider]);
    let ui_arg = NamedType {
        name: "ui".to_owned(),
        typ: FirType::UI,
    };
    let ui = fun(&mut b, "buildUserInterface", &[ui_arg], ui_body);
    let freq = b.load_var("fFreq", AccessType::Struct, ff.clone());
    let rec1 = b.load_table("fRec0", AccessType::Struct, one, ff.clone());
    let sum = b.binop(FirBinOp::Add, freq, rec1, ff.clone());
    let set_rec = b.store_table("fRec0", AccessType::Struct, zero, sum);
    let iota = b.load_var("fIOTA", AccessType::Struct, i32t.clone());
    let seven = b.int32(7);
    let at = b.binop(FirBinOp::And, iota, seven, i32t.clone());
    let rec0 = b.load_table("fRec0", AccessType::Struct, zero, ff.clone());
    let set_vec = b.store_table("fVec0", AccessType::Struct, at, rec0);
    let next = b.binop(FirBinOp::Add, iota, one, i32t.clone());
    let set_iota = b.store_var("fIOTA", AccessType::Struct, next);
    let compute_body = b.block(&[set_rec, set_vec, set_iota]);
    let streams = FirType::Ptr(Box::new(FirType::Ptr(Box::new(ff.clone()))));
    let compute = fun(
        &mut b,
        "compute",
        &[
            NamedType {
                name: "count".to_owned(),
                typ: i32t.clone(),
            },
            NamedType {
                name: "inputs".to_owned(),
                typ: streams.clone(),
            },
            NamedType {
                name: "outputs".to_owned(),
                typ: streams,
            },
        ],
        compute_body,
    );
    let dsp_struct = b.block(&fields);
    let globals = b.block(&[]);
    let functions = b.block(&[constants, reset, clear, ui, compute]);
    let static_decls = b.block(&[]);
    let root = b.module(
        0,
        1,
        "mydsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, root)
}

#[test]
fn batch_mode_emits_the_batch_contract() {
    let (store, module) = build_batch_test_module();
    let out = generate_mojo_module(&store, module, &batch_options()).unwrap();
    for needle in [
        "struct mydsp(FaustDspBatch):",
        "    def __init__(out dsp, voices: S32, state: Ptr[UInt8]):",
        "        dsp.voices = voices",
        "    def state_bytes(voices: Int) -> Int:",
        "    def num_controls() -> Int:\n        return 1\n",
        "    def set_control(mut dsp, var voice: S32, index: Int, value: FaustFloat) -> None:",
        "    def get_control(imm dsp, var voice: S32, index: Int) -> FaustFloat:",
        "def instance_constants(mut dsp, var voice: S32, imm sample_rate: S32) -> None:",
        "def instance_reset_user_interface(mut dsp, var voice: S32) -> None:",
        "def class_init(mut dsp, imm sample_rate: S32) -> None:",
        "ui.add_horizontal_slider(\"freq\", dsp.null_val, FaustFloat(",
        "    def state_faust_float_elements() -> Int:\n        return 11\n",
        "    def state_int32_elements() -> Int:\n        return 2\n",
    ] {
        assert!(out.contains(needle), "missing `{needle}`\n{out}");
    }
    assert!(
        out.contains("(mut self, var voice: S32, var count: S32, var inputs: ImmStreams")
            || out.contains("(mut dsp, var voice: S32, var count: S32, var inputs: ImmStreams"),
        "{out}"
    );
    // State is reached through the arenas only: no per-voice field remains.
    assert!(!out.contains("dsp.freq"), "{out}");
    assert!(!out.contains("dsp.rec0"), "{out}");
    assert!(out.contains("[unsafe_offset = "), "{out}");
    // Every arena is carved from the state buffer and sized.
    let carved = out.matches(".unsafe_bitcast[").count();
    let sized = out.matches("_elements() -> Int:").count();
    assert!(
        carved > 0 && carved == sized,
        "{carved} carved, {sized} sized\n{out}"
    );
}

/// With `batch_registers` (the default), `compute` reads the arenas only to
/// initialise its `_reg` locals and writes them only as stores: no
/// expression reads state from memory, except a delay line at a dynamic
/// index (`fir::batch::promote_state`). Without the option, expressions read
/// the arenas directly.
#[test]
fn batch_registers_keep_compute_expressions_off_the_arenas() {
    let (store, module) = build_batch_test_module();
    // Lines of `compute` that read an arena inside an expression.
    let arena_reads = |out: &str| -> Vec<String> {
        let start = out
            .find("var count: S32, var inputs: ImmStreams")
            .expect("compute");
        let body = &out[start..];
        let end = body[1..].find("\n    def ").map_or(body.len(), |e| e + 1);
        body[..end]
            .lines()
            .skip(1)
            .filter_map(|line| {
                let line = line.trim();
                let rhs = if line.starts_with("var ") && line.contains("_reg") {
                    return None;
                } else if line.starts_with("dsp.state_") {
                    line.split_once("] = ").map_or(line, |(_, rhs)| rhs)
                } else {
                    line
                };
                rhs.contains("dsp.state_").then(|| line.to_owned())
            })
            .collect()
    };
    let promoted = generate_mojo_module(&store, module, &batch_options()).unwrap();
    assert_eq!(arena_reads(&promoted), Vec::<String>::new(), "{promoted}");
    assert!(
        promoted.contains("var rec0_reg1: FaustFloat = dsp.state_"),
        "{promoted}"
    );
    let plain = MojoOptions {
        batch_registers: false,
        ..batch_options()
    };
    let unpromoted = generate_mojo_module(&store, module, &plain).unwrap();
    assert!(!arena_reads(&unpromoted).is_empty(), "{unpromoted}");
    assert!(!unpromoted.contains("_reg"), "{unpromoted}");
}

/// The lifecycle contract (`porting/backend-lifecycle-contract-en.md`) in
/// its batch form: `class_init` once for all voices, `instance_init(voice)`
/// = constants, reset, clear of that voice, never `class_init`.
#[test]
fn batch_lifecycle_conformance() {
    for (name, build) in [
        (
            "batch",
            build_batch_test_module as fn() -> (FirStore, fir::FirId),
        ),
        (
            "passthrough",
            crate::fixtures::build_passthrough_test_module,
        ),
        (
            "math_intrinsics",
            crate::fixtures::build_math_intrinsics_test_module,
        ),
    ] {
        let (store, module) = build();
        let out = generate_mojo_module(&store, module, &batch_options()).unwrap();
        let init = method_body(
            &out,
            "def init(mut dsp, var voice: S32, imm sample_rate: S32) -> None:",
        );
        assert_eq!(
            init.trim(),
            "dsp.class_init(sample_rate)\n        dsp.instance_init(voice, sample_rate)",
            "{name}"
        );
        let instance_init = method_body(
            &out,
            "def instance_init(mut dsp, var voice: S32, imm sample_rate: S32) -> None:",
        );
        assert_eq!(
            instance_init.trim(),
            "dsp.instance_constants(voice, sample_rate)\n        dsp.instance_reset_user_interface(voice)\n        dsp.instance_clear(voice)",
            "{name}"
        );
        assert!(!instance_init.contains("class_init"), "{name}");
    }
}

/// Without the option the contract is the scalar one.
#[test]
fn the_default_mode_keeps_the_scalar_contract() {
    let (store, module) = build_batch_test_module();
    let out = generate_mojo_module(&store, module, &MojoOptions::default()).unwrap();
    assert!(out.contains("struct mydsp(FaustDsp):"));
    assert!(out.contains("    def __init__(out dsp):"));
    assert!(!out.contains("voice"), "{out}");
}

/// A module the voice layout refuses (here a field with an initialiser, as
/// the shared fixtures declare) fails with the layout's reason.
#[test]
fn a_module_the_layout_refuses_is_a_codegen_error() {
    let (store, module) = build_sine_phasor_test_module();
    let err = generate_mojo_module(&store, module, &batch_options()).unwrap_err();
    assert_eq!(err.code(), CodegenErrorCode::InvalidModuleSection);
    assert!(err.to_string().contains("batch voice layout"), "{err}");
}

// ------------------------------------------------- outlining, packed tables
// (`porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §5, §6)

/// A module whose sample loop holds an `ondemand`-like block of `n`
/// statements, then `tail` statements: the block stores into `iDone0`
/// (declared before the loop) and into a field, and reads the loop counter.
fn outlined_block_module(n: usize, tail: usize) -> (FirStore, fir::FirId) {
    use crate::fixtures::{compute_fun_args, compute_fun_type};
    let mut s = FirStore::new();
    let mut b = FirBuilder::new(&mut s);
    let field = b.declare_var("fHeld", FirType::Float32, AccessType::Struct, None);
    let dsp_struct = b.block(&[field]);
    let zero = b.int32(0);
    let one = b.int32(1);
    let done = b.load_var("iDone0", AccessType::Stack, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let mut then = Vec::new();
    for k in 0..n {
        let fi = b.cast(FirType::Float32, i0);
        let kf = b.float32(k as f32);
        let v = b.binop(FirBinOp::Add, fi, kf, FirType::Float32);
        then.push(b.store_var("fHeld", AccessType::Struct, v));
    }
    let next = b.binop(FirBinOp::Add, done, one, FirType::Int32);
    then.push(b.store_var("iDone0", AccessType::Stack, next));
    let then = b.block(&then);
    let cond = b.binop(FirBinOp::Eq, done, zero, FirType::Int32);
    let iff = b.if_(cond, then, None);
    let mut body = vec![iff];
    for _ in 0..tail {
        let fi = b.cast(FirType::Float32, i0);
        body.push(b.store_var("fHeld", AccessType::Struct, fi));
    }
    let body = b.block(&body);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let lp = b.simple_for_loop("i0", count, body, false);
    let decl = b.declare_var("iDone0", FirType::Int32, AccessType::Stack, Some(zero));
    let compute_body = b.block(&[decl, lp]);
    let compute = b.declare_fun(
        "compute",
        compute_fun_type(),
        &compute_fun_args(),
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let globals = b.block(&[]);
    let statics = b.block(&[]);
    let root = b.module(
        0,
        0,
        "outlined",
        dsp_struct,
        globals,
        functions,
        statics,
        &[],
    );
    (s, root)
}

fn outline_every_block() -> MojoOptions {
    MojoOptions {
        outline: Some(fir::outline::OutlinePolicy {
            budget: 1,
            min_block: 1,
            split_budget: fir::outline::DEFAULT_SPLIT_BUDGET,
        }),
        ..MojoOptions::default()
    }
}

#[test]
fn an_outlined_block_is_a_method_with_mut_references() {
    let (store, root) = outlined_block_module(3, 0);
    let text = generate_mojo_module(&store, root, &outline_every_block()).expect("emits");
    // The `if` stays in `compute`, around the call; the reference is the
    // caller's variable itself.
    assert!(text.contains("            if (i_done0 == S32(0)):\n                dsp.compute_outline0(i0, i_done0)\n"), "{text}");
    // No `@always_inline`: Mojo would merge the method back into `compute`.
    assert!(
        text.contains(
            "\n\n    def compute_outline0(mut dsp, i0: S32, mut i_done0: S32) -> None:\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("        i_done0 = (i_done0 + S32(1))\n"),
        "{text}"
    );
    assert!(
        text.contains("        dsp.held = (F32(i0) + F32(2.0))\n"),
        "{text}"
    );
    assert!(!text.contains("unsafe_offset = S32(0)]"), "{text}");
}

#[test]
fn a_program_under_the_budget_is_emitted_unchanged() {
    let (store, root) = outlined_block_module(3, 0);
    let default = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    let whole = generate_mojo_module(
        &store,
        root,
        &MojoOptions {
            outline: None,
            ..MojoOptions::default()
        },
    )
    .expect("emits");
    assert_eq!(default, whole);
    assert!(!default.contains("def compute_outline"));
}

#[test]
fn a_large_program_is_outlined_by_default() {
    // The block, and as many statements again after it in the loop.
    let n = fir::outline::DEFAULT_BUDGET / 2 + 10;
    let (store, root) = outlined_block_module(n, n);
    let text = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    assert!(text.contains("def compute_outline0(mut dsp, i0: S32, mut i_done0: S32)"));
    // A block that is nearly all of its method is left whole: outlining it
    // would not make the largest function smaller.
    let (store, root) = outlined_block_module(fir::outline::DEFAULT_BUDGET + 10, 0);
    let text = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    assert!(!text.contains("def compute_outline"));
}

/// A module with a struct table of `n` entries of type `elem`, values
/// `values(k)`.
fn table_module(
    elem: FirType,
    n: usize,
    value: impl Fn(&mut FirBuilder<'_>, usize) -> fir::FirId,
) -> (FirStore, fir::FirId) {
    use crate::fixtures::{compute_fun_args, compute_fun_type};
    let mut s = FirStore::new();
    let mut b = FirBuilder::new(&mut s);
    let values: Vec<_> = (0..n).map(|k| value(&mut b, k)).collect();
    let table = b.declare_table("ftbl0", AccessType::Struct, elem, &values);
    let dsp_struct = b.block(&[table]);
    let body = b.block(&[]);
    let compute = b.declare_fun(
        "compute",
        compute_fun_type(),
        &compute_fun_args(),
        Some(body),
        false,
    );
    let functions = b.block(&[compute]);
    let globals = b.block(&[]);
    let statics = b.block(&[]);
    let root = b.module(0, 0, "tables", dsp_struct, globals, functions, statics, &[]);
    (s, root)
}

#[test]
fn a_large_struct_table_is_packed_as_bit_patterns() {
    let n = super::PACKED_TABLE_MIN_ENTRIES;
    let (store, root) = table_module(FirType::Float32, n, |b, k| match k {
        0 => b.float32(-0.0),
        1 => b.float64(0.1),
        2 => b.int32(3),
        _ => b.float32(k as f32 * 0.5),
    });
    let text = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    let hex = format!(
        "{:08x}{:08x}{:08x}{:08x}",
        0.0f32.to_bits(),
        0.1f32.to_bits(),
        3.0f32.to_bits(),
        1.5f32.to_bits()
    );
    assert!(
        text.contains(&format!("dsp.ftbl0 = faust_unpack32[f32, {n}](\"{hex}")),
        "{text}"
    );
    assert_eq!(text.matches("def faust_unpack32[").count(), 1);
    assert!(!text.contains("faust_unpack64"));

    let literals = generate_mojo_module(
        &store,
        root,
        &MojoOptions {
            packed_tables: false,
            ..MojoOptions::default()
        },
    )
    .expect("emits");
    assert!(literals.contains("dsp.ftbl0 = [F32(0.0), F32(0.1), F32(3.0), F32(1.5),"));
    assert!(!literals.contains("faust_unpack"));
}

#[test]
fn integer_and_double_tables_are_packed_and_short_tables_are_not() {
    let n = super::PACKED_TABLE_MIN_ENTRIES;
    let (store, root) = table_module(FirType::Int32, n, |b, k| {
        b.int32(if k == 0 { -2 } else { 7 })
    });
    let text = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    assert!(
        text.contains(&format!("faust_unpack32[s32, {n}](\"fffffffe00000007")),
        "{text}"
    );

    let (store, root) = table_module(FirType::Float64, n, |b, _| b.float64(0.1));
    let text = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    assert!(
        text.contains(&format!(
            "faust_unpack64[f64, {n}](\"{:016x}",
            0.1f64.to_bits()
        )),
        "{text}"
    );

    let (store, root) = table_module(FirType::Float32, n - 1, |b, _| b.float32(0.5));
    let text = generate_mojo_module(&store, root, &MojoOptions::default()).expect("emits");
    assert!(!text.contains("faust_unpack"), "{text}");
}

/// A sample loop of `n` chained statements `tK = tK-1 + t0`, the last one
/// stored into a field.
fn straight_body_module(n: usize) -> (FirStore, fir::FirId) {
    use crate::fixtures::{compute_fun_args, compute_fun_type};
    let mut s = FirStore::new();
    let mut b = FirBuilder::new(&mut s);
    let field = b.declare_var("fOut", FirType::Float32, AccessType::Struct, None);
    let dsp_struct = b.block(&[field]);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let fi = b.cast(FirType::Float32, i0);
    let mut body = vec![b.declare_var("t0", FirType::Float32, AccessType::Stack, Some(fi))];
    let t0 = b.load_var("t0", AccessType::Stack, FirType::Float32);
    let mut prev = t0;
    for k in 1..n {
        let v = b.binop(FirBinOp::Add, prev, t0, FirType::Float32);
        let name = format!("t{k}");
        body.push(b.declare_var(&name, FirType::Float32, AccessType::Stack, Some(v)));
        prev = b.load_var(name, AccessType::Stack, FirType::Float32);
    }
    body.push(b.store_var("fOut", AccessType::Struct, prev));
    let body = b.block(&body);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let lp = b.simple_for_loop("i0", count, body, false);
    let compute_body = b.block(&[lp]);
    let compute = b.declare_fun(
        "compute",
        compute_fun_type(),
        &compute_fun_args(),
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let globals = b.block(&[]);
    let statics = b.block(&[]);
    let root = b.module(
        0,
        0,
        "straight",
        dsp_struct,
        globals,
        functions,
        statics,
        &[],
    );
    (s, root)
}

#[test]
fn a_split_run_receives_the_spill_array_as_a_mut_argument() {
    let (store, root) = straight_body_module(30);
    let options = MojoOptions {
        outline: Some(fir::outline::OutlinePolicy {
            budget: 8,
            min_block: 2,
            split_budget: 16,
        }),
        ..MojoOptions::default()
    };
    let text = generate_mojo_module(&store, root, &options).expect("emits");
    // The caller declares the spill array zero-filled, then calls the runs:
    // `t0` and the last value of each run cross the cuts (4 slots).
    assert!(
        text.contains(
            "            var compute_spill0: Arr[F32, 4] = Arr[F32, 4](fill=F32(0.0))\n\
             \x20           dsp.compute_outline0(compute_spill0, i0)\n"
        ),
        "{text}"
    );
    // The first run stores `t0` into its slot; the later ones read it there.
    assert!(
        text.contains("def compute_outline0(mut dsp, mut compute_spill0: Arr[F32, 4], i0: S32)"),
        "{text}"
    );
    assert!(
        text.contains("        compute_spill0[S32(0)] = t0\n"),
        "{text}"
    );
    assert!(
        text.contains("        var t9: F32 = (t8 + compute_spill0[S32(0)])\n"),
        "{text}"
    );
}
