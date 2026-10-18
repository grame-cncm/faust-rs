use super::{
    LocalSpecs, WasmJsonDescription, WasmMemoryLayout, WasmOptions, collect_compute_locals,
    generate_wasm_module,
};
use crate::fixtures::{
    build_control_flow_test_module, build_gain_bias_ui_meta_test_module,
    build_math_intrinsics_test_module, build_passthrough_test_module,
    build_sine_phasor_test_module, build_table_state_delay_test_module,
};

use fir::{AccessType, FirBuilder, FirId, FirMathOp, FirStore, FirType, NamedType, SliderRange};

use wasmparser::{Operator, Parser, Payload, Validator};

#[test]
fn wasm_scaffold_emits_valid_module_for_passthrough_fixture() {
    let (store, module) = build_passthrough_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit a valid module");

    Validator::new()
        .validate_all(&out.wasm_binary)
        .expect("generated scaffold should validate as WASM");
    assert!(out.dsp_json.contains("\"inputs\": 1"));
    assert!(out.dsp_json.contains("\"outputs\": 1"));
    assert!(out.dsp_json.contains("\"ui\": []"));
}

#[test]
fn wasm_compute_lowers_local_array_buffers() {
    let (store, module) = build_local_array_buffer_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM backend should lower stack-local array buffers");

    Validator::new()
        .validate_all(&out.wasm_binary)
        .expect("generated module with local array should validate");
    let vbuf = out
        .memory_layout
        .local_offsets
        .get("vbuf0")
        .expect("local array should be reserved in memory layout");
    assert_eq!(vbuf.size, 128);
    assert!(vbuf.offset < out.memory_layout.io_zone_offset);
}

#[test]
fn wasm_json_description_renders_stable_scaffold_shape() {
    let json = WasmJsonDescription {
        name: "passthrough".to_owned(),
        backend: None,
        jit_compiled: None,
        compute_body_lowered: None,
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: Some(4),
        inputs: 1,
        outputs: 2,
        sr_index: None,
        memory: None,
        meta: Vec::new(),
        ui: Vec::new(),
    }
    .render();

    assert_eq!(
        json,
        "{\n\t\"name\": \"passthrough\",\n\t\"size\": 4,\n\t\"inputs\": 1,\n\t\"outputs\": 2,\n\t\"meta\": [],\n\t\"ui\": []\n}"
    );
}

#[test]
fn wasm_json_description_escapes_string_fields() {
    let json = WasmJsonDescription {
        name: "quote\"slash\\tab\tline\n".to_owned(),
        backend: None,
        jit_compiled: None,
        compute_body_lowered: None,
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: Some(0),
        inputs: 0,
        outputs: 0,
        sr_index: None,
        memory: None,
        meta: Vec::new(),
        ui: Vec::new(),
    }
    .render();

    assert!(
        json.contains("\"name\": \"quote\\\"slash\\\\tab\\tline\\n\""),
        "escaped JSON should preserve control characters and quotes: {json}"
    );
}

#[test]
fn wasm_json_description_replays_fir_ui_and_metadata() {
    let (store, module) = build_gain_bias_ui_meta_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM backend should emit FIR-derived UI and metadata JSON");

    assert!(out.dsp_json.contains("\"size\": 16"));
    assert!(
        out.dsp_json
            .contains("\t\"meta\": [ \n\t\t{ \"name\": \"gain-bias-ui-meta\" },")
    );
    assert!(out.dsp_json.contains("\"type\": \"vgroup\""));
    assert!(out.dsp_json.contains("\"label\": \"GainBias\""));
    assert!(out.dsp_json.contains("\"address\": \"/GainBias/gate\""));
    assert!(out.dsp_json.contains("\"index\": 8"));
    assert!(out.dsp_json.contains("\"address\": \"/GainBias/gain\""));
    assert!(out.dsp_json.contains("\"index\": 0"));
    assert!(out.dsp_json.contains("\"address\": \"/GainBias/bias\""));
    assert!(out.dsp_json.contains("\"index\": 4"));
    assert!(out.dsp_json.contains("\"address\": \"/GainBias/level\""));
    assert!(out.dsp_json.contains("\"index\": 12"));
}

#[test]
fn wasm_scaffold_exports_canonical_faust_api_names() {
    let (store, module) = build_passthrough_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit export section");

    let mut exports = Vec::new();
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        let payload = payload.expect("payload should decode");
        if let Payload::ExportSection(section) = payload {
            for export in section {
                let export = export.expect("export should decode");
                exports.push(export.name.to_owned());
            }
        }
    }

    assert_eq!(
        exports,
        vec![
            "compute",
            "getNumInputs",
            "getNumOutputs",
            "getParamValue",
            "getSampleRate",
            "init",
            "instanceClear",
            "instanceConstants",
            "instanceInit",
            "instanceResetUserInterface",
            "setParamValue",
            "memory",
        ]
    );
}

#[test]
fn wasm_compute_passthrough_lowers_loop_and_sample_io() {
    let (store, module) = build_passthrough_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit lowered passthrough compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::Loop { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::BrIf { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Load { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Store { .. })));
}

#[test]
fn wasm_compute_ignores_label_markers() {
    let (store, module) = build_labeled_passthrough_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should lower compute despite FIR labels");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::Loop { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::BrIf { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Load { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Store { .. })));
}

#[test]
fn wasm_compute_lowers_struct_state_and_casts() {
    let (store, module) = build_struct_state_cast_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit struct-state compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Add)));
    assert!(ops.iter().any(|op| matches!(op, Operator::F64PromoteF32)));
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Store { .. })));
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::F32Load { .. }))
            .count()
            >= 2
    );
}

#[test]
fn wasm_instance_reset_ui_lowers_control_reinitialization() {
    let (store, module) = build_gain_bias_ui_meta_test_module();
    let out = generate_wasm_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
    )
    .expect("WASM backend should lower instanceResetUserInterface body");

    let body = code_body_at(&out.wasm_binary, 10);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .any(|op| matches!(op, Operator::F64Const { value } if *value == 0.5f64.into()))
    );
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::F64Store { .. }))
            .count()
            >= 3
    );
}

#[test]
fn wasm_instance_constants_lowers_struct_table_initialization() {
    let (store, module) = build_struct_table_instance_constants_module();
    let out = generate_wasm_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
    )
    .expect("WASM backend should lower instanceConstants table stores");

    let body = code_body_at(&out.wasm_binary, 8);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::I32Store { .. } | Operator::F64Store { .. }))
            .count()
            >= 3
    );
}

#[test]
fn wasm_get_and_set_param_value_use_f64_in_double_mode() {
    let (store, module) = build_passthrough_test_module();
    let out = generate_wasm_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
    )
    .expect("WASM scaffold should emit double-precision param ABI");

    let get_body = code_body_at(&out.wasm_binary, 4);
    let get_ops = decode_ops(get_body);
    assert!(matches!(get_ops.as_slice(),
        [
            Operator::LocalGet { local_index: 0 },
            Operator::LocalGet { local_index: 1 },
            Operator::I32Add,
            Operator::F64Load { memarg },
            Operator::End
        ] if memarg.offset == 0
    ));

    let set_body = code_body_at(&out.wasm_binary, 13);
    let set_ops = decode_ops(set_body);
    assert!(matches!(set_ops.as_slice(),
        [
            Operator::LocalGet { local_index: 0 },
            Operator::LocalGet { local_index: 1 },
            Operator::I32Add,
            Operator::LocalGet { local_index: 2 },
            Operator::F64Store { memarg },
            Operator::End
        ] if memarg.offset == 0
    ));
}

#[test]
fn wasm_reports_clear_error_for_foreign_variable_access() {
    let (store, module) = build_foreign_global_var_module();
    let err = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect_err("foreign variables should be rejected in the current WASM mode");
    assert!(err.to_string().contains("FRS-CGEN-WASM-0003"));
    assert!(
        err.to_string().contains(
            "accessing foreign variable 'extvar' is not supported in this compilation mode"
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn wasm_module_imports_generic_foreign_function() {
    let (store, module) = build_generic_foreign_function_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("generic foreign functions should lower to host imports in WASM");

    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        let payload = payload.expect("payload should decode");
        if let Payload::ImportSection(section) = payload {
            for import in section {
                let import = import.expect("import should decode");
                imports.push(import.name.to_owned());
            }
        }
    }

    assert_eq!(imports, vec!["myhost"]);

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .any(|op| matches!(op, Operator::Call { function_index } if *function_index == 0))
    );
}

#[test]
fn wasm_float_comparisons_follow_operand_type_even_when_result_type_is_int32() {
    let (store, module) = build_float_compare_result_int32_module();
    let out = generate_wasm_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
    )
    .expect("WASM scaffold should lower float comparisons from operand type");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Gt)));
    assert!(!ops.iter().any(|op| matches!(op, Operator::I32GtS)));
}

#[test]
fn wasm_compute_lowers_struct_tables_and_select2() {
    let (store, module) = build_table_state_delay_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit struct-table compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::Select)));
    assert!(ops.iter().any(|op| matches!(op, Operator::I32GeS)));
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::F32Load { .. }))
            .count()
            >= 2
    );
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::F32Store { .. }))
            .count()
            >= 2
    );
}

#[test]
fn wasm_compute_loads_static_tables() {
    let (store, module) = build_static_table_compute_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit static-table compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .any(|op| matches!(op, Operator::I32Const { value } if *value == 4))
    );
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Load { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Store { .. })));
}

/// W1 of `porting/wasm-compute-speed-analysis-and-plan-2026-10-16-en.md`: a
/// constant address goes whole into the memarg `offset=` over a bare base, and
/// a dynamic index keeps the wrapping sum `base + (index * size + K)`.
#[test]
fn wasm_constant_addresses_fold_into_memarg_offset() {
    for internal_memory in [true, false] {
        let (store, module) = build_region_address_module();
        let options = WasmOptions {
            internal_memory,
            ..WasmOptions::default()
        };
        let out = generate_wasm_module(&store, module, &options)
            .expect("WASM backend should lower every region address form");
        Validator::new()
            .validate_all(&out.wasm_binary)
            .expect("generated module should validate");
        let offset = |name: &str| {
            out.memory_layout
                .field_offsets
                .get(name)
                .unwrap_or_else(|| panic!("`{name}` should be in the layout"))
                .offset
        };
        let (state, rec, idx, wav) = (
            offset("fState"),
            offset("fRec"),
            offset("fIdx"),
            offset("wav"),
        );
        assert!(
            rec > 0 && wav > 0,
            "the checks below need non-zero region offsets"
        );

        let ops = decode_ops(code_body_at(&out.wasm_binary, 1));
        let tokens = address_tokens(&ops);
        let has = |window: &[String]| tokens.windows(window.len()).any(|w| w == window);
        let t = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

        // Struct scalar, and struct table at constant indices 1 and 0.
        assert!(has(&t(&["local.get 0", &format!("f32.load {state}")])));
        assert!(has(&t(&["local.get 0", &format!("f32.load {}", rec + 4)])));
        assert!(has(&t(&["local.get 0", &format!("f32.load {rec}")])));
        assert!(tokens.contains(&format!("f32.store {rec}")));
        assert!(tokens.contains(&format!("f32.store {state}")));
        // Struct table at a dynamic index: dsp + (fIdx * 4 + K), offset 0.
        let dynamic = t(&[
            "local.get 0",
            "local.get 0",
            &format!("i32.load {idx}"),
            "i32.const 4",
            "i32.mul",
            &format!("i32.const {rec}"),
            "i32.add",
            "i32.add",
        ]);
        assert!(has(&[dynamic.clone(), t(&["f32.load 0"])].concat()));
        // Static table: absolute, constant index over `i32.const 0`, dynamic
        // index as the scaled index plus the table's address.
        assert!(has(&t(&["i32.const 0", &format!("f32.load {}", wav + 4)])));
        assert!(has(&t(&[
            "local.get 0",
            &format!("i32.load {idx}"),
            "i32.const 4",
            "i32.mul",
            &format!("i32.const {wav}"),
            "i32.add",
            "f32.load 0",
        ])));
        // The former `dsp + K` sequence is gone.
        assert!(
            !has(&t(&["local.get 0", "i32.const", "i32.add"])),
            "no `local.get 0; i32.const K; i32.add` address may remain"
        );
    }
}

#[test]
fn wasm_module_emits_static_table_data_segments() {
    let (store, module) = build_static_table_layout_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit static table data segment");

    assert!(out.dsp_json.contains("\"size\": 16"));
    // The emitter places the static tables after the mutable fields (W5).
    let wav_offset = out.memory_layout.field_offsets["wav"].offset;
    assert_eq!(wav_offset, out.memory_layout.tables_offset);
    assert!(wav_offset > out.memory_layout.field_offsets["fGain"].offset);

    let mut saw_json = false;
    let mut saw_table = false;
    let mut json_offset = None;
    let mut table_offset = None;
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        let payload = payload.expect("payload should decode");
        if let Payload::DataSection(section) = payload {
            for segment in section {
                let segment = segment.expect("data segment should decode");
                let wasmparser::DataKind::Active {
                    memory_index: 0,
                    offset_expr,
                } = segment.kind
                else {
                    continue;
                };
                let mut ops = offset_expr.get_operators_reader();
                let op = ops.read().expect("offset opcode");
                let offset = match op {
                    Operator::I32Const { value } => value,
                    _ => continue,
                };
                let data = segment.data;
                if data.starts_with(b"{") {
                    saw_json = true;
                    json_offset = Some(offset);
                }
                if offset == wav_offset as i32 && data == [0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 0, 64] {
                    saw_table = true;
                    table_offset = Some(offset);
                }
            }
        }
    }
    assert!(saw_json);
    assert!(saw_table);
    assert_eq!(table_offset, Some(wav_offset as i32));
    assert!(
        json_offset.expect("json data segment offset should be recorded")
            >= out.memory_layout.io_zone_offset as i32
    );
}

#[test]
fn wasm_compute_lowers_native_math_fun_calls() {
    let (store, module) = build_native_math_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit native math compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Abs)));
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Nearest)));
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Min)));
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Max)));
}

#[test]
fn wasm_compute_lowers_int_abs_inline() {
    let (store, module) = build_int_abs_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit inline int abs");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::I32Sub)));
    assert!(ops.iter().any(|op| matches!(op, Operator::I32LtS)));
    assert!(ops.iter().any(|op| matches!(op, Operator::Select)));
}

#[test]
fn wasm_compute_calls_internal_max_i_helper() {
    let (store, module) = build_internal_max_i_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit internal int helper call");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .any(|op| matches!(op, Operator::Call { function_index } if *function_index == 11))
    );
}

#[test]
fn wasm_module_imports_external_math_functions_for_compute() {
    let (store, module) = build_math_intrinsics_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit imported math declarations");

    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        let payload = payload.expect("payload should decode");
        if let Payload::ImportSection(section) = payload {
            for import in section {
                let import = import.expect("import should decode");
                imports.push(import.name.to_owned());
            }
        }
    }

    assert_eq!(imports, vec!["_atan2", "_cos", "_pow", "_sin"]);
}

/// A math call found only under a `WhileLoop` is imported: the import walk
/// used a hand-rolled child list that stopped at `WhileLoop`, `Switch`,
/// `TeeVar`, `Bitcast` and the soundfile loads.
#[test]
fn wasm_imports_a_math_call_found_only_inside_a_while_loop() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let x = b.float32(0.5);
    let call = b.fun_call("sin", &[x], FirType::Float32);
    let scratch = b.declare_var("fScratch", FirType::Float32, AccessType::Stack, Some(call));
    let loop_body = b.block(&[scratch]);
    let never = b.int32(0);
    let while_loop = b.while_loop(never, loop_body);
    let body = b.block(&[while_loop]);
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &crate::fixtures::compute_fun_args(),
        Some(body),
        false,
    );
    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "while_import",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );

    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("a sin call under a while loop compiles");
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        if let Payload::ImportSection(section) = payload.expect("payload should decode") {
            for import in section {
                imports.push(import.expect("import should decode").name.to_owned());
            }
        }
    }
    assert_eq!(imports, vec!["_sinf"]);
}

#[test]
fn wasm_compute_calls_imported_math_functions() {
    let (store, module) = build_math_intrinsics_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit imported math compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::Call { function_index } if *function_index < 4))
            .count()
            >= 4
    );
}

#[test]
fn wasm_module_imports_foreign_float_helpers_without_falling_back_to_empty_compute() {
    let (store, module) = build_foreign_float_helper_module();
    let out = generate_wasm_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
    )
    .expect("WASM backend should lower foreign float helpers in compute");

    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        let payload = payload.expect("payload should decode");
        if let Payload::ImportSection(section) = payload {
            for import in section {
                let import = import.expect("import should decode");
                imports.push(import.name.to_owned());
            }
        }
    }

    assert_eq!(imports, vec!["_acosh", "_copysignf", "_isnanf"]);

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::Call { function_index } if *function_index < 3))
            .count()
            >= 3
    );
    assert!(ops.iter().any(|op| matches!(op, Operator::Loop { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F64Store { .. })));
}

#[test]
fn wasm_compute_lowers_control_flow_statements() {
    let (store, module) = build_control_flow_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit control-flow compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::If { .. }))
            .count()
            >= 2
    );
    assert!(ops.iter().any(|op| matches!(op, Operator::Else)));
    // Raw codegen accepts manually constructed FIR too; canonical compiler
    // output has already swept these pure Drop roots before reaching WASM.
    assert!(ops.iter().any(|op| matches!(op, Operator::Drop)));
    assert!(ops.iter().any(|op| matches!(op, Operator::I32Eq)));
}

#[test]
fn wasm_compute_lowers_for_and_while_loops() {
    let (store, module) = build_loop_statement_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit for/while compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::Loop { .. }))
            .count()
            >= 2
    );
    assert!(ops.iter().any(|op| matches!(op, Operator::I32Eqz)));
    assert!(ops.iter().any(|op| matches!(op, Operator::I32GeS)));
    assert!(ops.iter().any(|op| matches!(op, Operator::I32Add)));
}

#[test]
fn wasm_get_sample_rate_loads_struct_field_when_present() {
    let (store, module) = build_sample_rate_state_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit getSampleRate body");

    let body = code_body_at(&out.wasm_binary, 5);
    let ops = decode_ops(body);
    assert!(matches!(ops.as_slice(),
        [
            Operator::LocalGet { local_index: 0 },
            Operator::I32Load { memarg },
            Operator::End
        ] if memarg.offset == 0
    ));
}

#[test]
fn wasm_get_param_value_loads_real_from_dsp_plus_index() {
    let (store, module) = build_passthrough_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit getParamValue body");

    let body = code_body_at(&out.wasm_binary, 4);
    let ops = decode_ops(body);
    assert!(matches!(ops.as_slice(),
        [
            Operator::LocalGet { local_index: 0 },
            Operator::LocalGet { local_index: 1 },
            Operator::I32Add,
            Operator::F32Load { memarg },
            Operator::End
        ] if memarg.offset == 0
    ));
}

#[test]
fn wasm_set_param_value_stores_real_to_dsp_plus_index() {
    let (store, module) = build_passthrough_test_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit setParamValue body");

    let body = code_body_at(&out.wasm_binary, 13);
    let ops = decode_ops(body);
    assert!(matches!(ops.as_slice(),
        [
            Operator::LocalGet { local_index: 0 },
            Operator::LocalGet { local_index: 1 },
            Operator::I32Add,
            Operator::LocalGet { local_index: 2 },
            Operator::F32Store { memarg },
            Operator::End
        ] if memarg.offset == 0
    ));
}

#[test]
fn wasm_instance_constants_stores_sample_rate_when_field_exists() {
    let (store, module) = build_sample_rate_state_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit instanceConstants body");

    let body = code_body_at(&out.wasm_binary, 8);
    let ops = decode_ops(body);
    assert!(matches!(ops.as_slice(),
        [
            Operator::LocalGet { local_index: 0 },
            Operator::LocalGet { local_index: 1 },
            Operator::I32Store { memarg },
            Operator::End
        ] if memarg.offset == 0
    ));
}

#[test]
fn wasm_instance_clear_lowers_struct_state_reset_body() {
    let (store, module) = build_instance_clear_state_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM scaffold should emit instanceClear body");

    let body = code_body_at(&out.wasm_binary, 7);
    let ops = decode_ops(body);
    assert!(ops.iter().any(|op| matches!(op, Operator::Loop { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::BrIf { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::I32Store { .. })));
    assert!(ops.iter().any(|op| matches!(op, Operator::F32Store { .. })));
}

#[test]
fn wasm_layout_tracks_struct_offsets_for_sine_fixture() {
    let (store, module) = build_sine_phasor_test_module();
    let layout = WasmMemoryLayout::from_module(&store, module, &WasmOptions::default(), 64)
        .expect("sine fixture layout should compute");

    assert_eq!(layout.struct_size, 16);
    assert_eq!(layout.tables_offset, 0);
    assert_eq!(layout.io_zone_offset, 16);
    assert_eq!(layout.field_offsets["fFreq"].offset, 0);
    assert_eq!(layout.field_offsets["fFreq"].size, 4);
    assert_eq!(layout.field_offsets["fGain"].offset, 4);
    assert_eq!(layout.field_offsets["fPhase"].offset, 8);
    assert_eq!(layout.field_offsets["fPhase"].size, 8);
    assert_eq!(layout.pages, 1);
}

#[test]
fn wasm_layout_pads_i32_fields_to_audio_slot_in_double_mode() {
    let (store, module) = build_single_i32_state_module();
    let layout = WasmMemoryLayout::from_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
        32,
    )
    .expect("double-precision layout should compute");

    assert_eq!(layout.struct_size, 8);
    assert_eq!(layout.field_offsets["fMode"].offset, 0);
    assert_eq!(layout.field_offsets["fMode"].size, 8);
}

#[test]
fn wasm_layout_uses_f64_storage_for_faustfloat_in_double_mode() {
    let (store, module) = build_sine_phasor_test_module();
    let layout = WasmMemoryLayout::from_module(
        &store,
        module,
        &WasmOptions {
            double_precision: true,
            ..WasmOptions::default()
        },
        64,
    )
    .expect("double-precision sine fixture layout should compute");

    assert_eq!(layout.struct_size, 24);
    assert_eq!(layout.field_offsets["fFreq"].offset, 0);
    assert_eq!(layout.field_offsets["fFreq"].size, 8);
    assert_eq!(layout.field_offsets["fGain"].offset, 8);
    assert_eq!(layout.field_offsets["fGain"].size, 8);
    assert_eq!(layout.field_offsets["fPhase"].offset, 16);
    assert_eq!(layout.field_offsets["fPhase"].size, 8);
}

#[test]
fn wasm_layout_places_static_tables_after_struct_region() {
    let (store, module) = build_static_table_layout_module();
    let layout = WasmMemoryLayout::from_module(&store, module, &WasmOptions::default(), 32)
        .expect("layout with static table should compute");

    assert_eq!(layout.struct_size, 16);
    assert_eq!(layout.tables_offset, 0);
    assert_eq!(layout.field_offsets["wav"].offset, 0);
    assert_eq!(layout.field_offsets["wav"].size, 12);
    assert_eq!(layout.field_offsets["fGain"].offset, 12);
    assert_eq!(layout.io_zone_offset, 16);
    assert!(layout.json_offset >= layout.io_zone_offset);
}

#[test]
fn wasm_json_indices_follow_the_emitted_layout() {
    let (store, module) = build_static_table_ui_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM backend should expose UI indices as runtime byte offsets");

    assert!(
        out.dsp_json
            .contains("\"address\": \"/StaticTableUI/gain\"")
    );
    // The control is a 4-byte field: it precedes the static table, which the
    // emitter places last, and its JSON index is its emitted offset.
    let gain = out.memory_layout.field_offsets["fGain"].offset;
    assert_eq!(gain, 0);
    assert!(out.memory_layout.tables_offset > gain);
    assert!(out.dsp_json.contains(&format!("\"index\": {gain}")));
}

/// W5 of `porting/wasm-compute-speed-analysis-and-plan-2026-10-16-en.md`: the
/// emitter lays the mutable fields out by increasing size, then the static
/// tables, while `WasmMemoryLayout::from_module` keeps the declaration order
/// (static tables first) that the C++-style JSON of the other backends uses.
#[test]
fn wasm_emitter_lays_out_small_fields_first_and_static_tables_last() {
    let (store, module) = build_mixed_size_fields_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM backend should lay out the mixed-size fixture");
    Validator::new()
        .validate_all(&out.wasm_binary)
        .expect("generated module should validate");
    let emitted = |name: &str| out.memory_layout.field_offsets[name].offset;
    assert_eq!(emitted("fState"), 0);
    assert_eq!(emitted("fRec"), 4);
    assert_eq!(emitted("fDelay"), 12);
    assert_eq!(out.memory_layout.tables_offset, 12 + 64 * 4);
    assert_eq!(emitted("wav"), out.memory_layout.tables_offset);
    assert_eq!(out.memory_layout.struct_size, 12 + 64 * 4 + 12);

    let declared = WasmMemoryLayout::from_module(&store, module, &WasmOptions::default(), 0)
        .expect("declaration-order layout should compute");
    let declared = |name: &str| declared.field_offsets[name].offset;
    assert_eq!(declared("wav"), 0);
    assert_eq!(declared("fDelay"), 12);
    assert_eq!(declared("fState"), 12 + 64 * 4);
    assert_eq!(declared("fRec"), 12 + 64 * 4 + 4);
}

#[test]
fn wasm_soundfile_modules_import_memory_and_expose_soundfile_offset_zero() {
    let (store, module) = build_soundfile_access_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("WASM backend should lower soundfile modules without empty compute fallback");

    assert!(out.dsp_json.contains("\"type\": \"soundfile\""));
    assert!(out.dsp_json.contains("\"varname\": \"fSound0\""));
    assert!(out.dsp_json.contains("\"index\": 0"));

    let mut imported_memory = false;
    let mut exported_memory = false;
    for payload in Parser::new(0).parse_all(&out.wasm_binary) {
        let payload = payload.expect("payload should decode");
        match payload {
            Payload::ImportSection(section) => {
                for import in section {
                    let import = import.expect("import should decode");
                    if import.module == "env"
                        && import.name == "memory"
                        && matches!(import.ty, wasmparser::TypeRef::Memory(_))
                    {
                        imported_memory = true;
                    }
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.expect("export should decode");
                    if export.name == "memory" {
                        exported_memory = true;
                    }
                }
            }
            _ => {}
        }
    }

    assert!(imported_memory, "soundfile modules should import memory");
    assert!(
        !exported_memory,
        "soundfile modules should not export an internal memory"
    );
}

#[test]
fn wasm_compute_lowers_soundfile_access_nodes() {
    let (store, module) = build_soundfile_access_module();
    let out = generate_wasm_module(&store, module, &WasmOptions::default())
        .expect("soundfile access should lower into a real compute body");

    let body = code_body_at(&out.wasm_binary, 1);
    let ops = decode_ops(body);

    assert!(ops.iter().any(|op| matches!(op, Operator::Loop { .. })));
    assert!(
        ops.iter()
            .filter(|op| matches!(op, Operator::I32Load { .. }))
            .count()
            >= 4,
        "soundfile compute should chase pointer/int arrays"
    );
    assert!(
        ops.iter().any(|op| matches!(op, Operator::F32Load { .. })),
        "soundfile compute should load sample data"
    );
    // `fChannels` is the int32 after the four 4-byte pointers of `Soundfile`.
    assert!(
        ops.windows(3).any(|w| matches!(
            w,
            [
                Operator::I32Const { value: 16 },
                Operator::I32Add,
                Operator::I32Load { .. }
            ]
        )),
        "soundfile compute should load fChannels at offset 16"
    );
    assert!(
        ops.iter().any(|op| matches!(op, Operator::I32RemS)),
        "soundfile compute should wrap the channel modulo fChannels"
    );
    assert!(
        ops.iter().any(|op| matches!(op, Operator::F32Store { .. })),
        "soundfile compute should write output samples"
    );
}

fn build_sample_rate_state_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let zero = b.int32(0);
    let sample_rate = b.declare_var(
        "fSampleRate",
        FirType::Int32,
        AccessType::Struct,
        Some(zero),
    );
    let globals = b.block(&[sample_rate]);
    let dsp_struct = b.block(&[]);
    let compute = declare_trivial_compute(&mut b);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "sr_dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_soundfile_access_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let sample_rate_init = b.int32(48_000);
    let sample_rate = b.declare_var(
        "fSampleRate",
        FirType::Int32,
        AccessType::Struct,
        Some(sample_rate_init),
    );
    let soundfile = b.declare_var("fSound0", FirType::Sound, AccessType::Struct, None);
    let gain = b.declare_var("fGain", FirType::FaustFloat, AccessType::Struct, None);
    let dsp_struct = b.block(&[sample_rate, soundfile, gain]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let open = b.open_box(fir::UiBoxType::Vertical, "SoundfileTest");
    let add_soundfile = b.add_soundfile_with_url(
        "Drone_1",
        "{'Alonepad_reverb_stereo_instru1.flac'; 'Dronepad_test_stereo_instru1.flac'}",
        "fSound0",
    );
    let slider = b.add_slider(
        fir::SliderType::Horizontal,
        "gain",
        "fGain",
        SliderRange {
            init: 0.5,
            lo: 0.0,
            hi: 1.0,
            step: 0.01,
        },
    );
    let close = b.close_box();
    let ui_body = b.block(&[open, add_soundfile, slider, close]);
    let ui = b.declare_fun(
        "buildUserInterface",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::UI],
            ret: Box::new(FirType::Void),
        },
        &[
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "ui_interface".to_owned(),
                typ: FirType::UI,
            },
        ],
        Some(ui_body),
        false,
    );

    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let args = crate::fixtures::compute_fun_args();
    let zero_output = b.int32(0);
    let output0_ptr = b.load_table("outputs", AccessType::FunArgs, zero_output, ptr_ty.clone());
    let output0 = b.declare_var(
        "output0",
        ptr_ty.clone(),
        AccessType::Stack,
        Some(output0_ptr),
    );
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let zero = b.int32(0);
    let rate_i32 = b.load_soundfile_rate("fSound0", zero);
    let rate = b.cast(FirType::Float32, rate_i32);
    let len_i32 = b.load_soundfile_length("fSound0", zero);
    let len = b.cast(FirType::Float32, len_i32);
    // Channel 1 wrapped as the lowerer builds it: `1 % fSound0->fChannels`.
    let one = b.int32(1);
    let channels = b.load_soundfile_channels("fSound0");
    let chan = b.binop(fir::FirBinOp::Rem, one, channels, FirType::Int32);
    let sample = b.load_soundfile_buffer("fSound0", chan, zero, i0, FirType::Float32);
    let gain_cur = b.load_var("fGain", AccessType::Struct, FirType::FaustFloat);
    let gain_f32 = b.cast(FirType::Float32, gain_cur);
    let sample_gain = b.binop(fir::FirBinOp::Mul, sample, gain_f32, FirType::Float32);
    let rate_len = b.binop(fir::FirBinOp::Add, rate, len, FirType::Float32);
    let output_sample = b.binop(fir::FirBinOp::Add, sample_gain, rate_len, FirType::Float32);
    let store_out = b.store_table("output0", AccessType::Stack, i0, output_sample);
    let loop_body = b.block(&[store_out]);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let loop_stmt = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[output0, loop_stmt]);
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );

    let reset_gain = b.float32(0.5);
    let reset_store = b.store_var("fGain", AccessType::Struct, reset_gain);
    let reset_body = b.block(&[reset_store]);
    let reset_ui = b.declare_fun(
        "instanceResetUserInterface",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj))],
            ret: Box::new(FirType::Void),
        },
        &[NamedType {
            name: "dsp".to_owned(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        }],
        Some(reset_body),
        false,
    );

    let sample_rate_arg = b.load_var("sample_rate", AccessType::FunArgs, FirType::Int32);
    let sample_rate_store = b.store_var("fSampleRate", AccessType::Struct, sample_rate_arg);
    let constants_body = b.block(&[sample_rate_store]);
    let constants = b.declare_fun(
        "instanceConstants",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Int32],
            ret: Box::new(FirType::Void),
        },
        &[
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "sample_rate".to_owned(),
                typ: FirType::Int32,
            },
        ],
        Some(constants_body),
        false,
    );

    let clear = declare_trivial_instance_clear(&mut b);
    let functions = b.block(&[constants, reset_ui, clear, ui, compute]);
    let module = b.module(
        0,
        1,
        "soundfile_test",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_static_table_ui_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let gain = b.declare_var("fGain", FirType::FaustFloat, AccessType::Struct, None);
    let dsp_struct = b.block(&[gain]);
    let globals = b.block(&[]);

    let zero = b.float32(0.0);
    let one = b.float32(1.0);
    let two = b.float32(2.0);
    let wav = b.declare_table(
        "wav",
        AccessType::Static,
        FirType::Float32,
        &[zero, one, two],
    );
    let static_decls = b.block(&[wav]);

    let open = b.open_box(fir::UiBoxType::Vertical, "StaticTableUI");
    let add = b.add_slider(
        fir::SliderType::Horizontal,
        "gain",
        "fGain",
        SliderRange {
            init: 0.5,
            lo: 0.0,
            hi: 1.0,
            step: 0.01,
        },
    );
    let close = b.close_box();
    let ui_body = b.block(&[open, add, close]);
    let ui = b.declare_fun(
        "buildUserInterface",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::UI],
            ret: Box::new(FirType::Void),
        },
        &[
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "ui_interface".to_owned(),
                typ: FirType::UI,
            },
        ],
        Some(ui_body),
        false,
    );

    let compute = declare_trivial_compute(&mut b);
    let functions = b.block(&[ui, compute]);
    let module = b.module(
        0,
        1,
        "static_table_ui",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_float_compare_result_int32_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let threshold_init = b.float64(0.25);
    let threshold = b.declare_var(
        "fThreshold",
        FirType::Float64,
        AccessType::Struct,
        Some(threshold_init),
    );
    let globals = b.block(&[threshold]);
    let dsp_struct = b.block(&[]);

    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let count_arg = NamedType {
        name: "count".to_string(),
        typ: FirType::Int32,
    };
    let inputs_arg = NamedType {
        name: "inputs".to_string(),
        typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
    };
    let outputs_arg = NamedType {
        name: "outputs".to_string(),
        typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
    };
    let input0 = NamedType {
        name: "input0".to_string(),
        typ: ptr_ty.clone(),
    };
    let output0 = NamedType {
        name: "output0".to_string(),
        typ: ptr_ty.clone(),
    };

    let inputs_ptr = b.load_var("inputs", AccessType::FunArgs, inputs_arg.typ.clone());
    let zero_index = b.int32(0);
    let input0_ptr = b.load_table(
        "inputs",
        AccessType::FunArgs,
        zero_index,
        input0.typ.clone(),
    );
    let in_alias = b.declare_var(
        "input0",
        input0.typ.clone(),
        AccessType::Stack,
        Some(input0_ptr),
    );

    let outputs_ptr = b.load_var("outputs", AccessType::FunArgs, outputs_arg.typ.clone());
    let zero_out_index = b.int32(0);
    let output0_ptr = b.load_table(
        "outputs",
        AccessType::FunArgs,
        zero_out_index,
        output0.typ.clone(),
    );
    let out_alias = b.declare_var(
        "output0",
        output0.typ.clone(),
        AccessType::Stack,
        Some(output0_ptr),
    );

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let sample = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let sample_f64 = b.cast(FirType::Float64, sample);
    let threshold_cur = b.load_var("fThreshold", AccessType::Struct, FirType::Float64);
    let cmp = b.binop(fir::FirBinOp::Gt, sample_f64, threshold_cur, FirType::Int32);
    let zero_sample = b.float64(0.0);
    let gated = b.select2(cmp, sample, zero_sample, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, gated);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                inputs_arg.typ.clone(),
                outputs_arg.typ.clone(),
            ],
            ret: Box::new(FirType::Void),
        },
        &[
            NamedType {
                name: "dsp".to_string(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            count_arg,
            inputs_arg,
            outputs_arg,
        ],
        Some(compute_body),
        false,
    );

    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "float_compare_result_int32",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    let _ = inputs_ptr;
    let _ = outputs_ptr;
    (store, module)
}

fn build_labeled_passthrough_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let in_ptr = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let in_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in_ptr));
    let out_alias = b.declare_var("output0", ptr_ty.clone(), AccessType::Stack, Some(out_ptr));

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let write = b.store_table("output0", AccessType::Stack, i0, x);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let label = b.label("signal_fir_fastlane_step2a: executable base slice");
    let compute_body = b.block(&[label, in_alias, out_alias, sample_loop]);
    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );

    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        1,
        1,
        "labeled_passthrough",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_local_array_buffer_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let out_chan = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    let vbuf = b.declare_var(
        "vbuf0",
        FirType::Array(Box::new(FirType::FaustFloat), 32),
        AccessType::Stack,
        None,
    );
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let value = b.float32(0.25);
    let store_vbuf = b.store_table("vbuf0", AccessType::Stack, i0, value);
    let read_vbuf = b.load_table("vbuf0", AccessType::Stack, i0, FirType::FaustFloat);
    let write_out = b.store_table("output0", AccessType::Stack, i0, read_vbuf);
    let loop_body = b.block(&[store_vbuf, write_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, vbuf, sample_loop]);
    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: args.iter().map(|arg| arg.typ.clone()).collect(),
            ret: Box::new(FirType::Void),
        },
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        1,
        "local_array_buffer",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_instance_clear_state_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let zero_i = b.int32(0);
    let scalar = b.declare_var("fIOTA", FirType::Int32, AccessType::Struct, Some(zero_i));
    let delay = b.declare_var(
        "fDelay",
        FirType::Array(Box::new(FirType::FaustFloat), 4),
        AccessType::Struct,
        None,
    );
    let dsp_struct = b.block(&[scalar, delay]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let zero_f = b.float32(0.0);
    let clear_scalar = b.store_var("fIOTA", AccessType::Struct, zero_i);
    let clear_delay = b.store_table("fDelay", AccessType::Struct, i0, zero_f);
    let clear_body = b.block(&[clear_delay]);
    let four = b.int32(4);
    let clear_loop = b.simple_for_loop("i0", four, clear_body, false);
    let instance_clear_body = b.block(&[clear_scalar, clear_loop]);

    let clear_args = [NamedType {
        name: "dsp".to_owned(),
        typ: FirType::Ptr(Box::new(FirType::Obj)),
    }];
    let instance_clear = b.declare_fun(
        "instanceClear",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj))],
            ret: Box::new(FirType::Void),
        },
        &clear_args,
        Some(instance_clear_body),
        false,
    );

    let compute = declare_trivial_compute(&mut b);
    let functions = b.block(&[compute, instance_clear]);
    let module = b.module(
        0,
        0,
        "instance_clear_state",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_struct_state_cast_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let bias_init = b.float32(0.5);
    let level_init = b.float64(0.0);
    let bias = b.declare_var(
        "fBias",
        FirType::FaustFloat,
        AccessType::Struct,
        Some(bias_init),
    );
    let level = b.declare_var(
        "fLevel",
        FirType::Float64,
        AccessType::Struct,
        Some(level_init),
    );
    let globals = b.block(&[bias, level]);
    let dsp_struct = b.block(&[]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let in_ptr = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let in_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in_ptr));
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let bias_cur = b.load_var("fBias", AccessType::Struct, FirType::FaustFloat);
    let y = b.binop(fir::FirBinOp::Add, x, bias_cur, FirType::FaustFloat);
    let y_f64 = b.cast(FirType::Float64, y);
    let store_level = b.store_var("fLevel", AccessType::Struct, y_f64);
    let store_bias = b.store_var("fBias", AccessType::Struct, y);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_level, store_bias, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);

    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        1,
        1,
        "state_cast_dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_native_math_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let in_ptr = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let in_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in_ptr));
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let x_f64 = b.cast(FirType::Float64, x);
    let absx = b.math_call(FirMathOp::Abs, &[x_f64], FirType::Float64);
    let one = b.float64(1.0);
    let zero = b.float64(0.0);
    let minv = b.math_call(FirMathOp::Min, &[absx, one], FirType::Float64);
    let maxv = b.math_call(FirMathOp::Max, &[minv, zero], FirType::Float64);
    let rounded = b.math_call(FirMathOp::Rint, &[maxv], FirType::Float64);
    let y = b.cast(FirType::FaustFloat, rounded);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);

    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        1,
        1,
        "native_math_dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_internal_max_i_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let clamped = b.fun_call("max_i", &[i0, count], FirType::Int32);
    let clamped_f32 = b.cast(FirType::FaustFloat, clamped);
    let write = b.store_table("output0", AccessType::Stack, i0, clamped_f32);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        1,
        "internal_max_i",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_int_abs_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let neg_i0 = b.neg(i0, FirType::Int32);
    let abs_i0 = b.fun_call("abs", &[neg_i0], FirType::Int32);
    let abs_f32 = b.cast(FirType::FaustFloat, abs_i0);
    let write = b.store_table("output0", AccessType::Stack, i0, abs_f32);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        1,
        "int_abs",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_loop_statement_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));

    let acc_zero = b.int32(0);
    let acc_decl = b.declare_var("acc", FirType::Int32, AccessType::Stack, Some(acc_zero));

    let false_cond = b.bool_(false);
    let drop_zero = b.drop_(acc_zero);
    let while_body = b.block(&[drop_zero]);
    let while_loop = b.while_loop(false_cond, while_body);

    let init = b.int32(0);
    let end = b.int32(4);
    let step = b.int32(1);
    let i = b.load_var("i", AccessType::Loop, FirType::Int32);
    let acc_cur = b.load_var("acc", AccessType::Stack, FirType::Int32);
    let acc_next = b.binop(fir::FirBinOp::Add, acc_cur, i, FirType::Int32);
    let store_acc = b.store_var("acc", AccessType::Stack, acc_next);
    let i_f32 = b.cast(FirType::FaustFloat, i);
    let store_out = b.store_table("output0", AccessType::Stack, i, i_f32);
    let for_body = b.block(&[store_acc, store_out]);
    let for_loop = b.for_loop("i", init, end, step, for_body, false);

    let compute_body = b.block(&[out_alias, acc_decl, while_loop, for_loop]);
    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "loop_stmt_dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_single_i32_state_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let zero = b.int32(0);
    let mode = b.declare_var("fMode", FirType::Int32, AccessType::Struct, Some(zero));
    let globals = b.block(&[mode]);
    let dsp_struct = b.block(&[]);
    let compute = declare_trivial_compute(&mut b);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "mode_dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_static_table_layout_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let gain_init = b.float32(0.0);
    let gain = b.declare_var(
        "fGain",
        FirType::FaustFloat,
        AccessType::Struct,
        Some(gain_init),
    );
    let globals = b.block(&[gain]);
    let w0 = b.float32(0.0);
    let w1 = b.float32(1.0);
    let w2 = b.float32(2.0);
    let wav = b.declare_table(
        "wav",
        AccessType::Static,
        FirType::FaustFloat,
        &[w0, w1, w2],
    );
    let dsp_struct = b.block(&[]);
    let compute = declare_trivial_compute(&mut b);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[wav]);
    let module = b.module(
        0,
        1,
        "table_dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

/// `compute` touching every address form of `emit_region_address`: a struct
/// scalar (`fState`), a struct table at constant and dynamic indices (`fRec`,
/// indexed by the struct scalar `fIdx`) and a static table at a constant and a
/// dynamic index (`wav`, after a leading static table `lead`).
fn build_region_address_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let z = b.float32(0.0);
    let zero_i = b.int32(0);
    let state = b.declare_var("fState", FirType::FaustFloat, AccessType::Struct, Some(z));
    let rec = b.declare_table("fRec", AccessType::Struct, FirType::FaustFloat, &[z, z]);
    let idx = b.declare_var("fIdx", FirType::Int32, AccessType::Struct, Some(zero_i));
    let dsp_struct = b.block(&[state, rec, idx]);
    let globals = b.block(&[]);
    let w0 = b.float32(0.0);
    let w1 = b.float32(1.0);
    let w2 = b.float32(2.0);
    // Static tables are laid out first: `lead` puts `wav` at a non-zero address.
    let lead = b.declare_table("lead", AccessType::Static, FirType::FaustFloat, &[w0]);
    let wav = b.declare_table(
        "wav",
        AccessType::Static,
        FirType::FaustFloat,
        &[w0, w1, w2],
    );
    let static_decls = b.block(&[lead, wav]);

    let ff = FirType::FaustFloat;
    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);

    let one = b.int32(1);
    let rec1 = b.load_table("fRec", AccessType::Struct, one, ff.clone());
    let state_cur = b.load_var("fState", AccessType::Struct, ff.clone());
    let sum = b.binop(fir::FirBinOp::Add, rec1, state_cur, ff.clone());
    let write_rec0 = b.store_table("fRec", AccessType::Struct, zero_i, sum);
    let rec0 = b.load_table("fRec", AccessType::Struct, zero_i, ff.clone());
    let write_state = b.store_var("fState", AccessType::Struct, rec0);
    let dyn_idx = b.load_var("fIdx", AccessType::Struct, FirType::Int32);
    let rec_dyn = b.load_table("fRec", AccessType::Struct, dyn_idx, ff.clone());
    let wav_const = b.load_table("wav", AccessType::Static, one, ff.clone());
    let wav_dyn = b.load_table("wav", AccessType::Static, dyn_idx, ff.clone());
    let mix = b.binop(fir::FirBinOp::Add, rec_dyn, wav_const, ff.clone());
    let mix = b.binop(fir::FirBinOp::Add, mix, wav_dyn, ff);
    let tmp = b.declare_var("tmpMix", FirType::FaustFloat, AccessType::Stack, Some(mix));
    let tmp_cur = b.load_var("tmpMix", AccessType::Stack, FirType::FaustFloat);
    let write_rec_dyn = b.store_table("fRec", AccessType::Struct, dyn_idx, tmp_cur);
    let write_out = b.store_table("output0", AccessType::Stack, i0, tmp_cur);
    let loop_body = b.block(&[write_rec0, write_state, tmp, write_rec_dyn, write_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);

    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        1,
        "region_address",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

/// A delay line (64 floats), a scalar and a two-slot state, declared in that
/// order, plus one static table: the fields `from_module` and the emitter
/// order differently.
fn build_mixed_size_fields_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let z = b.float32(0.0);
    let delay = b.declare_var(
        "fDelay",
        FirType::Array(Box::new(FirType::FaustFloat), 64),
        AccessType::Struct,
        None,
    );
    let state = b.declare_var("fState", FirType::FaustFloat, AccessType::Struct, Some(z));
    let rec = b.declare_var(
        "fRec",
        FirType::Array(Box::new(FirType::FaustFloat), 2),
        AccessType::Struct,
        None,
    );
    let dsp_struct = b.block(&[delay, state, rec]);
    let globals = b.block(&[]);
    let w0 = b.float32(0.0);
    let w1 = b.float32(1.0);
    let w2 = b.float32(2.0);
    let wav = b.declare_table(
        "wav",
        AccessType::Static,
        FirType::FaustFloat,
        &[w0, w1, w2],
    );
    let static_decls = b.block(&[wav]);
    let compute = declare_trivial_compute(&mut b);
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        1,
        "mixed_size_fields",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_static_table_compute_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);

    let w0 = b.float32(0.0);
    let w1 = b.float32(1.0);
    let w2 = b.float32(2.0);
    let wav = b.declare_table(
        "wav",
        AccessType::Static,
        FirType::FaustFloat,
        &[w0, w1, w2],
    );
    let static_decls = b.block(&[wav]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let sample = b.load_table("wav", AccessType::Static, i0, FirType::FaustFloat);
    let write = b.store_table("output0", AccessType::Stack, i0, sample);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);

    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        1,
        "static_table_compute",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_struct_table_instance_constants_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let sample_rate = b.declare_var("fSampleRate", FirType::Int32, AccessType::Struct, None);
    let zero0 = b.float32(0.0);
    let zero1 = b.float32(0.0);
    let table = b.declare_table(
        "fTable",
        AccessType::Struct,
        FirType::FaustFloat,
        &[zero0, zero1],
    );
    let globals = b.block(&[sample_rate, table]);
    let static_decls = b.block(&[]);

    let sample_rate_arg = b.load_var("sample_rate", AccessType::FunArgs, FirType::Int32);
    let sr_store = b.store_var("fSampleRate", AccessType::Struct, sample_rate_arg);
    let table_idx0 = b.int32(0);
    let table_idx1 = b.int32(1);
    let table_val0 = b.float32(4.0);
    let table_val1 = b.float32(7.0);
    let slot0 = b.store_table("fTable", AccessType::Struct, table_idx0, table_val0);
    let slot1 = b.store_table("fTable", AccessType::Struct, table_idx1, table_val1);
    let instance_constants_body = b.block(&[sr_store, slot0, slot1]);

    let instance_constants = b.declare_fun(
        "instanceConstants",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Int32],
            ret: Box::new(FirType::Void),
        },
        &[
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "sample_rate".to_owned(),
                typ: FirType::Int32,
            },
        ],
        Some(instance_constants_body),
        false,
    );

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let one = b.int32(1);
    let index = b.binop(fir::FirBinOp::And, i0, one, FirType::Int32);
    let sample = b.load_table("fTable", AccessType::Struct, index, FirType::FaustFloat);
    let write = b.store_table("output0", AccessType::Stack, i0, sample);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);

    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &crate::fixtures::compute_fun_args(),
        Some(compute_body),
        false,
    );

    let functions = b.block(&[instance_constants, compute]);
    let module = b.module(
        0,
        1,
        "instance_constants_struct_table",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_foreign_float_helper_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let chan0 = b.int32(0);
    let chan1 = b.int32(1);
    let chan2 = b.int32(2);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));

    let in0 = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let in1 = b.load_table("inputs", AccessType::FunArgs, chan1, ptr_ty.clone());
    let in2 = b.load_table("inputs", AccessType::FunArgs, chan2, ptr_ty.clone());
    let out0 = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out1 = b.load_table("outputs", AccessType::FunArgs, chan1, ptr_ty.clone());
    let out2 = b.load_table("outputs", AccessType::FunArgs, chan2, ptr_ty.clone());

    let in0_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in0));
    let in1_alias = b.declare_var("input1", ptr_ty.clone(), AccessType::Stack, Some(in1));
    let in2_alias = b.declare_var("input2", ptr_ty.clone(), AccessType::Stack, Some(in2));
    let out0_alias = b.declare_var("output0", ptr_ty.clone(), AccessType::Stack, Some(out0));
    let out1_alias = b.declare_var("output1", ptr_ty.clone(), AccessType::Stack, Some(out1));
    let out2_alias = b.declare_var("output2", ptr_ty, AccessType::Stack, Some(out2));

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);

    let input0_sample = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let input1_sample = b.load_table("input1", AccessType::Stack, i0, FirType::FaustFloat);
    let input2_sample = b.load_table("input2", AccessType::Stack, i0, FirType::FaustFloat);

    let input0_f32 = b.cast(FirType::Float32, input0_sample);
    let input1_f32 = b.cast(FirType::Float32, input1_sample);
    let input2_f32 = b.cast(FirType::Float32, input2_sample);
    let ten = b.float32(10.0);

    let isnanf = b.fun_call("isnanf", &[input0_f32], FirType::Int32);
    let copysignf = b.fun_call("copysignf", &[input1_f32, input2_f32], FirType::Float32);
    let shifted = b.binop(fir::FirBinOp::Add, input2_f32, ten, FirType::Float32);
    let acoshf = b.fun_call("acoshf", &[shifted], FirType::Float32);
    let isnanf_out = b.cast(FirType::FaustFloat, isnanf);
    let copysignf_out = b.cast(FirType::FaustFloat, copysignf);
    let acoshf_out = b.cast(FirType::FaustFloat, acoshf);

    let out0_store = b.store_table("output0", AccessType::Stack, i0, isnanf_out);
    let out1_store = b.store_table("output1", AccessType::Stack, i0, copysignf_out);
    let out2_store = b.store_table("output2", AccessType::Stack, i0, acoshf_out);
    let loop_body = b.block(&[out0_store, out1_store, out2_store]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[
        in0_alias,
        in1_alias,
        in2_alias,
        out0_alias,
        out1_alias,
        out2_alias,
        sample_loop,
    ]);

    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        3,
        3,
        "foreign_float_helpers",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn build_generic_foreign_function_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let static_decls = b.block(&[]);

    let proto_args = [NamedType {
        name: "arg0".to_owned(),
        typ: FirType::Float32,
    }];
    let myhost_proto = b.declare_fun(
        "myhost",
        FirType::Fun {
            args: vec![FirType::Float32],
            ret: Box::new(FirType::Float32),
        },
        &proto_args,
        None,
        false,
    );
    let globals = b.block(&[myhost_proto]);

    let chan0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));

    let in0 = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out0 = b.load_table("outputs", AccessType::FunArgs, chan0, ptr_ty.clone());

    let in0_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in0));
    let out0_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out0));

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let input0_sample = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let input0_f32 = b.cast(FirType::Float32, input0_sample);
    let myhost = b.fun_call("myhost", &[input0_f32], FirType::Float32);
    let myhost_out = b.cast(FirType::FaustFloat, myhost);
    let out0_store = b.store_table("output0", AccessType::Stack, i0, myhost_out);
    let loop_body = b.block(&[out0_store]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in0_alias, out0_alias, sample_loop]);

    let args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(compute_body),
        false,
    );

    let functions = b.block(&[compute]);
    let module = b.module(
        1,
        1,
        "generic_foreign_function",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn declare_trivial_compute(b: &mut FirBuilder<'_>) -> FirId {
    let args = crate::fixtures::compute_fun_args();
    let body = b.block(&[]);
    b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &args,
        Some(body),
        false,
    )
}

fn build_foreign_global_var_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let ext = b.declare_var("extvar", FirType::Float32, AccessType::Global, None);
    let globals = b.block(&[ext]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let ext = b.load_var("extvar", AccessType::Global, FirType::Float32);
    let ext = b.cast(FirType::FaustFloat, ext);
    let store_out = b.store_table("output0", AccessType::Stack, i0, ext);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = crate::fixtures::compute_fun_args();
    let compute = b.declare_fun(
        "compute",
        crate::fixtures::compute_fun_type(),
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "foreign_global_var",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn declare_trivial_instance_clear(b: &mut FirBuilder<'_>) -> FirId {
    let body = b.block(&[]);
    b.declare_fun(
        "instanceClear",
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj))],
            ret: Box::new(FirType::Void),
        },
        &[NamedType {
            name: "dsp".to_owned(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        }],
        Some(body),
        false,
    )
}

fn code_body_at<'a>(wasm: &'a [u8], index: usize) -> wasmparser::FunctionBody<'a> {
    let mut current = 0usize;
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("payload should decode");
        if let Payload::CodeSectionEntry(body) = payload {
            if current == index {
                return body;
            }
            current += 1;
        }
    }
    panic!("code body index {index} not found");
}

/// Renders the operators that build and use addresses as short tokens
/// (`local.get 0`, `i32.const 4`, `f32.load 12`, ...); every other operator
/// becomes `_`, so a window of tokens matches one exact emitted sequence.
fn address_tokens(ops: &[Operator<'_>]) -> Vec<String> {
    ops.iter()
        .map(|op| match op {
            Operator::LocalGet { local_index } => format!("local.get {local_index}"),
            Operator::I32Const { value } => format!("i32.const {value}"),
            Operator::I32Add => "i32.add".to_owned(),
            Operator::I32Mul => "i32.mul".to_owned(),
            Operator::I32Load { memarg } => format!("i32.load {}", memarg.offset),
            Operator::F32Load { memarg } => format!("f32.load {}", memarg.offset),
            Operator::F64Load { memarg } => format!("f64.load {}", memarg.offset),
            Operator::I32Store { memarg } => format!("i32.store {}", memarg.offset),
            Operator::F32Store { memarg } => format!("f32.store {}", memarg.offset),
            Operator::F64Store { memarg } => format!("f64.store {}", memarg.offset),
            _ => "_".to_owned(),
        })
        .collect()
}

fn decode_ops(body: wasmparser::FunctionBody<'_>) -> Vec<Operator<'_>> {
    body.get_operators_reader()
        .expect("operators reader")
        .into_iter()
        .map(|op| op.expect("operator should decode"))
        .collect()
}

/// Locals are collected once each, in first-declaration order, with the type
/// of their first declaration: the WASM local indices follow that order.
#[test]
fn compute_locals_are_collected_once_in_first_declaration_order() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let one = b.float32(1.0);
    let fa = b.declare_var("fA", FirType::Float32, AccessType::Stack, Some(one));
    let two = b.int32(2);
    let ib = b.declare_var("iB", FirType::Int32, AccessType::Stack, Some(two));
    let three = b.float64(3.0);
    let fa_again = b.declare_var("fA", FirType::Float64, AccessType::Stack, Some(three));
    let four = b.float64(4.0);
    let fc = b.declare_var("fC", FirType::Float64, AccessType::Stack, Some(four));
    let count = b.int32(8);
    let first_body = b.block(&[fa_again, fc]);
    let first = b.simple_for_loop("i0", count, first_body, false);
    let second_body = b.block(&[ib]);
    let second = b.simple_for_loop("i0", count, second_body, false);
    let body = b.block(&[fa, ib, first, second]);

    let mut locals = LocalSpecs::default();
    collect_compute_locals(&store, body, &mut locals).expect("supported FIR");
    let expected = [
        ("fA", FirType::Float32),
        ("iB", FirType::Int32),
        ("i0", FirType::Int32),
        ("fC", FirType::Float64),
    ]
    .map(|(name, typ)| (name.to_owned(), typ));
    assert_eq!(locals.specs, expected);
}
