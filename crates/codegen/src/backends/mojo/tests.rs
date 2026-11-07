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
    let mut emitter = Emitter::new(store, &options);
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
    let mut emitter = Emitter::new(&store, &options);
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
    let mut emitter = Emitter::new(&store, &options);
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
    let mut emitter = Emitter::new(&store, &options);
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
