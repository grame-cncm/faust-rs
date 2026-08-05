use super::*;
use crate::backends::interp::bytecode::FbcBlock;

/// Helper: creates a trivial factory for round-trip testing.
fn make_test_factory() -> FbcDspFactory<f32> {
    let mut arena = FbcBlockArena::<f32>::new();

    // static_init_block: StoreRealValue(0.0) at offset 0, Return
    let mut b1 = FbcBlock::new();
    b1.push(FbcInstruction::with_values_and_offsets(
        FbcOpcode::StoreRealValue,
        0,
        0.0,
        0,
        -1,
    ));
    b1.push(FbcInstruction::new(FbcOpcode::Return));
    let static_init = arena.alloc(b1);

    // init_block: Return only
    let mut b2 = FbcBlock::new();
    b2.push(FbcInstruction::new(FbcOpcode::Return));
    let init = arena.alloc(b2);

    // reset_ui_block: Return only
    let mut b3 = FbcBlock::new();
    b3.push(FbcInstruction::new(FbcOpcode::Return));
    let reset_ui = arena.alloc(b3);

    // clear_block: Return only
    let mut b4 = FbcBlock::new();
    b4.push(FbcInstruction::new(FbcOpcode::Return));
    let clear = arena.alloc(b4);

    // compute_block (control): Return only
    let mut b5 = FbcBlock::new();
    b5.push(FbcInstruction::new(FbcOpcode::Return));
    let compute = arena.alloc(b5);

    // compute_dsp_block: Return only
    let mut b6 = FbcBlock::new();
    b6.push(FbcInstruction::new(FbcOpcode::Return));
    let compute_dsp = arena.alloc(b6);

    FbcDspFactory::new(
        "test_dsp",
        "abc123",
        "-lang interp -ct 1 -es 1 -mcd 16",
        INTERP_FILE_VERSION,
        2,  // inputs
        2,  // outputs
        32, // int_heap_size
        64, // real_heap_size
        0,  // sr_offset
        1,  // count_offset
        2,  // iota_offset
        4,  // opt_level
        arena,
        vec![
            FbcMetaInstruction::new("name", "test_dsp"),
            FbcMetaInstruction::new("author", "Faust"),
        ],
        vec![FbcUiInstruction::widget(
            FbcOpcode::AddHorizontalSlider,
            5,
            "gain",
            0.5,
            0.0,
            1.0,
            0.01,
        )],
        static_init,
        init,
        reset_ui,
        clear,
        compute,
        compute_dsp,
    )
}

#[test]
fn test_quote_unquote() {
    assert_eq!(quote1("hello"), "\"hello\"");
    assert_eq!(unquote1("\"hello\""), "hello");
    assert_eq!(unquote1("hello"), "hello");
    assert_eq!(unquote1("\"\""), "");
}

#[test]
fn test_extract_nth_quoted() {
    let s = r#"label "gain" key "freq" value "Hz""#;
    assert_eq!(extract_nth_quoted(s, 0), Some("gain".to_string()));
    assert_eq!(extract_nth_quoted(s, 1), Some("freq".to_string()));
    assert_eq!(extract_nth_quoted(s, 2), Some("Hz".to_string()));
    assert_eq!(extract_nth_quoted(s, 3), None);
}

#[test]
fn test_write_normal_header() {
    let factory = make_test_factory();
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).unwrap();
    let output = String::from_utf8(buf).unwrap();

    assert!(output.starts_with("interpreter_dsp_factory float\n"));
    assert!(output.contains("file_version 9\n"));
    assert!(output.contains("name test_dsp\n"));
    assert!(output.contains("sha_key abc123\n"));
    assert!(output.contains("opt_level 4\n"));
    assert!(output.contains("inputs 2 outputs 2\n"));
    assert!(
        output.contains(
            "int_heap_size 32 real_heap_size 64 sr_offset 0 count_offset 1 iota_offset 2\n"
        )
    );
    assert!(output.contains("meta_block\n"));
    assert!(output.contains("user_interface_block\n"));
    assert!(output.contains("static_init_block\n"));
    assert!(output.contains("constants_block\n"));
    assert!(output.contains("reset_ui\n"));
    assert!(output.contains("clear_block\n"));
    assert!(output.contains("control_block\n"));
    assert!(output.contains("dsp_block\n"));
}

#[test]
fn test_write_small_header() {
    let factory = make_test_factory();
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, true).unwrap();
    let output = String::from_utf8(buf).unwrap();

    assert!(output.starts_with("i float\n"));
    assert!(output.contains("f 9\n"));
    assert!(output.contains("n test_dsp\n"));
}

#[test]
fn test_write_meta_block() {
    let meta = vec![
        FbcMetaInstruction::new("name", "test"),
        FbcMetaInstruction::new("author", "Faust"),
    ];
    let mut buf = Vec::new();
    write_meta_block(&meta, &mut buf, false).unwrap();
    let output = String::from_utf8(buf).unwrap();

    assert!(output.starts_with("block_size 2\n"));
    assert!(output.contains(r#"meta key "name" value "test""#));
    assert!(output.contains(r#"meta key "author" value "Faust""#));
}

#[test]
fn test_write_read_roundtrip() {
    let factory = make_test_factory();

    // Write.
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).unwrap();
    let serialized = String::from_utf8(buf).unwrap();

    // Read back.
    let mut cursor = io::Cursor::new(serialized.as_bytes());
    let factory2: FbcDspFactory<f32> = read_fbc(&mut cursor).unwrap();

    // Verify fields match.
    assert_eq!(factory2.name, "test_dsp");
    assert_eq!(factory2.sha_key, "abc123");
    assert_eq!(factory2.num_inputs, 2);
    assert_eq!(factory2.num_outputs, 2);
    assert_eq!(factory2.int_heap_size, 32);
    assert_eq!(factory2.real_heap_size, 64);
    assert_eq!(factory2.sr_offset, 0);
    assert_eq!(factory2.count_offset, 1);
    assert_eq!(factory2.iota_offset, 2);
    assert_eq!(factory2.opt_level, 4);
    assert_eq!(factory2.version, INTERP_FILE_VERSION);

    // Verify meta block.
    assert_eq!(factory2.meta_block.len(), 2);
    assert_eq!(factory2.meta_block[0].key, "name");
    assert_eq!(factory2.meta_block[0].value, "test_dsp");
    assert_eq!(factory2.meta_block[1].key, "author");
    assert_eq!(factory2.meta_block[1].value, "Faust");

    // Verify UI block.
    assert_eq!(factory2.ui_block.len(), 1);
    assert_eq!(factory2.ui_block[0].opcode, FbcOpcode::AddHorizontalSlider);
    assert_eq!(factory2.ui_block[0].offset, 5);
    assert_eq!(factory2.ui_block[0].label, "gain");

    // Verify code blocks exist and have correct sizes.
    assert_eq!(factory2.arena.get(factory2.static_init_block).len(), 2);
    assert_eq!(factory2.arena.get(factory2.init_block).len(), 1);
}

#[test]
fn test_write_read_roundtrip_small() {
    let factory = make_test_factory();

    // Write in small (compact) format.
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, true).unwrap();
    let serialized = String::from_utf8(buf).unwrap();

    // Read back — `read_fbc` must auto-detect the small format from the
    // leading "i float|double" header token.
    let mut cursor = io::Cursor::new(serialized.as_bytes());
    let factory2: FbcDspFactory<f32> = read_fbc(&mut cursor).unwrap();

    // Same assertions as `test_write_read_roundtrip`, since both formats
    // must decode to the same semantic factory.
    assert_eq!(factory2.name, "test_dsp");
    assert_eq!(factory2.sha_key, "abc123");
    assert_eq!(factory2.num_inputs, 2);
    assert_eq!(factory2.num_outputs, 2);
    assert_eq!(factory2.int_heap_size, 32);
    assert_eq!(factory2.real_heap_size, 64);
    assert_eq!(factory2.sr_offset, 0);
    assert_eq!(factory2.count_offset, 1);
    assert_eq!(factory2.iota_offset, 2);
    assert_eq!(factory2.opt_level, 4);
    assert_eq!(factory2.version, INTERP_FILE_VERSION);

    assert_eq!(factory2.meta_block.len(), 2);
    assert_eq!(factory2.meta_block[0].key, "name");
    assert_eq!(factory2.meta_block[0].value, "test_dsp");
    assert_eq!(factory2.meta_block[1].key, "author");
    assert_eq!(factory2.meta_block[1].value, "Faust");

    assert_eq!(factory2.ui_block.len(), 1);
    assert_eq!(factory2.ui_block[0].opcode, FbcOpcode::AddHorizontalSlider);
    assert_eq!(factory2.ui_block[0].offset, 5);
    assert_eq!(factory2.ui_block[0].label, "gain");

    assert_eq!(factory2.arena.get(factory2.static_init_block).len(), 2);
    assert_eq!(factory2.arena.get(factory2.init_block).len(), 1);
}

#[test]
fn test_instruction_name_field_roundtrips_in_both_formats() {
    // `FbcInstruction::name` carries foreign-call signatures (see
    // `foreign::ForeignSignature::encode`). Its header token spells "name" in
    // normal mode and "n" in small mode, so both must be exercised.
    for small in [false, true] {
        let mut arena = FbcBlockArena::<f32>::new();
        let mut block = FbcBlock::new();
        block.push(FbcInstruction::with_name(
            FbcOpcode::ForeignCallReal,
            "myFunc|d|dd",
        ));
        block.push(FbcInstruction::new(FbcOpcode::Return));
        let block_id = arena.alloc(block);

        let factory = FbcDspFactory::new(
            "test_dsp",
            "",
            "",
            INTERP_FILE_VERSION,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            arena,
            vec![],
            vec![],
            block_id,
            block_id,
            block_id,
            block_id,
            block_id,
            block_id,
        );

        let mut buf = Vec::new();
        write_fbc(&factory, &mut buf, small).unwrap();
        let serialized = String::from_utf8(buf).unwrap();

        let mut cursor = io::Cursor::new(serialized.as_bytes());
        let factory2: FbcDspFactory<f32> = read_fbc(&mut cursor).unwrap();
        let block2 = factory2.arena.get(factory2.static_init_block);
        assert_eq!(
            block2.instructions[0].name,
            "myFunc|d|dd",
            "name field lost roundtripping through {} format",
            if small { "small" } else { "normal" }
        );
    }
}

#[test]
fn test_version_check() {
    // Build a .fbc string with wrong version.
    let bad_fbc = "interpreter_dsp_factory float\nfile_version 99\n";
    let mut cursor = io::Cursor::new(bad_fbc.as_bytes());
    let result = read_fbc::<f32>(&mut cursor);
    assert!(result.is_err());
    match result.unwrap_err() {
        FbcSerialError::VersionMismatch { expected, got } => {
            assert_eq!(expected, INTERP_FILE_VERSION);
            assert_eq!(got, 99);
        }
        other => panic!("expected VersionMismatch, got {:?}", other),
    }
}

#[test]
fn test_type_mismatch() {
    let bad_fbc = "interpreter_dsp_factory double\nfile_version 8\n";
    let mut cursor = io::Cursor::new(bad_fbc.as_bytes());
    let result = read_fbc::<f32>(&mut cursor);
    assert!(result.is_err());
    match result.unwrap_err() {
        FbcSerialError::TypeMismatch { expected, got } => {
            assert_eq!(expected, "float");
            assert_eq!(got, "double");
        }
        other => panic!("expected TypeMismatch, got {:?}", other),
    }
}

#[test]
fn test_read_meta_block() {
    let input =
        "block_size 2\nmeta key \"name\" value \"sine\"\nmeta key \"author\" value \"Faust\"\n";
    let mut cursor = io::Cursor::new(input.as_bytes());
    let meta = read_meta_block(&mut cursor).unwrap();
    assert_eq!(meta.len(), 2);
    assert_eq!(meta[0].key, "name");
    assert_eq!(meta[0].value, "sine");
    assert_eq!(meta[1].key, "author");
    assert_eq!(meta[1].value, "Faust");
}

#[test]
fn test_roundtrip_with_branching() {
    // Build a factory with an If instruction (has sub-blocks).
    let mut arena = FbcBlockArena::<f32>::new();

    // Branch blocks for the If instruction.
    let mut then_block = FbcBlock::new();
    then_block.push(FbcInstruction::with_values(FbcOpcode::RealValue, 0, 1.0));
    then_block.push(FbcInstruction::new(FbcOpcode::Return));
    let then_id = arena.alloc(then_block);

    let mut else_block = FbcBlock::new();
    else_block.push(FbcInstruction::with_values(FbcOpcode::RealValue, 0, 0.0));
    else_block.push(FbcInstruction::new(FbcOpcode::Return));
    let else_id = arena.alloc(else_block);

    // Main block with If instruction.
    let mut main_block = FbcBlock::new();
    main_block.push(FbcInstruction::full(
        FbcOpcode::If,
        "",
        0,
        0.0,
        -1,
        -1,
        Some(then_id),
        Some(else_id),
    ));
    main_block.push(FbcInstruction::new(FbcOpcode::Return));
    let main_id = arena.alloc(main_block);

    // Trivial blocks for other slots.
    let mut trivials = Vec::new();
    for _ in 0..5 {
        let mut b = FbcBlock::new();
        b.push(FbcInstruction::new(FbcOpcode::Return));
        trivials.push(arena.alloc(b));
    }

    let factory = FbcDspFactory::new(
        "if_test",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        0,
        4,
        4,
        0,
        1,
        -1,
        0,
        arena,
        vec![],
        vec![],
        main_id, // static_init has the If instruction
        trivials[0],
        trivials[1],
        trivials[2],
        trivials[3],
        trivials[4],
    );

    // Write.
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).unwrap();
    let serialized = String::from_utf8(buf).unwrap();

    // Read back.
    let mut cursor = io::Cursor::new(serialized.as_bytes());
    let factory2: FbcDspFactory<f32> = read_fbc(&mut cursor).unwrap();

    // Verify the If instruction's sub-blocks survived round-trip.
    let static_block = factory2.arena.get(factory2.static_init_block);
    assert_eq!(static_block.len(), 2);
    assert_eq!(static_block.instructions[0].opcode, FbcOpcode::If);
    assert!(static_block.instructions[0].branch1.is_some());
    assert!(static_block.instructions[0].branch2.is_some());

    // Verify sub-block contents.
    let b1 = factory2
        .arena
        .get(static_block.instructions[0].branch1.unwrap());
    assert_eq!(b1.len(), 2);
    assert_eq!(b1.instructions[0].opcode, FbcOpcode::RealValue);
    assert!((b1.instructions[0].real_value - 1.0).abs() < 1e-6);

    let b2 = factory2
        .arena
        .get(static_block.instructions[0].branch2.unwrap());
    assert_eq!(b2.len(), 2);
    assert_eq!(b2.instructions[0].opcode, FbcOpcode::RealValue);
    assert!((b2.instructions[0].real_value - 0.0).abs() < 1e-6);
}

#[test]
fn test_roundtrip_block_store_real() {
    let mut arena = FbcBlockArena::<f32>::new();

    let mut block = FbcBlock::new();
    let instr = FbcInstruction::with_values_and_offsets(FbcOpcode::BlockStoreReal, 0, 0.0, 0, 4);
    let data = BlockStoreData::Real(vec![1.0, 2.0, 3.0, 4.0]);
    block.push_block_store(instr, data);
    block.push(FbcInstruction::new(FbcOpcode::Return));
    let block_id = arena.alloc(block);

    // Create trivial blocks for other slots.
    let mut trivials = Vec::new();
    for _ in 0..5 {
        let mut b = FbcBlock::new();
        b.push(FbcInstruction::new(FbcOpcode::Return));
        trivials.push(arena.alloc(b));
    }

    let factory = FbcDspFactory::new(
        "blockstore",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        0,
        4,
        8,
        0,
        1,
        -1,
        0,
        arena,
        vec![],
        vec![],
        block_id,
        trivials[0],
        trivials[1],
        trivials[2],
        trivials[3],
        trivials[4],
    );

    // Write.
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).unwrap();
    let serialized = String::from_utf8(buf).unwrap();

    // Read back.
    let mut cursor = io::Cursor::new(serialized.as_bytes());
    let factory2: FbcDspFactory<f32> = read_fbc(&mut cursor).unwrap();

    // Verify block-store data survived.
    let block = factory2.arena.get(factory2.static_init_block);
    assert_eq!(block.len(), 2);
    assert_eq!(block.instructions[0].opcode, FbcOpcode::BlockStoreReal);
    match &block.instructions[0].block_store {
        Some(BlockStoreData::Real(v)) => {
            assert_eq!(v.len(), 4);
            assert!((v[0] - 1.0).abs() < 1e-6);
            assert!((v[1] - 2.0).abs() < 1e-6);
            assert!((v[2] - 3.0).abs() < 1e-6);
            assert!((v[3] - 4.0).abs() < 1e-6);
        }
        Some(BlockStoreData::Int(_)) => panic!("expected Real data"),
        None => panic!("expected inline block store payload"),
    }
}

#[test]
fn test_roundtrip_double() {
    // Test with f64 to verify type-specific serialization.
    let mut arena = FbcBlockArena::<f64>::new();

    let mut b = FbcBlock::new();
    b.push(FbcInstruction::with_values(
        FbcOpcode::RealValue,
        0,
        std::f64::consts::PI,
    ));
    b.push(FbcInstruction::new(FbcOpcode::Return));
    let block_id = arena.alloc(b);

    let mut trivials = Vec::new();
    for _ in 0..5 {
        let mut b = FbcBlock::new();
        b.push(FbcInstruction::new(FbcOpcode::Return));
        trivials.push(arena.alloc(b));
    }

    let factory = FbcDspFactory::new(
        "pi_test",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        0,
        4,
        4,
        0,
        1,
        -1,
        0,
        arena,
        vec![],
        vec![],
        block_id,
        trivials[0],
        trivials[1],
        trivials[2],
        trivials[3],
        trivials[4],
    );

    // Write.
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).unwrap();
    let serialized = String::from_utf8(buf).unwrap();

    // Verify header says "double".
    assert!(serialized.starts_with("interpreter_dsp_factory double\n"));

    // Read back.
    let mut cursor = io::Cursor::new(serialized.as_bytes());
    let factory2: FbcDspFactory<f64> = read_fbc(&mut cursor).unwrap();

    // Verify PI survived round-trip with full f64 precision.
    let block = factory2.arena.get(factory2.static_init_block);
    let val = block.instructions[0].real_value;
    assert!(
        (val - std::f64::consts::PI).abs() < 1e-14,
        "PI round-trip: got {val}, expected {}",
        std::f64::consts::PI
    );
}

#[test]
fn test_f32_roundtrip_preserves_small_gain_constant() {
    let gain = 1.0f32 / 48_000.0f32;
    let mut arena = FbcBlockArena::<f32>::new();

    let mut block = FbcBlock::new();
    block.push(FbcInstruction::with_values(FbcOpcode::RealValue, 0, gain));
    block.push(FbcInstruction::new(FbcOpcode::Return));
    let block_id = arena.alloc(block);

    let factory = FbcDspFactory::new(
        "test_dsp",
        "",
        "",
        INTERP_FILE_VERSION,
        0,
        1,
        1,
        0,
        0,
        0,
        0,
        0,
        arena,
        vec![],
        vec![],
        block_id,
        block_id,
        block_id,
        block_id,
        block_id,
        block_id,
    );

    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).unwrap();
    let serialized = String::from_utf8(buf).unwrap();
    assert!(
        serialized.contains("real 0.000020833"),
        "serialized FBC should keep round-trip precision: {serialized}"
    );

    let mut cursor = io::Cursor::new(serialized.as_bytes());
    let roundtrip: FbcDspFactory<f32> = read_fbc(&mut cursor).unwrap();
    let instr = &roundtrip
        .arena
        .get(roundtrip.static_init_block)
        .instructions[0];
    assert_eq!(instr.real_value, gain);
}

/// A label that contains a literal embedded newline must parse correctly.
///
/// Reproduces the failure seen with `elecGuitarMIDI.fbc` where
/// `label "sustain\n"` caused a parse error because `read_line` stopped
/// at the `\n` inside the quoted string, leaving the rest of the
/// instruction on the next physical line.
#[test]
fn test_ui_instruction_label_with_embedded_newline() {
    // Simulate the exact layout from elecGuitarMIDI.fbc:
    //   opcode 286 kAddHorizontalSlider offset 8272 label "sustain
    //   " key "" value "" init 0.0 min 0.0 max 1.0 step 1.0
    let input = concat!(
        "block_size 1\n",
        "opcode 286 kAddHorizontalSlider offset 8272 label \"sustain\n",
        "\" key \"\" value \"\" init 0.0000000 min 0.0000000 max 1.0000000 step 1.0000000\n",
    );
    let mut cursor = io::Cursor::new(input.as_bytes());
    let ui = read_ui_block::<f32>(&mut cursor).unwrap();
    assert_eq!(ui.len(), 1);
    assert_eq!(ui[0].label, "sustain\n");
    assert_eq!(ui[0].key, "");
    assert_eq!(ui[0].value, "");
    assert!((ui[0].init - 0.0_f32).abs() < 1e-6);
    assert!((ui[0].max - 1.0_f32).abs() < 1e-6);
    assert!((ui[0].step - 1.0_f32).abs() < 1e-6);
}
