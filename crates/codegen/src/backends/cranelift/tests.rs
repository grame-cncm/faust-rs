use super::{
    BACKEND_NAME, CraneliftBackendErrorCode, CraneliftOptions, StructFieldKind, backend_id,
    generate_cranelift_module,
};
use crate::fixtures::build_sine_phasor_test_module;
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirType, NamedType};
use std::ffi::c_void;

#[test]
/// Verifies the public backend identifier remains stable.
fn backend_id_is_stable() {
    assert_eq!(BACKEND_NAME, "cranelift");
    assert_eq!(backend_id(), "cranelift");
}

#[test]
/// Verifies non-module FIR roots are rejected with the stable error code.
fn compile_rejects_non_module_root() {
    let mut store = fir::FirStore::new();
    let root = {
        let mut b = fir::FirBuilder::new(&mut store);
        b.int32(0)
    };
    let err = generate_cranelift_module(&store, root, &CraneliftOptions::default())
        .expect_err("non-module root should be rejected");
    assert_eq!(err.code, CraneliftBackendErrorCode::UnsupportedModuleShape);
    assert!(err.to_string().contains("FRS-CGEN-CLIF-0002"));
}

#[test]
/// Verifies a representative fixture produces a live finalized compute symbol.
fn compile_module_emits_real_cranelift_compute_stub() {
    let (store, module) = build_sine_phasor_test_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("sine phasor fixture should compile to a Cranelift compute stub");
    assert_eq!(compiled.module_name(), "mydsp");
    assert_eq!(compiled.compute_symbol_name(), "mydsp::compute");
    assert!(compiled.has_compute_entry());
    assert_ne!(compiled.compute_entry_addr(), 0);
    assert!(compiled.compute_body_lowered());
    let layout = compiled.struct_layout();
    assert_eq!(layout.align_bytes(), 8);
    assert_eq!(layout.size_bytes(), 16);
    assert_eq!(layout.fields().len(), 3);
    assert_eq!(layout.field("fFreq").expect("fFreq field").offset_bytes, 0);
    assert_eq!(layout.field("fGain").expect("fGain field").offset_bytes, 4);
    assert_eq!(
        layout.field("fPhase").expect("fPhase field").offset_bytes,
        8
    );
    assert!(compiled.jit_module_is_alive());
}

#[test]
fn compile_module_emits_instance_constants_entry_when_present() {
    let (store, module) = build_instance_constants_test_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("instanceConstants fixture should compile to a Cranelift module");
    assert_ne!(compiled.instance_constants_entry_addr(), 0);
    assert!(
        compiled
            .generated_functions_clif()
            .iter()
            .any(|(name, _)| name == "mydsp::instanceConstants")
    );
}

fn build_instance_constants_test_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let sample_rate = b.declare_var("fSampleRate", FirType::Int32, AccessType::Struct, None);
    let konst = b.declare_var("fConst0", FirType::Float32, AccessType::Struct, None);
    let dsp_struct = b.block(&[sample_rate, konst]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let dsp_arg = NamedType {
        name: "dsp".to_string(),
        typ: FirType::Ptr(Box::new(FirType::Obj)),
    };
    let sample_rate_arg = NamedType {
        name: "sample_rate".to_string(),
        typ: FirType::Int32,
    };
    let sr_value = b.load_var("sample_rate", AccessType::FunArgs, FirType::Int32);
    let sr_store = b.store_var("fSampleRate", AccessType::Struct, sr_value);
    let sr_load = b.load_var("fSampleRate", AccessType::Struct, FirType::Int32);
    let sr_as_float = b.cast(FirType::Float32, sr_load);
    let const_store = b.store_var("fConst0", AccessType::Struct, sr_as_float);
    let instance_constants_body = b.block(&[sr_store, const_store]);
    let instance_constants = b.declare_fun(
        "instanceConstants",
        FirType::Fun {
            args: vec![dsp_arg.typ.clone(), FirType::Int32],
            ret: Box::new(FirType::Void),
        },
        &[dsp_arg.clone(), sample_rate_arg],
        Some(instance_constants_body),
        false,
    );

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
    let out0_index = b.int32(0);
    let out0_ptr = b.load_table(
        "outputs",
        AccessType::FunArgs,
        out0_index,
        FirType::Ptr(Box::new(FirType::FaustFloat)),
    );
    let out0_alias = b.declare_var(
        "output0",
        FirType::Ptr(Box::new(FirType::FaustFloat)),
        AccessType::Stack,
        Some(out0_ptr),
    );
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let loop_index = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let value = b.load_var("fConst0", AccessType::Struct, FirType::Float32);
    let out_value = b.cast(FirType::FaustFloat, value);
    let out_store = b.store_table("output0", AccessType::Stack, loop_index, out_value);
    let loop_body = b.block(&[out_store]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out0_alias, sample_loop]);
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                dsp_arg.typ.clone(),
                FirType::Int32,
                inputs_arg.typ.clone(),
                outputs_arg.typ.clone(),
            ],
            ret: Box::new(FirType::Void),
        },
        &[dsp_arg, count_arg, inputs_arg, outputs_arg],
        Some(compute_body),
        false,
    );

    let functions = b.block(&[instance_constants, compute]);
    let module = b.module(0, 1, "mydsp", dsp_struct, globals, functions, static_decls);
    (store, module)
}

/// Builds a module whose `compute` body intentionally exceeds the current lowering subset.
fn build_subset_gap_fun_call_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.float32(0.25);
    // Deliberately unsupported in current subset matcher.
    let y = b.fun_call("std::erf", &[x], FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "subset_gap_fun_call",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

/// Builds a minimal `compute` body using one supported foreign math call.
fn build_subset_supported_foreign_fun_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.float32(0.25);
    let y = b.fun_call("isnanf", &[x], FirType::Int32);
    let y = b.cast(FirType::FaustFloat, y);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "subset_supported_foreign_fun",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

/// Builds a `compute` body that declares a loop-local temporary inside a loop body.
fn build_loop_local_declare_var_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let jrec1_decl = b.declare_var("jRec1", FirType::Int32, AccessType::Loop, Some(i0));
    let jrec1 = b.load_var("jRec1", AccessType::Loop, FirType::Int32);
    let sample = b.cast(FirType::FaustFloat, jrec1);
    let store_out = b.store_table("output0", AccessType::Stack, i0, sample);
    let loop_body = b.block(&[jrec1_decl, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "loop_local_declare_var",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
/// Verifies non-strict mode falls back to the no-op compute stub on subset gaps.
fn compile_module_falls_back_to_stub_without_strict_subset_mode() {
    let (store, module) = build_subset_gap_fun_call_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("default mode should allow subset-gap fallback");
    assert!(!compiled.compute_body_lowered());
}

#[test]
/// Verifies strict mode rejects subset-gap fallback.
fn compile_module_fails_on_subset_gap_with_strict_mode() {
    let (store, module) = build_subset_gap_fun_call_module();
    let options = CraneliftOptions {
        fail_on_subset_gap: true,
        ..CraneliftOptions::default()
    };
    let err = generate_cranelift_module(&store, module, &options)
        .expect_err("strict mode must reject subset-gap fallback");
    assert_eq!(err.code, CraneliftBackendErrorCode::UnsupportedModuleShape);
    assert!(err.message.contains("strict mode rejected fallback"));
}

#[test]
fn compile_module_lowers_supported_foreign_fun_subset_call() {
    let (store, module) = build_subset_supported_foreign_fun_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("supported foreign subset call should lower");
    assert!(compiled.compute_body_lowered());
}

/// The lockstep register-carry shape: a Stack scalar written once per sample
/// inside the loop and persisted to a struct field after it. Before locals
/// were backed by builder SSA variables, the tail read returned the last
/// in-loop CLIF value and `define_function` failed SSA dominance verification.
fn build_cross_block_local_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let state_field = b.declare_var("fState", FirType::Float64, AccessType::Struct, None);
    let dsp_struct = b.block(&[state_field]);

    let zero = b.float64(0.0);
    let local_decl = b.declare_var(
        "vlock_local",
        FirType::Float64,
        AccessType::Stack,
        Some(zero),
    );
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let carried = b.cast(FirType::Float64, i0);
    let write_local = b.store_var("vlock_local", AccessType::Stack, carried);
    let loop_body = b.block(&[write_local]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let tail_read = b.load_var("vlock_local", AccessType::Stack, FirType::Float64);
    let persist = b.store_var("fState", AccessType::Struct, tail_read);
    let compute_body = b.block(&[local_decl, sample_loop, persist]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "cross_block_local",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
/// A `StoreVar` may assign a differently typed value to a local, as a C
/// assignment converts to the declared type; the SSA variable's declared type
/// must not reject it. This is the `grain3`/`gate_compressor` shape that
/// panicked (`declared type of variable ... doesn't match type of value`)
/// when bindings first became SSA variables without assignment coercion.
fn compile_module_coerces_retyped_local_assignment() {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let state_field = b.declare_var("iState", FirType::Int32, AccessType::Struct, None);
    let dsp_struct = b.block(&[state_field]);

    let zero = b.int32(0);
    let local_decl = b.declare_var("iGate", FirType::Int32, AccessType::Stack, Some(zero));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let five = b.int32(5);
    let gate = b.binop(FirBinOp::Lt, i0, five, FirType::Bool);
    let write_local = b.store_var("iGate", AccessType::Stack, gate);
    let loop_body = b.block(&[write_local]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let tail_read = b.load_var("iGate", AccessType::Stack, FirType::Int32);
    let persist = b.store_var("iState", AccessType::Struct, tail_read);
    let compute_body = b.block(&[local_decl, sample_loop, persist]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "retyped_local",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    let options = CraneliftOptions {
        fail_on_subset_gap: true,
        ..CraneliftOptions::default()
    };
    let compiled = generate_cranelift_module(&store, module, &options)
        .expect("a retyped local assignment must coerce, not panic");
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_lowers_a_cross_block_stack_local() {
    let (store, module) = build_cross_block_local_module();
    let options = CraneliftOptions {
        fail_on_subset_gap: true,
        ..CraneliftOptions::default()
    };
    let compiled = generate_cranelift_module(&store, module, &options)
        .expect("a loop-written local read after the loop must pass SSA verification");
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_lowers_loop_local_declare_var() {
    let (store, module) = build_loop_local_declare_var_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("loop-local DeclareVar should lower");
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_lowers_stack_local_array_buffer() {
    let (store, module) = build_stack_local_array_buffer_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("stack-local array buffer should lower");
    assert!(compiled.compute_body_lowered());
}

extern "C" fn test_foreign_gain(x: f32) -> f32 {
    x * 0.5
}

/// Builds a minimal `compute` body using a non-built-in foreign function
/// symbol so the extern-function registry path is exercised directly.
fn build_subset_custom_foreign_fun_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.float32(0.25);
    let y = b.fun_call("test_foreign_gain", &[x], FirType::Float32);
    let y = b.cast(FirType::FaustFloat, y);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "subset_custom_foreign_fun",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_custom_foreign_fun_with_extern_function_symbol() {
    let (store, module) = build_subset_custom_foreign_fun_module();
    let options = CraneliftOptions {
        extern_function_symbols: [(
            "test_foreign_gain".to_string(),
            (test_foreign_gain as *const ()).cast::<c_void>(),
        )]
        .into_iter()
        .collect(),
        ..CraneliftOptions::default()
    };
    let compiled = generate_cranelift_module(&store, module, &options)
        .expect("custom foreign function fixture should compile with external function binding");
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_falls_back_when_custom_foreign_fun_symbol_is_missing() {
    let (store, module) = build_subset_custom_foreign_fun_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("missing foreign function binding should fall back to the stub in default mode");
    assert!(!compiled.compute_body_lowered());
}

/// Builds a minimal `compute` body that reads one FIR `AccessType::Global`
/// scalar and writes it to the output buffer.
fn build_subset_global_scalar_load_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
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
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "subset_global_scalar_load",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_global_scalar_load_with_extern_data_symbol() {
    let (store, module) = build_subset_global_scalar_load_module();
    let extvar: f32 = 0.75;
    let options = CraneliftOptions {
        extern_data_symbols: [(
            "extvar".to_string(),
            (&extvar as *const f32).cast::<c_void>(),
        )]
        .into_iter()
        .collect(),
        ..CraneliftOptions::default()
    };
    let compiled = generate_cranelift_module(&store, module, &options)
        .expect("global scalar fixture should compile with external data binding");
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_falls_back_when_global_scalar_symbol_is_missing() {
    let (store, module) = build_subset_global_scalar_load_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("missing global scalar binding should fall back to the stub in default mode");
    assert!(!compiled.compute_body_lowered());
}

/// Builds a minimal `compute` body that should lower fully through the subset path.
fn build_subset_lowerable_compute_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.float32(0.5);
    let half = b.float32(0.5);
    let cond = b.binop(FirBinOp::Ge, x, half, FirType::Bool);
    let s = b.fun_call("std::sin", &[x], FirType::FaustFloat);
    let g = b.float32(0.25);
    let sg = b.binop(FirBinOp::Mul, s, g, FirType::FaustFloat);
    let y = b.select2(cond, sg, g, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "subset_lowerable",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

fn build_select2_mixed_bool_int_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let zero = b.int32(0);
    let flag = b.declare_var("fFlag", FirType::Int32, AccessType::Struct, Some(zero));
    let dsp_struct = b.block(&[flag]);
    let globals = b.block(&[]);
    let static_decls = b.block(&[]);

    let lhs = b.float64(1.0);
    let rhs = b.float64(0.0);
    let cond = b.binop(FirBinOp::Gt, lhs, rhs, FirType::Bool);
    let previous = b.load_var("fFlag", AccessType::Struct, FirType::Int32);
    let selected = b.select2(cond, cond, previous, FirType::Int32);
    let store_flag = b.store_var("fFlag", AccessType::Struct, selected);
    let compute_body = b.block(&[store_flag]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let module = b.module(
        0,
        0,
        "select2_mixed_bool_int",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
/// Verifies the current arithmetic/control subset lowers without fallback.
fn compile_module_lowers_requested_compute_subset_body() {
    let (store, module) = build_subset_lowerable_compute_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_coerces_select2_branches_to_result_type() {
    let (store, module) = build_select2_mixed_bool_int_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("mixed bool/int select2 should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

/// Builds a subset fixture covering stack aliases over input/output tables.
fn build_stack_input_load_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
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
    let half = b.float32(0.5);
    let y = b.binop(FirBinOp::Mul, x, half, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "stack_input_load_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
/// Verifies stack-local input/output alias lowering works in the subset path.
fn compile_module_lowers_stack_input_load_subset_body() {
    let (store, module) = build_stack_input_load_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("stack-input-load subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

/// Builds a subset fixture covering the currently supported math intrinsics.
fn build_math_intrinsics_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.float32(0.25);
    let y = b.float32(0.75);
    let cosx = b.fun_call("std::cos", &[x], FirType::FaustFloat);
    let ex = b.fun_call("std::exp", &[x], FirType::FaustFloat);
    let sqrt_ex = b.fun_call("std::sqrt", &[ex], FirType::FaustFloat);
    let m = b.fun_call("std::fmax", &[cosx, sqrt_ex], FirType::FaustFloat);
    let p = b.fun_call("std::pow", &[m, y], FirType::FaustFloat);
    let z = b.fun_call("std::fmod", &[p, y], FirType::FaustFloat);
    let r = b.fun_call("std::remainder", &[p, y], FirType::FaustFloat);
    let out = b.binop(FirBinOp::Add, z, r, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, out);
    let loop_body = b.block(&[store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "math_intrinsics_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
/// Verifies the supported math intrinsic family lowers without fallback.
fn compile_module_lowers_common_math_intrinsics_subset() {
    let (store, module) = build_math_intrinsics_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("math intrinsics subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_label_and_uninitialized_stack_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let tmp_decl = b.declare_var("tmp", FirType::FaustFloat, AccessType::Stack, None);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.float32(0.125);
    let store_tmp = b.store_var("tmp", AccessType::Stack, x);
    let tmp = b.load_var("tmp", AccessType::Stack, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, tmp);
    let loop_body = b.block(&[store_tmp, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let label_phase = b.label("signal_fir_fastlane_step2a: executable base slice");
    let label_io = b.label("io: inputs=0 outputs=1");
    let compute_body = b.block(&[label_phase, label_io, out_alias, tmp_decl, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "labels_uninit_stack_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_labels_and_uninitialized_stack_subset() {
    let (store, module) = build_label_and_uninitialized_stack_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("label/uninitialized-stack subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_switch_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let v0 = b.float32(0.0);
    let v1 = b.float32(1.0);
    let v2 = b.float32(2.0);
    let v3 = b.float32(3.0);
    let store_case0 = b.store_table("output0", AccessType::Stack, i0, v0);
    let store_case1 = b.store_table("output0", AccessType::Stack, i0, v1);
    let store_case2 = b.store_table("output0", AccessType::Stack, i0, v2);
    let store_default = b.store_table("output0", AccessType::Stack, i0, v3);
    let case0 = b.block(&[store_case0]);
    let case1 = b.block(&[store_case1]);
    let case2 = b.block(&[store_case2]);
    let default_case = b.block(&[store_default]);
    let switch_stmt = b.switch(
        i0,
        &[(0, case0), (1, case1), (2, case2)],
        Some(default_case),
    );
    let loop_body = b.block(&[switch_stmt]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "switch_subset",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_switch_subset_body() {
    let (store, module) = build_switch_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("switch subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_if_control_neg_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let one_i = b.int32(1);
    let cond = b.binop(FirBinOp::Ge, count, one_i, FirType::Bool);
    let base = b.float32(0.125);
    let neg = b.neg(base, FirType::FaustFloat);
    let store_then = b.store_table("output0", AccessType::Stack, i0, neg);
    let then_block = b.block(&[store_then]);
    let else_store = b.store_table("output0", AccessType::Stack, i0, base);
    let else_block = b.block(&[else_store]);
    let if_stmt = b.if_(cond, then_block, Some(else_block));
    let loop_body = b.block(&[if_stmt]);
    let loop_ = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "if_control_neg_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_if_control_and_neg_subset_body() {
    let (store, module) = build_if_control_neg_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("if/control/neg subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_for_while_local_store_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);
    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let tmp0 = b.float32(0.2);
    let tmp = b.declare_var("tmp", FirType::FaustFloat, AccessType::Stack, Some(tmp0));
    let tmpv = b.load_var("tmp", AccessType::Stack, FirType::FaustFloat);
    let neg = b.neg(tmpv, FirType::FaustFloat);
    let tmp_set = b.store_var("tmp", AccessType::Stack, neg);
    let false_cond = b.bool_(false);
    let empty = b.block(&[]);
    let while_ = b.while_loop(false_cond, empty);

    let init = b.int32(0);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let step = b.int32(1);
    let i = b.load_var("i", AccessType::Loop, FirType::Int32);
    let tmp_cur = b.load_var("tmp", AccessType::Stack, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i, tmp_cur);
    let for_body = b.block(&[store_out]);
    let for_ = b.for_loop("i", init, count, step, for_body, false);

    let compute_body = b.block(&[out_alias, tmp, tmp_set, while_, for_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "for_while_local_store_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_for_while_and_local_store_subset_body() {
    let (store, module) = build_for_while_local_store_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("for/while/local-store subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_for_loop_declared_init_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let zero = b.int32(0);
    let init = b.declare_var("i", FirType::Int32, AccessType::Loop, Some(zero));
    let step = b.int32(1);
    let i = b.load_var("i", AccessType::Loop, FirType::Int32);
    let sample = b.cast(FirType::FaustFloat, i);
    let store_out = b.store_table("output0", AccessType::Stack, i, sample);
    let for_body = b.block(&[store_out]);
    let loop_ = b.for_loop("i", init, count, step, for_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "for_loop_declared_init_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_for_loop_with_declared_loop_init() {
    let (store, module) = build_for_loop_declared_init_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("for-loop init DeclareVar(kLoop) should stay inside the supported subset");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_global_table_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let t0 = b.float64(0.0);
    let t1 = b.float64(1.0);
    let t2 = b.float64(2.0);
    let table = b.declare_table(
        "fTbl0",
        AccessType::Struct,
        FirType::FaustFloat,
        &[t0, t1, t2],
    );
    let globals = b.block(&[table]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let zero = b.int32(0);
    let read0 = b.load_table("fTbl0", AccessType::Struct, zero, FirType::FaustFloat);
    let write0 = b.store_table("fTbl0", AccessType::Struct, zero, read0);
    let i = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let out_val = b.load_table("fTbl0", AccessType::Struct, zero, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i, out_val);
    let loop_body = b.block(&[write0, store_out]);
    let loop_ = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "global_table_subset",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

fn build_struct_array_var_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let z0 = b.float32(0.0);
    let z1 = b.float32(0.0);
    let init = b.value_array(&[z0, z1], FirType::Array(Box::new(FirType::Float32), 2));
    let rec = b.declare_var(
        "fRec0",
        FirType::Array(Box::new(FirType::Float32), 2),
        AccessType::Struct,
        Some(init),
    );
    let globals = b.block(&[]);
    let dsp_struct = b.block(&[rec]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let zero = b.int32(0);
    let one = b.int32(1);
    let prev = b.load_table("fRec0", AccessType::Struct, one, FirType::Float32);
    let write_cur = b.store_table("fRec0", AccessType::Struct, zero, prev);
    let outv = b.load_table("fRec0", AccessType::Struct, zero, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, outv);
    let loop_body = b.block(&[write_cur, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "struct_array_var_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

fn build_stack_local_array_buffer_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let globals = b.block(&[]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
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
    let store_out = b.store_table("output0", AccessType::Stack, i0, read_vbuf);
    let loop_body = b.block(&[store_vbuf, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, vbuf, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: compute_args.iter().map(|arg| arg.typ.clone()).collect(),
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        1,
        "stack_local_array_buffer",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

fn build_int32_and_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let iota_init = b.int32(0);
    let iota = b.declare_var("fIOTA", FirType::Int32, AccessType::Struct, Some(iota_init));
    let globals = b.block(&[]);
    let dsp_struct = b.block(&[iota]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let cur = b.load_var("fIOTA", AccessType::Struct, FirType::Int32);
    let mask = b.int32(16383);
    let masked = b.binop(FirBinOp::And, cur, mask, FirType::Int32);
    let store_iota = b.store_var("fIOTA", AccessType::Struct, masked);
    let zero = b.float32(0.0);
    let store_out = b.store_table("output0", AccessType::Stack, i0, zero);
    let loop_body = b.block(&[store_iota, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "int32_and_subset",
        dsp_struct,
        globals,
        functions,
        static_decls,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_global_struct_table_subset_body() {
    let (store, module) = build_global_table_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("global-struct-table subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
    let table = compiled
        .struct_layout()
        .field("fTbl0")
        .expect("table field in layout");
    assert!(matches!(
        &table.kind,
        StructFieldKind::Table {
            elem_type: FirType::FaustFloat,
            len: 3
        }
    ));
}

#[test]
fn compile_module_lowers_struct_array_var_subset_body() {
    let (store, module) = build_struct_array_var_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("struct-array var subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
    let field = compiled
        .struct_layout()
        .field("fRec0")
        .expect("array-backed struct field in layout");
    assert!(matches!(
        &field.kind,
        StructFieldKind::Table {
            elem_type: FirType::Float32,
            len: 2
        }
    ));
}

#[test]
fn compile_module_lowers_int32_and_subset_body() {
    let (store, module) = build_int32_and_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("int32-and subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

fn build_globals_with_helper_prototype_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let init = b.float64(0.5);
    let gain = b.declare_var("fGain", FirType::FaustFloat, AccessType::Struct, Some(init));
    let helper_args = [
        NamedType {
            name: "arg0".to_string(),
            typ: FirType::FaustFloat,
        },
        NamedType {
            name: "arg1".to_string(),
            typ: FirType::FaustFloat,
        },
    ];
    let helper_proto = b.declare_fun(
        "fmin",
        FirType::Fun {
            args: vec![FirType::FaustFloat, FirType::FaustFloat],
            ret: Box::new(FirType::FaustFloat),
        },
        &helper_args,
        None,
        false,
    );
    let globals = b.block(&[gain, helper_proto]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let g = b.load_var("fGain", AccessType::Struct, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, g);
    let loop_body = b.block(&[store_out]);
    let loop_ = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "globals_helper_proto_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
fn compile_module_ignores_helper_prototypes_in_globals_layout() {
    let (store, module) = build_globals_with_helper_prototype_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("helper prototypes in globals should be ignored for dsp* layout");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
    let layout = compiled.struct_layout();
    assert!(layout.field("fGain").is_some());
    assert!(layout.field("fmin").is_none());
}

fn build_shift_array_var_struct_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let z = b.float32(0.0);
    let o = b.float32(1.0);
    let t = b.float32(2.0);
    let tbl = b.declare_table("hist", AccessType::Struct, FirType::FaustFloat, &[z, o, t]);
    let globals = b.block(&[tbl]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let idx0 = b.int32(0);
    let sample = b.load_table("hist", AccessType::Struct, idx0, FirType::FaustFloat);
    let push = b.store_table("hist", AccessType::Struct, idx0, sample);
    let shift = b.shift_array_var("hist", AccessType::Struct, 2);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let outv = b.load_table("hist", AccessType::Struct, idx0, FirType::FaustFloat);
    let store_out = b.store_table("output0", AccessType::Stack, i0, outv);
    let loop_body = b.block(&[shift, push, store_out]);
    let loop_ = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "shift_array_var_struct_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

fn build_int_to_float_cast_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let one = b.int32(1);
    let sum = b.binop(FirBinOp::Add, i0, one, FirType::Int32);
    let sample = b.cast(FirType::Float32, sum);
    let store_out = b.store_table("output0", AccessType::Stack, i0, sample);
    let loop_body = b.block(&[store_out]);
    let loop_ = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "int_to_float_cast_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

fn build_float_to_int_cast_subset_module() -> (fir::FirStore, FirId) {
    let mut store = fir::FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let zero = b.float32(10.0);
    let one = b.float32(20.0);
    let two = b.float32(30.0);
    let table = b.declare_table(
        "fTbl0",
        AccessType::Struct,
        FirType::Float32,
        &[zero, one, two],
    );
    let globals = b.block(&[table]);
    let dsp_struct = b.block(&[]);

    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let idx_f = b.float32(1.75);
    let idx_i = b.cast(FirType::Int32, idx_f);
    let sample = b.load_table("fTbl0", AccessType::Struct, idx_i, FirType::Float32);
    let store_out = b.store_table("output0", AccessType::Stack, i0, sample);
    let loop_body = b.block(&[store_out]);
    let loop_ = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[out_alias, loop_]);
    let compute_args = [
        NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = b.declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            ret: Box::new(FirType::Void),
        },
        &compute_args,
        Some(compute_body),
        false,
    );
    let functions = b.block(&[compute]);
    let sd = b.block(&[]);
    let module = b.module(
        0,
        0,
        "float_to_int_cast_subset",
        dsp_struct,
        globals,
        functions,
        sd,
    );
    (store, module)
}

#[test]
fn compile_module_lowers_shift_array_var_struct_subset_body() {
    let (store, module) = build_shift_array_var_struct_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("shift-array-var struct subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
}

#[test]
fn compile_module_lowers_int_to_float_cast_subset_body() {
    let (store, module) = build_int_to_float_cast_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("int-to-float cast subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
    let compute_clif = &compiled.generated_functions_clif()[0].1;
    assert!(
        compute_clif.contains("fcvt_from_sint"),
        "expected int-to-float cast lowering in CLIF, got:\n{compute_clif}"
    );
}

#[test]
fn compile_module_lowers_float_to_int_cast_subset_body() {
    let (store, module) = build_float_to_int_cast_subset_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .expect("float-to-int cast subset fixture should compile with body lowering");
    assert!(compiled.has_compute_entry());
    assert!(compiled.compute_body_lowered());
    let compute_clif = &compiled.generated_functions_clif()[0].1;
    assert!(
        compute_clif.contains("fcvt_to_sint"),
        "expected float-to-int cast lowering in CLIF, got:\n{compute_clif}"
    );
}
