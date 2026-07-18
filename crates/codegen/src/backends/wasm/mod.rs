//! WebAssembly backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! - `compiler/generator/wasm/wasm_code_container.cpp`
//! - `compiler/generator/wasm/wasm_binary.hh`
//! - `compiler/generator/wasm/wasm_instructions.hh`
//! - `compiler/generator/code_container.hh` (`CodeContainer::generateJSON`)
//!
//! # Current role
//! This backend is the Rust port of Faust's production WASM path:
//! - lower one FIR `Module` into one valid `.wasm` binary,
//! - emit the companion JSON metadata consumed by WebAudio/Web runtimes,
//! - preserve the canonical Faust DSP export contract (`init`, `compute`,
//!   `getParamValue`, `setParamValue`, `instance*`, `getNumInputs/Outputs`).
//!
//! # Runtime contract
//! The emitted `.wasm` and companion JSON must be treated as one matched pair.
//! Higher-level runtimes such as `faustwasm` rely on both artifacts:
//! - JSON `ui[*].index` values are raw byte offsets inside the WASM runtime
//!   prefix described by [`layout::WasmMemoryLayout`],
//! - `getParamValue(dsp, index)` and `setParamValue(dsp, index, value)` consume
//!   those exact offsets,
//! - JSON `size` marks the start of the audio heap / I/O zone, not merely the
//!   size of mutable DSP fields,
//! - JSON `compile_options` only advertises options that are actually supported
//!   by the Rust CLI/backend path.
//!
//! Mixing a Rust-generated `.wasm` with a C++-generated JSON companion, or the
//! reverse, is not a supported configuration unless the two artifacts are known
//! to be ABI-identical.
//!
//! # Current slice
//! The backend now covers real production-facing pieces:
//! - deterministic runtime memory layout,
//! - embedded companion JSON data segment,
//! - UI metadata extraction with runtime offsets,
//! - lifecycle methods (`init`, `instanceInit`, `instanceConstants`,
//!   `instanceResetUserInterface`, `instanceClear`),
//! - a partial but growing `compute` lowering subset.
//!
//! Unsupported FIR shapes still fail fast with typed backend diagnostics rather
//! than silently emitting a lossy module.

use fir::{AccessType, FirId, FirMatch, FirMathOp, FirStore, FirType, match_fir};
use std::collections::HashMap;

use crate::backends::purity::is_obviously_side_effect_free_value;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, ImportSection, Instruction, MemArg, MemorySection, MemoryType,
    Module, TypeSection, ValType,
};

use crate::json::{JsonBuildError, JsonBuildOptions, JsonDescription, JsonMetaEntry};

pub mod layout;

pub use layout::{FieldLayout, WasmMemoryLayout, WasmValType};

#[cfg(test)]
mod tests;

/// Stable backend identifier used by the compiler facade and CLI.
pub const BACKEND_NAME: &str = "wasm";

/// Fallback minimum page count when auto-sizing would otherwise pick zero.
const DEFAULT_MEMORY_PAGES: u32 = 1;
const SOUNDFILE_BUFFERS_OFFSET: u32 = 0;
const SOUNDFILE_LENGTH_OFFSET: u32 = 4;
const SOUNDFILE_RATE_OFFSET: u32 = 8;
const SOUNDFILE_FRAME_OFFSET_OFFSET: u32 = 12;

/// WASM backend compilation options.
///
/// Mapping status: `adapted`.
/// These options describe the Rust emitter/runtime surface, not the full set of
/// historical C++ CLI flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmOptions {
    /// Emit `f64` (double) instead of `f32` for parameter/value APIs.
    pub double_precision: bool,
    /// Request companion WAT text. Deferred in the current scaffold.
    pub emit_wat: bool,
    /// Internal memory size in WASM pages (64 KiB each). `0` means auto-size.
    pub memory_pages: u32,
    /// Enable internal memory (`true`) or import memory from the host (`false`).
    pub internal_memory: bool,
}

impl Default for WasmOptions {
    fn default() -> Self {
        Self {
            double_precision: false,
            emit_wat: false,
            memory_pages: 0,
            internal_memory: true,
        }
    }
}

/// Compile-context metadata carried into WASM companion JSON emission.
///
/// Source provenance (C++):
/// - `CodeContainer::generateJSON(...)` in `compiler/generator/code_container.hh`
/// - `global::printCompilationOptions1()`
/// - `SourceReader::listLibraryFiles()`
///
/// This struct intentionally keeps only metadata that is externally observable
/// in the companion JSON. It is not a general backend configuration object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WasmJsonContext {
    pub filename: Option<String>,
    pub version: Option<String>,
    pub compile_options: Option<String>,
    pub library_list: Vec<String>,
    pub include_pathnames: Vec<String>,
    pub top_level_meta: Vec<JsonMetaEntry>,
}

/// Compiled WASM backend output.
///
/// The `wasm_binary`, `dsp_json`, and [`WasmModule::memory_layout`] fields
/// describe the same module instance and must remain coherent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmModule {
    /// WASM binary (valid `.wasm` file).
    pub wasm_binary: Vec<u8>,
    /// Optional WAT text companion. Deferred in the current scaffold.
    pub wat_text: Option<String>,
    /// JSON metadata string consumed by higher-level runtimes.
    pub dsp_json: String,
    /// Current linear-memory layout descriptor.
    pub memory_layout: WasmMemoryLayout,
}

/// Stable machine-readable error codes for the WASM backend emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasmBackendErrorCode {
    UnsupportedModuleShape,
    MissingCompute,
    UnsupportedFirNode,
    EncodingFailure,
    MemoryLayoutOverflow,
}

impl WasmBackendErrorCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedModuleShape => "FRS-CGEN-WASM-0001",
            Self::MissingCompute => "FRS-CGEN-WASM-0002",
            Self::UnsupportedFirNode => "FRS-CGEN-WASM-0003",
            Self::EncodingFailure => "FRS-CGEN-WASM-0004",
            Self::MemoryLayoutOverflow => "FRS-CGEN-WASM-0005",
        }
    }
}

/// Typed backend error returned by the WASM emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmBackendError {
    code: WasmBackendErrorCode,
    message: String,
}

impl WasmBackendError {
    #[must_use]
    pub fn new(code: WasmBackendErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn code(&self) -> WasmBackendErrorCode {
        self.code
    }
}

impl std::fmt::Display for WasmBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for WasmBackendError {}

/// Canonical exported/runtime helper functions emitted by the backend.
///
/// The order in [`WasmFunc::ALL`] is ABI-relevant because type indices,
/// function indices, and export bindings are derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WasmFunc {
    ClassInit,
    Compute,
    GetNumInputs,
    GetNumOutputs,
    GetParamValue,
    GetSampleRate,
    Init,
    InstanceClear,
    InstanceConstants,
    InstanceInit,
    InstanceResetUserInterface,
    MaxI,
    MinI,
    SetParamValue,
}

impl WasmFunc {
    /// Canonical function order used for type/function section emission.
    const ALL: [Self; 14] = [
        Self::ClassInit,
        Self::Compute,
        Self::GetNumInputs,
        Self::GetNumOutputs,
        Self::GetParamValue,
        Self::GetSampleRate,
        Self::Init,
        Self::InstanceClear,
        Self::InstanceConstants,
        Self::InstanceInit,
        Self::InstanceResetUserInterface,
        Self::MaxI,
        Self::MinI,
        Self::SetParamValue,
    ];

    #[must_use]
    /// Returns the WASM signature for one canonical Faust runtime function.
    fn signature(self, real_ty: ValType) -> (Vec<ValType>, Vec<ValType>) {
        match self {
            Self::ClassInit => (vec![ValType::I32, ValType::I32], vec![]),
            Self::Compute => (
                vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
                vec![],
            ),
            Self::GetNumInputs => (vec![ValType::I32], vec![ValType::I32]),
            Self::GetNumOutputs => (vec![ValType::I32], vec![ValType::I32]),
            Self::GetParamValue => (vec![ValType::I32, ValType::I32], vec![real_ty]),
            Self::GetSampleRate => (vec![ValType::I32], vec![ValType::I32]),
            Self::Init => (vec![ValType::I32, ValType::I32], vec![]),
            Self::InstanceClear => (vec![ValType::I32], vec![]),
            Self::InstanceConstants => (vec![ValType::I32, ValType::I32], vec![]),
            Self::InstanceInit => (vec![ValType::I32, ValType::I32], vec![]),
            Self::InstanceResetUserInterface => (vec![ValType::I32], vec![]),
            Self::MaxI => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
            Self::MinI => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
            Self::SetParamValue => (vec![ValType::I32, ValType::I32, real_ty], vec![]),
        }
    }
}

/// Descriptor for one imported host function referenced by lowered WASM
/// bodies.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WasmImport {
    field_name: String,
    params: Vec<ValType>,
    results: Vec<ValType>,
}

/// Local alias used to emphasize when JSON objects are already in rendered
/// companion-description form.
type WasmJsonDescription = JsonDescription;

/// Emits one valid WASM module for a FIR `Module` root.
///
/// The returned [`WasmModule`] contains both the binary module and its matched
/// companion JSON. Callers that persist the `.wasm` should also persist the
/// `.json` emitted from the same invocation.
pub fn generate_wasm_module(
    store: &FirStore,
    module: FirId,
    options: &WasmOptions,
) -> Result<WasmModule, WasmBackendError> {
    generate_wasm_module_with_context(store, module, options, &WasmJsonContext::default())
}

/// Emits one valid WASM module for a FIR `Module` root with explicit JSON
/// context.
///
/// Use this entry point when the caller already knows source-level provenance
/// that must appear in the companion JSON (`filename`, `compile_options`,
/// library/include lists, top-level metadata).
pub fn generate_wasm_module_with_context(
    store: &FirStore,
    module: FirId,
    options: &WasmOptions,
    json_context: &WasmJsonContext,
) -> Result<WasmModule, WasmBackendError> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        ref name,
        globals,
        functions,
        static_decls,
        ..
    } = match_fir(store, module)
    else {
        return Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedModuleShape,
            "WASM backend expects a FIR Module root",
        ));
    };

    let FirMatch::Block(function_items) = match_fir(store, functions) else {
        return Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            "WASM backend expects the functions section to be a FIR Block",
        ));
    };
    let has_compute = function_items.iter().copied().any(|id| {
        matches!(
            match_fir(store, id),
            FirMatch::DeclareFun { ref name, .. } if name == "compute"
        )
    });
    if !has_compute {
        return Err(WasmBackendError::new(
            WasmBackendErrorCode::MissingCompute,
            "WASM backend requires a compute function in the FIR module",
        ));
    }

    let compute_body = find_function_body(store, &function_items, "compute");
    let instance_constants_body = find_function_body(store, &function_items, "instanceConstants");
    let instance_clear_body = find_function_body(store, &function_items, "instanceClear");
    let instance_reset_ui_body =
        find_function_body(store, &function_items, "instanceResetUserInterface");
    let foreign_fun_imports = collect_foreign_fun_imports(store, globals, options)?;

    let has_soundfiles = module_has_soundfiles(store, module, &function_items);
    let effective_internal_memory = options.internal_memory && !has_soundfiles;

    let real_ty = if options.double_precision {
        ValType::F64
    } else {
        ValType::F32
    };

    let mut memory_layout = WasmMemoryLayout::from_module(store, module, options, 0)?;
    let dsp_json = build_wasm_json_description(
        store,
        name,
        &function_items,
        &memory_layout,
        num_inputs,
        num_outputs,
        json_context,
    )?
    .render();
    memory_layout = WasmMemoryLayout::from_module(store, module, options, dsp_json.len())?;
    let pages = if options.memory_pages == 0 {
        memory_layout.pages.max(DEFAULT_MEMORY_PAGES)
    } else {
        options.memory_pages
    };
    memory_layout.pages = pages;

    let mut wasm = Module::new();
    let imports = collect_wasm_imports(
        store,
        &[
            compute_body,
            instance_constants_body,
            instance_clear_body,
            instance_reset_ui_body,
        ],
        &foreign_fun_imports,
        options,
    )?;
    if let Some(body) = compute_body {
        let _ = lower_compute_subset(store, body, &memory_layout, &imports, options)?;
    }
    let imported_function_count = imports.len() as u32;

    let mut types = TypeSection::new();
    for func in WasmFunc::ALL {
        let (params, results) = func.signature(real_ty);
        types.ty().function(params, results);
    }
    for import in &imports {
        types
            .ty()
            .function(import.params.clone(), import.results.clone());
    }
    wasm.section(&types);

    if !effective_internal_memory || !imports.is_empty() {
        let mut import_section = ImportSection::new();
        if !effective_internal_memory {
            import_section.import(
                "env",
                "memory",
                EntityType::Memory(MemoryType {
                    minimum: u64::from(pages),
                    maximum: None,
                    memory64: false,
                    shared: false,
                    page_size_log2: None,
                }),
            );
        }
        for (index, import) in imports.iter().enumerate() {
            import_section.import(
                "env",
                &import.field_name,
                EntityType::Function(WasmFunc::ALL.len() as u32 + index as u32),
            );
        }
        wasm.section(&import_section);
    }

    let mut functions = FunctionSection::new();
    for type_index in 0..WasmFunc::ALL.len() {
        functions.function(type_index as u32);
    }
    wasm.section(&functions);

    if effective_internal_memory {
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: u64::from(pages),
            maximum: Some(u64::from(pages.saturating_add(1000))),
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        wasm.section(&memories);
    }

    let mut exports = ExportSection::new();
    exports.export(
        "compute",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::Compute),
    );
    exports.export(
        "getNumInputs",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::GetNumInputs),
    );
    exports.export(
        "getNumOutputs",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::GetNumOutputs),
    );
    exports.export(
        "getParamValue",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::GetParamValue),
    );
    exports.export(
        "getSampleRate",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::GetSampleRate),
    );
    exports.export(
        "init",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::Init),
    );
    exports.export(
        "instanceClear",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::InstanceClear),
    );
    exports.export(
        "instanceConstants",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::InstanceConstants),
    );
    exports.export(
        "instanceInit",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::InstanceInit),
    );
    exports.export(
        "instanceResetUserInterface",
        ExportKind::Func,
        function_index(
            imported_function_count,
            WasmFunc::InstanceResetUserInterface,
        ),
    );
    exports.export(
        "setParamValue",
        ExportKind::Func,
        function_index(imported_function_count, WasmFunc::SetParamValue),
    );
    if effective_internal_memory {
        exports.export("memory", ExportKind::Memory, 0);
    }
    wasm.section(&exports);

    let mut code = CodeSection::new();
    for func in WasmFunc::ALL {
        code.function(&scaffold_function_body(
            func,
            num_inputs as i32,
            num_outputs as i32,
            real_ty,
            memory_layout.field_offsets.get("fSampleRate"),
            &memory_layout,
            imported_function_count,
            &imports,
            store,
            compute_body,
            instance_constants_body,
            instance_clear_body,
            instance_reset_ui_body,
            options,
        ));
    }
    wasm.section(&code);

    let mut data = DataSection::new();
    data.active(
        0,
        &ConstExpr::i32_const(memory_layout.json_offset as i32),
        dsp_json.as_bytes().iter().copied(),
    );
    for (offset, bytes) in static_table_segments(store, static_decls, &memory_layout, options)? {
        data.active(0, &ConstExpr::i32_const(offset as i32), bytes);
    }
    wasm.section(&data);

    Ok(WasmModule {
        wasm_binary: wasm.finish(),
        wat_text: None,
        dsp_json,
        memory_layout,
    })
}

/// Detects whether one FIR module contains soundfile state/UI and therefore
/// must follow the external-memory WASM contract used by `faustwasm`.
fn module_has_soundfiles(store: &FirStore, module: FirId, _function_items: &[FirId]) -> bool {
    if let FirMatch::Module {
        dsp_struct,
        globals,
        static_decls,
        ..
    } = match_fir(store, module)
    {
        for block in [dsp_struct, globals, static_decls] {
            if let FirMatch::Block(items) = match_fir(store, block)
                && items.iter().copied().any(|id| {
                    matches!(
                        match_fir(store, id),
                        FirMatch::DeclareVar {
                            typ: FirType::Sound,
                            ..
                        }
                    )
                })
            {
                return true;
            }
        }
    }

    false
}

/// Maps one canonical backend function to its final module function index.
///
/// Imported math helpers occupy the leading function-index space, so emitted
/// body functions start after `imported_function_count`.
fn function_index(imported_function_count: u32, func: WasmFunc) -> u32 {
    imported_function_count
        + WasmFunc::ALL
            .iter()
            .position(|item| *item == func)
            .expect("function present in static WASM function list") as u32
}

/// Builds the companion JSON object for one lowered WASM module.
///
/// This is the point where backend-specific layout facts become externally
/// visible JSON fields such as `size`, `sr_index`, and widget `index` offsets.
fn build_wasm_json_description(
    store: &FirStore,
    module_name: &str,
    function_items: &[FirId],
    memory_layout: &WasmMemoryLayout,
    num_inputs: usize,
    num_outputs: usize,
    json_context: &WasmJsonContext,
) -> Result<WasmJsonDescription, WasmBackendError> {
    crate::json::build_json_description_from_fir(
        store,
        function_items,
        JsonBuildOptions {
            name: module_name.to_owned(),
            filename: json_context.filename.clone(),
            version: json_context.version.clone(),
            compile_options: json_context.compile_options.clone(),
            library_list: json_context.library_list.clone(),
            include_pathnames: json_context.include_pathnames.clone(),
            top_level_meta: json_context.top_level_meta.clone(),
            // C++ parity: the WASM companion JSON `size` is the start of the
            // host audio heap, not just the raw DSP-struct byte size.
            // Standard Faust wrappers place their input/output pointer tables
            // and sample buffers immediately after this offset.
            size: Some(memory_layout.io_zone_offset),
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: memory_layout
                .field_offsets
                .get("fSampleRate")
                .map(|field| field.offset),
        },
        |var| {
            memory_layout
                .field_offsets
                .get(var)
                .map(|field| field.offset)
        },
    )
    .map_err(map_json_build_error)
}

/// Recasts generic JSON-builder failures into backend-typed WASM diagnostics.
fn map_json_build_error(error: JsonBuildError) -> WasmBackendError {
    match error {
        JsonBuildError::UnsupportedFirNode(message) => {
            WasmBackendError::new(WasmBackendErrorCode::UnsupportedFirNode, message)
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Emits one canonical exported/runtime function body.
///
/// The backend keeps a valid scaffold implementation for every canonical Faust
/// export and opportunistically replaces those stubs with subset-lowered FIR
/// bodies when the relevant function is supported.
fn scaffold_function_body(
    func: WasmFunc,
    num_inputs: i32,
    num_outputs: i32,
    real_ty: ValType,
    sample_rate_field: Option<&FieldLayout>,
    memory_layout: &WasmMemoryLayout,
    imported_function_count: u32,
    imports: &[WasmImport],
    store: &FirStore,
    compute_body: Option<FirId>,
    instance_constants_body: Option<FirId>,
    instance_clear_body: Option<FirId>,
    instance_reset_ui_body: Option<FirId>,
    options: &WasmOptions,
) -> Function {
    let mut function = Function::new(Vec::new());
    match func {
        WasmFunc::ClassInit => {}
        WasmFunc::Compute => {
            if let Some(body) = compute_body
                && let Ok(lowered) =
                    lower_compute_subset(store, body, memory_layout, imports, options)
            {
                return lowered;
            }
        }
        WasmFunc::InstanceClear => {
            if let Some(body) = instance_clear_body
                && let Ok(lowered) =
                    lower_instance_clear_subset(store, body, memory_layout, imports, options)
            {
                return lowered;
            }
        }
        WasmFunc::InstanceConstants => {
            if let Some(body) = instance_constants_body
                && let Ok(lowered) =
                    lower_instance_constants_subset(store, body, memory_layout, imports, options)
            {
                return lowered;
            }
            if let Some(field) = sample_rate_field {
                function.instruction(&Instruction::LocalGet(0));
                function.instruction(&Instruction::LocalGet(1));
                function.instruction(&Instruction::I32Store(memarg(field.offset)));
            }
        }
        WasmFunc::InstanceResetUserInterface => {
            if let Some(body) = instance_reset_ui_body
                && let Ok(lowered) =
                    lower_instance_reset_ui_subset(store, body, memory_layout, imports, options)
            {
                return lowered;
            }
        }
        WasmFunc::GetNumInputs => {
            function.instruction(&Instruction::I32Const(num_inputs));
        }
        WasmFunc::GetNumOutputs => {
            function.instruction(&Instruction::I32Const(num_outputs));
        }
        WasmFunc::GetParamValue => {
            // C++ parity: the WASM ABI still treats `index` as a byte offset
            // inside the DSP struct, not as a UI ordinal that must be decoded.
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::I32Add);
            function.instruction(&load_instruction_for_valtype(real_ty).expect("real type load"));
        }
        WasmFunc::GetSampleRate => {
            if let Some(field) = sample_rate_field {
                function.instruction(&Instruction::LocalGet(0));
                function.instruction(&Instruction::I32Load(memarg(field.offset)));
            } else {
                function.instruction(&Instruction::I32Const(0));
            }
        }
        WasmFunc::Init => {
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::Call(function_index_for_body(
                WasmFunc::ClassInit,
                imported_function_count,
            )));
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::Call(function_index_for_body(
                WasmFunc::InstanceInit,
                imported_function_count,
            )));
        }
        WasmFunc::InstanceInit => {
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::Call(function_index_for_body(
                WasmFunc::InstanceConstants,
                imported_function_count,
            )));
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::Call(function_index_for_body(
                WasmFunc::InstanceResetUserInterface,
                imported_function_count,
            )));
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::Call(function_index_for_body(
                WasmFunc::InstanceClear,
                imported_function_count,
            )));
        }
        WasmFunc::MaxI => {
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::I32LtS);
            function.instruction(&Instruction::Select);
        }
        WasmFunc::MinI => {
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::I32LtS);
            function.instruction(&Instruction::Select);
        }
        WasmFunc::SetParamValue => {
            // C++ parity: `index` is the byte offset of the control zone field.
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::I32Add);
            function.instruction(&Instruction::LocalGet(2));
            function.instruction(&store_instruction_for_valtype(real_ty).expect("real type store"));
        }
    }
    function.instruction(&Instruction::End);
    function
}

/// Builds a default memory operand for linear-memory accesses at `offset`.
fn memarg(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

/// Alias of [`function_index`] used from body-lowering helpers.
fn function_index_for_body(func: WasmFunc, imported_function_count: u32) -> u32 {
    function_index(imported_function_count, func)
}

/// One lowered local variable bound to a concrete WASM local index.
#[derive(Clone, Debug)]
struct WasmLocal {
    storage: WasmLocalStorage,
    typ: FirType,
}

#[derive(Clone, Debug)]
enum WasmLocalStorage {
    Local(u32),
    Memory {
        offset: u32,
        elem_type: FirType,
        storage_type: ValType,
    },
}

/// Partial `compute` subset lowerer for the current WASM bring-up phase.
///
/// # Source provenance (C++)
/// - `compiler/generator/wasm/wasm_code_container.cpp`
/// - `compiler/generator/wasm/wasm_instructions.hh`
///
/// # Supported subset
/// - `Block`
/// - local `DeclareVar(kStack)`
/// - `SimpleForLoop` (forward only)
/// - `ForLoop`
/// - `WhileLoop`
/// - statement-level `If` / `Control` / `Switch` / `Drop`
/// - `Label` markers as structural no-ops
/// - `Bool` / `Int32` / `Float32` / `Float64`
/// - `LoadVar(kFunArgs=count | kLoop | kStack)`
/// - `LoadTable(kFunArgs=inputs/outputs | kStack aliases)`
/// - `StoreTable(kStack aliases)`
/// - `StoreVar(kLoop | kStack)`
/// - `LoadTable/StoreTable(kStruct)`
/// - `Select2`
/// - internal integer helpers `max_i` / `min_i`
/// - math `FunCall` subset:
///   - integer `abs` lowered inline
///   - native WASM `fabs/fmin/fmax/sqrt/floor/ceil/rint`
///
/// This is intentionally narrow so the backend can start executing the
/// canonical mono passthrough fixture while unsupported FIR still falls back to
/// the valid no-op body.
fn lower_compute_subset(
    store: &FirStore,
    body: FirId,
    memory_layout: &WasmMemoryLayout,
    imports: &[WasmImport],
    options: &WasmOptions,
) -> Result<Function, WasmBackendError> {
    lower_function_subset(store, body, memory_layout, imports, options, 4)
}

/// Partial `instanceClear` subset lowerer for the current WASM bring-up phase.
///
/// Reuses the same statement/value subset as `compute`, but with the
/// `instanceClear(dsp)` ABI so stack locals start at local index 1.
fn lower_instance_clear_subset(
    store: &FirStore,
    body: FirId,
    memory_layout: &WasmMemoryLayout,
    imports: &[WasmImport],
    options: &WasmOptions,
) -> Result<Function, WasmBackendError> {
    lower_function_subset(store, body, memory_layout, imports, options, 1)
}

/// Partial `instanceConstants` subset lowerer for the current WASM bring-up phase.
///
/// Reuses the same statement/value subset as `compute`, but with the
/// `instanceConstants(dsp, sample_rate)` ABI so stack locals start at local
/// index 2.
fn lower_instance_constants_subset(
    store: &FirStore,
    body: FirId,
    memory_layout: &WasmMemoryLayout,
    imports: &[WasmImport],
    options: &WasmOptions,
) -> Result<Function, WasmBackendError> {
    lower_function_subset(store, body, memory_layout, imports, options, 2)
}

/// Partial `instanceResetUserInterface` subset lowerer for the current WASM bring-up phase.
///
/// Reuses the same statement/value subset as `instanceClear`, with the same
/// single-parameter ABI: `instanceResetUserInterface(dsp)`.
fn lower_instance_reset_ui_subset(
    store: &FirStore,
    body: FirId,
    memory_layout: &WasmMemoryLayout,
    imports: &[WasmImport],
    options: &WasmOptions,
) -> Result<Function, WasmBackendError> {
    lower_function_subset(store, body, memory_layout, imports, options, 1)
}

/// Shared lowering entry point for the currently supported non-UI FIR bodies.
///
/// `param_count` is the number of ABI parameters already occupying the leading
/// local slots for the target exported function.
fn lower_function_subset(
    store: &FirStore,
    body: FirId,
    memory_layout: &WasmMemoryLayout,
    imports: &[WasmImport],
    options: &WasmOptions,
    param_count: u32,
) -> Result<Function, WasmBackendError> {
    let mut local_specs = Vec::new();
    collect_compute_locals(store, body, &mut local_specs)?;

    let mut local_map = HashMap::with_capacity(local_specs.len());
    let mut wasm_locals = Vec::with_capacity(local_specs.len());
    let mut next_local = param_count;
    for (name, typ) in local_specs {
        let storage = if let FirType::Array(inner, _) = &typ {
            let field = memory_layout.local_offsets.get(&name).ok_or_else(|| {
                WasmBackendError::new(
                    WasmBackendErrorCode::UnsupportedFirNode,
                    format!("WASM local array buffer `{name}` is missing from memory layout"),
                )
            })?;
            WasmLocalStorage::Memory {
                offset: field.offset,
                elem_type: (**inner).clone(),
                storage_type: wasm_val_type_for_field(field),
            }
        } else {
            let index = next_local;
            next_local += 1;
            wasm_locals.push((1, wasm_val_type_for_fir(&typ, options)?));
            WasmLocalStorage::Local(index)
        };
        local_map.insert(
            name,
            WasmLocal {
                storage,
                typ: typ.clone(),
            },
        );
    }

    let mut function = Function::new(wasm_locals);
    let mut lowerer = ComputeSubsetLowerer {
        store,
        memory_layout,
        options,
        locals: local_map,
        imports: imports
            .iter()
            .enumerate()
            .map(|(index, import)| (import.field_name.clone(), index as u32))
            .collect(),
    };
    lowerer.lower_block_into(body, &mut function)?;
    function.instruction(&Instruction::End);
    Ok(function)
}

/// Collects stack/loop locals that need explicit WASM local slots.
///
/// The traversal is intentionally conservative and only accepts FIR nodes that
/// the current subset lowerer understands.
fn collect_compute_locals(
    store: &FirStore,
    id: FirId,
    out: &mut Vec<(String, FirType)>,
) -> Result<(), WasmBackendError> {
    match match_fir(store, id) {
        FirMatch::Block(items) => {
            for item in items {
                collect_compute_locals(store, item, out)?;
            }
            Ok(())
        }
        FirMatch::DeclareVar {
            name,
            typ,
            access: AccessType::Stack,
            ..
        }
        | FirMatch::DeclareVar {
            name,
            typ,
            access: AccessType::Loop,
            ..
        } => {
            if !out.iter().any(|(known, _)| known == &name) {
                out.push((name, typ));
            }
            Ok(())
        }
        FirMatch::SimpleForLoop {
            var,
            body,
            is_reverse: false,
            ..
        } => {
            if !out.iter().any(|(known, _)| known == &var) {
                out.push((var, FirType::Int32));
            }
            collect_compute_locals(store, body, out)
        }
        FirMatch::ForLoop {
            var,
            body,
            is_reverse: false | true,
            ..
        } => {
            if !out.iter().any(|(known, _)| known == &var) {
                out.push((var, FirType::Int32));
            }
            collect_compute_locals(store, body, out)
        }
        FirMatch::If {
            cond: _,
            then_block,
            else_block,
        } => {
            collect_compute_locals(store, then_block, out)?;
            if let Some(else_block) = else_block {
                collect_compute_locals(store, else_block, out)?;
            }
            Ok(())
        }
        FirMatch::Control { stmt, .. } => collect_compute_locals(store, stmt, out),
        FirMatch::WhileLoop { body, .. } => collect_compute_locals(store, body, out),
        FirMatch::Switch { cases, default, .. } => {
            for (_, case_stmt) in cases {
                collect_compute_locals(store, case_stmt, out)?;
            }
            if let Some(default_stmt) = default {
                collect_compute_locals(store, default_stmt, out)?;
            }
            Ok(())
        }
        FirMatch::Label(_)
        | FirMatch::DeclareFun { .. }
        | FirMatch::StoreTable { .. }
        | FirMatch::StoreVar { .. }
        | FirMatch::Drop(_)
        | FirMatch::NullStatement
        | FirMatch::Return(None) => Ok(()),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported compute local collector node in WASM subset: {other:?}"),
        )),
    }
}

/// Stateful FIR-to-WASM subset lowerer for body emission.
///
/// The lowerer owns just enough contextual state to translate supported FIR
/// statements/expressions into stack-machine instructions while preserving the
/// runtime memory contract carried by [`WasmMemoryLayout`].
struct ComputeSubsetLowerer<'a> {
    store: &'a FirStore,
    memory_layout: &'a WasmMemoryLayout,
    options: &'a WasmOptions,
    locals: HashMap<String, WasmLocal>,
    imports: HashMap<String, u32>,
}

impl ComputeSubsetLowerer<'_> {
    /// Lowers one FIR `Block` into a sequence of WASM instructions.
    fn lower_block_into(
        &mut self,
        id: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let FirMatch::Block(items) = match_fir(self.store, id) else {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                "compute subset expected FIR Block body",
            ));
        };
        for item in items {
            self.lower_stmt(item, function)?;
        }
        Ok(())
    }

    /// Lowers one FIR statement supported by the current subset.
    fn lower_stmt(&mut self, id: FirId, function: &mut Function) -> Result<(), WasmBackendError> {
        match match_fir(self.store, id) {
            FirMatch::Block(_) => self.lower_block_into(id, function),
            FirMatch::DeclareVar {
                name,
                access: AccessType::Stack | AccessType::Loop,
                init,
                ..
            } => {
                let local = self.local(&name)?.clone();
                if matches!(local.storage, WasmLocalStorage::Memory { .. }) {
                    if init.is_some() {
                        return Err(WasmBackendError::new(
                            WasmBackendErrorCode::UnsupportedFirNode,
                            format!("WASM local array `{name}` does not support inline init yet"),
                        ));
                    }
                    return Ok(());
                }
                let WasmLocalStorage::Local(index) = local.storage else {
                    unreachable!("memory local handled above");
                };
                if let Some(init) = init {
                    self.lower_expr(init, function)?;
                } else {
                    self.emit_default_value(&local.typ, function)?;
                }
                function.instruction(&Instruction::LocalSet(index));
                Ok(())
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse: false,
            } => self.lower_simple_for(var, upper, body, function),
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => self.lower_for_loop(var, init, end, step, body, is_reverse, function),
            FirMatch::WhileLoop { cond, body } => self.lower_while_loop(cond, body, function),
            FirMatch::StoreTable {
                name,
                access: AccessType::Stack,
                index,
                value,
            } => self.lower_store_table_stack(&name, index, value, function),
            FirMatch::StoreTable {
                name,
                access: AccessType::Struct,
                index,
                value,
            } => self.lower_store_table_struct(&name, index, value, function),
            FirMatch::StoreVar {
                name,
                access: AccessType::Stack | AccessType::Loop,
                value,
            } => self.lower_store_var_local(&name, value, function),
            FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            } => self.lower_store_var_struct(&name, value, function),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => self.lower_if_stmt(cond, then_block, else_block, function),
            FirMatch::Control { cond, stmt } => self.lower_if_stmt(cond, stmt, None, function),
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => self.lower_switch_stmt(cond, &cases, default, function),
            FirMatch::Drop(value) => {
                if is_obviously_side_effect_free_value(self.store, value) {
                    return Ok(());
                }
                self.lower_expr(value, function)?;
                function.instruction(&Instruction::Drop);
                Ok(())
            }
            FirMatch::Label(_) => Ok(()),
            FirMatch::NullStatement | FirMatch::Return(None) => Ok(()),
            other => Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("unsupported compute statement in WASM subset: {other:?}"),
            )),
        }
    }

    /// Lowers a canonical `for (i = 0; i < upper; ++i)` loop shape.
    fn lower_simple_for(
        &mut self,
        var: String,
        upper: FirId,
        body: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let local = self.local(&var)?.clone();
        let WasmLocalStorage::Local(index) = local.storage else {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("WASM loop variable `{var}` is not a scalar local"),
            ));
        };
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::LocalSet(index));
        function.instruction(&Instruction::Block(BlockType::Empty));
        function.instruction(&Instruction::Loop(BlockType::Empty));
        function.instruction(&Instruction::LocalGet(index));
        self.lower_expr(upper, function)?;
        function.instruction(&Instruction::I32GeS);
        function.instruction(&Instruction::BrIf(1));
        self.lower_block_into(body, function)?;
        function.instruction(&Instruction::LocalGet(index));
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(index));
        function.instruction(&Instruction::Br(0));
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::End);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    /// Lowers the more general FIR `ForLoop` shape.
    fn lower_for_loop(
        &mut self,
        var: String,
        init: FirId,
        end: FirId,
        step: FirId,
        body: FirId,
        is_reverse: bool,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let local = self.local(&var)?.clone();
        let WasmLocalStorage::Local(index) = local.storage else {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("WASM loop variable `{var}` is not a scalar local"),
            ));
        };
        // init is a DeclareVar(kLoop) per FIR contract; extract its value.
        let init_val =
            if let FirMatch::DeclareVar { init: Some(v), .. } = match_fir(self.store, init) {
                v
            } else {
                init
            };
        self.lower_expr(init_val, function)?;
        function.instruction(&Instruction::LocalSet(index));
        function.instruction(&Instruction::Block(BlockType::Empty));
        function.instruction(&Instruction::Loop(BlockType::Empty));
        function.instruction(&Instruction::LocalGet(index));
        self.lower_expr(end, function)?;
        function.instruction(if is_reverse {
            &Instruction::I32LeS
        } else {
            &Instruction::I32GeS
        });
        function.instruction(&Instruction::BrIf(1));
        self.lower_block_into(body, function)?;
        function.instruction(&Instruction::LocalGet(index));
        self.lower_expr(step, function)?;
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(index));
        function.instruction(&Instruction::Br(0));
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::End);
        Ok(())
    }

    /// Lowers a FIR `WhileLoop` into nested `block`/`loop` control flow.
    fn lower_while_loop(
        &mut self,
        cond: FirId,
        body: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        function.instruction(&Instruction::Block(BlockType::Empty));
        function.instruction(&Instruction::Loop(BlockType::Empty));
        self.lower_expr(cond, function)?;
        self.emit_cast_if_needed(&FirType::Bool, ValType::I32, function)?;
        function.instruction(&Instruction::I32Eqz);
        function.instruction(&Instruction::BrIf(1));
        self.lower_block_into(body, function)?;
        function.instruction(&Instruction::Br(0));
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::End);
        Ok(())
    }

    /// Lowers a store through one stack-local pointer alias.
    fn lower_store_table_stack(
        &mut self,
        name: &str,
        index: FirId,
        value: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let local = self.local(name)?.clone();
        let (elem_type, storage_type) = match &local.storage {
            WasmLocalStorage::Local(local_index) => {
                let elem_type = stack_alias_pointee(&local.typ)?;
                function.instruction(&Instruction::LocalGet(*local_index));
                self.lower_index_offset(index, &elem_type, function)?;
                function.instruction(&Instruction::I32Add);
                let storage_type = wasm_val_type_for_fir(&elem_type, self.options)?;
                (elem_type, storage_type)
            }
            WasmLocalStorage::Memory {
                offset,
                elem_type,
                storage_type,
            } => {
                function.instruction(&Instruction::LocalGet(0));
                function.instruction(&Instruction::I32Const(*offset as i32));
                function.instruction(&Instruction::I32Add);
                self.lower_index_offset(index, elem_type, function)?;
                function.instruction(&Instruction::I32Add);
                (elem_type.clone(), *storage_type)
            }
        };
        self.lower_expr(value, function)?;
        let value_type = self.store.value_type(value).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("missing value type for stack table store `{name}`"),
            )
        })?;
        self.emit_cast_if_needed(&value_type, storage_type, function)?;
        if matches!(local.storage, WasmLocalStorage::Local(_)) {
            function.instruction(&store_instruction_for_type(&elem_type, self.options)?);
        } else {
            function.instruction(&store_instruction_for_valtype(storage_type)?);
        }
        Ok(())
    }

    /// Lowers a scalar store into one struct/global runtime field.
    fn lower_store_var_struct(
        &mut self,
        name: &str,
        value: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let field = self.struct_field(name)?.clone();
        let field_val_type = wasm_val_type_for_field(&field);
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::I32Const(field.offset as i32));
        function.instruction(&Instruction::I32Add);
        self.lower_expr(value, function)?;
        let value_type = self.store.value_type(value).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("missing value type for struct store `{name}`"),
            )
        })?;
        self.emit_cast_if_needed(&value_type, field_val_type, function)?;
        function.instruction(&store_instruction_for_valtype(field_val_type)?);
        Ok(())
    }

    /// Lowers an assignment to a loop or stack local.
    fn lower_store_var_local(
        &mut self,
        name: &str,
        value: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let local = self.local(name)?.clone();
        let WasmLocalStorage::Local(index) = local.storage else {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("WASM local array `{name}` cannot be assigned as a scalar"),
            ));
        };
        self.lower_expr(value, function)?;
        function.instruction(&Instruction::LocalSet(index));
        Ok(())
    }

    /// Lowers an indexed store into one struct/static table region.
    fn lower_store_table_struct(
        &mut self,
        name: &str,
        index: FirId,
        value: FirId,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let field = self.struct_field(name)?.clone();
        let field_val_type = wasm_val_type_for_field(&field);
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::I32Const(field.offset as i32));
        function.instruction(&Instruction::I32Add);
        self.lower_index_offset(index, &field_fir_type(&field, self.options), function)?;
        function.instruction(&Instruction::I32Add);
        self.lower_expr(value, function)?;
        let value_type = self.store.value_type(value).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("missing value type for struct table store `{name}`"),
            )
        })?;
        self.emit_cast_if_needed(&value_type, field_val_type, function)?;
        function.instruction(&store_instruction_for_valtype(field_val_type)?);
        Ok(())
    }

    /// Lowers FIR `If`/`Control` statements to structured WASM control flow.
    fn lower_if_stmt(
        &mut self,
        cond: FirId,
        then_block: FirId,
        else_block: Option<FirId>,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        self.lower_expr(cond, function)?;
        self.emit_cast_if_needed(&FirType::Bool, ValType::I32, function)?;
        function.instruction(&Instruction::If(BlockType::Empty));
        self.lower_stmt(then_block, function)?;
        if let Some(else_block) = else_block {
            function.instruction(&Instruction::Else);
            self.lower_stmt(else_block, function)?;
        }
        function.instruction(&Instruction::End);
        Ok(())
    }

    /// Dispatches FIR `Switch` lowering after resolving the condition type.
    fn lower_switch_stmt(
        &mut self,
        cond: FirId,
        cases: &[(i64, FirId)],
        default: Option<FirId>,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let cond_ty = self.store.value_type(cond).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                "missing value type for WASM switch condition",
            )
        })?;
        self.lower_switch_cases(cond, &cond_ty, cases, default, function)
    }

    fn lower_switch_cases(
        &mut self,
        cond: FirId,
        cond_ty: &FirType,
        cases: &[(i64, FirId)],
        default: Option<FirId>,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let Some(((case_value, case_stmt), rest)) = cases.split_first() else {
            if let Some(default_stmt) = default {
                self.lower_stmt(default_stmt, function)?;
            }
            return Ok(());
        };
        self.lower_expr(cond, function)?;
        emit_switch_case_const(*case_value, cond_ty, function)?;
        function.instruction(&switch_eq_instruction(cond_ty)?);
        function.instruction(&Instruction::If(BlockType::Empty));
        self.lower_stmt(*case_stmt, function)?;
        if !rest.is_empty() || default.is_some() {
            function.instruction(&Instruction::Else);
            self.lower_switch_cases(cond, cond_ty, rest, default, function)?;
        }
        function.instruction(&Instruction::End);
        Ok(())
    }

    /// Recursively lowers one FIR expression onto the WASM operand stack.
    fn lower_expr(&mut self, id: FirId, function: &mut Function) -> Result<(), WasmBackendError> {
        match match_fir(self.store, id) {
            FirMatch::Bool { value, .. } => {
                function.instruction(&Instruction::I32Const(i32::from(value)));
                Ok(())
            }
            FirMatch::Int32 { value, .. } => {
                function.instruction(&Instruction::I32Const(value));
                Ok(())
            }
            FirMatch::Float32 { value, .. } => {
                function.instruction(&Instruction::F32Const(value));
                Ok(())
            }
            FirMatch::Float64 { value, .. } => {
                function.instruction(&Instruction::F64Const(value));
                Ok(())
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::FunArgs,
                ..
            } if name == "count" => {
                function.instruction(&Instruction::LocalGet(1));
                Ok(())
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::FunArgs,
                ..
            } if name == "sample_rate" => {
                function.instruction(&Instruction::LocalGet(1));
                Ok(())
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct,
                typ,
            } => {
                let field = self.struct_field(&name)?;
                function.instruction(&Instruction::LocalGet(0));
                function.instruction(&Instruction::I32Const(field.offset as i32));
                function.instruction(&Instruction::I32Add);
                let storage_ty = wasm_val_type_for_field(field);
                function.instruction(&load_instruction_for_valtype(storage_ty)?);
                self.emit_cast_if_needed(
                    &field_fir_type(field, self.options),
                    wasm_val_type_for_fir(&typ, self.options)?,
                    function,
                )?;
                Ok(())
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Loop | AccessType::Stack,
                ..
            } => {
                let local = self.local(&name)?;
                match local.storage {
                    WasmLocalStorage::Local(index) => {
                        function.instruction(&Instruction::LocalGet(index));
                        Ok(())
                    }
                    WasmLocalStorage::Memory { .. } => Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedFirNode,
                        format!("WASM local array `{name}` cannot be loaded as a scalar"),
                    )),
                }
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Global,
                ..
            } => Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!(
                    "accessing foreign variable '{name}' is not supported in this compilation mode"
                ),
            )),
            FirMatch::LoadTable {
                name,
                access: AccessType::FunArgs,
                index,
                typ,
            } if name == "inputs" || name == "outputs" => {
                function.instruction(&Instruction::LocalGet(fun_arg_local_index(&name)));
                self.lower_index_offset(index, &FirType::Ptr(Box::new(FirType::Void)), function)?;
                function.instruction(&Instruction::I32Add);
                if matches!(typ, FirType::Ptr(_)) {
                    function.instruction(&Instruction::I32Load(memarg(0)));
                    Ok(())
                } else {
                    Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedFirNode,
                        format!(
                            "expected pointer type for function-arg table `{name}`, got {typ:?}"
                        ),
                    ))
                }
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::Static,
                index,
                typ,
            } => {
                let field = self.struct_field(&name)?.clone();
                let field_fir = field_fir_type(&field, self.options);
                let storage_ty = wasm_val_type_for_field(&field);
                function.instruction(&Instruction::I32Const(field.offset as i32));
                self.lower_index_offset(index, &field_fir, function)?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&load_instruction_for_valtype(storage_ty)?);
                self.emit_cast_if_needed(
                    &field_fir,
                    wasm_val_type_for_fir(&typ, self.options)?,
                    function,
                )?;
                Ok(())
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::Stack,
                index,
                typ,
            } => {
                let local = self.local(&name)?.clone();
                match &local.storage {
                    WasmLocalStorage::Local(local_index) => {
                        let elem_type = stack_alias_pointee(&local.typ)?;
                        function.instruction(&Instruction::LocalGet(*local_index));
                        self.lower_index_offset(index, &elem_type, function)?;
                        function.instruction(&Instruction::I32Add);
                        function.instruction(&load_instruction_for_type(&typ, self.options)?);
                    }
                    WasmLocalStorage::Memory {
                        offset,
                        elem_type,
                        storage_type,
                    } => {
                        function.instruction(&Instruction::LocalGet(0));
                        function.instruction(&Instruction::I32Const(*offset as i32));
                        function.instruction(&Instruction::I32Add);
                        self.lower_index_offset(index, elem_type, function)?;
                        function.instruction(&Instruction::I32Add);
                        function.instruction(&load_instruction_for_valtype(*storage_type)?);
                        self.emit_cast_if_needed(
                            elem_type,
                            wasm_val_type_for_fir(&typ, self.options)?,
                            function,
                        )?;
                    }
                }
                Ok(())
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::Struct,
                index,
                typ,
            } => {
                let field = self.struct_field(&name)?.clone();
                let field_fir = field_fir_type(&field, self.options);
                let storage_ty = wasm_val_type_for_field(&field);
                function.instruction(&Instruction::LocalGet(0));
                function.instruction(&Instruction::I32Const(field.offset as i32));
                function.instruction(&Instruction::I32Add);
                self.lower_index_offset(index, &field_fir, function)?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&load_instruction_for_valtype(storage_ty)?);
                self.emit_cast_if_needed(
                    &field_fir,
                    wasm_val_type_for_fir(&typ, self.options)?,
                    function,
                )?;
                Ok(())
            }
            FirMatch::LoadSoundfileLength { var, part } => {
                self.emit_soundfile_field_ptr(&var, SOUNDFILE_LENGTH_OFFSET, function)?;
                self.lower_index_offset(part, &FirType::Int32, function)?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(memarg(0)));
                Ok(())
            }
            FirMatch::LoadSoundfileRate { var, part } => {
                self.emit_soundfile_field_ptr(&var, SOUNDFILE_RATE_OFFSET, function)?;
                self.lower_index_offset(part, &FirType::Int32, function)?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(memarg(0)));
                Ok(())
            }
            FirMatch::LoadSoundfileBuffer {
                var,
                chan,
                part,
                idx,
                typ,
            } => {
                self.emit_soundfile_field_ptr(&var, SOUNDFILE_BUFFERS_OFFSET, function)?;
                self.lower_index_offset(chan, &FirType::Ptr(Box::new(typ.clone())), function)?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(memarg(0)));

                self.emit_soundfile_field_ptr(&var, SOUNDFILE_FRAME_OFFSET_OFFSET, function)?;
                self.lower_index_offset(part, &FirType::Int32, function)?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Load(memarg(0)));

                self.lower_expr(idx, function)?;
                self.emit_cast_if_needed(
                    &self.store.value_type(idx).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::UnsupportedFirNode,
                            format!("missing idx value type for soundfile buffer load `{var}`"),
                        )
                    })?,
                    ValType::I32,
                    function,
                )?;
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::I32Const(elem_size_bytes(&typ, self.options)?));
                function.instruction(&Instruction::I32Mul);
                function.instruction(&Instruction::I32Add);
                function.instruction(&load_instruction_for_type(&typ, self.options)?);
                Ok(())
            }
            FirMatch::Cast { typ, value } => {
                let src_ty = self.store.value_type(value).ok_or_else(|| {
                    WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedFirNode,
                        "missing value type for WASM cast",
                    )
                })?;
                self.lower_expr(value, function)?;
                self.emit_cast_if_needed(
                    &src_ty,
                    wasm_val_type_for_fir(&typ, self.options)?,
                    function,
                )?;
                Ok(())
            }
            FirMatch::Neg { value, typ } => {
                match wasm_val_type_for_fir(&typ, self.options)? {
                    ValType::I32 => {
                        function.instruction(&Instruction::I32Const(0));
                        self.lower_expr(value, function)?;
                        function.instruction(&Instruction::I32Sub);
                    }
                    ValType::I64 => {
                        function.instruction(&Instruction::I64Const(0));
                        self.lower_expr(value, function)?;
                        function.instruction(&Instruction::I64Sub);
                    }
                    ValType::F32 => {
                        function.instruction(&Instruction::F32Const(0.0));
                        self.lower_expr(value, function)?;
                        function.instruction(&Instruction::F32Sub);
                    }
                    ValType::F64 => {
                        function.instruction(&Instruction::F64Const(0.0));
                        self.lower_expr(value, function)?;
                        function.instruction(&Instruction::F64Sub);
                    }
                    other => {
                        return Err(WasmBackendError::new(
                            WasmBackendErrorCode::UnsupportedFirNode,
                            format!("unsupported WASM neg type in compute subset: {other:?}"),
                        ));
                    }
                }
                Ok(())
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                self.lower_expr(lhs, function)?;
                self.lower_expr(rhs, function)?;
                let op_typ = if is_comparison_binop(op) {
                    // C++ parity: FIR comparison binops produce an integer/bool-like
                    // result, so the node result type describes the output on the
                    // WASM stack, not the operand lane used by the comparison opcode.
                    //
                    // Example: `Gt(Float64, Float64) -> Int32` must still lower to
                    // `f64.gt`; the resulting condition value is the `i32` that later
                    // feeds `if`/`select`.
                    //
                    // The reference C++ WASM backend makes the same choice from
                    // `TypingVisitor::getType(inst->fInst1/2)` in
                    // `compiler/generator/wasm/wasm_instructions.hh`.
                    self.store.value_type(lhs).ok_or_else(|| {
                        WasmBackendError::new(
                            WasmBackendErrorCode::UnsupportedFirNode,
                            format!("missing lhs value type for WASM comparison binop: {op:?}"),
                        )
                    })?
                } else {
                    typ
                };
                function.instruction(&binop_instruction(op, &op_typ, self.options)?);
                Ok(())
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                self.lower_expr(then_value, function)?;
                self.lower_expr(else_value, function)?;
                self.lower_expr(cond, function)?;
                self.emit_cast_if_needed(&FirType::Bool, ValType::I32, function)?;
                let _ = wasm_val_type_for_fir(&typ, self.options)?;
                function.instruction(&Instruction::Select);
                Ok(())
            }
            FirMatch::FunCall { name, args, typ }
                if name == "abs" || name == "max_i" || name == "min_i" =>
            {
                self.lower_internal_int_fun_call(&name, &args, &typ, function)
            }
            FirMatch::FunCall { name, args, typ } => {
                if let Some(math) = FirMathOp::from_symbol(&name) {
                    self.lower_math_call(math, &args, &typ, function)
                } else if let Some(import) =
                    imported_foreign_signature(&name, &args, &typ, self.options)?
                {
                    self.lower_imported_call(&import, &args, function)
                } else if let Some(func_index) = self.imports.get(&name).copied() {
                    for arg in &args {
                        self.lower_expr(*arg, function)?;
                    }
                    function.instruction(&Instruction::Call(func_index));
                    Ok(())
                } else {
                    Err(WasmBackendError::new(
                        WasmBackendErrorCode::UnsupportedFirNode,
                        format!("unsupported function call in WASM subset: `{name}`"),
                    ))
                }
            }
            other => Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("unsupported compute expression in WASM subset: {other:?}"),
            )),
        }
    }

    /// Converts an element index expression into a byte offset.
    fn lower_index_offset(
        &mut self,
        index: FirId,
        elem_type: &FirType,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        self.lower_expr(index, function)?;
        function.instruction(&Instruction::I32Const(elem_size_bytes(
            elem_type,
            self.options,
        )?));
        function.instruction(&Instruction::I32Mul);
        Ok(())
    }

    /// Emits the zero/default literal for one local type.
    fn emit_default_value(
        &self,
        typ: &FirType,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        match wasm_val_type_for_fir(typ, self.options)? {
            ValType::I32 => function.instruction(&Instruction::I32Const(0)),
            ValType::I64 => function.instruction(&Instruction::I64Const(0)),
            ValType::F32 => function.instruction(&Instruction::F32Const(0.0)),
            ValType::F64 => function.instruction(&Instruction::F64Const(0.0)),
            other => {
                return Err(WasmBackendError::new(
                    WasmBackendErrorCode::UnsupportedFirNode,
                    format!("unsupported WASM local default type: {other:?}"),
                ));
            }
        };
        Ok(())
    }

    /// Resolves one named local allocated during subset lowering.
    fn local(&self, name: &str) -> Result<&WasmLocal, WasmBackendError> {
        self.locals.get(name).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("compute subset local `{name}` not found"),
            )
        })
    }

    /// Resolves one named runtime field from the computed memory layout.
    fn struct_field(&self, name: &str) -> Result<&FieldLayout, WasmBackendError> {
        self.memory_layout.field_offsets.get(name).ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("compute subset struct field `{name}` not found in WASM layout"),
            )
        })
    }

    /// Pushes the pointer stored in one DSP `Soundfile*` field, then follows one
    /// field pointer inside the flattened runtime `Soundfile` struct.
    fn emit_soundfile_field_ptr(
        &mut self,
        var: &str,
        soundfile_field_offset: u32,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let field = self.struct_field(var)?;
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::I32Const(field.offset as i32));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::I32Load(memarg(0)));
        function.instruction(&Instruction::I32Const(soundfile_field_offset as i32));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::I32Load(memarg(0)));
        Ok(())
    }

    /// Emits an explicit numeric conversion when FIR and storage types differ.
    fn emit_cast_if_needed(
        &self,
        src: &FirType,
        dst: ValType,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let src = wasm_val_type_for_fir(src, self.options)?;
        if src == dst {
            return Ok(());
        }
        let instr = match (src, dst) {
            (ValType::F32, ValType::F64) => Instruction::F64PromoteF32,
            (ValType::F64, ValType::F32) => Instruction::F32DemoteF64,
            (ValType::I32, ValType::F32) => Instruction::F32ConvertI32S,
            (ValType::I32, ValType::F64) => Instruction::F64ConvertI32S,
            (ValType::F32, ValType::I32) => Instruction::I32TruncSatF32S,
            (ValType::F64, ValType::I32) => Instruction::I32TruncSatF64S,
            (ValType::I32, ValType::I64) => Instruction::I64ExtendI32S,
            (ValType::I64, ValType::I32) => Instruction::I32WrapI64,
            _ => {
                return Err(WasmBackendError::new(
                    WasmBackendErrorCode::UnsupportedFirNode,
                    format!("unsupported WASM cast in compute subset: {src:?} -> {dst:?}"),
                ));
            }
        };
        function.instruction(&instr);
        Ok(())
    }

    /// Lowers a recognized math intrinsic, using native WASM opcodes when
    /// available and host imports otherwise.
    fn lower_math_call(
        &mut self,
        math: FirMathOp,
        args: &[FirId],
        typ: &FirType,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        match (math, wasm_val_type_for_fir(typ, self.options)?, args) {
            (FirMathOp::Abs, ValType::I32, [x]) => {
                // Adapted from the C++ WASM backend: instead of importing host
                // `abs(int)`, we lower directly to WASM using the existing
                // subset expression machinery. FIR expressions are pure, so
                // re-evaluating `x` to synthesize `select(0 - x, x, x < 0)` is
                // semantically safe during this bring-up phase.
                function.instruction(&Instruction::I32Const(0));
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::I32Sub);
                self.lower_expr(*x, function)?;
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32LtS);
                function.instruction(&Instruction::Select);
                Ok(())
            }
            (FirMathOp::Abs, ValType::F32, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F32Abs);
                Ok(())
            }
            (FirMathOp::Abs, ValType::F64, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F64Abs);
                Ok(())
            }
            (FirMathOp::Sqrt, ValType::F32, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F32Sqrt);
                Ok(())
            }
            (FirMathOp::Sqrt, ValType::F64, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F64Sqrt);
                Ok(())
            }
            (FirMathOp::Floor, ValType::F32, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F32Floor);
                Ok(())
            }
            (FirMathOp::Floor, ValType::F64, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F64Floor);
                Ok(())
            }
            (FirMathOp::Ceil, ValType::F32, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F32Ceil);
                Ok(())
            }
            (FirMathOp::Ceil, ValType::F64, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F64Ceil);
                Ok(())
            }
            (FirMathOp::Rint, ValType::F32, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F32Nearest);
                Ok(())
            }
            (FirMathOp::Rint, ValType::F64, [x]) => {
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::F64Nearest);
                Ok(())
            }
            (FirMathOp::Min, ValType::F32, [x, y]) => {
                self.lower_expr(*x, function)?;
                self.lower_expr(*y, function)?;
                function.instruction(&Instruction::F32Min);
                Ok(())
            }
            (FirMathOp::Min, ValType::F64, [x, y]) => {
                self.lower_expr(*x, function)?;
                self.lower_expr(*y, function)?;
                function.instruction(&Instruction::F64Min);
                Ok(())
            }
            (FirMathOp::Max, ValType::F32, [x, y]) => {
                self.lower_expr(*x, function)?;
                self.lower_expr(*y, function)?;
                function.instruction(&Instruction::F32Max);
                Ok(())
            }
            (FirMathOp::Max, ValType::F64, [x, y]) => {
                self.lower_expr(*x, function)?;
                self.lower_expr(*y, function)?;
                function.instruction(&Instruction::F64Max);
                Ok(())
            }
            _ => self.lower_imported_math_call(math, args, typ, function),
        }
    }

    /// Lowers backend-internal helper calls such as `max_i` / `min_i`.
    fn lower_internal_int_fun_call(
        &mut self,
        name: &str,
        args: &[FirId],
        typ: &FirType,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        match (name, typ, args) {
            ("abs", FirType::Int32, [x]) => {
                // Adapted from the C++ WASM backend: instead of importing host
                // `abs(int)`, lower directly to WASM using the existing subset
                // expression machinery. FIR expressions are pure, so
                // re-evaluating `x` to synthesize `select(0 - x, x, x < 0)` is
                // semantically safe during this bring-up phase.
                function.instruction(&Instruction::I32Const(0));
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::I32Sub);
                self.lower_expr(*x, function)?;
                self.lower_expr(*x, function)?;
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32LtS);
                function.instruction(&Instruction::Select);
                return Ok(());
            }
            ("min_i" | "max_i", FirType::Int32, [_, _]) => {}
            _ => {
                return Err(WasmBackendError::new(
                    WasmBackendErrorCode::UnsupportedFirNode,
                    format!(
                        "unsupported internal WASM helper call in compute subset: `{name}` / {typ:?} / argc={}",
                        args.len()
                    ),
                ));
            }
        }
        if args.len() != 2 {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!(
                    "unsupported internal WASM helper call in compute subset: `{name}` / {typ:?} / argc={}",
                    args.len()
                ),
            ));
        }
        for arg in args {
            self.lower_expr(*arg, function)?;
        }
        let callee = match name {
            "max_i" => WasmFunc::MaxI,
            "min_i" => WasmFunc::MinI,
            _ => unreachable!("guarded by caller"),
        };
        function.instruction(&Instruction::Call(function_index_for_body(
            callee,
            self.imports.len() as u32,
        )));
        Ok(())
    }

    /// Lowers a math intrinsic through the imported-helper path.
    fn lower_imported_math_call(
        &mut self,
        math: FirMathOp,
        args: &[FirId],
        typ: &FirType,
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        let import = imported_math_signature(math, typ, self.options)?.ok_or_else(|| {
            WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!(
                    "unsupported imported WASM math subset call: {math:?} / {typ:?} / argc={}",
                    args.len()
                ),
            )
        })?;
        self.lower_imported_call(&import, args, function)
    }

    /// Emits one call to an already-registered imported host helper.
    fn lower_imported_call(
        &mut self,
        import: &WasmImport,
        args: &[FirId],
        function: &mut Function,
    ) -> Result<(), WasmBackendError> {
        if import.params.len() != args.len() {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!(
                    "imported WASM function arity mismatch: {} expects {}, got {}",
                    import.field_name,
                    import.params.len(),
                    args.len()
                ),
            ));
        }
        for arg in args {
            self.lower_expr(*arg, function)?;
        }
        let func_index = self
            .imports
            .get(&import.field_name)
            .copied()
            .ok_or_else(|| {
                WasmBackendError::new(
                    WasmBackendErrorCode::UnsupportedFirNode,
                    format!(
                        "missing imported WASM function index for `{}`",
                        import.field_name
                    ),
                )
            })?;
        function.instruction(&Instruction::Call(func_index));
        Ok(())
    }
}

/// Scans lowered bodies for host imports that must be declared in the module
/// import section.
fn collect_wasm_imports(
    store: &FirStore,
    bodies: &[Option<FirId>],
    foreign_fun_imports: &HashMap<String, WasmImport>,
    options: &WasmOptions,
) -> Result<Vec<WasmImport>, WasmBackendError> {
    let mut imports = std::collections::BTreeSet::new();
    for body in bodies.iter().flatten() {
        collect_wasm_imports_in_node(store, *body, foreign_fun_imports, options, &mut imports)?;
    }
    Ok(imports.into_iter().collect())
}

/// Finds the FIR body id for one named function declaration inside a module.
fn find_function_body(store: &FirStore, function_items: &[FirId], name: &str) -> Option<FirId> {
    function_items
        .iter()
        .copied()
        .find_map(|id| match match_fir(store, id) {
            FirMatch::DeclareFun {
                name: ref fun_name,
                body: Some(body),
                ..
            } if fun_name == name => Some(body),
            _ => None,
        })
}

/// Serializes FIR static tables into WASM data segments.
fn static_table_segments(
    store: &FirStore,
    static_decls: FirId,
    memory_layout: &WasmMemoryLayout,
    options: &WasmOptions,
) -> Result<Vec<(u32, Vec<u8>)>, WasmBackendError> {
    let FirMatch::Block(items) = match_fir(store, static_decls) else {
        return Ok(Vec::new());
    };
    let mut segments = Vec::new();
    for item in items {
        let FirMatch::DeclareTable {
            name,
            access: AccessType::Static,
            elem_type,
            values,
        } = match_fir(store, item)
        else {
            continue;
        };
        let offset = memory_layout
            .field_offsets
            .get(&name)
            .ok_or_else(|| {
                WasmBackendError::new(
                    WasmBackendErrorCode::UnsupportedModuleShape,
                    format!("missing WASM layout entry for static table `{name}`"),
                )
            })?
            .offset;
        let mut bytes = Vec::new();
        for value in values {
            append_static_scalar_bytes(store, value, &elem_type, options, &mut bytes)?;
        }
        segments.push((offset, bytes));
    }
    Ok(segments)
}

/// Appends one scalar table initializer value to a raw data-segment buffer.
fn append_static_scalar_bytes(
    store: &FirStore,
    value: FirId,
    elem_type: &FirType,
    options: &WasmOptions,
    out: &mut Vec<u8>,
) -> Result<(), WasmBackendError> {
    match (elem_type, match_fir(store, value)) {
        (FirType::Bool, FirMatch::Bool { value, .. }) => {
            out.extend_from_slice(&u32::from(value).to_le_bytes());
        }
        (FirType::Int32, FirMatch::Int32 { value, .. }) => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        (FirType::Float32 | FirType::FaustFloat, FirMatch::Float32 { value, .. }) => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        (FirType::Float64, FirMatch::Float64 { value, .. }) => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        (FirType::FaustFloat, FirMatch::Float64 { value, .. }) if !options.double_precision => {
            out.extend_from_slice(&(value as f32).to_le_bytes());
        }
        (FirType::FaustFloat, FirMatch::Float64 { value, .. }) if options.double_precision => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        _ => {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedModuleShape,
                format!(
                    "unsupported static table initializer for WASM data segment: elem_type={elem_type:?}, value={:?}",
                    match_fir(store, value)
                ),
            ));
        }
    }
    Ok(())
}

/// Recursive helper used by [`collect_wasm_imports`] to discover imported
/// helpers in one FIR subtree.
fn collect_wasm_imports_in_node(
    store: &FirStore,
    id: FirId,
    foreign_fun_imports: &HashMap<String, WasmImport>,
    options: &WasmOptions,
    out: &mut std::collections::BTreeSet<WasmImport>,
) -> Result<(), WasmBackendError> {
    if let FirMatch::FunCall { name, typ, args } = match_fir(store, id) {
        if name == "abs" || name == "max_i" || name == "min_i" {
            // Internal lowered helpers, not host imports.
        } else if let Some(math) = FirMathOp::from_symbol(&name)
            && !is_native_wasm_math(math, &typ, options)
            && let Some(import) = imported_math_signature(math, &typ, options)?
        {
            out.insert(import);
        } else if let Some(import) = imported_foreign_signature(&name, &args, &typ, options)? {
            out.insert(import);
        } else if let Some(import) = foreign_fun_imports.get(&name) {
            out.insert(import.clone());
        }
    }
    for child in fir_children(store, id) {
        collect_wasm_imports_in_node(store, child, foreign_fun_imports, options, out)?;
    }
    Ok(())
}

/// Returns the direct child nodes visited by backend-local FIR traversals.
fn fir_children(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        FirMatch::DeclareVar { init, .. } => init.into_iter().collect(),
        FirMatch::StoreVar { value, .. } => vec![value],
        FirMatch::StoreTable { index, value, .. } => vec![index, value],
        FirMatch::LoadTable { index, .. } => vec![index],
        FirMatch::SimpleForLoop { upper, body, .. } => vec![upper, body],
        FirMatch::ForLoop {
            init,
            end,
            step,
            body,
            ..
        } => vec![init, end, step, body],
        FirMatch::BinOp { lhs, rhs, .. } => vec![lhs, rhs],
        FirMatch::Cast { value, .. } => vec![value],
        FirMatch::Neg { value, .. } => vec![value],
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => vec![cond, then_value, else_value],
        FirMatch::FunCall { args, .. } => args,
        FirMatch::Return(Some(value)) => vec![value],
        FirMatch::Drop(value) => vec![value],
        FirMatch::Control { cond, stmt } => vec![cond, stmt],
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => else_block
            .into_iter()
            .fold(vec![cond, then_block], |mut acc, item| {
                acc.push(item);
                acc
            }),
        _ => Vec::new(),
    }
}

/// Returns `true` when an intrinsic can map directly to a native WASM opcode.
fn is_native_wasm_math(math: FirMathOp, typ: &FirType, options: &WasmOptions) -> bool {
    matches!(
        (math, wasm_val_type_for_fir(typ, options)),
        (FirMathOp::Abs, Ok(ValType::F32 | ValType::F64))
            | (FirMathOp::Sqrt, Ok(ValType::F32 | ValType::F64))
            | (FirMathOp::Floor, Ok(ValType::F32 | ValType::F64))
            | (FirMathOp::Ceil, Ok(ValType::F32 | ValType::F64))
            | (FirMathOp::Rint, Ok(ValType::F32 | ValType::F64))
            | (FirMathOp::Min, Ok(ValType::F32 | ValType::F64))
            | (FirMathOp::Max, Ok(ValType::F32 | ValType::F64))
    )
}

/// Maps one Faust math intrinsic to the host import expected by the standard
/// JS/WASM runtime when no native WASM opcode exists.
fn imported_math_signature(
    math: FirMathOp,
    typ: &FirType,
    options: &WasmOptions,
) -> Result<Option<WasmImport>, WasmBackendError> {
    let val_ty = wasm_val_type_for_fir(typ, options)?;
    let import = match (math, val_ty) {
        (FirMathOp::Sin, ValType::F32) => Some(WasmImport {
            field_name: "_sinf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Cos, ValType::F32) => Some(WasmImport {
            field_name: "_cosf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Exp, ValType::F32) => Some(WasmImport {
            field_name: "_expf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Exp10, ValType::F32) => Some(WasmImport {
            field_name: "_exp10f".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Log, ValType::F32) => Some(WasmImport {
            field_name: "_logf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Log10, ValType::F32) => Some(WasmImport {
            field_name: "_log10f".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Tan, ValType::F32) => Some(WasmImport {
            field_name: "_tanf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Atan, ValType::F32) => Some(WasmImport {
            field_name: "_atanf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Asin, ValType::F32) => Some(WasmImport {
            field_name: "_asinf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Acos, ValType::F32) => Some(WasmImport {
            field_name: "_acosf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Round, ValType::F32) => Some(WasmImport {
            field_name: "_roundf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Pow, ValType::F32) => Some(WasmImport {
            field_name: "_powf".to_owned(),
            params: vec![ValType::F32, ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Atan2, ValType::F32) => Some(WasmImport {
            field_name: "_atan2f".to_owned(),
            params: vec![ValType::F32, ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Fmod, ValType::F32) => Some(WasmImport {
            field_name: "_fmodf".to_owned(),
            params: vec![ValType::F32, ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Remainder, ValType::F32) => Some(WasmImport {
            field_name: "_remainderf".to_owned(),
            params: vec![ValType::F32, ValType::F32],
            results: vec![ValType::F32],
        }),
        (FirMathOp::Sin, ValType::F64) => Some(WasmImport {
            field_name: "_sin".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Cos, ValType::F64) => Some(WasmImport {
            field_name: "_cos".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Exp, ValType::F64) => Some(WasmImport {
            field_name: "_exp".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Exp10, ValType::F64) => Some(WasmImport {
            field_name: "_exp10".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Log, ValType::F64) => Some(WasmImport {
            field_name: "_log".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Log10, ValType::F64) => Some(WasmImport {
            field_name: "_log10".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Tan, ValType::F64) => Some(WasmImport {
            field_name: "_tan".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Atan, ValType::F64) => Some(WasmImport {
            field_name: "_atan".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Asin, ValType::F64) => Some(WasmImport {
            field_name: "_asin".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Acos, ValType::F64) => Some(WasmImport {
            field_name: "_acos".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Round, ValType::F64) => Some(WasmImport {
            field_name: "_round".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Pow, ValType::F64) => Some(WasmImport {
            field_name: "_pow".to_owned(),
            params: vec![ValType::F64, ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Atan2, ValType::F64) => Some(WasmImport {
            field_name: "_atan2".to_owned(),
            params: vec![ValType::F64, ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Fmod, ValType::F64) => Some(WasmImport {
            field_name: "_fmod".to_owned(),
            params: vec![ValType::F64, ValType::F64],
            results: vec![ValType::F64],
        }),
        (FirMathOp::Remainder, ValType::F64) => Some(WasmImport {
            field_name: "_remainder".to_owned(),
            params: vec![ValType::F64, ValType::F64],
            results: vec![ValType::F64],
        }),
        _ => None,
    };
    Ok(import)
}

/// Maps one foreign FIR helper call to the corresponding standard host import.
///
/// This covers helper names that appear in FIR exactly as C/C++ runtime names
/// but are still expected to be provided by the surrounding Faust WASM host.
fn imported_foreign_signature(
    name: &str,
    args: &[FirId],
    typ: &FirType,
    options: &WasmOptions,
) -> Result<Option<WasmImport>, WasmBackendError> {
    let val_ty = wasm_val_type_for_fir(typ, options)?;
    let import = match (name, val_ty, args.len()) {
        // C++/wrapper parity: these foreign helpers are exported by the
        // standard Faust JS wrappers under fixed `env` names, even when FIR
        // keeps the original C float-suffixed symbol.
        ("isnanf", ValType::I32, 1) => Some(WasmImport {
            field_name: "_isnanf".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::I32],
        }),
        ("isinff", ValType::I32, 1) => Some(WasmImport {
            field_name: "_isinff".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::I32],
        }),
        ("copysignf", ValType::F32, 2) => Some(WasmImport {
            field_name: "_copysignf".to_owned(),
            params: vec![ValType::F32, ValType::F32],
            results: vec![ValType::F32],
        }),
        ("acoshf", ValType::F32, 1) => Some(WasmImport {
            field_name: "_acosh".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        ("asinhf", ValType::F32, 1) => Some(WasmImport {
            field_name: "_asinh".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        ("atanhf", ValType::F32, 1) => Some(WasmImport {
            field_name: "_atanh".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        ("coshf", ValType::F32, 1) => Some(WasmImport {
            field_name: "_cosh".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        ("sinhf", ValType::F32, 1) => Some(WasmImport {
            field_name: "_sinh".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        ("tanhf", ValType::F32, 1) => Some(WasmImport {
            field_name: "_tanh".to_owned(),
            params: vec![ValType::F32],
            results: vec![ValType::F32],
        }),
        ("isnan", ValType::I32, 1) => Some(WasmImport {
            field_name: "_isnan".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::I32],
        }),
        ("isinf", ValType::I32, 1) => Some(WasmImport {
            field_name: "_isinf".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::I32],
        }),
        ("copysign", ValType::F64, 2) => Some(WasmImport {
            field_name: "_copysign".to_owned(),
            params: vec![ValType::F64, ValType::F64],
            results: vec![ValType::F64],
        }),
        ("acosh", ValType::F64, 1) => Some(WasmImport {
            field_name: "_acosh".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        ("asinh", ValType::F64, 1) => Some(WasmImport {
            field_name: "_asinh".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        ("atanh", ValType::F64, 1) => Some(WasmImport {
            field_name: "_atanh".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        ("cosh", ValType::F64, 1) => Some(WasmImport {
            field_name: "_cosh".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        ("sinh", ValType::F64, 1) => Some(WasmImport {
            field_name: "_sinh".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        ("tanh", ValType::F64, 1) => Some(WasmImport {
            field_name: "_tanh".to_owned(),
            params: vec![ValType::F64],
            results: vec![ValType::F64],
        }),
        _ => None,
    };
    Ok(import)
}

/// Collects generic foreign-function import descriptors from prototype-only FIR
/// declarations stored in the module globals block.
fn collect_foreign_fun_imports(
    store: &FirStore,
    globals: FirId,
    options: &WasmOptions,
) -> Result<HashMap<String, WasmImport>, WasmBackendError> {
    let FirMatch::Block(items) = match_fir(store, globals) else {
        return Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedModuleShape,
            "WASM backend expects the globals section to be a FIR Block",
        ));
    };
    let mut imports = HashMap::new();
    for item in items {
        let FirMatch::DeclareFun {
            name,
            typ: FirType::Fun { args, ret },
            body: None,
            ..
        } = match_fir(store, item)
        else {
            continue;
        };

        if FirMathOp::from_symbol(&name).is_some()
            || name == "abs"
            || name == "max_i"
            || name == "min_i"
        {
            continue;
        }

        let params = args
            .iter()
            .map(|typ| wasm_val_type_for_fir(typ, options))
            .collect::<Result<Vec<_>, _>>()?;
        let results = if matches!(ret.as_ref(), FirType::Void) {
            Vec::new()
        } else {
            vec![wasm_val_type_for_fir(ret.as_ref(), options)?]
        };
        imports.insert(
            name.clone(),
            WasmImport {
                field_name: name,
                params,
                results,
            },
        );
    }
    Ok(imports)
}

/// Converts one FIR scalar type to its emitted WASM stack type.
fn wasm_val_type_for_fir(
    typ: &FirType,
    options: &WasmOptions,
) -> Result<ValType, WasmBackendError> {
    match typ {
        FirType::Int32 | FirType::Bool | FirType::Ptr(_) | FirType::Obj | FirType::Sound => {
            Ok(ValType::I32)
        }
        FirType::Int64 => Ok(ValType::I64),
        FirType::Float32 => Ok(ValType::F32),
        FirType::Float64 => Ok(ValType::F64),
        FirType::FaustFloat => Ok(if options.double_precision {
            ValType::F64
        } else {
            ValType::F32
        }),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported FIR type in WASM subset: {other:?}"),
        )),
    }
}

/// Returns the byte width of one scalar/table element type.
fn elem_size_bytes(typ: &FirType, options: &WasmOptions) -> Result<i32, WasmBackendError> {
    match wasm_val_type_for_fir(typ, options)? {
        ValType::I32 | ValType::F32 => Ok(4),
        ValType::I64 | ValType::F64 => Ok(8),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported element type width in WASM subset: {other:?}"),
        )),
    }
}

/// Extracts the pointee type from a stack-local pointer alias.
fn stack_alias_pointee(typ: &FirType) -> Result<FirType, WasmBackendError> {
    match typ {
        FirType::Ptr(inner) => Ok((**inner).clone()),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("expected stack alias pointer type, got {other:?}"),
        )),
    }
}

/// Returns the canonical WASM local index for function-argument tables.
fn fun_arg_local_index(name: &str) -> u32 {
    match name {
        "inputs" => 2,
        "outputs" => 3,
        other => panic!("unexpected function-arg table local `{other}`"),
    }
}

/// Returns the scalar load instruction matching one FIR type.
fn load_instruction_for_type(
    typ: &FirType,
    options: &WasmOptions,
) -> Result<Instruction<'static>, WasmBackendError> {
    load_instruction_for_valtype(wasm_val_type_for_fir(typ, options)?)
}

/// Returns the scalar store instruction matching one FIR type.
fn store_instruction_for_type(
    typ: &FirType,
    options: &WasmOptions,
) -> Result<Instruction<'static>, WasmBackendError> {
    store_instruction_for_valtype(wasm_val_type_for_fir(typ, options)?)
}

/// Returns the scalar load instruction for one WASM value type.
fn load_instruction_for_valtype(typ: ValType) -> Result<Instruction<'static>, WasmBackendError> {
    match typ {
        ValType::I32 => Ok(Instruction::I32Load(memarg(0))),
        ValType::I64 => Ok(Instruction::I64Load(memarg(0))),
        ValType::F32 => Ok(Instruction::F32Load(memarg(0))),
        ValType::F64 => Ok(Instruction::F64Load(memarg(0))),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported load type in WASM subset: {other:?}"),
        )),
    }
}

/// Returns the scalar store instruction for one WASM value type.
fn store_instruction_for_valtype(typ: ValType) -> Result<Instruction<'static>, WasmBackendError> {
    match typ {
        ValType::I32 => Ok(Instruction::I32Store(memarg(0))),
        ValType::I64 => Ok(Instruction::I64Store(memarg(0))),
        ValType::F32 => Ok(Instruction::F32Store(memarg(0))),
        ValType::F64 => Ok(Instruction::F64Store(memarg(0))),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported store type in WASM subset: {other:?}"),
        )),
    }
}

/// Converts one computed field layout entry to its concrete storage type.
fn wasm_val_type_for_field(field: &FieldLayout) -> ValType {
    match field.typ {
        layout::WasmValType::I32 => ValType::I32,
        layout::WasmValType::F32 => ValType::F32,
        layout::WasmValType::F64 => ValType::F64,
    }
}

/// Reconstructs the FIR-side semantic type represented by one storage field.
///
/// This is mainly used when an expression must be cast between semantic FIR
/// types and the physical storage type chosen by the runtime layout.
fn field_fir_type(field: &FieldLayout, options: &WasmOptions) -> FirType {
    match field.typ {
        layout::WasmValType::I32 => FirType::Int32,
        layout::WasmValType::F32 => {
            if options.double_precision {
                FirType::Float32
            } else {
                FirType::FaustFloat
            }
        }
        layout::WasmValType::F64 => FirType::Float64,
    }
}

/// Returns the arithmetic/comparison opcode for one FIR binary operator.
fn binop_instruction(
    op: fir::FirBinOp,
    typ: &FirType,
    options: &WasmOptions,
) -> Result<Instruction<'static>, WasmBackendError> {
    let val_ty = wasm_val_type_for_fir(typ, options)?;
    match (op, val_ty) {
        (fir::FirBinOp::Add, ValType::I32) => Ok(Instruction::I32Add),
        (fir::FirBinOp::Sub, ValType::I32) => Ok(Instruction::I32Sub),
        (fir::FirBinOp::Mul, ValType::I32) => Ok(Instruction::I32Mul),
        (fir::FirBinOp::Div, ValType::I32) => Ok(Instruction::I32DivS),
        (fir::FirBinOp::Rem, ValType::I32) => Ok(Instruction::I32RemS),
        (fir::FirBinOp::And, ValType::I32) => Ok(Instruction::I32And),
        (fir::FirBinOp::Or, ValType::I32) => Ok(Instruction::I32Or),
        (fir::FirBinOp::Xor, ValType::I32) => Ok(Instruction::I32Xor),
        (fir::FirBinOp::Lsh, ValType::I32) => Ok(Instruction::I32Shl),
        (fir::FirBinOp::ARsh, ValType::I32) => Ok(Instruction::I32ShrS),
        (fir::FirBinOp::LRsh, ValType::I32) => Ok(Instruction::I32ShrU),
        (fir::FirBinOp::Eq, ValType::I32) => Ok(Instruction::I32Eq),
        (fir::FirBinOp::Ne, ValType::I32) => Ok(Instruction::I32Ne),
        (fir::FirBinOp::Lt, ValType::I32) => Ok(Instruction::I32LtS),
        (fir::FirBinOp::Le, ValType::I32) => Ok(Instruction::I32LeS),
        (fir::FirBinOp::Gt, ValType::I32) => Ok(Instruction::I32GtS),
        (fir::FirBinOp::Ge, ValType::I32) => Ok(Instruction::I32GeS),
        (fir::FirBinOp::Add, ValType::F32) => Ok(Instruction::F32Add),
        (fir::FirBinOp::Sub, ValType::F32) => Ok(Instruction::F32Sub),
        (fir::FirBinOp::Mul, ValType::F32) => Ok(Instruction::F32Mul),
        (fir::FirBinOp::Div, ValType::F32) => Ok(Instruction::F32Div),
        (fir::FirBinOp::Eq, ValType::F32) => Ok(Instruction::F32Eq),
        (fir::FirBinOp::Ne, ValType::F32) => Ok(Instruction::F32Ne),
        (fir::FirBinOp::Lt, ValType::F32) => Ok(Instruction::F32Lt),
        (fir::FirBinOp::Le, ValType::F32) => Ok(Instruction::F32Le),
        (fir::FirBinOp::Gt, ValType::F32) => Ok(Instruction::F32Gt),
        (fir::FirBinOp::Ge, ValType::F32) => Ok(Instruction::F32Ge),
        (fir::FirBinOp::Add, ValType::F64) => Ok(Instruction::F64Add),
        (fir::FirBinOp::Sub, ValType::F64) => Ok(Instruction::F64Sub),
        (fir::FirBinOp::Mul, ValType::F64) => Ok(Instruction::F64Mul),
        (fir::FirBinOp::Div, ValType::F64) => Ok(Instruction::F64Div),
        (fir::FirBinOp::Eq, ValType::F64) => Ok(Instruction::F64Eq),
        (fir::FirBinOp::Ne, ValType::F64) => Ok(Instruction::F64Ne),
        (fir::FirBinOp::Lt, ValType::F64) => Ok(Instruction::F64Lt),
        (fir::FirBinOp::Le, ValType::F64) => Ok(Instruction::F64Le),
        (fir::FirBinOp::Gt, ValType::F64) => Ok(Instruction::F64Gt),
        (fir::FirBinOp::Ge, ValType::F64) => Ok(Instruction::F64Ge),
        _ => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported WASM binop in compute subset: {op:?} / {val_ty:?}"),
        )),
    }
}

/// Returns `true` when the FIR binop is a comparison producing a condition.
fn is_comparison_binop(op: fir::FirBinOp) -> bool {
    matches!(
        op,
        fir::FirBinOp::Eq
            | fir::FirBinOp::Ne
            | fir::FirBinOp::Lt
            | fir::FirBinOp::Le
            | fir::FirBinOp::Gt
            | fir::FirBinOp::Ge
    )
}

/// Returns the equality opcode used by switch-case comparisons.
fn switch_eq_instruction(typ: &FirType) -> Result<Instruction<'static>, WasmBackendError> {
    match typ {
        FirType::Int32 | FirType::Bool => Ok(Instruction::I32Eq),
        FirType::Int64 => Ok(Instruction::I64Eq),
        other => Err(WasmBackendError::new(
            WasmBackendErrorCode::UnsupportedFirNode,
            format!("unsupported switch condition type in WASM subset: {other:?}"),
        )),
    }
}

/// Emits one switch-case constant literal onto the operand stack.
fn emit_switch_case_const(
    value: i64,
    typ: &FirType,
    function: &mut Function,
) -> Result<(), WasmBackendError> {
    match typ {
        FirType::Int32 | FirType::Bool => {
            function.instruction(&Instruction::I32Const(value as i32))
        }
        FirType::Int64 => function.instruction(&Instruction::I64Const(value)),
        other => {
            return Err(WasmBackendError::new(
                WasmBackendErrorCode::UnsupportedFirNode,
                format!("unsupported switch constant type in WASM subset: {other:?}"),
            ));
        }
    };
    Ok(())
}
