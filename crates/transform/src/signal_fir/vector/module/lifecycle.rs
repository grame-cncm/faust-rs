//! Final module assembly: lifecycle functions, prototypes, and compute
//! drivers.

use super::model::VectorModuleFailure;
use crate::signal_fir::VectorFallbackReason;
use crate::signal_fir::module::{INT_FUN_PROTO_ORDER, MATH_PROTO_ORDER};
use crate::signal_fir::vector::assemble::VectorFirAssembly;
use crate::signal_fir::vector::ui::VectorUiFir;
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirStore, FirType, NamedType};
pub(super) struct FinalModuleContext<'a> {
    pub(super) module_name: &'a str,
    pub(super) num_inputs: usize,
    pub(super) num_outputs: usize,
    pub(super) real_type: &'a FirType,
    pub(super) vec_size: u32,
    pub(super) loop_variant: u8,
    pub(super) control_statements: &'a [FirId],
    pub(super) table_declarations: &'a [FirId],
    pub(super) table_init_statements: &'a [FirId],
    pub(super) math_ops: &'a std::collections::HashSet<fir::FirMathOp>,
    pub(super) int_helpers: &'a std::collections::BTreeSet<&'static str>,
    pub(super) assembly: &'a VectorFirAssembly,
    pub(super) control_output_stores: &'a [FirId],
    pub(super) ui_fir: &'a VectorUiFir,
    pub(super) static_declarations: &'a [FirId],
    /// Externalizable control statements: the `control(dsp)` body under
    /// `-ec` (plan phase 5). Empty in classic mode.
    pub(super) external_control_statements: &'a [FirId],
    /// DSP struct fields created by external-control promotion.
    pub(super) control_state_fields: &'a [(String, FirType)],
    /// Fill statements for file-scope generated tables: the `staticInit` body
    /// (rendered as `classInit`). Empty unless `--table-init runtime` produced
    /// a read-only generated table.
    pub(super) static_init_statements: &'a [FirId],
    /// Generated-table `SubModule` nodes carried by this module.
    pub(super) sub_modules: &'a [FirId],
}
/// Declares the instance lifecycle functions: an empty `metadata`,
/// `instanceConstants` (sample-rate store plus once-at-init table fills),
/// `instanceResetUserInterface`, `instanceClear`, and `buildUserInterface`.
fn declare_instance_functions(
    store: &mut FirStore,
    context: &FinalModuleContext<'_>,
    dsp_arg: &NamedType,
    dsp_arg_type: &FirType,
) -> (FirId, FirId, FirId, FirId, FirId) {
    let table_init_statements = context.table_init_statements;
    let ui_fir = context.ui_fir;
    let assembly = context.assembly;
    let empty = FirBuilder::new(store).block(&[]);
    let metadata = FirBuilder::new(store).declare_fun(
        "metadata",
        FirType::Fun {
            args: vec![dsp_arg_type.clone(), FirType::Meta],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "m".to_owned(),
                typ: FirType::Meta,
            },
        ],
        Some(empty),
        false,
    );

    let sample_rate =
        FirBuilder::new(store).load_var("sample_rate", AccessType::FunArgs, FirType::Int32);
    let sample_rate_store =
        FirBuilder::new(store).store_var("fSampleRate", AccessType::Struct, sample_rate);
    // Mutable-table content is written once at init and persists across
    // compute calls; emitting it in the per-block control section would reset
    // runtime writes every block.
    let mut constants_statements = vec![sample_rate_store];
    constants_statements.extend(table_init_statements.iter().copied());
    let constants_body = FirBuilder::new(store).block(&constants_statements);
    let instance_constants = FirBuilder::new(store).declare_fun(
        "instanceConstants",
        FirType::Fun {
            args: vec![dsp_arg_type.clone(), FirType::Int32],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "sample_rate".to_owned(),
                typ: FirType::Int32,
            },
        ],
        Some(constants_body),
        false,
    );

    let reset_body = FirBuilder::new(store).block(&ui_fir.reset_statements);
    let instance_reset_ui =
        lifecycle_function(store, "instanceResetUserInterface", dsp_arg, reset_body);
    let clear_body = FirBuilder::new(store).block(&assembly.clear_statements);
    let instance_clear = lifecycle_function(store, "instanceClear", dsp_arg, clear_body);
    let ui_body = FirBuilder::new(store).block(&ui_fir.build_statements);
    let build_ui = FirBuilder::new(store).declare_fun(
        "buildUserInterface",
        FirType::Fun {
            args: vec![dsp_arg_type.clone(), FirType::UI],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "ui_interface".to_owned(),
                typ: FirType::UI,
            },
        ],
        Some(ui_body),
        false,
    );

    (
        metadata,
        instance_constants,
        instance_reset_ui,
        instance_clear,
        build_ui,
    )
}

/// Declares the vector `compute` entry point: the control preamble, the
/// assembly's local declarations, and the `-lv`-selected chunk driver over
/// the assembled top-level statement (plus the control-output fill loop).
fn declare_compute_function(
    store: &mut FirStore,
    context: &FinalModuleContext<'_>,
    dsp_arg: &NamedType,
    dsp_arg_type: &FirType,
) -> Result<FirId, VectorModuleFailure> {
    let assembly = context.assembly;
    let control_statements = context.control_statements;
    let control_output_stores = context.control_output_stores;
    let vec_size = context.vec_size;
    let loop_variant = context.loop_variant;
    let chunk = if control_output_stores.is_empty() {
        assembly.top_level_statement
    } else {
        let fill = sample_loop_for_statements(store, control_output_stores);
        FirBuilder::new(store).block(&[assembly.top_level_statement, fill])
    };
    let driver = build_chunk_driver(store, chunk, vec_size, loop_variant)?;
    let mut compute_statements = control_statements.to_vec();
    compute_statements.extend(assembly.local_declarations.iter().copied());
    compute_statements.extend(driver);
    let compute_body = FirBuilder::new(store).block(&compute_statements);
    let audio_ptr = FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat))));
    let compute = FirBuilder::new(store).declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                dsp_arg_type.clone(),
                FirType::Int32,
                audio_ptr.clone(),
                audio_ptr.clone(),
            ],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "count".to_owned(),
                typ: FirType::Int32,
            },
            NamedType {
                name: "inputs".to_owned(),
                typ: audio_ptr.clone(),
            },
            NamedType {
                name: "outputs".to_owned(),
                typ: audio_ptr,
            },
        ],
        Some(compute_body),
        false,
    );

    Ok(compute)
}

pub(super) fn assemble_module(
    store: &mut FirStore,
    context: &FinalModuleContext<'_>,
) -> Result<FirId, VectorModuleFailure> {
    let module_name = context.module_name;
    let num_inputs = context.num_inputs;
    let num_outputs = context.num_outputs;
    let real_type = context.real_type.clone();
    let table_declarations = context.table_declarations;
    let math_ops = context.math_ops;
    let int_helpers = context.int_helpers;
    let assembly = context.assembly;
    let static_declarations = context.static_declarations;
    let dsp_arg_type = FirType::Ptr(Box::new(FirType::Obj));
    let dsp_arg = NamedType {
        name: "dsp".to_owned(),
        typ: dsp_arg_type.clone(),
    };
    let (metadata, instance_constants, instance_reset_ui, instance_clear, build_ui) =
        declare_instance_functions(store, context, &dsp_arg, &dsp_arg_type);
    let compute = declare_compute_function(store, context, &dsp_arg, &dsp_arg_type)?;
    // Execution-options port phase 5: emit `control(dsp)` when externalized
    // control statements exist. The host owns its scheduling; compute never
    // calls it implicitly (plan provenance: §2.3).
    let control = (!context.external_control_statements.is_empty()).then(|| {
        let control_body = FirBuilder::new(store).block(context.external_control_statements);
        FirBuilder::new(store).declare_fun(
            "control",
            FirType::Fun {
                args: vec![dsp_arg_type.clone()],
                ret: Box::new(FirType::Void),
            },
            std::slice::from_ref(&dsp_arg),
            Some(control_body),
            false,
        )
    });
    let globals = build_prototypes(store, real_type, math_ops, int_helpers);
    // `staticInit` is emitted only when there is something to fill. A module
    // with no generated table keeps the pre-sub-module shape, so nothing in
    // the 16-mode certification sees a new function.
    let static_init = (!context.static_init_statements.is_empty()).then(|| {
        let body = FirBuilder::new(store).block(context.static_init_statements);
        FirBuilder::new(store).declare_fun(
            "staticInit",
            FirType::Fun {
                args: vec![dsp_arg_type.clone(), FirType::Int32],
                ret: Box::new(FirType::Void),
            },
            &[
                dsp_arg.clone(),
                NamedType {
                    name: "sample_rate".to_owned(),
                    typ: FirType::Int32,
                },
            ],
            Some(body),
            false,
        )
    });

    let mut function_items = vec![metadata];
    function_items.extend(static_init);
    function_items.extend([
        instance_constants,
        instance_reset_ui,
        instance_clear,
        build_ui,
    ]);
    function_items.extend(control);
    function_items.push(compute);
    let functions = FirBuilder::new(store).block(&function_items);
    let sample_rate_field =
        FirBuilder::new(store).declare_var("fSampleRate", FirType::Int32, AccessType::Struct, None);
    let mut fields = vec![sample_rate_field];
    fields.extend(context.ui_fir.struct_declarations.iter().copied());
    for (name, typ) in context.control_state_fields {
        let decl =
            FirBuilder::new(store).declare_var(name.clone(), typ.clone(), AccessType::Struct, None);
        fields.push(decl);
    }
    fields.extend(assembly.state_declarations.iter().copied());
    fields.extend(table_declarations.iter().copied());
    let dsp_struct = FirBuilder::new(store).block(&fields);
    let static_declarations = FirBuilder::new(store).block(static_declarations);
    Ok(FirBuilder::new(store).module(
        num_inputs,
        num_outputs,
        module_name,
        dsp_struct,
        globals,
        functions,
        static_declarations,
        context.sub_modules,
    ))
}
pub(super) fn sample_loop_for_statements(store: &mut FirStore, statements: &[FirId]) -> FirId {
    let body = FirBuilder::new(store).block(statements);
    let mut builder = FirBuilder::new(store);
    let start = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let init = builder.declare_var("i0", FirType::Int32, AccessType::Loop, Some(start));
    let start = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
    let end = builder.binop(FirBinOp::Add, start, count, FirType::Int32);
    let step = builder.int32(1);
    builder.for_loop("i0", init, end, step, body, false)
}
pub(super) fn lifecycle_function(
    store: &mut FirStore,
    name: &str,
    dsp_arg: &NamedType,
    body: FirId,
) -> FirId {
    FirBuilder::new(store).declare_fun(
        name,
        FirType::Fun {
            args: vec![dsp_arg.typ.clone()],
            ret: Box::new(FirType::Void),
        },
        std::slice::from_ref(dsp_arg),
        Some(body),
        false,
    )
}
pub(super) fn build_prototypes(
    store: &mut FirStore,
    real_type: FirType,
    math_ops: &std::collections::HashSet<fir::FirMathOp>,
    int_helpers: &std::collections::BTreeSet<&'static str>,
) -> FirId {
    let mut prototypes = Vec::new();
    for op in MATH_PROTO_ORDER {
        if !math_ops.contains(op) {
            continue;
        }
        let arity = match op {
            fir::FirMathOp::Pow
            | fir::FirMathOp::Min
            | fir::FirMathOp::Max
            | fir::FirMathOp::Atan2
            | fir::FirMathOp::Fmod
            | fir::FirMathOp::Remainder => 2,
            _ => 1,
        };
        let args = (0..arity)
            .map(|index| NamedType {
                name: format!("arg{index}"),
                typ: real_type.clone(),
            })
            .collect::<Vec<_>>();
        prototypes.push(FirBuilder::new(store).declare_fun(
            op.symbol(),
            FirType::Fun {
                args: vec![real_type.clone(); arity],
                ret: Box::new(real_type.clone()),
            },
            &args,
            None,
            false,
        ));
    }
    for name in INT_FUN_PROTO_ORDER {
        if !int_helpers.contains(name) {
            continue;
        }
        let arity = usize::from(*name != "abs") + 1;
        let args = (0..arity)
            .map(|index| NamedType {
                name: format!("arg{index}"),
                typ: FirType::Int32,
            })
            .collect::<Vec<_>>();
        prototypes.push(FirBuilder::new(store).declare_fun(
            *name,
            FirType::Fun {
                args: vec![FirType::Int32; arity],
                ret: Box::new(FirType::Int32),
            },
            &args,
            None,
            false,
        ));
    }
    FirBuilder::new(store).block(&prototypes)
}
pub(super) fn build_chunk_driver(
    store: &mut FirStore,
    chunk: FirId,
    vec_size: u32,
    loop_variant: u8,
) -> Result<Vec<FirId>, VectorModuleFailure> {
    let vec_size = i32::try_from(vec_size).map_err(|_| {
        VectorModuleFailure::new(
            VectorFallbackReason::ModuleVerification,
            "vector size exceeds FIR i32",
        )
    })?;
    match loop_variant {
        0 => Ok(build_fast_driver(store, chunk, vec_size)),
        1 => Ok(vec![build_simple_driver(store, chunk, vec_size)]),
        _ => Err(VectorModuleFailure::new(
            VectorFallbackReason::ModuleVerification,
            format!("unsupported vector loop variant {loop_variant}"),
        )),
    }
}
pub(super) fn build_simple_driver(store: &mut FirStore, chunk: FirId, vec_size: i32) -> FirId {
    let mut builder = FirBuilder::new(store);
    let index = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let remaining = builder.binop(FirBinOp::Sub, count, index, FirType::Int32);
    let width = builder.int32(vec_size);
    let smaller = builder.binop(FirBinOp::Lt, remaining, width, FirType::Bool);
    let vcount = builder.select2(smaller, remaining, width, FirType::Int32);
    let vcount = builder.declare_var("vcount", FirType::Int32, AccessType::Stack, Some(vcount));
    let body = builder.block(&[vcount, chunk]);
    let zero = builder.int32(0);
    let init = builder.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(zero));
    let end = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let step = builder.int32(vec_size);
    builder.for_loop("vindex", init, end, step, body, false)
}
pub(super) fn build_fast_driver(store: &mut FirStore, chunk: FirId, vec_size: i32) -> Vec<FirId> {
    let mut builder = FirBuilder::new(store);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let width = builder.int32(vec_size);
    let rem = builder.binop(FirBinOp::Rem, count, width, FirType::Int32);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let limit = builder.binop(FirBinOp::Sub, count, rem, FirType::Int32);

    let width_value = builder.int32(vec_size);
    let main_vcount = builder.declare_var(
        "vcount",
        FirType::Int32,
        AccessType::Stack,
        Some(width_value),
    );
    let main_body = builder.block(&[main_vcount, chunk]);
    let zero = builder.int32(0);
    let main_init = builder.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(zero));
    let main_step = builder.int32(vec_size);
    let main_loop = builder.for_loop("vindex", main_init, limit, main_step, main_body, false);
    let zero = builder.int32(0);
    let has_main = builder.binop(FirBinOp::Gt, limit, zero, FirType::Bool);
    let main_then = builder.block(&[main_loop]);
    let guarded_main = builder.if_(has_main, main_then, None);

    let rem_init = builder.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(limit));
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let remaining = builder.binop(FirBinOp::Sub, count, limit, FirType::Int32);
    let rem_vcount =
        builder.declare_var("vcount", FirType::Int32, AccessType::Stack, Some(remaining));
    let rem_body = builder.block(&[rem_init, rem_vcount, chunk]);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let has_rem = builder.binop(FirBinOp::Lt, limit, count, FirType::Bool);
    let guarded_rem = builder.if_(has_rem, rem_body, None);
    vec![guarded_main, guarded_rem]
}
