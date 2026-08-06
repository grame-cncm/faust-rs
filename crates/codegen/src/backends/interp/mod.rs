//! Interpreter (`interp`) backend — FBC bytecode types, compiler, execution
//! engine, optimizer, factory, instance, and `.fbc` serialization.
//!
//! # Source provenance (C++)
//! - `compiler/generator/interpreter/` — full C++ interpreter subsystem.
//!
//! # Intended role
//! - Define the Faust Byte Code (FBC) instruction set, instruction/block types,
//!   and the `FbcReal` trait for generic f32/f64 dispatch.
//! - Host the FIR → FBC compiler, bytecode optimizer, execution engine,
//!   DSP factory/instance, and `.fbc` serialization.
//!
//! # Current status
//! **Step 5 complete**: Factory, serialization, DSP interface — the final
//! integration step. Complete pipeline: FIR → compile → optimize → serialize
//! → deserialize → instantiate → compute.
//! Built on Steps 1–4 (types, executor, compiler, optimizer).
//!
//! # Module layout
//! - [`opcode`]: `FbcOpcode` enum, `FBC_INSTRUCTION_NAMES`, `INTERP_FILE_VERSION`.
//! - [`bytecode`]: `FbcInstruction`, `FbcBlock`, `FbcBlockArena`, `BlockId`,
//!   `FbcUiInstruction`, `FbcMetaInstruction`.
//! - [`real`]: `FbcReal` trait with f32/f64 implementations.
//! - [`compiler`]: `FirToFbcCompiler` — FIR → FBC bytecode compiler.
//! - [`optimizer`]: `optimize_block` — peephole bytecode optimizer (6 levels).
//! - [`executor`]: `FbcExecutor` — bytecode execution engine with audio I/O.
//! - [`factory`]: `FbcDspFactory` — compiled bytecode program with optimization.
//! - [`instance`]: `FbcDspInstance` — runtime DSP state with `compute()` loop.
//! - [`serial`]: `write_fbc` / `read_fbc` — `.fbc` text format serialization.
//! - [`fbc_to_cpp`]: `generate_cpp_from_fbc` — AOT FBC → native C++ code generator.

pub mod bytecode;
pub mod compiler;
pub mod executor;
pub mod factory;
pub mod fbc_to_cpp;
pub mod foreign;
pub mod instance;
pub mod opcode;
pub mod optimizer;
pub mod real;
pub mod serial;
pub mod soundfile;

// Re-exports for convenient access.
pub use bytecode::{
    BlockId, BlockStoreData, FbcBlock, FbcBlockArena, FbcInstruction, FbcMetaInstruction,
    FbcUiInstruction,
};
pub use compiler::{CompileError, FbcCompileResult, FirToFbcCompiler, HeapType, MemoryDesc};
pub use executor::{FbcExecError, FbcExecutor, FbcStackKind};
pub use factory::FbcDspFactory;
pub use fbc_to_cpp::{FbcCppError, FbcCppOptions, generate_cpp_from_fbc};
pub use foreign::{
    ForeignScalarType, ForeignSignature, clear_foreign_functions, is_registered_foreign_function,
    lookup_foreign_function, register_foreign_function, unregister_foreign_function,
};
pub use instance::{FbcDspInstance, FbcDspRuntimeError};
pub use opcode::{FBC_INSTRUCTION_NAMES, FBC_OPCODE_COUNT, FbcOpcode, INTERP_FILE_VERSION};
pub use optimizer::{MAX_OPT_LEVEL, optimize_block};
pub use real::FbcReal;
pub use serial::{FAUST_VERSION, FbcSerialError, read_fbc, write_fbc};
pub use soundfile::Soundfile;

pub const BACKEND_NAME: &str = "interp";

/// Returns the stable backend identifier (`"interp"`).
#[must_use]
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

// ─── Options ────────────────────────────────────────────────────────────────

/// Options controlling interpreter bytecode generation.
///
/// Parallel to `COptions` and `CppOptions` in the C/C++ backends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InterpOptions {
    /// Bytecode optimizer level (0 = none, up to [`MAX_OPT_LEVEL`]).
    pub opt_level: i32,
    /// Override module/class name.  When `None`, the name embedded in the FIR
    /// module is used.
    pub module_name: Option<String>,
}

// ─── Error types ────────────────────────────────────────────────────────────

/// Error codes for interpreter code-generation failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// FIR root node is not a `Module`.
    RootNotModule,
    /// A module section could not be decoded as a `Block`.
    InvalidModuleSection,
    /// A FIR node could not be lowered to FBC bytecode.
    CompilationFailed,
}

impl CodegenErrorCode {
    /// Returns the stable machine-readable error code string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-INTERP-0001",
            Self::InvalidModuleSection => "FRS-CGEN-INTERP-0002",
            Self::CompilationFailed => "FRS-CGEN-INTERP-0003",
        }
    }
}

/// An interpreter code-generation error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodegenError {
    /// Machine-readable error code.
    pub code: CodegenErrorCode,
    /// Human-readable message.
    pub message: String,
}

impl CodegenError {
    /// Creates a typed interpreter backend error.
    fn new(code: CodegenErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CodegenError {
    /// Formats the typed error as `[CODE] message`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CodegenError {}

// ─── Module generator ────────────────────────────────────────────────────────

/// Generates an [`FbcDspFactory`] from a FIR module.
///
/// This is the interpreter backend's entry point, parallel to
/// `generate_cpp_module` and `generate_c_module` in the C/C++ backends.
/// It is also the assembly boundary between FIR lifecycle sections and the
/// interpreter runtime layout expected by [`FbcDspFactory`].
///
/// # Function-to-block mapping
///
/// The FIR module's `functions` block is scanned for `DeclareFun` nodes.
/// Known function names are mapped to the factory code blocks.
///
/// | FIR function name              | Factory block          |
/// |--------------------------------|------------------------|
/// | `"staticInit"`                 | `static_init_block`    |
/// | `"instanceConstants"`          | `init_block`           |
/// | `"instanceResetUserInterface"` | `reset_ui_block`       |
/// | `"instanceClear"`              | `clear_block`          |
/// | `"compute"`                    | `compute_dsp_block`    |
///
/// DSP sections absent from the FIR module produce an empty block (only
/// `kReturn`). The Rust FIR fast-lane uses a single explicit-loop `compute`
/// function as the DSP block; `computeThread` is no longer part of the FIR
/// contract for this backend.
///
/// The current adaptation keeps both runtime compute slots for parity with the
/// interpreter runtime:
/// - `compute_block` hosts the control prefix when `compute` can be split;
/// - `compute_dsp_block` hosts the sample loop body or the whole `compute`
///   function when no split is possible.
///
/// # Source provenance (C++)
/// - `InterpreterInstVisitor<REAL>` + `interpreter_dsp_factory_aux` in
///   `interpreter_instructions.hh` / `interpreter_dsp_aux.hh`.
pub fn generate_interp_module<R: real::FbcReal>(
    store: &fir::FirStore,
    module: fir::FirId,
    options: &InterpOptions,
) -> Result<FbcDspFactory<R>, CodegenError> {
    use fir::match_fir;
    use std::collections::HashMap;

    // 0. Flatten generated-table sub-modules.
    //
    // The interpreter has no notion of a nested container: it has one int heap
    // and one real heap per instance, and a flat list of code blocks. So a
    // table generator is inlined the way upstream inlines it for the flat
    // backends, with its state merged into the DSP's own storage
    // (`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md` §5.9).
    //
    // The table itself needs no promotion out of `static_decls`: unlike C++'s
    // file-scope `static`, interpreter static storage is already per instance —
    // `class_init_instance` runs `static_init_block` on the instance's own
    // executor — so leaving it there gives per-instance tables for free.
    let flattened = if fir::subcontainer::has_sub_modules(store, module) {
        let (owned, root) = fir::subcontainer::flatten_sub_modules_owned(
            store,
            module,
            fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
        )
        .map_err(|err| {
            CodegenError::new(
                CodegenErrorCode::CompilationFailed,
                format!("flattening generated tables failed: {err}"),
            )
        })?;
        Some((owned, root))
    } else {
        None
    };
    let (store, module) = flattened
        .as_ref()
        .map_or((store, module), |(s, m)| (s, *m));

    // 1. Decode module root.
    let (
        module_num_inputs,
        module_num_outputs,
        module_name_fir,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    ) = match match_fir(store, module) {
        fir::FirMatch::Module {
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
        } => (
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
        ),
        _ => {
            return Err(CodegenError::new(
                CodegenErrorCode::RootNotModule,
                "FIR root is not a Module",
            ));
        }
    };

    // Step 0 flattened them away; anything left is a flattening bug, not an
    // unsupported feature, and must not reach the factory — a table declared
    // and never filled reads as zeros.
    let sub_module_names = crate::backends::sub_module_names(store, sub_modules);
    if !sub_module_names.is_empty() {
        return Err(CodegenError::new(
            CodegenErrorCode::CompilationFailed,
            format!(
                "sub-modules survived flattening ({}); this is an internal error",
                sub_module_names.join(", ")
            ),
        ));
    }

    let module_name = options.module_name.clone().unwrap_or(module_name_fir);

    // 2. Extract declared functions from the functions block.
    let decl_ids = match match_fir(store, functions) {
        fir::FirMatch::Block(ids) => ids,
        _ => {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                "functions is not a FIR Block",
            ));
        }
    };

    // 3. Compile each function body using a shared FirToFbcCompiler.
    let mut compiler: compiler::FirToFbcCompiler<R> = compiler::FirToFbcCompiler::new();
    compiler
        .predeclare_storage_block(store, dsp_struct)
        .map_err(|e| {
            CodegenError::new(
                CodegenErrorCode::CompilationFailed,
                format!("in module dsp_struct predeclare: {e}"),
            )
        })?;
    compiler
        .predeclare_storage_block(store, globals)
        .map_err(|e| {
            CodegenError::new(
                CodegenErrorCode::CompilationFailed,
                format!("in module globals predeclare: {e}"),
            )
        })?;
    // Pre-declare static tables so function bodies can reference them by name.
    compiler
        .predeclare_storage_block(store, static_decls)
        .map_err(|e| {
            CodegenError::new(
                CodegenErrorCode::CompilationFailed,
                format!("in module static_decls predeclare: {e}"),
            )
        })?;
    let mut fn_blocks: HashMap<String, BlockId> = HashMap::new();
    let mut split_compute_blocks: Option<(BlockId, BlockId)> = None;

    for decl_id in &decl_ids {
        if let fir::FirMatch::DeclareFun {
            name: fn_name,
            body: Some(body),
            ..
        } = match_fir(store, *decl_id)
        {
            if fn_name == "compute"
                && let Some((control_prefix, dsp_loop_stmt)) =
                    detect_compute_control_dsp_split(store, body)
            {
                let control_block = compiler
                    .compile_fir_stmt_list_block(store, &control_prefix)
                    .map_err(|e| {
                        CodegenError::new(
                            CodegenErrorCode::CompilationFailed,
                            format!("in 'compute' control split: {e}"),
                        )
                    })?;
                let dsp_block = compiler
                    .compile_fir_stmt_list_block(store, &[dsp_loop_stmt])
                    .map_err(|e| {
                        CodegenError::new(
                            CodegenErrorCode::CompilationFailed,
                            format!("in 'compute' dsp split: {e}"),
                        )
                    })?;
                split_compute_blocks = Some((control_block, dsp_block));
                continue;
            }
            // Prototype-only DeclareFun nodes (body: None) have no bytecode to compile.
            let block_id = compiler.compile_fir_block(store, body).map_err(|e| {
                CodegenError::new(
                    CodegenErrorCode::CompilationFailed,
                    format!("in '{fn_name}': {e}"),
                )
            })?;
            fn_blocks.insert(fn_name, block_id);
        }
    }

    // 4a. Compile static table initializer block.  Static tables are emitted as
    //     `const static` arrays in the C/C++ backends; in the interpreter they
    //     live on the heap and must be bulk-initialised via BlockStoreInt /
    //     BlockStoreReal instructions before the first `compute()` call.
    let static_tbl_init_block = compiler
        .compile_static_decls_init_block(store, static_decls)
        .map_err(|e| {
            CodegenError::new(
                CodegenErrorCode::CompilationFailed,
                format!("in static_decls init: {e}"),
            )
        })?;

    // 4b. Map function names to factory block slots.
    //    The interpreter runtime still has separate control/DSP slots, but the
    //    FIR contract now provides a single `compute` body used as DSP block.
    //    We keep `compute_block` empty and ignore legacy `computeThread`.
    let static_init_block = fn_blocks
        .get("staticInit")
        .copied()
        .unwrap_or_else(|| compiler.alloc_empty_block());
    let init_block = fn_blocks
        .get("instanceConstants")
        .copied()
        .unwrap_or_else(|| compiler.alloc_empty_block());
    let reset_ui_block = fn_blocks
        .get("instanceResetUserInterface")
        .copied()
        .unwrap_or_else(|| compiler.alloc_empty_block());
    let clear_block = fn_blocks
        .get("instanceClear")
        .copied()
        .unwrap_or_else(|| compiler.alloc_empty_block());
    let (compute_block, compute_dsp_block) = if let Some((control, dsp)) = split_compute_blocks {
        (control, dsp)
    } else {
        (
            compiler.alloc_empty_block(),
            fn_blocks
                .get("compute")
                .copied()
                .unwrap_or_else(|| compiler.alloc_empty_block()),
        )
    };

    // 5. Extract arena, heap layout, UI instructions, and field table.
    let (mut arena, mut int_heap_size, real_heap_size, ui_block, field_table) =
        compiler.into_parts();

    // 5a. Merge static table init into the front of static_init_block.
    //     The static table data must be present in the heap before any user
    //     `staticInit` code and before `compute()`.  We prepend the
    //     BlockStoreInt/BlockStoreReal instructions from `static_tbl_init_block`
    //     (stripping its trailing Return) to the beginning of `static_init_block`.
    {
        use opcode::FbcOpcode;
        let tbl_instrs: Vec<_> = arena
            .get(static_tbl_init_block)
            .instructions
            .iter()
            .filter(|i| i.opcode != FbcOpcode::Return)
            .cloned()
            .collect();
        if !tbl_instrs.is_empty() {
            let si = arena.get_mut(static_init_block);
            let existing: Vec<_> = si.instructions.drain(..).collect();
            si.instructions = tbl_instrs;
            si.instructions.extend(existing);
        }
    }
    // Rebind as immutable now that mutation is done.
    let arena = arena;

    // 6. Resolve well-known heap offsets from the field table.
    let sr_offset_existing = field_table
        .get("fSamplingFreq")
        .or_else(|| field_table.get("fSampleRate"))
        .map(|d| d.offset);
    let count_offset_existing = field_table.get("count").map(|d| d.offset);
    // C++ interpreter runtime writes sample-rate/count unconditionally, so the
    // factory must provide valid int-heap offsets even when the FIR producer
    // (notably the temporary legacy bridge) did not materialize these symbols.
    let sr_offset = reserve_pseudo_int_slot(sr_offset_existing, &mut int_heap_size);
    let count_offset = reserve_pseudo_int_slot(count_offset_existing, &mut int_heap_size);
    let iota_offset = field_table
        .get("IOTA")
        .or_else(|| field_table.get("fIOTA"))
        .map(|d| d.offset)
        .unwrap_or(0);

    // 7. Resolve audio I/O counts.
    let num_inputs = module_num_inputs as i32;
    let num_outputs = module_num_outputs as i32;

    // 8. Build and optionally optimize the factory.
    let mut factory = FbcDspFactory::new(
        module_name,
        "", // sha_key: not computed at this layer
        "", // compile_options: not set at this layer
        INTERP_FILE_VERSION,
        num_inputs,
        num_outputs,
        int_heap_size,
        real_heap_size,
        sr_offset,
        count_offset,
        iota_offset,
        options.opt_level,
        arena,
        Vec::new(), // meta_block: populated by higher-level APIs
        ui_block,
        static_init_block,
        init_block,
        reset_ui_block,
        clear_block,
        compute_block,
        compute_dsp_block,
    );

    if options.opt_level > 0 {
        factory.optimize();
    }

    Ok(factory)
}

/// Returns an existing int-heap offset or reserves a new pseudo-slot at the
/// end of the int heap.
///
/// This keeps the Rust interpreter runtime aligned with the C++ contract where
/// `instanceConstants()` and `compute()` write `sampleRate`/`count`
/// unconditionally through well-known offsets.
fn reserve_pseudo_int_slot(existing: Option<i32>, int_heap_size: &mut i32) -> i32 {
    if let Some(offset) = existing {
        offset
    } else {
        let offset = *int_heap_size;
        *int_heap_size += 1;
        offset
    }
}

/// Detects a split-friendly `compute` body shape:
/// `Block(prefix..., <top-level ForLoop|SimpleForLoop as last stmt>)`.
///
/// Returns `(control_prefix_statements, dsp_loop_stmt)` when the shape matches.
fn detect_compute_control_dsp_split(
    store: &fir::FirStore,
    body: fir::FirId,
) -> Option<(Vec<fir::FirId>, fir::FirId)> {
    let fir::FirMatch::Block(stmts) = fir::match_fir(store, body) else {
        return None;
    };
    let (last, prefix) = stmts.split_last()?;
    if prefix.iter().any(|stmt| {
        matches!(
            fir::match_fir(store, *stmt),
            fir::FirMatch::SimpleForLoop { .. } | fir::FirMatch::ForLoop { .. }
        )
    }) {
        return None;
    }
    match fir::match_fir(store, *last) {
        fir::FirMatch::SimpleForLoop { .. } | fir::FirMatch::ForLoop { .. } => {
            Some((prefix.to_vec(), *last))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
