//! Public Cranelift backend entry points.
//!
//! This module keeps the exported generation and diagnostics functions separate
//! from lowering internals. The surrounding `mod.rs` re-exports these APIs as
//! the stable backend facade.

use super::*;

/// Compiles a FIR module to a Cranelift JIT module.
///
/// This is the main backend entry point used by higher-level crates (`compiler`,
/// `cranelift-ffi`, tests) to turn FIR into an owned Cranelift JIT artifact.
///
/// # What it does
/// - validates FIR module shape and locates `compute`,
/// - builds the current backend `dsp*` layout contract from FIR `globals`,
/// - initializes a Cranelift JIT module and registers required host symbols,
/// - emits and finalizes the `compute` function,
/// - returns an owned [`JitDspModule`] that keeps code memory alive.
///
/// # Lowering policy (current phase)
/// - If `compute` matches the currently supported FIR subset, the backend emits
///   a real lowered body and `JitDspModule::compute_body_lowered()` returns
///   `true`.
/// - Otherwise:
///   - when `options.fail_on_subset_gap == false` (default), the backend emits
///     a valid no-op `compute` stub and returns success with
///     `compute_body_lowered() == false`;
///   - when `options.fail_on_subset_gap == true`, compilation fails with
///     `UnsupportedModuleShape`.
///
/// This "compile-success + stub fallback" policy is intentional during bring-up
/// because it allows end-to-end integration and corpus diagnostics to progress
/// while lowering coverage is expanded.
///
/// # Errors
/// Returns [`CraneliftBackendError`] for:
/// - invalid FIR module/`compute` shapes,
/// - missing `compute`,
/// - Cranelift JIT initialization/verification/finalization failures.
pub fn generate_cranelift_module(
    store: &FirStore,
    module: FirId,
    options: &CraneliftOptions,
) -> Result<JitDspModule, CraneliftBackendError> {
    // Cranelift has no nested container: one JIT module, one flat set of
    // compiled functions. A table generator is therefore inlined with its state
    // merged into the DSP struct, as for the other flat backends
    // (`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`
    // §5.9). The table stays in `static_decls` and becomes a writable JIT data
    // object that `staticInit` fills.
    let flattened = if fir::subcontainer::has_sub_modules(store, module) {
        let (owned, root) = fir::subcontainer::flatten_sub_modules_owned(
            store,
            module,
            fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
        )
        .map_err(|err| {
            CraneliftBackendError::unsupported_module_shape(format!(
                "flattening generated tables failed: {err}"
            ))
        })?;
        Some((owned, root))
    } else {
        None
    };
    let (store, module) = flattened.as_ref().map_or((store, module), |(s, m)| (s, *m));

    // Flattening removed them; a survivor is an internal error, and must not
    // reach the JIT — a table declared and never filled reads as zeros.
    if let FirMatch::Module { sub_modules, .. } = match_fir(store, module) {
        let sub_module_names = crate::backends::sub_module_names(store, sub_modules);
        if !sub_module_names.is_empty() {
            return Err(CraneliftBackendError::unsupported_module_shape(format!(
                "sub-modules survived flattening ({}); this is an internal error",
                sub_module_names.join(", ")
            )));
        }
    }

    let (module_name, compute_decl) = find_module_and_compute(store, module)?;

    // Attempt full JIT compilation.  On some targets (notably AArch64) Cranelift
    // may panic when the generated function body is so large that conditional
    // branch offsets exceed the ±1 MiB `B.cond` displacement limit and the
    // island-emission logic fails to insert veneers in time.  We catch that
    // panic here and retry with `force_stub=true` so hosts get the controlled
    // no-op fallback instead of a process abort.
    let first_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        try_generate_cranelift_module(store, module, options, &module_name, compute_decl, false)
    }));

    match first_attempt {
        Ok(result) => result,
        Err(_panic) => {
            // Cranelift JIT panicked (most likely AArch64 branch-range exceeded).
            // The partial JIT module created in `try_generate_cranelift_module`
            // was dropped during stack unwinding; create a fresh one with a stub.
            eprintln!(
                "warning: Cranelift JIT panicked while compiling `{module_name}::compute` \
                 (branch-offset overflow?); falling back to no-op stub"
            );
            try_generate_cranelift_module(store, module, options, &module_name, compute_decl, true)
        }
    }
}

/// Inner body of [`generate_cranelift_module`] with an optional `force_stub` flag.
///
/// Separated so that `generate_cranelift_module` can wrap the first attempt in
/// `catch_unwind` and retry with `force_stub = true` when Cranelift panics.
pub(crate) fn try_generate_cranelift_module(
    store: &FirStore,
    module: FirId,
    options: &CraneliftOptions,
    module_name: &str,
    compute_decl: FirId,
    force_stub: bool,
) -> Result<JitDspModule, CraneliftBackendError> {
    let mut jit_builder = make_jit_builder(options)?;
    register_host_symbols(&mut jit_builder);
    for (name, addr) in &options.extern_data_symbols {
        jit_builder.symbol(name, (*addr).cast::<u8>());
    }
    for (name, addr) in &options.extern_function_symbols {
        jit_builder.symbol(name, (*addr).cast::<u8>());
    }
    let mut jit = JITModule::new(jit_builder);
    let ptr_size = jit.target_config().pointer_type().bytes();
    let struct_layout =
        build_struct_layout_for_module(store, module, ptr_size, options.double_precision)?;
    // Define file-scope static tables as JIT read-only data objects before
    // compiling `compute`, so function bodies can reference them by DataId.
    let (static_data_ids, static_table_elem_types) =
        define_static_tables_in_jit(store, module, &mut jit, options.double_precision)?;
    let extern_data_ids =
        declare_extern_data_symbols_in_jit(&mut jit, &options.extern_data_symbols)?;
    let (compute_symbol_name, compute_entry_addr, compute_body_lowered, compute_clif_text) =
        declare_jit_function(
            &format!("{module_name}::compute"),
            compute_decl,
            store,
            &struct_layout,
            options.fail_on_subset_gap,
            &options.extern_data_symbols,
            &options.extern_function_symbols,
            force_stub,
            &mut jit,
            &static_data_ids,
            &static_table_elem_types,
            &extern_data_ids,
            options.double_precision,
        )?;
    let mut generated_functions_clif = vec![(compute_symbol_name.clone(), compute_clif_text)];
    let instance_constants_entry_addr =
        match find_module_and_function(store, module, "instanceConstants") {
            Ok((_module_name, instance_constants_decl)) => {
                let (_symbol_name, entry_addr, _lowered, instance_constants_clif) =
                    declare_jit_function(
                        &format!("{module_name}::instanceConstants"),
                        instance_constants_decl,
                        store,
                        &struct_layout,
                        true,
                        &options.extern_data_symbols,
                        &options.extern_function_symbols,
                        false,
                        &mut jit,
                        &static_data_ids,
                        &static_table_elem_types,
                        &extern_data_ids,
                        options.double_precision,
                    )?;
                generated_functions_clif.push((
                    format!("{module_name}::instanceConstants"),
                    instance_constants_clif,
                ));
                entry_addr
            }
            Err(_) => 0,
        };
    // `staticInit` fills the generated tables. Without it a runtime-filled
    // table is a zeroed data object nothing ever writes, and every read of it
    // returns 0 — the silent wrong answer this port exists to prevent.
    let static_init_entry_addr = match find_module_and_function(store, module, "staticInit") {
        Ok((_module_name, static_init_decl)) => {
            let (_symbol_name, entry_addr, _lowered, static_init_clif) = declare_jit_function(
                &format!("{module_name}::staticInit"),
                static_init_decl,
                store,
                &struct_layout,
                true,
                &options.extern_data_symbols,
                &options.extern_function_symbols,
                false,
                &mut jit,
                &static_data_ids,
                &static_table_elem_types,
                &extern_data_ids,
                options.double_precision,
            )?;
            generated_functions_clif.push((format!("{module_name}::staticInit"), static_init_clif));
            entry_addr
        }
        Err(_) => 0,
    };
    // `instanceClear` resets state; some DSPs (e.g. `prefix`) fill buffers with
    // non-zero init values here, so it must be JIT-compiled and run at init.
    let instance_clear_entry_addr = match find_module_and_function(store, module, "instanceClear") {
        Ok((_module_name, instance_clear_decl)) => {
            let (_symbol_name, entry_addr, _lowered, instance_clear_clif) = declare_jit_function(
                &format!("{module_name}::instanceClear"),
                instance_clear_decl,
                store,
                &struct_layout,
                true,
                &options.extern_data_symbols,
                &options.extern_function_symbols,
                false,
                &mut jit,
                &static_data_ids,
                &static_table_elem_types,
                &extern_data_ids,
                options.double_precision,
            )?;
            generated_functions_clif
                .push((format!("{module_name}::instanceClear"), instance_clear_clif));
            entry_addr
        }
        Err(_) => 0,
    };
    if compute_entry_addr == 0 {
        return Err(CraneliftBackendError::jit_failure(
            "finalized compute symbol address is null",
        ));
    }

    Ok(JitDspModule {
        static_init_entry_addr,
        module_name: module_name.to_owned(),
        compute_symbol_name,
        compute_entry_addr,
        instance_constants_entry_addr,
        instance_clear_entry_addr,
        compute_body_lowered,
        generated_functions_clif,
        struct_layout,
        jit_module: jit,
    })
}

/// Diagnoses why the current Cranelift `compute` subset matcher would fall back
/// to the no-op stub for a given FIR module.
///
/// Returns `Ok(None)` when the `compute` body matches the current lowering
/// subset, and `Ok(Some(reason))` otherwise.
///
/// # Intended use
/// This helper is for diagnostics/tooling (tests, temporary corpus scanners,
/// future `xtask` checks), not for production runtime decisions.
///
/// The returned reason is intentionally human-readable and may include FIR
/// debug formatting (for example unsupported node variants). It is useful for
/// prioritizing backend work, but should not be treated as a stable machine
/// interface.
pub fn diagnose_cranelift_compute_subset_gap(
    store: &FirStore,
    module: FirId,
) -> Result<Option<String>, CraneliftBackendError> {
    let (_module_name, compute_decl) = find_module_and_compute(store, module)?;
    Ok(compute_body_subset_gap_reason_from_compute_decl(
        store,
        compute_decl,
        &HashMap::new(),
        &HashMap::new(),
    ))
}
