use super::*;
use crate::backends::interp::FbcInstruction;
use crate::backends::interp::bytecode::{FbcBlock, FbcBlockArena};
use crate::backends::interp::opcode::{FbcOpcode, INTERP_FILE_VERSION};

/// Builds a minimal factory with trivial (Return-only) blocks.
fn trivial_block(arena: &mut FbcBlockArena<f32>) -> BlockId {
    let mut b = FbcBlock::new();
    b.push(FbcInstruction::new(FbcOpcode::Return));
    arena.alloc(b)
}

fn make_factory() -> FbcDspFactory<f32> {
    let mut arena = FbcBlockArena::<f32>::new();
    let b1 = trivial_block(&mut arena);
    let b2 = trivial_block(&mut arena);
    let b3 = trivial_block(&mut arena);
    let b4 = trivial_block(&mut arena);
    let b5 = trivial_block(&mut arena);
    let b6 = trivial_block(&mut arena);
    FbcDspFactory::new(
        "test_dsp",
        "sha_abc",
        "-lang interp",
        INTERP_FILE_VERSION,
        1,
        1,
        8,
        8,
        0, // sr_offset
        1, // count_offset
        2, // iota_offset
        0, // opt_level
        arena,
        vec![FbcMetaInstruction::new("name", "test")],
        vec![],
        b1,
        b2,
        b3,
        b4,
        b5,
        b6,
    )
}

#[test]
fn generate_basic_structure() {
    let factory = make_factory();
    let opts = FbcCppOptions::default();
    let cpp = generate_cpp_from_fbc(&factory, &opts).expect("generation should succeed");

    // Class structure.
    assert!(cpp.contains("class test_dsp final : public dsp"), "{cpp}");
    assert!(
        cpp.contains("int getNumInputs() override { return 1; }"),
        "{cpp}"
    );
    assert!(
        cpp.contains("int getNumOutputs() override { return 1; }"),
        "{cpp}"
    );
    assert!(cpp.contains("void instanceInit(int sample_rate)"), "{cpp}");
    assert!(cpp.contains("void init(int sample_rate) override"), "{cpp}");
    assert!(
        cpp.contains("void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) override"),
        "{cpp}"
    );
    assert!(cpp.contains("dsp* clone() override"), "{cpp}");
}

#[test]
fn generate_lifecycle_matches_cpp_backend_contract() {
    let factory = make_factory();
    let opts = FbcCppOptions::default();
    let cpp = generate_cpp_from_fbc(&factory, &opts).expect("generation should succeed");

    let init_start = cpp
        .find("void init(int sample_rate) override {")
        .expect("init should be emitted");
    let init_body = &cpp[init_start..];
    let init_class_i = init_body
        .find("classInit(sample_rate);")
        .expect("init should call classInit");
    let init_instance_i = init_body
        .find("instanceInit(sample_rate);")
        .expect("init should call instanceInit");
    assert!(
        init_class_i < init_instance_i,
        "init should call classInit before instanceInit"
    );

    let instance_init_start = cpp
        .find("void instanceInit(int sample_rate) override {")
        .expect("instanceInit should be emitted");
    let instance_init_end = cpp[instance_init_start..]
        .find("\n\t}\n")
        .map(|offset| instance_init_start + offset)
        .expect("instanceInit body should close");
    let instance_init_body = &cpp[instance_init_start..instance_init_end];
    assert!(
        !instance_init_body.contains("classInit(sample_rate);"),
        "instanceInit must not call classInit"
    );
    let constants_i = instance_init_body
        .find("instanceConstants(sample_rate);")
        .expect("instanceInit should call instanceConstants");
    let reset_i = instance_init_body
        .find("instanceResetUserInterface();")
        .expect("instanceInit should call instanceResetUserInterface");
    let clear_i = instance_init_body
        .find("instanceClear();")
        .expect("instanceInit should call instanceClear");
    assert!(
        constants_i < reset_i && reset_i < clear_i,
        "instanceInit should call constants, resetUI, clear in order"
    );
}

#[test]
fn generate_with_pragma_once() {
    let factory = make_factory();
    let opts = FbcCppOptions {
        pragma_once: true,
        ..Default::default()
    };
    let cpp = generate_cpp_from_fbc(&factory, &opts).unwrap();
    assert!(cpp.starts_with("#pragma once"), "{cpp}");
}

#[test]
fn generate_without_pragma_once() {
    let factory = make_factory();
    let opts = FbcCppOptions {
        pragma_once: false,
        ..Default::default()
    };
    let cpp = generate_cpp_from_fbc(&factory, &opts).unwrap();
    assert!(!cpp.starts_with("#pragma once"), "{cpp}");
}

#[test]
fn generate_with_namespace() {
    let factory = make_factory();
    let opts = FbcCppOptions {
        namespace: Some("faust_native".to_owned()),
        ..Default::default()
    };
    let cpp = generate_cpp_from_fbc(&factory, &opts).unwrap();
    assert!(cpp.contains("namespace faust_native {"), "{cpp}");
    assert!(cpp.contains("} // namespace faust_native"), "{cpp}");
}

#[test]
fn generate_custom_class_name() {
    let factory = make_factory();
    let opts = FbcCppOptions {
        class_name: Some("MySynth".to_owned()),
        ..Default::default()
    };
    let cpp = generate_cpp_from_fbc(&factory, &opts).unwrap();
    assert!(cpp.contains("class MySynth final : public dsp"), "{cpp}");
}

#[test]
fn generate_with_loop_and_condbranch() {
    // Build a factory whose static_init_block contains a loop:
    //   init_block:  StoreIntValue(slot=2, value=0)  → iVec[2] = 0; Return
    //   body_block:  LoadInt(2) + Int32Value(5) + LTInt + CondBranch(→body) + Return
    //   main_block (clear_block):  Loop(init=init_id, body=body_id) + Return

    let mut arena = FbcBlockArena::<f32>::new();

    let mut init_b = FbcBlock::new();
    init_b.push(FbcInstruction::with_values_and_offsets(
        FbcOpcode::StoreIntValue,
        0,
        0.0,
        2,
        -1,
    ));
    init_b.push(FbcInstruction::new(FbcOpcode::Return));
    let init_id = arena.alloc(init_b);

    // Placeholder for body (need its ID for CondBranch's branch1).
    let body_placeholder = FbcBlock::new();
    let body_id = arena.alloc(body_placeholder);

    let mut body_b = FbcBlock::new();
    // Load counter, compare with 5, CondBranch.
    body_b.push(FbcInstruction::with_values_and_offsets(
        FbcOpcode::LoadInt,
        0,
        0.0,
        2,
        -1,
    ));
    body_b.push(FbcInstruction::with_values(FbcOpcode::Int32Value, 5, 0.0));
    body_b.push(FbcInstruction::new(FbcOpcode::LTInt));
    body_b.push(FbcInstruction::full(
        FbcOpcode::CondBranch,
        "",
        0,
        0.0,
        -1,
        -1,
        Some(body_id),
        None,
    ));
    body_b.push(FbcInstruction::new(FbcOpcode::Return));
    *arena.get_mut(body_id) = body_b;

    let mut main_b = FbcBlock::new();
    main_b.push(FbcInstruction::full(
        FbcOpcode::Loop,
        "",
        0,
        0.0,
        -1,
        -1,
        Some(init_id),
        Some(body_id),
    ));
    main_b.push(FbcInstruction::new(FbcOpcode::Return));
    let main_id = arena.alloc(main_b);

    let trivial = |a: &mut FbcBlockArena<f32>| {
        let mut b = FbcBlock::new();
        b.push(FbcInstruction::new(FbcOpcode::Return));
        a.alloc(b)
    };
    let b2 = trivial(&mut arena);
    let b3 = trivial(&mut arena);
    let b4 = trivial(&mut arena);
    let b5 = trivial(&mut arena);

    let factory = FbcDspFactory::new(
        "loop_test",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        0,
        8,
        4,
        0,
        1,
        -1,
        0,
        arena,
        vec![],
        vec![],
        main_id, // static_init_block  ← has the loop
        b2,
        b3,
        b4,
        b5,
        trivial(&mut FbcBlockArena::new()), // unused compute_dsp_block
    );

    // We need a separate arena for the last block. Let me fix this...
    // Actually the factory above is malformed because the last block is
    // in a fresh arena. Let me redo.
    drop(factory);

    // Rebuild properly.
    let mut arena2 = FbcBlockArena::<f32>::new();
    let init_id2;
    let body_id2;
    let main_id2;
    {
        let mut init_b = FbcBlock::new();
        init_b.push(FbcInstruction::with_values_and_offsets(
            FbcOpcode::StoreIntValue,
            0,
            0.0,
            2,
            -1,
        ));
        init_b.push(FbcInstruction::new(FbcOpcode::Return));
        init_id2 = arena2.alloc(init_b);

        let body_placeholder = FbcBlock::new();
        body_id2 = arena2.alloc(body_placeholder);

        let mut body_b = FbcBlock::new();
        body_b.push(FbcInstruction::with_values_and_offsets(
            FbcOpcode::LoadInt,
            0,
            0.0,
            2,
            -1,
        ));
        body_b.push(FbcInstruction::with_values(FbcOpcode::Int32Value, 5, 0.0));
        body_b.push(FbcInstruction::new(FbcOpcode::LTInt));
        body_b.push(FbcInstruction::full(
            FbcOpcode::CondBranch,
            "",
            0,
            0.0,
            -1,
            -1,
            Some(body_id2),
            None,
        ));
        body_b.push(FbcInstruction::new(FbcOpcode::Return));
        *arena2.get_mut(body_id2) = body_b;

        let mut main_b = FbcBlock::new();
        main_b.push(FbcInstruction::full(
            FbcOpcode::Loop,
            "",
            0,
            0.0,
            -1,
            -1,
            Some(init_id2),
            Some(body_id2),
        ));
        main_b.push(FbcInstruction::new(FbcOpcode::Return));
        main_id2 = arena2.alloc(main_b);
    }
    let trivial2 = |a: &mut FbcBlockArena<f32>| {
        let mut b = FbcBlock::new();
        b.push(FbcInstruction::new(FbcOpcode::Return));
        a.alloc(b)
    };
    let b2 = trivial2(&mut arena2);
    let b3 = trivial2(&mut arena2);
    let b4 = trivial2(&mut arena2);
    let b5 = trivial2(&mut arena2);
    let b6 = trivial2(&mut arena2);

    let factory2 = FbcDspFactory::new(
        "loop_test",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        0,
        8,
        4,
        0,
        1,
        -1,
        0,
        arena2,
        vec![],
        vec![],
        main_id2,
        b2,
        b3,
        b4,
        b5,
        b6,
    );

    let cpp = generate_cpp_from_fbc(&factory2, &FbcCppOptions::default()).unwrap();
    assert!(cpp.contains("while (true) {"), "{cpp}");
    assert!(cpp.contains("if (!"), "{cpp}");
    assert!(cpp.contains("break;"), "{cpp}");
    assert!(cpp.contains("iVec[2] = 0;"), "{cpp}");
}

#[test]
fn meta_block_is_emitted() {
    let factory = make_factory();
    let cpp = generate_cpp_from_fbc(&factory, &FbcCppOptions::default()).unwrap();
    assert!(cpp.contains("m->declare(\"name\", \"test\");"), "{cpp}");
}

#[test]
fn sanitize_cpp_ident_handles_special_chars() {
    assert_eq!(sanitize_cpp_ident("my-dsp"), "my_dsp");
    assert_eq!(sanitize_cpp_ident("3way"), "_3way");
    assert_eq!(sanitize_cpp_ident("foo bar"), "foo_bar");
    assert_eq!(sanitize_cpp_ident(""), "");
    assert_eq!(sanitize_cpp_ident("valid_id123"), "valid_id123");
}

#[test]
fn fmt_real_lit_produces_correct_suffix() {
    assert!(
        fmt_real_lit(0.5f32, "float").ends_with('f'),
        "{}",
        fmt_real_lit(0.5f32, "float")
    );
    assert!(!fmt_real_lit(0.5f64, "double").ends_with('f'));
    // NaN and Inf handling.
    assert!(fmt_real_lit(f32::NAN, "float").contains("NaN"));
    assert!(fmt_real_lit(f32::INFINITY, "float").contains("infinity"));
    assert!(fmt_real_lit(f32::NEG_INFINITY, "float").starts_with('-'));
}
