//! JIT data-section materialization for Cranelift.
//!
//! FIR static tables are lowered to Cranelift data objects before function
//! lowering so compute bodies can reference them through stable `DataId`
//! handles.

use super::*;

/// Declares and defines every `AccessType::Static` table from the FIR
/// `static_decls` block as a JIT read-only data object.
///
/// Static (file-scope constant) tables are emitted as `const static` arrays in
/// the C/C++ backends.  In the Cranelift JIT they must be materialized as named
/// data sections before the `compute` function is compiled, because function
/// bodies reference them via `GlobalValue` handles obtained from the `DataId`.
///
/// Returns a map `name → DataId` that `LoadTable { access: Static }` lowering
/// uses inside `ComputeLowering::lower_expr`.
pub(crate) fn define_static_tables_in_jit(
    store: &FirStore,
    module: FirId,
    jit: &mut JITModule,
    double: bool,
) -> Result<HashMap<String, DataId>, CraneliftBackendError> {
    let static_decls_block = match match_fir(store, module) {
        FirMatch::Module { static_decls, .. } => static_decls,
        _ => return Ok(HashMap::new()),
    };
    let items = match match_fir(store, static_decls_block) {
        FirMatch::Block(ids) => ids,
        _ => return Ok(HashMap::new()),
    };

    let mut result = HashMap::new();
    for id in items {
        let FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } = match_fir(store, id)
        else {
            continue;
        };
        if values.is_empty() {
            continue;
        }

        // Serialise element values to little-endian bytes.
        let bytes: Box<[u8]> = match &elem_type {
            FirType::Int32 => {
                let mut buf = Vec::with_capacity(values.len() * 4);
                for &v in &values {
                    if let FirMatch::Int32 { value, .. } = match_fir(store, v) {
                        buf.extend_from_slice(&value.to_le_bytes());
                    }
                }
                buf.into_boxed_slice()
            }
            FirType::Float32 | FirType::FaustFloat
                if !(matches!(elem_type, FirType::FaustFloat) && double) =>
            {
                let mut buf = Vec::with_capacity(values.len() * 4);
                for &v in &values {
                    match match_fir(store, v) {
                        FirMatch::Float32 { value, .. } => {
                            buf.extend_from_slice(&value.to_le_bytes());
                        }
                        FirMatch::Float64 { value, .. } => {
                            buf.extend_from_slice(&(value as f32).to_le_bytes());
                        }
                        _ => {}
                    }
                }
                buf.into_boxed_slice()
            }
            // `FaustFloat` static table under `-double`: emit 64-bit elements.
            FirType::Float64 | FirType::FaustFloat => {
                let mut buf = Vec::with_capacity(values.len() * 8);
                for &v in &values {
                    match match_fir(store, v) {
                        FirMatch::Float64 { value, .. } => {
                            buf.extend_from_slice(&value.to_le_bytes());
                        }
                        FirMatch::Float32 { value, .. } => {
                            buf.extend_from_slice(&(value as f64).to_le_bytes());
                        }
                        _ => {}
                    }
                }
                buf.into_boxed_slice()
            }
            other => {
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "static table `{name}` has unsupported element type for JIT data: {other:?}"
                )));
            }
        };

        let align: u64 = match &elem_type {
            FirType::Float64 | FirType::Int64 => 8,
            FirType::FaustFloat if double => 8,
            _ => 4,
        };

        // Declare as local (not exported), read-only, not thread-local.
        let data_id = jit
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| {
                CraneliftBackendError::jit_failure(format!("declare_data `{name}` failed: {e}"))
            })?;

        let mut desc = DataDescription::new();
        desc.init = Init::Bytes { contents: bytes };
        desc.align = Some(align);
        jit.define_data(data_id, &desc).map_err(|e| {
            CraneliftBackendError::jit_failure(format!("define_data `{name}` failed: {e}"))
        })?;

        result.insert(name, data_id);
    }
    Ok(result)
}

/// Declares imported data symbols for FIR `AccessType::Global` scalar loads.
///
/// The actual addresses are provided by the caller through
/// [`CraneliftOptions::extern_data_symbols`]. Cranelift resolves them via the
/// JIT symbol table, mirroring how imported math functions are handled.
pub(crate) fn declare_extern_data_symbols_in_jit(
    jit: &mut JITModule,
    extern_data_symbols: &HashMap<String, *const c_void>,
) -> Result<HashMap<String, DataId>, CraneliftBackendError> {
    let mut result = HashMap::new();
    for name in extern_data_symbols.keys() {
        let data_id = jit
            .declare_data(name, Linkage::Import, false, false)
            .map_err(|e| {
                CraneliftBackendError::jit_failure(format!(
                    "declare imported data `{name}` failed: {e}"
                ))
            })?;
        result.insert(name.clone(), data_id);
    }
    Ok(result)
}

/// Declares, defines and finalizes the exported `compute` function in the JIT.
///
/// # Behavior
/// - Creates the Cranelift function signature for the Faust `compute` ABI.
/// - Tries real subset lowering when the subset pre-check accepts the body.
/// - Emits a no-op `return` stub otherwise (or when lowering reports an
///   unsupported shape).
/// - Finalizes definitions and returns:
///   - exported symbol name,
///   - finalized function address,
///   - whether a real body was lowered.
///
/// # Why the name says `stub`
/// Historically this helper started as pure stub emission during bring-up; it
/// now owns both real subset lowering and stub fallback while keeping the same
/// outer responsibility (emit/finalize `compute`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn declare_jit_function(
    symbol_name: &str,
    function_decl: FirId,
    store: &FirStore,
    struct_layout: &StructLayoutPlan,
    fail_on_subset_gap: bool,
    extern_data_symbols: &HashMap<String, *const c_void>,
    extern_function_symbols: &HashMap<String, *const c_void>,
    // When `true`, skip subset lowering and always emit a `return` stub.
    // Used by the JIT-panic fallback path in `generate_cranelift_module`.
    force_stub: bool,
    jit: &mut JITModule,
    static_data_ids: &HashMap<String, DataId>,
    extern_data_ids: &HashMap<String, DataId>,
    double: bool,
) -> Result<(String, usize, bool, String), CraneliftBackendError> {
    let ptr_ty = jit.target_config().pointer_type();
    let function_symbol_name = symbol_name.to_owned();

    let (arg_types, function_name) = match match_fir(store, function_decl) {
        FirMatch::DeclareFun { name, typ, .. } => {
            let FirType::Fun { args, ret } = typ else {
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "function `{name}` does not have FIR function type"
                )));
            };
            if !matches!(*ret, FirType::Void) {
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "Cranelift JIT helper lowering currently expects `{name}` to return Void"
                )));
            }
            (args, name)
        }
        other => {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "expected FIR DeclareFun for JIT function lowering, got {other:?}"
            )));
        }
    };

    let mut ctx = jit.make_context();
    ctx.func.signature.params = arg_types
        .iter()
        .map(|typ| {
            let clif_ty = fir_type_to_clif_type(ptr_ty, typ, double).map_err(|e| {
                CraneliftBackendError::unsupported_module_shape(format!(
                    "unsupported JIT helper arg type for `{function_name}`: {e}"
                ))
            })?;
            Ok(AbiParam::new(clif_ty))
        })
        .collect::<Result<Vec<_>, CraneliftBackendError>>()?;
    // void return

    let func_id = jit
        .declare_function(&function_symbol_name, Linkage::Export, &ctx.func.signature)
        .map_err(|e| {
            CraneliftBackendError::jit_failure(format!(
                "declare_function `{function_symbol_name}` failed: {e}"
            ))
        })?;

    let mut fb_ctx = FunctionBuilderContext::new();
    let compute_body_lowered;
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);
        if !force_stub
            && function_body_matches_current_subset(
                store,
                function_decl,
                extern_data_symbols,
                extern_function_symbols,
            )
        {
            match try_lower_function_body(
                FunctionBodyLoweringContext {
                    store,
                    jit,
                    struct_layout,
                    ptr_ty,
                    static_data_ids,
                    extern_data_ids,
                    extern_function_symbols,
                    double,
                },
                &mut fb,
                function_decl,
            ) {
                Ok(lowered) => compute_body_lowered = lowered,
                Err(LoweringError::Unsupported(reason)) => {
                    return Err(CraneliftBackendError::unsupported_module_shape(format!(
                        "Cranelift subset matcher drift: pre-check accepted `compute`, but lowering rejected it: {reason}"
                    )));
                }
                Err(LoweringError::Jit(msg)) => {
                    return Err(CraneliftBackendError::jit_failure(msg));
                }
            }
        } else {
            // Emit a valid no-op `compute` stub when either:
            // - the FIR body exceeds the currently supported lowering subset, or
            // - `force_stub` was set by the JIT-panic fallback path.
            if fail_on_subset_gap && !force_stub {
                let reason = function_body_subset_gap_reason_from_decl(
                    store,
                    function_decl,
                    extern_data_symbols,
                    extern_function_symbols,
                )
                .unwrap_or_else(|| "unknown subset-gap reason".to_owned());
                return Err(CraneliftBackendError::unsupported_module_shape(format!(
                    "Cranelift strict mode rejected fallback to `{function_name}` stub: {reason}"
                )));
            }
            emit_return_stub(&mut fb);
            compute_body_lowered = false;
        }
        fb.seal_all_blocks();
        fb.finalize();
    }

    let compute_clif_text = ctx.func.display().to_string();

    jit.define_function(func_id, &mut ctx).map_err(|e| {
        // `ModuleError`'s `Display` collapses verifier failures to the bare
        // "Verifier errors" heading, and the FFI error buffer truncates long
        // messages, so the instruction-level detail would otherwise be
        // unreachable. Debug-format the error and offer a full-function dump.
        if let Ok(dump) = std::env::var("FAUST_RS_CLIF_DUMP") {
            let _ = std::fs::write(&dump, format!("{e:?}\n{}", ctx.func.display()));
        }
        CraneliftBackendError::jit_failure(format!(
            "define_function `{function_symbol_name}` failed: {e:?}\nCLIF:\n{}",
            ctx.func.display()
        ))
    })?;
    jit.clear_context(&mut ctx);
    jit.finalize_definitions().map_err(|e| {
        CraneliftBackendError::jit_failure(format!("finalize_definitions failed: {e}"))
    })?;
    let addr = jit.get_finalized_function(func_id) as usize;
    Ok((
        function_symbol_name,
        addr,
        compute_body_lowered,
        compute_clif_text,
    ))
}
