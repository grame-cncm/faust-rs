use super::*;
use crate::{FirBinOp, FirBuilder, FirStore, NamedType};

// ══ Phase 1 helpers (unchanged) ═══════════════════════════════════════════

fn make_dsp_struct(b: &mut FirBuilder<'_>) -> FirId {
    b.block(&[])
}

fn make_empty_block(b: &mut FirBuilder<'_>) -> FirId {
    b.block(&[])
}

fn make_void_fun(b: &mut FirBuilder<'_>, name: &str) -> FirId {
    let typ = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let body = b.block(&[]);
    b.declare_fun(name, typ, &[], Some(body), false)
}

fn make_full_functions(b: &mut FirBuilder<'_>) -> FirId {
    let compute = {
        let params = vec![
            FirType::Ptr(Box::new(FirType::Obj)),
            FirType::Int32,
            FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        ];
        let args: Vec<NamedType> = params
            .iter()
            .enumerate()
            .map(|(i, t)| NamedType {
                name: format!("p{i}"),
                typ: t.clone(),
            })
            .collect();
        let typ = FirType::Fun {
            args: params,
            ret: Box::new(FirType::Void),
        };
        let body = b.block(&[]);
        b.declare_fun("compute", typ, &args, Some(body), false)
    };
    let mut funs: Vec<FirId> = DSP_API_FUNCTIONS
        .iter()
        .filter(|&&n| n != "compute")
        .map(|&n| make_void_fun(b, n))
        .collect();
    funs.push(compute);
    b.block(&funs)
}

fn make_valid_module(store: &mut FirStore) -> FirId {
    let mut b = FirBuilder::new(store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    }
}

// ── Helper: build a minimal module with one custom function ───────────────

/// Wrap a custom function `DeclareFun` node inside a minimal module.
fn module_with_fun(store: &mut FirStore, fun_id: FirId) -> FirId {
    let mut b = FirBuilder::new(store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[fun_id]);
    {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    }
}

// ══ Phase 1 tests (unchanged) ═════════════════════════════════════════════

#[test]
fn valid_module_has_no_errors() {
    let mut store = FirStore::new();
    let module_id = make_valid_module(&mut store);
    let report = verify_fir_module(&store, module_id);
    report.assert_ok();
}

#[test]
fn m01_non_module_root() {
    let mut store = FirStore::new();
    let not_a_module = FirBuilder::new(&mut store).int32(0);
    let report = verify_fir_module(&store, not_a_module);
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-M01"));
}

#[test]
fn m02_bad_dsp_struct_not_block() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let bad_struct = b.int32(0);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", bad_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-M02"));
}

#[test]
fn m02_bad_dsp_struct_non_struct_type() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let bad_struct = b.declare_struct_type(FirType::Int32);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", bad_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-M02"));
}

#[test]
fn m03_globals_not_block() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let bad_globals = b.int32(0);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, bad_globals, functions, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-M03")
    );
}

#[test]
fn m04_declarations_not_block() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let bad_decls = b.int32(0);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, bad_decls, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-M04")
    );
}

#[test]
fn m05_non_declarefun_in_declarations() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let intruder = b.int32(99);
    let functions = b.block(&[intruder]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-M05")
    );
}

#[test]
fn m06_duplicate_function_name() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let f1 = make_void_fun(&mut b, "myFun");
    let f2 = make_void_fun(&mut b, "myFun");
    let functions = b.block(&[f1, f2]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-M06"));
}

#[test]
fn m07_missing_api_function() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = make_empty_block(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code == "FIR-M07")
            .count(),
        DSP_API_FUNCTIONS.len()
    );
}

#[test]
fn s03_void_struct_field() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let bad_field = b.declare_var("f", FirType::Void, AccessType::Struct, None);
    let bad_struct = b.block(&[bad_field]);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", bad_struct, globals, functions, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-S03")
    );
}

#[test]
fn s04_zero_size_array_field() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let bad_field = b.declare_var(
        "arr",
        FirType::Array(Box::new(FirType::Float32), 0),
        AccessType::Struct,
        None,
    );
    let bad_struct = b.block(&[bad_field]);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", bad_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-S04"));
}

#[test]
fn struct_fields_registered_in_symbols() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let fields = vec![FirType::Int32, FirType::Float32];
    let f0 = b.declare_var("a", FirType::Int32, AccessType::Struct, None);
    let f1 = b.declare_var("b", FirType::Float32, AccessType::Struct, None);
    let dsp_struct = b.block(&[f0, f1]);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let (_report, symbols) = verify_module_structure(&store, module_id);
    assert_eq!(symbols.struct_name.as_deref(), Some("dsp"));
    assert_eq!(symbols.struct_fields, fields);
    assert!(symbols.struct_field_names.contains("a"));
    assert!(symbols.struct_field_names.contains("b"));
}

#[test]
fn s01_struct_field_wrong_access() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let bad_field = b.declare_var("f", FirType::Int32, AccessType::Stack, None);
    let dsp_struct = b.block(&[bad_field]);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-S01"),
        "{report:?}"
    );
}

#[test]
fn s02_duplicate_struct_field_name() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let f1 = b.declare_var("f", FirType::Int32, AccessType::Struct, None);
    let f2 = b.declare_table("f", AccessType::Struct, FirType::FaustFloat, &[]);
    let dsp_struct = b.block(&[f1, f2]);
    let globals = make_empty_block(&mut b);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-S02"),
        "{report:?}"
    );
}

#[test]
fn g01_non_declarevar_in_globals() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let intruder = b.int32(0);
    let globals = b.block(&[intruder]);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-G01")
    );
}

#[test]
fn g02_wrong_access_type_in_globals() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let bad_var = b.declare_var("x", FirType::Int32, AccessType::Stack, None);
    let globals = b.block(&[bad_var]);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-G02")
    );
}

#[test]
fn g03_duplicate_global_name() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let v1 = b.declare_var("g", FirType::Int32, AccessType::Global, None);
    let v2 = b.declare_var("g", FirType::Int32, AccessType::Global, None);
    let globals = b.block(&[v1, v2]);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-G03")
    );
}

#[test]
fn globals_registered_in_symbols() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let var = b.declare_var("gRate", FirType::Int32, AccessType::Global, None);
    let globals = b.block(&[var]);
    let functions = make_full_functions(&mut b);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let (_report, symbols) = verify_module_structure(&store, module_id);
    assert!(symbols.globals.contains_key("gRate"));
}

#[test]
fn f01_non_fun_type() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let body = b.block(&[]);
    let bad_fun = b.declare_fun("bad", FirType::Int32, &[], Some(body), false);
    let module_id = module_with_fun(&mut store, bad_fun);
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-F01")
    );
}

#[test]
fn f04_duplicate_param_name() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dup_args = vec![
        NamedType {
            name: "x".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "x".to_string(),
            typ: FirType::Int32,
        },
    ];
    let typ = FirType::Fun {
        args: vec![FirType::Int32, FirType::Int32],
        ret: Box::new(FirType::Void),
    };
    let body = b.block(&[]);
    let fun = b.declare_fun("f", typ, &dup_args, Some(body), false);
    let module_id = module_with_fun(&mut store, fun);
    assert!(
        verify_fir_module(&store, module_id)
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-F04")
    );
}

#[test]
fn f05_compute_non_void_return() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let params = vec![
        FirType::Ptr(Box::new(FirType::Obj)),
        FirType::Int32,
        FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
    ];
    let args: Vec<NamedType> = params
        .iter()
        .enumerate()
        .map(|(i, t)| NamedType {
            name: format!("p{i}"),
            typ: t.clone(),
        })
        .collect();
    let typ = FirType::Fun {
        args: params,
        ret: Box::new(FirType::Int32),
    };
    let body = b.block(&[]);
    let compute = b.declare_fun("compute", typ, &args, Some(body), false);
    let module_id = module_with_fun(&mut store, compute);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-F05"));
}

#[test]
fn f06_compute_wrong_arity() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let typ = FirType::Fun {
        args: vec![FirType::Int32],
        ret: Box::new(FirType::Void),
    };
    let args = vec![NamedType {
        name: "n".to_string(),
        typ: FirType::Int32,
    }];
    let body = b.block(&[]);
    let compute = b.declare_fun("compute", typ, &args, Some(body), false);
    let module_id = module_with_fun(&mut store, compute);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-F06"));
}

#[test]
fn f07_prototype_only_function() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let typ = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let proto = b.declare_fun("proto", typ, &[], None, false);
    let module_id = module_with_fun(&mut store, proto);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-F07"));
}

#[test]
fn functions_registered_in_symbols() {
    let mut store = FirStore::new();
    let module_id = make_valid_module(&mut store);
    let (_report, symbols) = verify_module_structure(&store, module_id);
    for &api_fn in DSP_API_FUNCTIONS {
        assert!(symbols.functions.contains_key(api_fn), "missing '{api_fn}'");
    }
}

// ══ Phase 2 helpers ═══════════════════════════════════════════════════════

/// Build a single-function module with the given body statements.
/// The function has signature `(x: Int32) -> Void` with param `x`.
/// Builds a DSP-struct block declaring exactly `fields`.
fn struct_fields(store: &mut FirStore, fields: &[(&str, FirType)]) -> FirId {
    let mut b = FirBuilder::new(store);
    let declarations = fields
        .iter()
        .map(|(name, typ)| b.declare_var(*name, typ.clone(), AccessType::Struct, None))
        .collect::<Vec<_>>();
    b.block(&declarations)
}

// A store into a table no declaration backs cannot compile in any backend;
// this is the exact shape a mid-implementation vector module emitted behind a
// green report before FIR-T04 existed.
#[test]
fn t04_undeclared_struct_store_table_is_an_error() {
    let mut store = FirStore::new();
    let (index, value) = {
        let mut b = FirBuilder::new(&mut store);
        (b.int32(0), b.float64(1.0))
    };
    let write =
        FirBuilder::new(&mut store).store_table("fVecMutTbl55", AccessType::Struct, index, value);
    let empty = struct_fields(&mut store, &[]);
    let module_id = module_with_struct_and_body(&mut store, empty, &[write]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-T04" && d.message.contains("fVecMutTbl55")),
        "{report:?}"
    );
}

#[test]
fn t04_undeclared_stack_load_table_is_an_error() {
    let mut store = FirStore::new();
    let index = FirBuilder::new(&mut store).int32(0);
    let load = FirBuilder::new(&mut store).load_table(
        "vstate_tmp",
        AccessType::Stack,
        index,
        FirType::Float64,
    );
    let drop = FirBuilder::new(&mut store).drop_(load);
    let empty = struct_fields(&mut store, &[]);
    let module_id = module_with_struct_and_body(&mut store, empty, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-T04"),
        "{report:?}"
    );
}

#[test]
fn t04_declared_struct_array_store_is_accepted() {
    let mut store = FirStore::new();
    let (index, value) = {
        let mut b = FirBuilder::new(&mut store);
        (b.int32(0), b.float64(1.0))
    };
    let write =
        FirBuilder::new(&mut store).store_table("fVecMutTbl55", AccessType::Struct, index, value);
    let fields = struct_fields(
        &mut store,
        &[(
            "fVecMutTbl55",
            FirType::Array(Box::new(FirType::Float64), 8),
        )],
    );
    let module_id = module_with_struct_and_body(&mut store, fields, &[write]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| matches!(d.code, "FIR-T03" | "FIR-T04")),
        "{report:?}"
    );
}

#[test]
fn t03_struct_scalar_field_store_warns_without_t04() {
    let mut store = FirStore::new();
    let (index, value) = {
        let mut b = FirBuilder::new(&mut store);
        (b.int32(0), b.float64(1.0))
    };
    let write = FirBuilder::new(&mut store).store_table("fGain", AccessType::Struct, index, value);
    let fields = struct_fields(&mut store, &[("fGain", FirType::Float64)]);
    let module_id = module_with_struct_and_body(&mut store, fields, &[write]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-T03"),
        "{report:?}"
    );
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "FIR-T04"),
        "{report:?}"
    );
}

fn module_with_body(store: &mut FirStore, stmts: &[FirId]) -> FirId {
    let body = FirBuilder::new(store).block(stmts);
    let mut b = FirBuilder::new(store);
    let arg = NamedType {
        name: "x".to_string(),
        typ: FirType::Int32,
    };
    let typ = FirType::Fun {
        args: vec![FirType::Int32],
        ret: Box::new(FirType::Void),
    };
    let fun = b.declare_fun("myFun", typ, &[arg], Some(body), false);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[fun]);
    {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    }
}

/// Build a single-function module with a custom `dsp_struct` block.
fn module_with_struct_and_body(store: &mut FirStore, dsp_struct: FirId, stmts: &[FirId]) -> FirId {
    let body = FirBuilder::new(store).block(stmts);
    let mut b = FirBuilder::new(store);
    let arg = NamedType {
        name: "x".to_string(),
        typ: FirType::Int32,
    };
    let typ = FirType::Fun {
        args: vec![FirType::Int32],
        ret: Box::new(FirType::Void),
    };
    let fun = b.declare_fun("myFun", typ, &[arg], Some(body), false);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[fun]);
    {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    }
}

/// Build a single-function module that returns an Int32 value.
fn module_with_int_body(store: &mut FirStore, stmts: &[FirId]) -> FirId {
    let body = FirBuilder::new(store).block(stmts);
    let mut b = FirBuilder::new(store);
    let typ = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Int32),
    };
    let fun = b.declare_fun("getVal", typ, &[], Some(body), false);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[fun]);
    {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    }
}

// ══ Phase 2 — SC checks ═══════════════════════════════════════════════════

#[test]
fn sc01_undeclared_load() {
    let mut store = FirStore::new();
    let load = FirBuilder::new(&mut store).load_var("z", AccessType::Stack, FirType::Int32);
    let drop = FirBuilder::new(&mut store).drop_(load);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC01"),
        "{report:?}"
    );
}

#[test]
fn sc02_access_type_mismatch_load() {
    let mut store = FirStore::new();
    // Declare as kStack, then load as kGlobal
    let zero = FirBuilder::new(&mut store).int32(0);
    let decl =
        FirBuilder::new(&mut store).declare_var("v", FirType::Int32, AccessType::Stack, Some(zero));
    let load = FirBuilder::new(&mut store).load_var("v", AccessType::Global, FirType::Int32);
    let drop = FirBuilder::new(&mut store).drop_(load);
    let module_id = module_with_body(&mut store, &[decl, drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC02"),
        "{report:?}"
    );
}

#[test]
fn sc03_load_uninitialized() {
    let mut store = FirStore::new();
    // Declare without initializer, then load
    let decl =
        FirBuilder::new(&mut store).declare_var("v", FirType::Int32, AccessType::Stack, None);
    let load = FirBuilder::new(&mut store).load_var("v", AccessType::Stack, FirType::Int32);
    let drop = FirBuilder::new(&mut store).drop_(load);
    let module_id = module_with_body(&mut store, &[decl, drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC03"),
        "{report:?}"
    );
}

#[test]
fn soundfile_access_missing_struct_slot_warns_sc09() {
    let mut store = FirStore::new();
    let zero = FirBuilder::new(&mut store).int32(0);
    let len = FirBuilder::new(&mut store).load_soundfile_length("fSound0", zero);
    let drop = FirBuilder::new(&mut store).drop_(len);
    let dsp_struct = FirBuilder::new(&mut store).block(&[]);
    let module_id = module_with_struct_and_body(&mut store, dsp_struct, &[drop]);

    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors(), "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-SC09" && d.message.contains("fSound0")),
        "{report:?}"
    );
}

#[test]
fn soundfile_access_wrong_struct_type_warns_sf01() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let sound_slot = b.declare_var("fSound0", FirType::Int32, AccessType::Struct, None);
    let dsp_struct = b.block(&[sound_slot]);
    let zero = b.int32(0);
    let rate = b.load_soundfile_rate("fSound0", zero);
    let drop = b.drop_(rate);
    let module_id = module_with_struct_and_body(&mut store, dsp_struct, &[drop]);

    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors(), "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-SF01" && d.message.contains("fSound0")),
        "{report:?}"
    );
}

#[test]
fn soundfile_buffer_requires_integer_indices() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let sound_slot = b.declare_var("fSound0", FirType::Sound, AccessType::Struct, None);
    let dsp_struct = b.block(&[sound_slot]);
    let zero = b.int32(0);
    let bad_idx = b.float32(0.5);
    let sample = b.load_soundfile_buffer("fSound0", zero, zero, bad_idx, FirType::Float32);
    let drop = b.drop_(sample);
    let module_id = module_with_struct_and_body(&mut store, dsp_struct, &[drop]);

    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors(), "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "FIR-T01" && d.message.contains("soundfile index")),
        "{report:?}"
    );
}

#[test]
fn sc03_initialized_after_store_no_warning() {
    let mut store = FirStore::new();
    let zero = FirBuilder::new(&mut store).int32(0);
    let decl =
        FirBuilder::new(&mut store).declare_var("v", FirType::Int32, AccessType::Stack, None);
    let store_v = FirBuilder::new(&mut store).store_var("v", AccessType::Stack, zero);
    let load = FirBuilder::new(&mut store).load_var("v", AccessType::Stack, FirType::Int32);
    let drop = FirBuilder::new(&mut store).drop_(load);
    let module_id = module_with_body(&mut store, &[decl, store_v, drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "FIR-SC03"),
        "{report:?}"
    );
}

#[test]
fn sc04_undeclared_store() {
    let mut store = FirStore::new();
    let zero = FirBuilder::new(&mut store).int32(0);
    let store_v = FirBuilder::new(&mut store).store_var("z", AccessType::Stack, zero);
    let module_id = module_with_body(&mut store, &[store_v]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC04"),
        "{report:?}"
    );
}

#[test]
fn sc05_access_type_mismatch_store() {
    let mut store = FirStore::new();
    // Declare as kStack, store as kGlobal
    let decl =
        FirBuilder::new(&mut store).declare_var("v", FirType::Int32, AccessType::Stack, None);
    let zero = FirBuilder::new(&mut store).int32(0);
    let bad_store = FirBuilder::new(&mut store).store_var("v", AccessType::Global, zero);
    let module_id = module_with_body(&mut store, &[decl, bad_store]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC05"),
        "{report:?}"
    );
}

#[test]
fn sc07_funargs_redeclared_in_body() {
    let mut store = FirStore::new();
    // Declare a kFunArgs variable inside the function body (x is already a param)
    let redecl =
        FirBuilder::new(&mut store).declare_var("x", FirType::Int32, AccessType::FunArgs, None);
    let module_id = module_with_body(&mut store, &[redecl]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC07"),
        "{report:?}"
    );
}

#[test]
fn sc10_local_declarevar_with_global_access() {
    let mut store = FirStore::new();
    let bad_decl =
        FirBuilder::new(&mut store).declare_var("g", FirType::Int32, AccessType::Global, None);
    let module_id = module_with_body(&mut store, &[bad_decl]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC10"),
        "{report:?}"
    );
}

#[test]
fn funarg_load_is_valid() {
    // Loading a kFunArgs parameter should not produce any scope errors
    let mut store = FirStore::new();
    let load_x = FirBuilder::new(&mut store).load_var("x", AccessType::FunArgs, FirType::Int32);
    let drop = FirBuilder::new(&mut store).drop_(load_x);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| matches!(d.code, "FIR-SC01" | "FIR-SC02")),
        "{report:?}"
    );
}

#[test]
fn sc01_implicit_compute_i0_is_rejected() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let idx = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let drop = b.drop_(idx);
    let body = b.block(&[drop]);

    let params = vec![
        FirType::Ptr(Box::new(FirType::Obj)),
        FirType::Int32,
        FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
    ];
    let args: Vec<NamedType> = params
        .iter()
        .enumerate()
        .map(|(i, t)| NamedType {
            name: format!("p{i}"),
            typ: t.clone(),
        })
        .collect();
    let typ = FirType::Fun {
        args: params,
        ret: Box::new(FirType::Void),
    };
    let compute = b.declare_fun("compute", typ, &args, Some(body), false);
    let module_id = module_with_fun(&mut store, compute);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors(), "{report:?}");
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC01"),
        "{report:?}"
    );
}

#[test]
fn global_load_is_valid() {
    // Loading a kGlobal variable declared in the globals block should be valid
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let g = b.declare_var("gRate", FirType::Int32, AccessType::Global, None);
    let globals = b.block(&[g]);
    let load_g = b.load_var("gRate", AccessType::Global, FirType::Int32);
    let drop = b.drop_(load_g);
    let body = b.block(&[drop]);
    let typ = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let fun = b.declare_fun("f", typ, &[], Some(body), false);
    let functions = b.block(&[fun]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "FIR-SC01"),
        "unexpected SC01: {report:?}"
    );
}

// ══ Phase 2 — loop checks ════════════════════════════════════════════════

#[test]
fn l01_forloop_non_loop_var() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    // init with kStack instead of kLoop
    let init_val = b.int32(0);
    let init_decl = b.declare_var("i", FirType::Int32, AccessType::Stack, Some(init_val));
    let cond = b.int32(10);
    let one = b.int32(1);
    let step = b.store_var("i", AccessType::Stack, one);
    let body_stmt = b.int32(0);
    let body = b.block(&[body_stmt]); // non-empty
    let loop_node = b.for_loop("i", init_decl, cond, step, body, false);
    let module_id = module_with_body(&mut store, &[loop_node]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-L01"),
        "{report:?}"
    );
}

#[test]
fn l02_forloop_float_var() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    // loop variable is Float32 instead of Int32
    let init_val = b.float32(0.0);
    let init_decl = b.declare_var("f", FirType::Float32, AccessType::Loop, Some(init_val));
    let cond = b.int32(10);
    let one = b.float32(1.0);
    let step = b.store_var("f", AccessType::Loop, one);
    let body_stmt = b.int32(0);
    let body = b.block(&[body_stmt]);
    let loop_node = b.for_loop("f", init_decl, cond, step, body, false);
    let module_id = module_with_body(&mut store, &[loop_node]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-L02"),
        "{report:?}"
    );
}

#[test]
fn l04_forloop_empty_body() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let init_val = b.int32(0);
    let init_decl = b.declare_var("i", FirType::Int32, AccessType::Loop, Some(init_val));
    let cond = b.int32(10);
    let one = b.int32(1);
    let step = b.store_var("i", AccessType::Loop, one);
    let body = b.block(&[]); // empty
    let loop_node = b.for_loop("i", init_decl, cond, step, body, false);
    let module_id = module_with_body(&mut store, &[loop_node]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-L04"),
        "{report:?}"
    );
}

#[test]
fn l04_simple_forloop_empty_body() {
    let mut store = FirStore::new();
    let upper = FirBuilder::new(&mut store).int32(64);
    let body = FirBuilder::new(&mut store).block(&[]);
    let loop_node = FirBuilder::new(&mut store).simple_for_loop("i", upper, body, false);
    let module_id = module_with_body(&mut store, &[loop_node]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-L04"),
        "{report:?}"
    );
}

#[test]
fn valid_forloop_no_errors() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let init_val = b.int32(0);
    let init_decl = b.declare_var("i", FirType::Int32, AccessType::Loop, Some(init_val));
    let load_i = b.load_var("i", AccessType::Loop, FirType::Int32);
    let limit = b.int32(64);
    let cond = b.binop(FirBinOp::Lt, load_i, limit, FirType::Bool);
    let load_i2 = b.load_var("i", AccessType::Loop, FirType::Int32);
    let one = b.int32(1);
    let step_val = b.binop(FirBinOp::Add, load_i2, one, FirType::Int32);
    let step = b.store_var("i", AccessType::Loop, step_val);
    let body_stmt = b.int32(0);
    let body = b.block(&[body_stmt]); // non-empty
    let loop_node = b.for_loop("i", init_decl, cond, step, body, false);
    let module_id = module_with_body(&mut store, &[loop_node]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| matches!(d.code, "FIR-L01" | "FIR-L02" | "FIR-SC01" | "FIR-SC02")),
        "{report:?}"
    );
}

// ══ Phase 2 — return checks ══════════════════════════════════════════════

#[test]
fn r02_return_none_in_non_void_function() {
    let mut store = FirStore::new();
    let ret = FirBuilder::new(&mut store).ret(None);
    let module_id = module_with_int_body(&mut store, &[ret]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-R02"),
        "{report:?}"
    );
}

#[test]
fn r03_dead_code_after_return() {
    let mut store = FirStore::new();
    let ret = FirBuilder::new(&mut store).ret(None);
    let dead = FirBuilder::new(&mut store).null_statement();
    let module_id = module_with_body(&mut store, &[ret, dead]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-R03"),
        "{report:?}"
    );
}

// ══ Phase 2 — switch checks ═══════════════════════════════════════════════

#[test]
fn sw02_duplicate_case_value() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.int32(0);
    let case_body = b.block(&[]);
    let sw = b.switch(cond, &[(0, case_body), (0, case_body)], None);
    let module_id = module_with_body(&mut store, &[sw]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SW02"),
        "{report:?}"
    );
}

#[test]
fn sw03_empty_switch() {
    let mut store = FirStore::new();
    let cond = FirBuilder::new(&mut store).int32(0);
    let sw = FirBuilder::new(&mut store).switch(cond, &[], None);
    let module_id = module_with_body(&mut store, &[sw]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SW03"),
        "{report:?}"
    );
}

#[test]
fn sw03_default_only_still_warns_no_cases() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.int32(0);
    let default_body = b.block(&[]);
    let sw = b.switch(cond, &[], Some(default_body));
    let module_id = module_with_body(&mut store, &[sw]);
    let report = verify_fir_module(&store, module_id);
    assert!(!report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SW03"),
        "{report:?}"
    );
}

// ══ Phase 2 — If / InitStatus merge ══════════════════════════════════════

#[test]
fn if_both_branches_initialize_var_marks_yes() {
    // var is uninitialized; both then and else store to it → must be Yes after If
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let zero = b.int32(0);
    let decl = b.declare_var("v", FirType::Int32, AccessType::Stack, None);
    let store_then = b.store_var("v", AccessType::Stack, zero);
    let store_else = b.store_var("v", AccessType::Stack, zero);
    let then_block = b.block(&[store_then]);
    let else_block = b.block(&[store_else]);
    let cond_val = b.bool_(true);
    let if_node = b.if_(cond_val, then_block, Some(else_block));
    // Load after If — should NOT trigger SC03
    let load = b.load_var("v", AccessType::Stack, FirType::Int32);
    let drop = b.drop_(load);
    let module_id = module_with_body(&mut store, &[decl, if_node, drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "FIR-SC03"),
        "unexpected SC03: {report:?}"
    );
}

#[test]
fn if_one_branch_initializes_var_marks_maybe() {
    // var is uninitialized; only then branch stores → Maybe after If.
    // Phase 2 treats `Maybe` as acceptable for SC03 (warning only on `No`).
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let zero = b.int32(0);
    let decl = b.declare_var("v", FirType::Int32, AccessType::Stack, None);
    let store_then = b.store_var("v", AccessType::Stack, zero);
    let then_block = b.block(&[store_then]);
    let cond_val = b.bool_(true);
    let if_node = b.if_(cond_val, then_block, None); // no else
    let load = b.load_var("v", AccessType::Stack, FirType::Int32);
    let drop = b.drop_(load);
    let module_id = module_with_body(&mut store, &[decl, if_node, drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "FIR-SC03"),
        "unexpected SC03: {report:?}"
    );
}

#[test]
fn if_non_block_branch_does_not_leak_scope() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.bool_(true);
    let zero = b.int32(0);
    let then_decl = b.declare_var("t", FirType::Int32, AccessType::Stack, Some(zero));
    let if_node = b.if_(cond, then_decl, None);
    let load_t = b.load_var("t", AccessType::Stack, FirType::Int32);
    let drop_t = b.drop_(load_t);
    let module_id = module_with_body(&mut store, &[if_node, drop_t]);
    let report = verify_fir_module(&store, module_id);
    assert!(report.has_errors());
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SC01"),
        "{report:?}"
    );
}

// ══ Phase 3 — type checks ════════════════════════════════════════════════

#[test]
fn b01_binop_type_mismatch() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let i = b.int32(1);
    let f = b.float32(2.0);
    let bad = b.binop(FirBinOp::Add, i, f, FirType::Float32);
    let drop = b.drop_(bad);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-B01"),
        "{report:?}"
    );
}

#[test]
fn u02_noop_cast() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let i = b.int32(1);
    let c = b.cast(FirType::Int32, i);
    let drop = b.drop_(c);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-U02"),
        "{report:?}"
    );
}

#[test]
fn u03_cast_non_numeric() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp = b.new_dsp("dsp", FirType::Obj);
    let c = b.cast(FirType::Int32, dsp);
    let drop = b.drop_(c);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-U03"),
        "{report:?}"
    );
}

#[test]
fn c01_select2_bad_condition_type() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.float32(0.5);
    let t = b.int32(1);
    let e = b.int32(0);
    let sel = b.select2(cond, t, e, FirType::Int32);
    let drop = b.drop_(sel);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-C01"),
        "{report:?}"
    );
}

#[test]
fn c04_if_bad_condition_type() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.float32(1.0);
    let then_block = b.block(&[]);
    let if_ = b.if_(cond, then_block, None);
    let module_id = module_with_body(&mut store, &[if_]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-C04"),
        "{report:?}"
    );
}

#[test]
fn fc01_call_undeclared_function() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let arg = b.int32(1);
    let call = b.fun_call("missing", &[arg], FirType::Int32);
    let drop = b.drop_(call);
    let module_id = module_with_body(&mut store, &[drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-FC01"),
        "{report:?}"
    );
}

#[test]
fn d01_warns_for_dropped_pure_math_but_not_foreign_call() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let base = b.float32(2.0);
    let exponent = b.float32(3.0);
    let pow = b.math_call(FirMathOp::Pow, &[base, exponent], FirType::Float32);
    let pure_drop = b.drop_(pow);
    let foreign = b.fun_call("observable", &[], FirType::Float32);
    let foreign_drop = b.drop_(foreign);
    let module_id = module_with_body(&mut store, &[pure_drop, foreign_drop]);

    let report = verify_fir_module(&store, module_id);
    let pure_drop_diagnostics = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "FIR-D01")
        .collect::<Vec<_>>();
    assert_eq!(pure_drop_diagnostics.len(), 1, "{report:?}");
    assert_eq!(pure_drop_diagnostics[0].node, pure_drop);
}

#[test]
fn fc02_fc03_call_arity_and_arg_type_mismatch() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let callee_ty = FirType::Fun {
        args: vec![FirType::Int32],
        ret: Box::new(FirType::Int32),
    };
    let callee_args = vec![NamedType {
        name: "x".to_string(),
        typ: FirType::Int32,
    }];
    let callee = b.declare_fun("foo", callee_ty, &callee_args, None, false);

    let farg = b.new_dsp("tmp", FirType::Obj);
    let extra = b.int32(2);
    let call = b.fun_call("foo", &[farg, extra], FirType::Int32);
    let drop = b.drop_(call);
    let body = b.block(&[drop]);
    let caller_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let caller = b.declare_fun("caller", caller_ty, &[], Some(body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[callee, caller]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-FC02"),
        "{report:?}"
    );
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-FC03"),
        "{report:?}"
    );
}

#[test]
fn r01_return_value_type_mismatch() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let f = b.float32(1.0);
    let ret = b.ret(Some(f));
    let module_id = module_with_int_body(&mut store, &[ret]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-R01"),
        "{report:?}"
    );
}

#[test]
fn l03_whileloop_bad_condition_type() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.float32(0.0);
    let body = b.block(&[]);
    let w = b.while_loop(cond, body);
    let module_id = module_with_body(&mut store, &[w]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-L03"),
        "{report:?}"
    );
}

#[test]
fn sw01_switch_bad_condition_type() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let cond = b.bool_(true);
    let body = b.block(&[]);
    let sw = b.switch(cond, &[(0, body)], None);
    let module_id = module_with_body(&mut store, &[sw]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SW01"),
        "{report:?}"
    );
}

#[test]
fn t01_t03_loadtable_bad_index_and_non_table_ref() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let decl = b.declare_var("x", FirType::Int32, AccessType::Stack, None);
    let idx = b.float32(1.5);
    let load = b.load_table("x", AccessType::Stack, idx, FirType::Int32);
    let drop = b.drop_(load);
    let module_id = module_with_body(&mut store, &[decl, drop]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-T01"),
        "{report:?}"
    );
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-T03"),
        "{report:?}"
    );
}

#[test]
fn t02_storetable_value_type_mismatch() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let v0 = b.int32(0);
    let table = b.declare_table("t", AccessType::Stack, FirType::Int32, &[v0]);
    let idx = b.int32(0);
    let val = b.float32(1.0);
    let st = b.store_table("t", AccessType::Stack, idx, val);
    let module_id = module_with_body(&mut store, &[table, st]);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-T02"),
        "{report:?}"
    );
}

#[test]
fn v01_storevar_rejects_void_expression_value() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let callee_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let callee_body = b.block(&[]);
    let callee = b.declare_fun("tick", callee_ty, &[], Some(callee_body), false);

    let local = b.declare_var("acc", FirType::Int32, AccessType::Stack, None);
    let call = b.fun_call("tick", &[], FirType::Void);
    let store_stmt = b.store_var("acc", AccessType::Stack, call);
    let caller_body = b.block(&[local, store_stmt]);
    let caller_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let caller = b.declare_fun("caller", caller_ty, &[], Some(caller_body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[callee, caller]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };

    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-V01"),
        "{report:?}"
    );
}

#[test]
fn v01_return_rejects_void_expression_value() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let callee_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let callee_body = b.block(&[]);
    let callee = b.declare_fun("tick", callee_ty, &[], Some(callee_body), false);

    let call = b.fun_call("tick", &[], FirType::Void);
    let ret = b.ret(Some(call));
    let caller_body = b.block(&[ret]);
    let caller_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let caller = b.declare_fun("caller", caller_ty, &[], Some(caller_body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[callee, caller]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };

    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-V01"),
        "{report:?}"
    );
}

#[test]
fn v01_value_array_rejects_void_elements() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let callee_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let callee_body = b.block(&[]);
    let callee = b.declare_fun("tick", callee_ty, &[], Some(callee_body), false);

    let call = b.fun_call("tick", &[], FirType::Void);
    let array = b.value_array(&[call], FirType::Array(Box::new(FirType::Int32), 1));
    let drop = b.drop_(array);
    let caller_body = b.block(&[drop]);
    let caller_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let caller = b.declare_fun("caller", caller_ty, &[], Some(caller_body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[callee, caller]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };

    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-V01"),
        "{report:?}"
    );
}

#[test]
fn ma03_and_ma04_math_call_warnings() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let proto_ty = FirType::Fun {
        args: vec![FirType::Float64],
        ret: Box::new(FirType::Float64),
    };
    let proto_args = vec![NamedType {
        name: "x".to_string(),
        typ: FirType::Float64,
    }];
    let sin_decl = b.declare_fun("sin", proto_ty.clone(), &proto_args, None, false);
    let fabs_decl = b.declare_fun("fabs", proto_ty, &proto_args, None, false);

    let i = b.int32(1);
    let call_sin = b.fun_call("sin", &[i], FirType::Float64);
    let call_fabs = b.fun_call("fabs", &[i], FirType::Float64);
    let d1 = b.drop_(call_sin);
    let d2 = b.drop_(call_fabs);
    let body = b.block(&[d1, d2]);
    let caller_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let caller = b.declare_fun("caller", caller_ty, &[], Some(body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[sin_decl, fabs_decl, caller]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-MA03"),
        "{report:?}"
    );
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-MA04"),
        "{report:?}"
    );
}

#[test]
fn m08_compute_input_index_out_of_module_arity() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let idx0 = b.int32(0);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let input_ptr = b.load_table("inputs", AccessType::FunArgs, idx0, ptr_ty.clone());
    let input_alias = b.declare_var("input0", ptr_ty, AccessType::Stack, Some(input_ptr));
    let body = b.block(&[input_alias]);
    let compute_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let compute = b.declare_fun("compute", compute_ty, &[], Some(body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[compute]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 1, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-M08"),
        "{report:?}"
    );
}

#[test]
fn m09_compute_output_index_out_of_module_arity() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let idx1 = b.int32(1);
    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let output_ptr = b.load_table("outputs", AccessType::FunArgs, idx1, ptr_ty.clone());
    let output_alias = b.declare_var("output1", ptr_ty, AccessType::Stack, Some(output_ptr));
    let body = b.block(&[output_alias]);
    let compute_ty = FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };
    let compute = b.declare_fun("compute", compute_ty, &[], Some(body), false);

    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let functions = b.block(&[compute]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(1, 1, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-M09"),
        "{report:?}"
    );
}

// ══ FIR-LC01 — lifecycle order (instanceConstants reads clear-only field) ═══

/// Build a minimal module that reproduces the `phasor.cpp` pattern:
///
/// ```cpp
/// // dsp_struct: int iWave48; float fConst0;
/// instanceConstants(sr) { fConst0 = fTbl48[iWave48]; }   // LC01: iWave48 not yet set
/// instanceClear()        { iWave48 = 0; }
/// ```
///
/// Expected: FIR-LC01 warning on the `LoadVar(kStruct, "iWave48")` inside
/// `instanceConstants`.
#[test]
fn lc01_instance_constants_reads_clear_only_field() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // dsp_struct: int iWave48; float fConst0;
    let wave_field = b.declare_var("iWave48", FirType::Int32, AccessType::Struct, None);
    let const_field = b.declare_var("fConst0", FirType::Float32, AccessType::Struct, None);
    let dsp_struct = b.block(&[wave_field, const_field]);

    let fun_ty = || FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };

    // instanceConstants body: load iWave48 then store into fConst0
    //   float tmp = (float) iWave48;   // LC01 load
    //   fConst0 = tmp;
    let iwave_load = b.load_var("iWave48", AccessType::Struct, FirType::Int32);
    let tmp_cast = b.cast(FirType::Float32, iwave_load);
    let store_const = b.store_var("fConst0", AccessType::Struct, tmp_cast);
    let constants_body = b.block(&[store_const]);
    let instance_constants = b.declare_fun(
        "instanceConstants",
        fun_ty(),
        &[],
        Some(constants_body),
        false,
    );

    // instanceClear body: iWave48 = 0;
    let zero = b.int32(0);
    let store_wave = b.store_var("iWave48", AccessType::Struct, zero);
    let clear_body = b.block(&[store_wave]);
    let instance_clear = b.declare_fun("instanceClear", fun_ty(), &[], Some(clear_body), false);

    let globals = b.block(&[]);
    let functions = b.block(&[instance_constants, instance_clear]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };

    let report = verify_fir_module(&store, module_id);
    let lc01: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "FIR-LC01")
        .collect();
    assert!(
        !lc01.is_empty(),
        "expected FIR-LC01 but got none; diagnostics: {report:?}"
    );
    let d = &lc01[0];
    assert_eq!(d.context.variable_name.as_deref(), Some("iWave48"));
    assert_eq!(
        d.context.function_name.as_deref(),
        Some("instanceConstants")
    );
    assert_eq!(d.severity, Severity::Error);
}

/// When the field IS initialized inside `instanceConstants` before being read,
/// no FIR-LC01 should fire even if `instanceClear` also writes the same field.
#[test]
fn lc01_no_warn_when_field_initialized_in_instance_constants() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    // dsp_struct: int iWave48; float fConst0;
    let wave_field = b.declare_var("iWave48", FirType::Int32, AccessType::Struct, None);
    let const_field = b.declare_var("fConst0", FirType::Float32, AccessType::Struct, None);
    let dsp_struct = b.block(&[wave_field, const_field]);

    let fun_ty = || FirType::Fun {
        args: vec![],
        ret: Box::new(FirType::Void),
    };

    // instanceConstants: first write iWave48, then read it — no LC01
    let zero = b.int32(0);
    let store_wave_first = b.store_var("iWave48", AccessType::Struct, zero);
    let iwave_load = b.load_var("iWave48", AccessType::Struct, FirType::Int32);
    let tmp_cast = b.cast(FirType::Float32, iwave_load);
    let store_const = b.store_var("fConst0", AccessType::Struct, tmp_cast);
    let constants_body = b.block(&[store_wave_first, store_const]);
    let instance_constants = b.declare_fun(
        "instanceConstants",
        fun_ty(),
        &[],
        Some(constants_body),
        false,
    );

    // instanceClear: also resets iWave48
    let zero2 = b.int32(0);
    let store_wave_clear = b.store_var("iWave48", AccessType::Struct, zero2);
    let clear_body = b.block(&[store_wave_clear]);
    let instance_clear = b.declare_fun("instanceClear", fun_ty(), &[], Some(clear_body), false);

    let globals = b.block(&[]);
    let functions = b.block(&[instance_constants, instance_clear]);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };

    let report = verify_fir_module(&store, module_id);
    let lc01: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "FIR-LC01")
        .collect();
    assert!(
        lc01.is_empty(),
        "unexpected FIR-LC01 when field is pre-initialized; diagnostics: {report:?}"
    );
}

// ══ Execution entry points (execution-options port §4.5) ═════════════════

/// Builds a `frame(dsp, inputs, outputs)` declaration with the given body.
fn make_frame_fun(b: &mut FirBuilder<'_>, body: FirId) -> FirId {
    let params = vec![
        FirType::Ptr(Box::new(FirType::Obj)),
        FirType::Ptr(Box::new(FirType::FaustFloat)),
        FirType::Ptr(Box::new(FirType::FaustFloat)),
    ];
    let args: Vec<NamedType> = ["dsp", "inputs", "outputs"]
        .iter()
        .zip(params.iter())
        .map(|(n, t)| NamedType {
            name: (*n).to_string(),
            typ: t.clone(),
        })
        .collect();
    let typ = FirType::Fun {
        args: params,
        ret: Box::new(FirType::Void),
    };
    b.declare_fun("frame", typ, &args, Some(body), false)
}

/// Builds the full API function block plus `frame`, with the given bodies.
fn make_functions_with_frame(
    b: &mut FirBuilder<'_>,
    compute_body: FirId,
    frame_body: FirId,
) -> FirId {
    let compute = {
        let params = vec![
            FirType::Ptr(Box::new(FirType::Obj)),
            FirType::Int32,
            FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        ];
        let args: Vec<NamedType> = params
            .iter()
            .enumerate()
            .map(|(i, t)| NamedType {
                name: format!("p{i}"),
                typ: t.clone(),
            })
            .collect();
        let typ = FirType::Fun {
            args: params,
            ret: Box::new(FirType::Void),
        };
        b.declare_fun("compute", typ, &args, Some(compute_body), false)
    };
    let frame = make_frame_fun(b, frame_body);
    let mut funs: Vec<FirId> = DSP_API_FUNCTIONS
        .iter()
        .filter(|&&n| n != "compute")
        .map(|&n| make_void_fun(b, n))
        .collect();
    funs.push(compute);
    funs.push(frame);
    b.block(&funs)
}

#[test]
fn f08_frame_with_empty_compute_is_clean() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let compute_body = b.block(&[]);
    let frame_body = b.block(&[]);
    let functions = make_functions_with_frame(&mut b, compute_body, frame_body);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(!report.diagnostics.iter().any(|d| d.code == "FIR-F08"));
    assert!(!report.diagnostics.iter().any(|d| d.code == "FIR-F09"));
}

#[test]
fn f08_frame_with_nonempty_compute_is_flagged() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let compute_body = {
        let zero = b.int32(0);
        let stmt = b.declare_var("dead", FirType::Int32, AccessType::Stack, Some(zero));
        b.block(&[stmt])
    };
    let frame_body = b.block(&[]);
    let functions = make_functions_with_frame(&mut b, compute_body, frame_body);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-F08"));
}

#[test]
fn f09_frame_referencing_count_is_flagged() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let dsp_struct = make_dsp_struct(&mut b);
    let globals = make_empty_block(&mut b);
    let compute_body = b.block(&[]);
    let frame_body = {
        let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        let stmt = b.declare_var("n", FirType::Int32, AccessType::Stack, Some(count));
        b.block(&[stmt])
    };
    let functions = make_functions_with_frame(&mut b, compute_body, frame_body);
    let module_id = {
        let sd = b.block(&[]);
        b.module(0, 0, "dsp", dsp_struct, globals, functions, sd, &[])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-F09"));
}

// ─── Generated-table sub-modules (FIR-SM01…T04) ───────────────────────────────
//
// Fixtures for `porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`
// phase S1. One well-formed module is accepted; each mutation below breaks
// exactly one rule and must be rejected. The mutations are the point: a checker
// that only ever sees valid input proves nothing.

/// Builds `fill{name}(dsp, count, table)` writing `table[i]` once.
fn make_fill_fun(b: &mut FirBuilder<'_>, name: &str, elem: FirType, table_arg: &str) -> FirId {
    let args = vec![
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: table_arg.to_string(),
            typ: FirType::Ptr(Box::new(elem.clone())),
        },
    ];
    let body = {
        let idx = b.int32(0);
        let value = b.float32(0.5);
        let store = b.store_table(table_arg, AccessType::FunArgs, idx, value);
        b.block(&[store])
    };
    let typ = FirType::Fun {
        args: args.iter().map(|a| a.typ.clone()).collect(),
        ret: Box::new(FirType::Void),
    };
    b.declare_fun(format!("fill{name}"), typ, &args, Some(body), false)
}

/// Builds `instanceInit{name}(dsp, sample_rate)`.
fn make_sub_init_fun(b: &mut FirBuilder<'_>, name: &str) -> FirId {
    let args = vec![
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "sample_rate".to_string(),
            typ: FirType::Int32,
        },
    ];
    let body = b.block(&[]);
    let typ = FirType::Fun {
        args: args.iter().map(|a| a.typ.clone()).collect(),
        ret: Box::new(FirType::Void),
    };
    b.declare_fun(format!("instanceInit{name}"), typ, &args, Some(body), false)
}

/// Builds a well-formed sub-module named `name` filling a table of `elem`.
fn make_sub_module(b: &mut FirBuilder<'_>, name: &str, elem: FirType) -> FirId {
    let init = make_sub_init_fun(b, name);
    let fill = make_fill_fun(b, name, elem.clone(), "table");
    let functions = b.block(&[init, fill]);
    let dsp_struct = b.block(&[]);
    let static_decls = b.block(&[]);
    let globals = b.block(&[]);
    b.sub_module(
        name,
        elem,
        dsp_struct,
        static_decls,
        globals,
        functions,
        &[],
    )
}

/// Builds `staticInit(dsp, sample_rate)` calling `fill{sub}(…, <table>)`.
fn make_static_init_fun(b: &mut FirBuilder<'_>, sub: &str, table: &str) -> FirId {
    let args = vec![
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "sample_rate".to_string(),
            typ: FirType::Int32,
        },
    ];
    let body = {
        let count = b.int32(4);
        let table_ref = b.load_var(
            table,
            AccessType::Static,
            FirType::Array(Box::new(FirType::Float32), 4),
        );
        let call = b.fun_call(format!("fill{sub}"), &[count, table_ref], FirType::Void);
        let dropped = b.drop_(call);
        b.block(&[dropped])
    };
    let typ = FirType::Fun {
        args: args.iter().map(|a| a.typ.clone()).collect(),
        ret: Box::new(FirType::Void),
    };
    b.declare_fun("staticInit", typ, &args, Some(body), false)
}

/// Builds a `compute` whose body reads `table[0]`.
fn make_compute_reading_table(b: &mut FirBuilder<'_>, table: &str) -> FirId {
    let params = vec![
        FirType::Ptr(Box::new(FirType::Obj)),
        FirType::Int32,
        FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
    ];
    let args: Vec<NamedType> = ["dsp", "count", "inputs", "outputs"]
        .iter()
        .zip(params.iter())
        .map(|(n, t)| NamedType {
            name: (*n).to_string(),
            typ: t.clone(),
        })
        .collect();
    let body = {
        let idx = b.int32(0);
        let read = b.load_table(table, AccessType::Static, idx, FirType::Float32);
        let decl = b.declare_var("v", FirType::Float32, AccessType::Stack, Some(read));
        b.block(&[decl])
    };
    let typ = FirType::Fun {
        args: params,
        ret: Box::new(FirType::Void),
    };
    b.declare_fun("compute", typ, &args, Some(body), false)
}

/// Assembles the accepted shape, optionally dropping the fill call so the
/// declared table is never written.
fn build_generated_table_module(store: &mut FirStore, with_fill: bool) -> FirId {
    let mut b = FirBuilder::new(store);
    let sub = make_sub_module(&mut b, "dspSIG0", FirType::Float32);
    let table = b.declare_var(
        "ftbl0dspSIG0",
        FirType::Array(Box::new(FirType::Float32), 4),
        AccessType::Static,
        None,
    );
    let static_decls = b.block(&[table]);
    let compute = make_compute_reading_table(&mut b, "ftbl0dspSIG0");
    let functions = if with_fill {
        let static_init = make_static_init_fun(&mut b, "dspSIG0", "ftbl0dspSIG0");
        b.block(&[static_init, compute])
    } else {
        b.block(&[compute])
    };
    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);
    b.module(
        0,
        0,
        "dsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[sub],
    )
}

#[test]
fn generated_table_module_with_fill_call_is_accepted() {
    let mut store = FirStore::new();
    let module_id = build_generated_table_module(&mut store, true);
    let report = verify_fir_module(&store, module_id);
    let table_diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.starts_with("FIR-SM"))
        .collect();
    assert!(
        table_diags.is_empty(),
        "well-formed sub-module rejected: {table_diags:?}"
    );
}

#[test]
fn sm01_table_declared_without_initializer_and_never_filled_is_flagged() {
    // Mutation: drop the `fill` call from staticInit. The table still exists
    // and `compute` still reads it, so without this rule the program compiles
    // and silently reads zeros.
    let mut store = FirStore::new();
    let module_id = build_generated_table_module(&mut store, false);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SM01"),
        "missing fill call was not reported: {:?}",
        report.diagnostics
    );
}

#[test]
fn sm02_sub_module_missing_its_fill_entry_point_is_flagged() {
    // Mutation: a sub-module that declares only `instanceInit`.
    let mut store = FirStore::new();
    let module_id = {
        let mut b = FirBuilder::new(&mut store);
        let sub = {
            let init = make_sub_init_fun(&mut b, "dspSIG0");
            let functions = b.block(&[init]);
            let dsp_struct = b.block(&[]);
            let static_decls = b.block(&[]);
            let globals = b.block(&[]);
            b.sub_module(
                "dspSIG0",
                FirType::Float32,
                dsp_struct,
                static_decls,
                globals,
                functions,
                &[],
            )
        };
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
        let static_decls = b.block(&[]);
        b.module(
            0,
            0,
            "dsp",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[sub],
        )
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SM02"),
        "sub-module without a fill function was accepted: {:?}",
        report.diagnostics
    );
}

#[test]
fn sm03_fill_writing_a_foreign_argument_table_is_flagged() {
    // Mutation: the fill body stores through an argument table that is not
    // `table` — a lowering bug that would corrupt parent state.
    let mut store = FirStore::new();
    let module_id = {
        let mut b = FirBuilder::new(&mut store);
        let sub = {
            let init = make_sub_init_fun(&mut b, "dspSIG0");
            let fill = make_fill_fun(&mut b, "dspSIG0", FirType::Float32, "other");
            let functions = b.block(&[init, fill]);
            let dsp_struct = b.block(&[]);
            let static_decls = b.block(&[]);
            let globals = b.block(&[]);
            b.sub_module(
                "dspSIG0",
                FirType::Float32,
                dsp_struct,
                static_decls,
                globals,
                functions,
                &[],
            )
        };
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
        let static_decls = b.block(&[]);
        b.module(
            0,
            0,
            "dsp",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[sub],
        )
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SM03"),
        "fill writing a foreign table was accepted: {:?}",
        report.diagnostics
    );
}

#[test]
fn sm04_duplicate_sub_module_names_are_flagged() {
    // Mutation: two generators sharing one name, which would collapse them
    // onto a single filler at emission.
    let mut store = FirStore::new();
    let module_id = {
        let mut b = FirBuilder::new(&mut store);
        let first = make_sub_module(&mut b, "dspSIG0", FirType::Float32);
        let second = make_sub_module(&mut b, "dspSIG0", FirType::Int32);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
        let static_decls = b.block(&[]);
        b.module(
            0,
            0,
            "dsp",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[first, second],
        )
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SM04"),
        "duplicate sub-module names were accepted: {:?}",
        report.diagnostics
    );
}

#[test]
fn sm05_nested_generator_whose_fill_is_never_called_is_flagged() {
    // Mutation: a sub-module that owns a nested generator but whose
    // `instanceInit` does not call the nested `fill`. This is the upstream
    // 2.87.1 defect — inner table declared, never written, outer table
    // computed from zeros — and the first implementation of the Rust producer
    // reproduced it by building the sub-module's `instanceInit` from
    // constants/reset/clear only.
    let mut store = FirStore::new();
    let module_id = {
        let mut b = FirBuilder::new(&mut store);
        let inner = make_sub_module(&mut b, "dspSIG0SIG0", FirType::Float32);
        let outer = {
            let init = make_sub_init_fun(&mut b, "dspSIG0");
            let fill = make_fill_fun(&mut b, "dspSIG0", FirType::Float32, "table");
            let functions = b.block(&[init, fill]);
            let empty = b.block(&[]);
            b.sub_module(
                "dspSIG0",
                FirType::Float32,
                empty,
                empty,
                empty,
                functions,
                &[inner],
            )
        };
        let empty = b.block(&[]);
        b.module(0, 0, "dsp", empty, empty, empty, empty, &[outer])
    };
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SM05"),
        "nested generator with no fill call was accepted: {:?}",
        report.diagnostics
    );
}

/// FIR-SM06 is invariant I2: a fill call must cover its table's whole length.
///
/// FIR-SM01 proves a fill happens; only this rule constrains how much it
/// writes. The mutation is the one the port plan names — a `count` of
/// `size - 1` — and it is invisible to numeric tests, since the untouched cell
/// is one of many and holds whatever the target's uninitialized storage did.
#[test]
fn sm06_a_fill_call_that_does_not_cover_the_whole_table_is_flagged() {
    fn module_with_fill_count(count: i32, length: usize) -> (FirStore, FirId) {
        let mut store = FirStore::new();
        let module_id = {
            let mut b = FirBuilder::new(&mut store);
            let sub = make_sub_module(&mut b, "dspSIG0", FirType::Float32);
            let table = b.declare_var(
                "ftbl0dspSIG0",
                FirType::Array(Box::new(FirType::Float32), length),
                AccessType::Static,
                None,
            );
            let static_decls = b.block(&[table]);
            let call = {
                let obj = b.load_var("sig0", AccessType::Stack, FirType::Ptr(Box::new(FirType::Obj)));
                let n = b.int32(count);
                let tbl = b.load_var(
                    "ftbl0dspSIG0",
                    AccessType::Static,
                    FirType::Array(Box::new(FirType::Float32), length),
                );
                let c = b.fun_call("filldspSIG0", &[obj, n, tbl], FirType::Void);
                b.drop_(c)
            };
            let body = b.block(&[call]);
            let static_init = b.declare_fun(
                "staticInit",
                FirType::Fun {
                    args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Int32],
                    ret: Box::new(FirType::Void),
                },
                &[
                    NamedType {
                        name: "dsp".to_string(),
                        typ: FirType::Ptr(Box::new(FirType::Obj)),
                    },
                    NamedType {
                        name: "sample_rate".to_string(),
                        typ: FirType::Int32,
                    },
                ],
                Some(body),
                false,
            );
            let functions = b.block(&[static_init]);
            let empty = b.block(&[]);
            b.module(0, 0, "dsp", empty, empty, functions, static_decls, &[sub])
        };
        (store, module_id)
    }

    // The producer's shape: count equals the declared length.
    let (store, module_id) = module_with_fill_count(8, 8);
    let report = verify_fir_module(&store, module_id);
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "FIR-SM06"),
        "a full-length fill was rejected: {:?}",
        report.diagnostics
    );

    // The mutation: one cell short.
    let (store, module_id) = module_with_fill_count(7, 8);
    let report = verify_fir_module(&store, module_id);
    assert!(
        report.diagnostics.iter().any(|d| d.code == "FIR-SM06"),
        "a short fill was accepted: {:?}",
        report.diagnostics
    );
}
