//! C backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! - `compiler/generator/c/c_code_container.cpp`
//! - `compiler/generator/c/c_instructions.hh`
//!
//! # Current slice
//! - Module-first emission from FIR `Module`.
//! - C API style output (`typedef struct`, `new/delete/init/buildUI/compute`).
//! - `compute` emits a sample loop and writes signal outputs to `outputs[]`.
//!
//! # Output contract
//! - Emits C header-style unit with include guard + `extern "C"` section.
//! - Emits `typedef struct { ... } <name>;` as DSP state container.
//! - Emits Faust C-style exported functions:
//!   `new*`, `delete*`, `metadata*`, `getNum*`, `init*`, `buildUserInterface*`,
//!   `compute*`.
//! - `instanceConstants*` always writes `dsp->fSampleRate = sample_rate` before
//!   section body statements, keeping lifecycle parity with Faust C++ init flow.
//! - Emits `compute*(..., int count, FAUSTFLOAT** RESTRICT, FAUSTFLOAT** RESTRICT)`
//!   with a per-sample loop and channel writes.
//!
//! # Limitations
//! Unsupported FIR nodes currently fail fast with `FRS-CGEN-C-0003`.

use std::fmt::Write as _;

use crate::backends::codegen_error::{BackendError, CodegenErrorCode as BackendErrorCode};
use fir::{AccessType, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

use crate::backends::c_family::{self, CFamilySyntax, EmitMode, StructInit, TableInit};
use crate::backends::faust_api;
use crate::memory_layout::{
    AllocationPhase, Mem0Analysis, Mem0AnalysisOptions, MemoryLayoutFlavor, MemoryManagerMode,
    MemoryRole, MemoryScope, MemoryZone, analyze_effective_mem0,
};

/// Canonical callback-table header embedded in self-contained `-mem0` C output.
/// `include_str!` makes the installed header the single textual authority used
/// by generated C and by the Rust ABI layout tests in `ffi-common`.
const FAUST_MEMORY_MANAGER_HEADER: &str =
    include_str!("../../../../ffi-common/include/faust-memory-manager.h");

pub const BACKEND_NAME: &str = "c";

/// C spellings for the shared C-family emission core.
const SYNTAX: CFamilySyntax = CFamilySyntax {
    bool_type: "int",
    ui_type: "UIGlue*",
    meta_type: "MetaGlue*",
    static_table_keywords: "static const",
    bool_true: "1",
    bool_false: "0",
    null_value: "NULL",
    ui_glue_arg: "ui_interface->uiInterface, ",
    ui_glue_solo: "ui_interface->uiInterface",
    faustfloat_cast_open: "(FAUSTFLOAT)",
    faustfloat_cast_close: "",
    switch_default_break: true,
    bitcast_open: "*((",
    bitcast_mid: "*)&",
    bitcast_close: ")",
};

/// C backend options for module-first emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct COptions {
    /// Custom memory-manager layout selected for generated state.
    ///
    /// Source provenance: Faust C++ `global::gMemoryManager` and
    /// `CCodeContainer` memory-manager branches. The typed per-request value is
    /// an `adapted` replacement for mutable global state.
    pub memory_manager_mode: MemoryManagerMode,
    /// Whether `FAUSTFLOAT`-derived layout entries use double precision.
    ///
    /// The compiler pipeline sets this from the effective source real type;
    /// direct FIR callers retain single precision by default.
    pub double_precision: bool,
    /// Optional C struct name override for the FIR module name.
    pub class_name: Option<String>,
    /// C spelling used for FIR `Quad` values.
    ///
    /// Kept configurable because C targets can differ on extended precision
    /// support and naming.
    pub quad_type_name: String,
    /// C spelling used for FIR `FixedPoint` values.
    ///
    /// Kept configurable because fixed-point backends may require a project
    /// specific typedef or include.
    pub fixed_type_name: String,
    /// Compilation options string printed in the generated-file header.
    ///
    /// `None` falls back to a minimal `-lang c` line for callers (mostly
    /// tests) that do not thread the real CLI flags through.
    pub compile_options: Option<String>,
    /// Source-level DSP name reported in the generated banner and metadata
    /// callback. This is independent from [`Self::class_name`].
    pub metadata_name: Option<String>,
    /// Source basename reported by the generated metadata callback.
    pub metadata_filename: Option<String>,
    /// Non-identity compilation metadata replayed by `metadata()`.
    pub metadata_entries: Vec<(String, String)>,
}

impl Default for COptions {
    /// Default backend options.
    ///
    /// Uses `class_name = Some("mydsp")` to match the current workspace
    /// convention for deterministic generated type names.
    fn default() -> Self {
        Self {
            memory_manager_mode: MemoryManagerMode::None,
            double_precision: false,
            class_name: Some("mydsp".to_owned()),
            quad_type_name: "quad".to_owned(),
            fixed_type_name: "fixed".to_owned(),
            compile_options: None,
            metadata_name: None,
            metadata_filename: None,
            metadata_entries: Vec::new(),
        }
    }
}

/// Stable machine-readable error codes for the C backend emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// Root FIR node is not a module (`FirMatch::Module`).
    RootNotModule,
    /// One module section is not a FIR block.
    InvalidModuleSection,
    /// The C emitter slice does not yet support this FIR node.
    UnsupportedNode,
    /// Canonical `mem0` analysis rejected the effective C FIR or target ABI.
    MemoryLayout,
}

impl CodegenErrorCode {
    /// Stable textual code used in diagnostics and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-C-0001",
            Self::InvalidModuleSection => "FRS-CGEN-C-0002",
            Self::UnsupportedNode => "FRS-CGEN-C-0003",
            Self::MemoryLayout => "FRS-CGEN-C-0004",
        }
    }
}

impl BackendErrorCode for CodegenErrorCode {
    fn as_str(&self) -> &'static str {
        Self::as_str(*self)
    }
}

/// One emission failure of this backend.
///
/// Alias of the shared [`crate::backends::codegen_error::BackendError`]
/// carrier; only the code enum above is specific to this backend.
pub type CodegenError = BackendError<CodegenErrorCode>;

/// Decoded FIR module header used to keep emission helpers independent from the
/// exact `FirMatch::Module` shape.
///
/// This is an internal normalization step, not a long-lived IR: helpers treat
/// these ids as section roots that must still be re-decoded before emission.
#[derive(Debug, Clone)]
struct ModuleView {
    name: String,
    dsp_struct: FirId,
    globals: FirId,
    functions: FirId,
    num_inputs: usize,
    num_outputs: usize,
    static_decls: FirId,
    sub_modules: FirId,
}

/// Normalized function declaration extracted from FIR before textual emission.
///
/// Keeping an owned view here avoids repeatedly borrowing through the FIR store
/// while the emitter walks several lifecycle/API synthesis passes.
#[derive(Debug, Clone)]
struct DeclareFunView {
    name: String,
    typ: FirType,
    named_args: Vec<NamedType>,
    /// `None` when this is a prototype-only declaration (no body).
    body: Option<FirId>,
}

#[must_use]
/// Returns the stable backend identifier (`"c"`).
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

/// Generates C code from a FIR module root.
///
/// Generated code follows Faust C backend conventions:
/// - header guard + `extern "C"` block
/// - `typedef struct { ... } <class_name>;`
/// - C API entrypoints:
///   `new*`, `delete*`, `metadata*`, `init*`, `buildUserInterface*`, `compute*`
/// - `compute*` signature:
///   `(<class>* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs)`
///   with a per-sample loop and output writes.
///
/// # Errors
/// Returns [`CodegenError`] if the root is not a FIR module or if the module
/// contains unsupported FIR nodes for the current C emitter slice.
///
/// # Options behavior
/// - `class_name`: overrides FIR module name.
/// - input/output arity is taken from FIR module metadata.
pub fn generate_c_module(
    store: &FirStore,
    module: FirId,
    options: &COptions,
) -> Result<String, CodegenError> {
    let module_id = module;
    let module = decode_module(store, module_id)?;
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(module.name.as_str())
        .to_owned();
    let effective_options = options.clone();
    let mem0 = if options.memory_manager_mode.is_mem0() {
        Some(
            analyze_effective_mem0(
                store,
                module_id,
                &Mem0AnalysisOptions::native(MemoryLayoutFlavor::C, options.double_precision),
            )
            .map_err(|error| {
                CodegenError::new(
                    CodegenErrorCode::MemoryLayout,
                    format!("cannot analyze -mem0 layout: {error}"),
                )
            })?,
        )
    } else {
        None
    };

    let declared_functions = collect_module_functions(store, module.functions)?;
    let struct_inits = collect_struct_initializers(store, module.dsp_struct, module.globals)?;
    let table_inits = collect_table_initializers(store, module.dsp_struct, module.globals)?;
    let mut out = String::new();
    emit_c_header(
        &mut out,
        &class_name,
        effective_options
            .metadata_name
            .as_deref()
            .unwrap_or(&module.name),
        effective_options.compile_options.as_deref(),
        mem0.is_some(),
    );
    if mem0.is_some() {
        let _ = writeln!(out, "static faust_memory_manager* fClassManager = NULL;");
        let _ = writeln!(out, "static int fClassSampleRate = 0;");
        let _ = writeln!(out, "static size_t fLiveInstances = 0;");
        let _ = writeln!(out);
    }
    emit_static_tables(store, &mut out, &effective_options, module.static_decls)?;
    let _ = writeln!(out);
    emit_sub_modules(store, &mut out, &effective_options, module.sub_modules)?;
    emit_struct_definition(
        store,
        &mut out,
        &effective_options,
        &class_name,
        module.dsp_struct,
        module.globals,
    )?;
    emit_c_api(
        store,
        &mut out,
        CApiEmitInput {
            options: &effective_options,
            class_name: &class_name,
            num_inputs: module.num_inputs,
            num_outputs: module.num_outputs,
            declared_functions: &declared_functions,
            struct_inits: &struct_inits,
            table_inits: &table_inits,
            mem0: mem0.as_ref(),
        },
    )?;
    emit_c_footer(&mut out);
    Ok(out)
}

/// Emits the prologue/header guard and platform macros for the generated unit.
fn emit_c_header(
    out: &mut String,
    class_name: &str,
    module_name: &str,
    compile_options: Option<&str>,
    mem0: bool,
) {
    let _ = writeln!(
        out,
        "/* ------------------------------------------------------------"
    );
    let _ = writeln!(out, "name: \"{module_name}\"");
    let _ = writeln!(
        out,
        "Code generated with Faust {} (https://faust.grame.fr)",
        crate::VERSION
    );
    let _ = writeln!(
        out,
        "Compilation options: {}",
        compile_options.unwrap_or("-lang c")
    );
    let _ = writeln!(
        out,
        "------------------------------------------------------------ */"
    );
    let _ = writeln!(out);
    let guard = format!("__{}_H__", class_name);
    let _ = writeln!(out, "#ifndef  {guard}");
    let _ = writeln!(out, "#define  {guard}");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifndef FAUSTFLOAT");
    let _ = writeln!(out, "#define FAUSTFLOAT float");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#if defined(__GNUC__) || defined(__clang__)");
    let _ = writeln!(out, "#define FAUST_UNUSED __attribute__((unused))");
    let _ = writeln!(out, "#else");
    let _ = writeln!(out, "#define FAUST_UNUSED");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifdef __cplusplus");
    let _ = writeln!(out, "extern \"C\" {{");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#if defined(_WIN32)");
    let _ = writeln!(out, "#define RESTRICT __restrict");
    let _ = writeln!(out, "#else");
    let _ = writeln!(out, "#define RESTRICT __restrict__");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#include <math.h>");
    let _ = writeln!(out, "#include <stdint.h>");
    let _ = writeln!(out, "#include <stdlib.h>");
    let _ = writeln!(out, "#include <string.h>");
    let _ = writeln!(out);
    if mem0 {
        let _ = writeln!(out, "{FAUST_MEMORY_MANAGER_HEADER}");
        let _ = writeln!(
            out,
            "static int faustMemoryManagerCompatible(const faust_memory_manager* manager) {{"
        );
        let _ = writeln!(out, "    return manager != NULL");
        let _ = writeln!(
            out,
            "        && manager->abi_version == FAUST_MEMORY_MANAGER_ABI_VERSION"
        );
        let _ = writeln!(
            out,
            "        && manager->struct_size >= sizeof(faust_memory_manager)"
        );
        let _ = writeln!(
            out,
            "        && manager->begin != NULL && manager->info != NULL && manager->end != NULL"
        );
        let _ = writeln!(
            out,
            "        && manager->allocate != NULL && manager->destroy != NULL;"
        );
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "#ifndef FAUSTCLASS");
    let _ = writeln!(out, "#define FAUSTCLASS {class_name}");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifdef __APPLE__");
    let _ = writeln!(out, "#define exp10f __exp10f");
    let _ = writeln!(out, "#define exp10 __exp10");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "static inline FAUST_UNUSED int faustmini(int a, int b) {{ return (a < b) ? a : b; }}"
    );
    let _ = writeln!(
        out,
        "static inline FAUST_UNUSED int faustmaxi(int a, int b) {{ return (a > b) ? a : b; }}"
    );
    // Two's-complement wrapping integer arithmetic: signed overflow is UB in
    // C, so add/sub/mul run on `uint32_t` (defined modulo 2^32) and convert
    // back — the semantics Faust-generated code (the noise LCG in
    // particular) has always assumed.
    let _ = writeln!(
        out,
        "static inline FAUST_UNUSED int faust_wrap_add(int a, int b) {{ return (int)(((uint32_t)a) + ((uint32_t)b)); }}"
    );
    let _ = writeln!(
        out,
        "static inline FAUST_UNUSED int faust_wrap_sub(int a, int b) {{ return (int)(((uint32_t)a) - ((uint32_t)b)); }}"
    );
    let _ = writeln!(
        out,
        "static inline FAUST_UNUSED int faust_wrap_mul(int a, int b) {{ return (int)(((uint32_t)a) * ((uint32_t)b)); }}"
    );
    let _ = writeln!(out);
}

/// Emits the closing `extern "C"` / include-guard footer.
fn emit_c_footer(out: &mut String) {
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifdef __cplusplus");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#endif");
}

/// Emits the DSP state `struct` definition from FIR state declarations.
fn emit_struct_definition(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    class_name: &str,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<(), CodegenError> {
    let has_sample_rate_field = block_declares_var(store, dsp_struct, "fSampleRate")
        || block_declares_var(store, globals, "fSampleRate");
    let _ = writeln!(out, "typedef struct {{");
    if options.memory_manager_mode.is_mem0() {
        let _ = writeln!(out, "    faust_memory_manager* fOwnerManager;");
    }
    emit_struct_fields(store, out, options, dsp_struct, true)?;
    emit_struct_fields(store, out, options, globals, true)?;
    if !has_sample_rate_field {
        let _ = writeln!(out, "    int fSampleRate;");
    }
    let _ = writeln!(out, "}} {class_name};");
    let _ = writeln!(out);
    Ok(())
}

/// Emits every generated-table sub-container as a struct plus free functions.
///
/// C++ parity: the same `CodeContainer` the C++ backend emits as a class
/// becomes, in C, a `typedef struct` holding the generator's state and a pair
/// of `static` functions taking it as their first parameter. State access
/// needs no special handling: the generator's fields are `AccessType::Struct`,
/// which `emit_var_ref` already renders as `dsp->field`, and `dsp` is exactly
/// what these functions receive.
///
/// Allocation goes through `calloc`/`free` rather than `new`/`delete`, which is
/// why a module with a generated table pulls in `<stdlib.h>`.
///
/// Sub-modules are emitted deepest-first, so a nested generator's struct and
/// functions precede the ones that call them.
fn emit_sub_modules(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    sub_modules: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, sub_modules) else {
        return Ok(());
    };
    for item in items {
        let FirMatch::SubModule {
            name,
            dsp_struct,
            static_decls,
            globals,
            functions,
            sub_modules: nested,
            ..
        } = match_fir(store, item)
        else {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                format!("sub_modules holds a non-SubModule node {}", item.as_u32()),
            ));
        };

        emit_sub_modules(store, out, options, nested)?;
        emit_static_tables(store, out, options, static_decls)?;

        let _ = writeln!(out, "typedef struct {{");
        if options.memory_manager_mode.is_mem0() {
            let _ = writeln!(out, "    faust_memory_manager* fOwnerManager;");
        }
        emit_struct_fields(store, out, options, dsp_struct, false)?;
        emit_struct_fields(store, out, options, globals, false)?;
        let _ = writeln!(out, "}} {name};");
        let _ = writeln!(out);
        if options.memory_manager_mode.is_mem0() {
            let _ = writeln!(
                out,
                "static {name}* new{name}(faust_memory_manager* manager) {{"
            );
            let _ = writeln!(out, "    void* storage;");
            let _ = writeln!(
                out,
                "    if (!faustMemoryManagerCompatible(manager)) return NULL;"
            );
            let _ = writeln!(
                out,
                "    storage = manager->allocate(manager->context, sizeof({name}), _Alignof({name}));"
            );
            let _ = writeln!(
                out,
                "    if (storage == NULL || ((uintptr_t)storage % _Alignof({name})) != 0) {{"
            );
            let _ = writeln!(
                out,
                "        if (storage != NULL) manager->destroy(manager->context, storage, sizeof({name}), _Alignof({name}));"
            );
            let _ = writeln!(out, "        return NULL;");
            let _ = writeln!(out, "    }}");
            let _ = writeln!(out, "    memset(storage, 0, sizeof({name}));");
            let _ = writeln!(out, "    (({name}*)storage)->fOwnerManager = manager;");
            let _ = writeln!(out, "    return ({name}*)storage;");
            let _ = writeln!(out, "}}");
            let _ = writeln!(out, "static void delete{name}({name}* dsp) {{");
            let _ = writeln!(out, "    faust_memory_manager* manager;");
            let _ = writeln!(out, "    if (dsp == NULL) return;");
            let _ = writeln!(out, "    manager = dsp->fOwnerManager;");
            let _ = writeln!(
                out,
                "    manager->destroy(manager->context, dsp, sizeof({name}), _Alignof({name}));"
            );
            let _ = writeln!(out, "}}");
        } else {
            let _ = writeln!(
                out,
                "static {name}* new{name}() {{ return ({name}*)calloc(1, sizeof({name})); }}"
            );
            let _ = writeln!(
                out,
                "static void delete{name}({name}* dsp) {{ free(dsp); }}"
            );
        }
        let _ = writeln!(out);
        // Arity getters exist for reference parity; a generator is 0-input /
        // 1-output by construction.
        let _ = writeln!(out, "int getNumInputs{name}({name}* RESTRICT dsp) {{");
        let _ = writeln!(out, "    return 0;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out, "int getNumOutputs{name}({name}* RESTRICT dsp) {{");
        let _ = writeln!(out, "    return 1;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);

        if let FirMatch::Block(fns) = match_fir(store, functions) {
            for f in fns {
                let FirMatch::DeclareFun {
                    name: fun_name,
                    args,
                    body: Some(body),
                    ..
                } = match_fir(store, f)
                else {
                    continue;
                };
                let params: Vec<String> = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        if index == 0 {
                            format!("{name}* dsp")
                        } else {
                            emit_named_type(&arg.typ, &arg.name, options)
                        }
                    })
                    .collect();
                let _ = writeln!(out, "static void {fun_name}({}) {{", params.join(", "));
                emit_block(store, out, options, body, 1)?;
                let _ = writeln!(out, "}}");
                let _ = writeln!(out);
            }
        }
    }
    Ok(())
}

/// Returns the `(variable, sub-module)` pairs allocated inside one block.
///
/// The FIR carries the allocation but not the release, which is bound to how
/// each language allocates; C pairs `calloc` with `free` once the fill has run.
fn allocated_sub_containers(store: &FirStore, block: FirId) -> Vec<(String, String)> {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| match match_fir(store, item) {
            FirMatch::DeclareVar {
                name,
                init: Some(init),
                ..
            } => match match_fir(store, init) {
                FirMatch::NewDsp { name: sub, .. } => Some((name, sub)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Emits one FIR block worth of struct fields.
///
/// Only `DeclareVar` and `DeclareTable` entries contribute concrete state
/// fields; helper declarations are ignored.
fn emit_struct_fields(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    block_id: FirId,
    externalize_mem0_arrays: bool,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block_id) else {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidModuleSection,
            format!(
                "struct section must be a FIR block, got {:?} at node {}",
                match_fir(store, block_id),
                block_id.as_u32()
            ),
        ));
    };

    for item in items {
        match match_fir(store, item) {
            FirMatch::DeclareVar { name, typ, .. } => {
                if options.memory_manager_mode.is_mem0()
                    && externalize_mem0_arrays
                    && let FirType::Array(elem, _) | FirType::Vector(elem, _) = typ
                {
                    let _ = write!(out, "    {}* {name}", emit_type(&elem, options));
                } else {
                    let _ = write!(out, "    {}", emit_named_type(&typ, &name, options));
                }
                let _ = writeln!(out, ";");
            }
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => {
                if options.memory_manager_mode.is_mem0() && externalize_mem0_arrays {
                    let _ = writeln!(out, "    {}* {name};", emit_type(&elem_type, options));
                } else {
                    let _ = writeln!(
                        out,
                        "    {} {}[{}];",
                        emit_type(&elem_type, options),
                        name,
                        values.len()
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Returns `true` when `block_id` declares a variable named `name`.
fn block_declares_var(store: &FirStore, block_id: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block_id) else {
        return false;
    };
    items.iter().any(|id| {
        matches!(
            match_fir(store, *id),
            FirMatch::DeclareVar { name: var_name, .. } if var_name == name
        )
    })
}

/// Returns `true` when `block_id` stores to a variable named `name`.
fn block_stores_var(store: &FirStore, block_id: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block_id) else {
        return false;
    };
    items.iter().any(|id| {
        matches!(
            match_fir(store, *id),
            FirMatch::StoreVar { name: var_name, .. } if var_name == name
        )
    })
}

/// Aggregated inputs required to synthesize the public Faust C API surface.
struct CApiEmitInput<'a> {
    options: &'a COptions,
    class_name: &'a str,
    num_inputs: usize,
    num_outputs: usize,
    declared_functions: &'a [DeclareFunView],
    struct_inits: &'a [StructInit],
    table_inits: &'a [TableInit],
    /// Retained canonical snapshot; `None` preserves the ordinary C ABI.
    mem0: Option<&'a Mem0Analysis>,
}

/// Emits the public Faust C API wrappers around the lowered FIR sections.
///
/// This function is where the module-first FIR contract is adapted back to the
/// legacy C backend surface: constructor/destructor functions, lifecycle hooks,
/// UI builder, and `compute`.
fn emit_c_api(
    store: &FirStore,
    out: &mut String,
    spec: CApiEmitInput<'_>,
) -> Result<(), CodegenError> {
    let CApiEmitInput {
        options,
        class_name,
        num_inputs,
        num_outputs,
        declared_functions,
        struct_inits,
        table_inits,
        mem0,
    } = spec;
    let names: Vec<&str> = declared_functions.iter().map(|f| f.name.as_str()).collect();

    if let Some(analysis) = mem0 {
        emit_mem0_instance_api(out, class_name, analysis);
        emit_mem0_memory_info(out, class_name, analysis);
    } else {
        let _ = writeln!(out, "{class_name}* new{class_name}() {{");
        let _ = writeln!(
            out,
            "    {class_name}* dsp = ({class_name}*)calloc(1, sizeof({class_name}));"
        );
        let _ = writeln!(out, "    return dsp;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);

        let _ = writeln!(out, "void delete{class_name}({class_name}* dsp) {{");
        let _ = writeln!(out, "    free(dsp);");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    emit_metadata(store, out, options, class_name, declared_functions)?;

    let _ = writeln!(
        out,
        "int getSampleRate{class_name}({class_name}* RESTRICT dsp) {{"
    );
    let _ = writeln!(out, "    return dsp->fSampleRate;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "int getNumInputs{class_name}({class_name}* RESTRICT dsp) {{"
    );
    let _ = writeln!(out, "    (void)dsp;");
    let _ = writeln!(out, "    return {};", num_inputs);
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "int getNumOutputs{class_name}({class_name}* RESTRICT dsp) {{"
    );
    let _ = writeln!(out, "    (void)dsp;");
    let _ = writeln!(out, "    return {};", num_outputs);
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    if let Some(analysis) = mem0 {
        emit_mem0_class_table_destroy(out, class_name, analysis);
        emit_mem0_class_init_prefix(out, class_name, analysis);
    } else {
        let _ = writeln!(out, "void classInit{class_name}(int sample_rate) {{");
    }
    if let Some(static_init) = declared_functions.iter().find(|f| f.name == "staticInit")
        && let Some(body) = static_init.body
    {
        emit_block(store, out, options, body, 1)?;
        for (var, sub) in allocated_sub_containers(store, body) {
            let _ = writeln!(out, "    delete{sub}({var});");
        }
    } else {
        let _ = writeln!(out, "    (void)sample_rate;");
    }
    if let Some(analysis) = mem0 {
        emit_mem0_class_init_suffix(out, class_name, analysis);
    } else {
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceConstants")
    {
        emit_named_fun(store, out, options, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "void instanceConstants{class_name}({class_name}* dsp, int sample_rate) {{"
        );
        let _ = writeln!(out, "    dsp->fSampleRate = sample_rate;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceResetUserInterface")
    {
        emit_named_fun(store, out, options, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "void instanceResetUserInterface{class_name}({class_name}* dsp) {{"
        );
        if struct_inits.is_empty() && table_inits.is_empty() {
            let _ = writeln!(out, "    (void)dsp;");
        } else {
            for init in struct_inits {
                let value = emit_value(store, options, init.init)?;
                let _ = writeln!(
                    out,
                    "    dsp->{} = ({})({value});",
                    init.name,
                    emit_type(&init.typ, options)
                );
            }
            for init in table_inits {
                for (index, value_id) in init.values.iter().copied().enumerate() {
                    let value = emit_value(store, options, value_id)?;
                    let table_ref = emit_var_ref(&init.name, init.access);
                    let _ = writeln!(
                        out,
                        "    {table_ref}[{index}] = ({})({value});",
                        emit_type(&init.elem_type, options)
                    );
                }
            }
        }
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceClear")
    {
        emit_named_fun(store, out, options, class_name, f)?;
    } else {
        let _ = writeln!(out, "void instanceClear{class_name}({class_name}* dsp) {{");
        let _ = writeln!(out, "    (void)dsp;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    let _ = writeln!(
        out,
        "void instanceInit{class_name}({class_name}* dsp, int sample_rate) {{"
    );
    let _ = writeln!(out, "    instanceConstants{class_name}(dsp, sample_rate);");
    let _ = writeln!(out, "    instanceResetUserInterface{class_name}(dsp);");
    let _ = writeln!(out, "    instanceClear{class_name}(dsp);");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "void init{class_name}({class_name}* dsp, int sample_rate) {{"
    );
    if mem0.is_some() {
        let _ = writeln!(
            out,
            "    classInit{class_name}(dsp->fOwnerManager, sample_rate);"
        );
    } else {
        let _ = writeln!(out, "    classInit{class_name}(sample_rate);");
    }
    let _ = writeln!(out, "    instanceInit{class_name}(dsp, sample_rate);");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "buildUserInterface")
    {
        emit_named_fun(store, out, options, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "void buildUserInterface{class_name}({class_name}* dsp, UIGlue* ui_interface) {{"
        );
        let _ = writeln!(out, "    (void)dsp;");
        let _ = writeln!(out, "    (void)ui_interface;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    // Execution entry points precede the canonical compute (§2.3).
    if let Some(f) = declared_functions.iter().find(|f| f.name == "control") {
        emit_named_fun(store, out, options, class_name, f)?;
    }
    if let Some(f) = declared_functions.iter().find(|f| f.name == "frame") {
        emit_named_fun(store, out, options, class_name, f)?;
    }
    if let Some(f) = declared_functions.iter().find(|f| f.name == "compute") {
        emit_named_fun(store, out, options, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "void compute{class_name}({class_name}* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {{"
        );
        let _ = writeln!(out, "    (void)dsp;");
        let _ = writeln!(out, "    (void)count;");
        let _ = writeln!(out, "    (void)inputs;");
        let _ = writeln!(out, "    (void)outputs;");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
    }

    for f in declared_functions {
        if crate::backends::is_lifecycle_function(&f.name) {
            continue;
        }
        if names.contains(&f.name.as_str()) {
            emit_helper_function(store, out, options, f)?;
        }
    }

    Ok(())
}

/// Emits manager-backed object and instance-buffer allocation for C `-mem0`.
///
/// Source provenance: Faust C++ `CodeContainer::generateMemoryMethods`. The C
/// adaptation is transactional, preserves explicit alignment, captures the
/// creator table, and releases completed allocations in reverse order via the
/// shared `releaseInstance{class_name}` helper — safe to call from a failure
/// midway through `create{class_name}` because `memset` zero-initializes every
/// not-yet-allocated field to `NULL`, so the helper's null guard only ever
/// destroys what actually got allocated.
fn emit_mem0_instance_api(out: &mut String, class_name: &str, analysis: &Mem0Analysis) {
    let buffers = mem0_instance_buffers(analysis);

    let _ = writeln!(
        out,
        "static void releaseInstance{class_name}(faust_memory_manager* manager, {class_name}* dsp) {{"
    );
    for zone in buffers.iter().rev() {
        let _ = writeln!(
            out,
            "    if (dsp->{0} != NULL) manager->destroy(manager->context, dsp->{0}, {1}, {2});",
            zone.name, zone.size_bytes, zone.alignment
        );
    }
    let _ = writeln!(
        out,
        "    manager->destroy(manager->context, dsp, sizeof({class_name}), _Alignof({class_name}));"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "{class_name}* create{class_name}(faust_memory_manager* manager) {{"
    );
    let _ = writeln!(out, "    {class_name}* dsp;");
    let _ = writeln!(out, "    void* storage;");
    let _ = writeln!(
        out,
        "    if (!faustMemoryManagerCompatible(manager)) return NULL;"
    );
    let _ = writeln!(
        out,
        "    storage = manager->allocate(manager->context, sizeof({class_name}), _Alignof({class_name}));"
    );
    let _ = writeln!(
        out,
        "    if (storage == NULL || ((uintptr_t)storage % _Alignof({class_name})) != 0) {{"
    );
    let _ = writeln!(
        out,
        "        if (storage != NULL) manager->destroy(manager->context, storage, sizeof({class_name}), _Alignof({class_name}));"
    );
    let _ = writeln!(out, "        return NULL;");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    memset(storage, 0, sizeof({class_name}));");
    let _ = writeln!(out, "    dsp = ({class_name}*)storage;");
    let _ = writeln!(out, "    dsp->fOwnerManager = manager;");
    for zone in &buffers {
        let _ = writeln!(
            out,
            "    dsp->{0} = manager->allocate(manager->context, {1}, {2});",
            zone.name, zone.size_bytes, zone.alignment
        );
        let _ = writeln!(
            out,
            "    if (dsp->{0} == NULL || ((uintptr_t)dsp->{0} % {1}) != 0) {{",
            zone.name, zone.alignment
        );
        let _ = writeln!(out, "        releaseInstance{class_name}(manager, dsp);");
        let _ = writeln!(out, "        return NULL;");
        let _ = writeln!(out, "    }}");
    }
    let _ = writeln!(out, "    ++fLiveInstances;");
    let _ = writeln!(out, "    return dsp;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    let _ = writeln!(out, "void destroy{class_name}({class_name}* dsp) {{");
    let _ = writeln!(out, "    if (dsp == NULL) return;");
    let _ = writeln!(
        out,
        "    releaseInstance{class_name}(dsp->fOwnerManager, dsp);"
    );
    let _ = writeln!(out, "    --fLiveInstances;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Emits the deterministic manager-description callbacks for runtime zones.
fn emit_mem0_memory_info(out: &mut String, class_name: &str, analysis: &Mem0Analysis) {
    let zones: Vec<_> = analysis
        .memory_layout
        .zones
        .iter()
        .filter(|zone| zone.runtime_allocated)
        .collect();
    let _ = writeln!(
        out,
        "int memoryInfoChecked{class_name}(faust_memory_manager* manager) {{"
    );
    let _ = writeln!(
        out,
        "    if (!faustMemoryManagerCompatible(manager)) return 0;"
    );
    let _ = writeln!(
        out,
        "    manager->begin(manager->context, {});",
        zones.len()
    );
    for zone in zones {
        let size = if zone.role == MemoryRole::DspObject {
            format!("sizeof({class_name})")
        } else {
            zone.size_bytes.to_string()
        };
        let alignment = if zone.role == MemoryRole::DspObject {
            format!("_Alignof({class_name})")
        } else {
            zone.alignment.to_string()
        };
        let _ = writeln!(
            out,
            "    manager->info(manager->context, {}, {}, {}, {size}, {alignment}, {}, {});",
            c_family::string_literal(&zone.name),
            zone.memory_type.c_abi_name(),
            zone.element_count,
            zone.reads,
            zone.writes
        );
    }
    let _ = writeln!(out, "    manager->end(manager->context);");
    let _ = writeln!(out, "    return 1;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(
        out,
        "void memoryInfo{class_name}(faust_memory_manager* manager) {{"
    );
    let _ = writeln!(
        out,
        "    if (!memoryInfoChecked{class_name}(manager)) abort();"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Emits `classDestroyTables{class_name}`, the shared reverse-order release of
/// every class-scope table. Defined ahead of `classInitChecked{class_name}` so
/// C's declare-before-use rule is satisfied; reused by both the allocation
/// failure path in [`emit_mem0_class_init_prefix`] and the public
/// `classDestroyChecked{class_name}` in [`emit_mem0_class_init_suffix`]. Safe
/// to call with only a prefix of tables allocated, since class-scope pointers
/// start `NULL` (static storage duration) until their own `allocate` call runs.
fn emit_mem0_class_table_destroy(out: &mut String, class_name: &str, analysis: &Mem0Analysis) {
    let zones = mem0_class_tables(analysis);
    let _ = writeln!(
        out,
        "static void classDestroyTables{class_name}(faust_memory_manager* manager) {{"
    );
    let _ = writeln!(out, "    (void)manager;");
    for zone in zones.iter().rev() {
        let _ = writeln!(
            out,
            "    if ({0} != NULL) manager->destroy(manager->context, {0}, {1}, {2});",
            zone.name, zone.size_bytes, zone.alignment
        );
        let _ = writeln!(out, "    {} = NULL;", zone.name);
    }
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Opens the checked class-initialization transaction before semantic
/// `staticInit` is emitted by the ordinary lifecycle path.
fn emit_mem0_class_init_prefix(out: &mut String, class_name: &str, analysis: &Mem0Analysis) {
    let zones = mem0_class_tables(analysis);
    let _ = writeln!(
        out,
        "int classInitChecked{class_name}(faust_memory_manager* manager, int sample_rate) {{"
    );
    let _ = writeln!(
        out,
        "    if (!faustMemoryManagerCompatible(manager)) return 0;"
    );
    let _ = writeln!(
        out,
        "    if (fClassManager != NULL) return fClassManager == manager && fClassSampleRate == sample_rate;"
    );
    let _ = writeln!(out, "    fClassManager = manager;");
    let _ = writeln!(out, "    fClassSampleRate = sample_rate;");
    for zone in &zones {
        let _ = writeln!(
            out,
            "    {0} = manager->allocate(manager->context, {1}, {2});",
            zone.name, zone.size_bytes, zone.alignment
        );
        let _ = writeln!(
            out,
            "    if ({0} == NULL || ((uintptr_t){0} % {1}) != 0) {{",
            zone.name, zone.alignment
        );
        let _ = writeln!(out, "        classDestroyTables{class_name}(manager);");
        let _ = writeln!(out, "        fClassManager = NULL;");
        let _ = writeln!(out, "        fClassSampleRate = 0;");
        let _ = writeln!(out, "        return 0;");
        let _ = writeln!(out, "    }}");
    }
}

/// Closes class initialization and emits checked/idempotent class destruction.
fn emit_mem0_class_init_suffix(out: &mut String, class_name: &str, _analysis: &Mem0Analysis) {
    let _ = writeln!(out, "    return 1;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(
        out,
        "void classInit{class_name}(faust_memory_manager* manager, int sample_rate) {{"
    );
    let _ = writeln!(
        out,
        "    if (!classInitChecked{class_name}(manager, sample_rate)) abort();"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "int classDestroyChecked{class_name}(faust_memory_manager* manager) {{"
    );
    let _ = writeln!(out, "    if (fClassManager == NULL) return 1;");
    let _ = writeln!(
        out,
        "    if (manager != fClassManager || fLiveInstances != 0) return 0;"
    );
    let _ = writeln!(out, "    classDestroyTables{class_name}(manager);");
    let _ = writeln!(out, "    fClassManager = NULL;");
    let _ = writeln!(out, "    fClassSampleRate = 0;");
    let _ = writeln!(out, "    return 1;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(
        out,
        "void classDestroy{class_name}(faust_memory_manager* manager) {{"
    );
    let _ = writeln!(
        out,
        "    if (!classDestroyChecked{class_name}(manager)) abort();"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

fn mem0_instance_buffers(analysis: &Mem0Analysis) -> Vec<&MemoryZone> {
    analysis
        .memory_layout
        .zones
        .iter()
        .filter(|zone| {
            zone.runtime_allocated
                && zone.scope == MemoryScope::Instance
                && zone.role == MemoryRole::InstanceBuffer
                && zone.allocation_phase == AllocationPhase::InstanceCreate
        })
        .collect()
}

fn mem0_class_tables(analysis: &Mem0Analysis) -> Vec<&MemoryZone> {
    analysis
        .memory_layout
        .zones
        .iter()
        .filter(|zone| {
            zone.runtime_allocated
                && zone.scope == MemoryScope::Class
                && zone.role == MemoryRole::StaticTable
        })
        .collect()
}

/// Emits the `metadata` function or a canonical default stub.
fn emit_metadata(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    class_name: &str,
    declared_functions: &[DeclareFunView],
) -> Result<(), CodegenError> {
    if let Some(f) = declared_functions.iter().find(|f| f.name == "metadata") {
        emit_named_fun(store, out, options, class_name, f)
    } else {
        let _ = writeln!(out, "void metadata{class_name}(MetaGlue* m) {{");
        let _ = writeln!(
            out,
            "    m->declare(m->metaInterface, \"faust-rs\", \"module-first c backend prototype\");"
        );
        let _ = writeln!(out, "}}");
        let _ = writeln!(out);
        Ok(())
    }
}

/// Emits one named DSP API method using the legacy C wrapper signature.
fn emit_named_fun(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    class_name: &str,
    decl: &DeclareFunView,
) -> Result<(), CodegenError> {
    faust_api::validate_canonical_dsp_api_signature(&decl.name, &decl.typ, &decl.named_args)
        .map_err(|msg| CodegenError::new(CodegenErrorCode::InvalidModuleSection, msg))?;
    let signature = match decl.name.as_str() {
        "metadata" => format!("void metadata{class_name}(MetaGlue* m)"),
        "instanceConstants" => {
            format!("void instanceConstants{class_name}({class_name}* dsp, int sample_rate)")
        }
        "instanceResetUserInterface" => {
            format!("void instanceResetUserInterface{class_name}({class_name}* dsp)")
        }
        "instanceClear" => format!("void instanceClear{class_name}({class_name}* dsp)"),
        "buildUserInterface" => {
            format!("void buildUserInterface{class_name}({class_name}* dsp, UIGlue* ui_interface)")
        }
        "compute" => format!(
            "void compute{class_name}({class_name}* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs)"
        ),
        // Execution-options port §5.1; shapes mirror the pinned reference:
        // `void controlmydsp(mydsp* dsp)` and
        // `void framemydsp(mydsp* dsp, FAUSTFLOAT* RESTRICT inputs, ...)`.
        "control" => format!("void control{class_name}({class_name}* dsp)"),
        "frame" => format!(
            "void frame{class_name}({class_name}* dsp, FAUSTFLOAT* RESTRICT inputs, FAUSTFLOAT* RESTRICT outputs)"
        ),
        _ => format!(
            "{} {}{class_name}({class_name}* dsp)",
            emit_return_type(&decl.typ, options),
            decl.name
        ),
    };
    // collect_declared_functions only includes body-bearing definitions.
    let body = decl
        .body
        .expect("emit_named_fun called with prototype-only DeclareFunView");
    let _ = writeln!(out, "{signature} {{");
    if decl.name == "instanceConstants" && !block_stores_var(store, body, "fSampleRate") {
        let _ = writeln!(out, "    dsp->fSampleRate = sample_rate;");
    }
    if decl.name == "compute" {
        emit_compute_body(store, out, options, body, 1)?;
    } else if decl.name == "metadata" && is_empty_block(store, body) {
        let filename = options
            .metadata_filename
            .clone()
            .unwrap_or_else(|| format!("{class_name}.dsp"));
        let name = options
            .metadata_name
            .clone()
            .unwrap_or_else(|| class_name.to_owned());
        for (key, value) in
            c_family::ordered_compilation_metadata(&options.metadata_entries, filename, name)
        {
            let _ = writeln!(
                out,
                "    m->declare(m->metaInterface, {}, {});",
                c_string_literal(&key),
                c_string_literal(&value)
            );
        }
    } else {
        let mut mode = match decl.name.as_str() {
            "metadata" => EmitMode::Metadata,
            "buildUserInterface" => EmitMode::Ui,
            _ => EmitMode::Default,
        };
        emit_block_with_mode(store, out, options, body, 1, &mut mode)?;
        if decl.name == "instanceConstants" {
            for (var, sub) in allocated_sub_containers(store, body) {
                let _ = writeln!(out, "    delete{sub}({var});");
            }
        }
    }
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    Ok(())
}

fn is_empty_block(store: &FirStore, body: FirId) -> bool {
    matches!(match_fir(store, body), FirMatch::Block(items) if items.is_empty())
}

/// Emits one non-DSP helper function as a `static` C function.
fn emit_helper_function(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    decl: &DeclareFunView,
) -> Result<(), CodegenError> {
    // collect_declared_functions only includes body-bearing definitions.
    let body = decl
        .body
        .expect("emit_helper_function called with prototype-only DeclareFunView");
    let (ret, params) = match &decl.typ {
        FirType::Fun {
            args: typed_args,
            ret,
        } => {
            let ret = emit_type(ret, options);
            let mut rendered = Vec::with_capacity(typed_args.len());
            for (index, arg_type) in typed_args.iter().enumerate() {
                let name = decl
                    .named_args
                    .get(index)
                    .map_or_else(|| format!("arg{index}"), |named| named.name.clone());
                rendered.push(emit_named_type(arg_type, &name, options));
            }
            (ret, rendered.join(", "))
        }
        other => (emit_type(other, options), String::new()),
    };
    let _ = writeln!(out, "static {ret} {}({params}) {{", decl.name);
    emit_block(store, out, options, body, 1)?;
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    Ok(())
}

/// Returns the rendered C return type for a FIR type or function signature.
fn emit_return_type(typ: &FirType, options: &COptions) -> String {
    match typ {
        FirType::Fun { ret, .. } => emit_type(ret, options),
        _ => emit_type(typ, options),
    }
}

/// Emits the FIR `compute` body in compute-specific rendering mode.
fn emit_compute_body(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    body: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let mut mode = EmitMode::Compute;
    emit_block_with_mode(store, out, options, body, indent, &mut mode)
}

/// Builds this backend's stable error for a malformed state section, used by
/// the shared initializer collectors.
fn invalid_struct_section(store: &FirStore, section: FirId) -> CodegenError {
    CodegenError::new(
        CodegenErrorCode::InvalidModuleSection,
        format!(
            "struct section must be a FIR block, got {:?} at node {}",
            match_fir(store, section),
            section.as_u32()
        ),
    )
}

/// Collects scalar struct/global initializers used by reset lifecycle methods.
///
/// Shared with the `cpp` backend via
/// [`c_family::collect_struct_initializers`].
fn collect_struct_initializers(
    store: &FirStore,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<Vec<StructInit>, CodegenError> {
    c_family::collect_struct_initializers(store, dsp_struct, globals, |section| {
        invalid_struct_section(store, section)
    })
}

/// Collects table initializers from FIR state declarations.
///
/// Shared with the `cpp` backend via
/// [`c_family::collect_table_initializers`].
fn collect_table_initializers(
    store: &FirStore,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<Vec<TableInit>, CodegenError> {
    c_family::collect_table_initializers(store, dsp_struct, globals, |section| {
        invalid_struct_section(store, section)
    })
}

/// Extracts all body-bearing helper/function definitions from the module.
fn collect_module_functions(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<DeclareFunView>, CodegenError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidModuleSection,
            format!(
                "section 'functions' must be a FIR block, got {:?} at node {}",
                match_fir(store, functions),
                functions.as_u32()
            ),
        ));
    };

    let mut names = Vec::new();
    for item in items {
        if let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body: Some(body),
            ..
        } = match_fir(store, item)
        {
            // Only collect function *definitions* (with body). Prototype-only
            // DeclareFun nodes (body: None) are forward declarations and do not
            // displace the canonical stub generation in emit_c_api.
            names.push(DeclareFunView {
                name,
                typ,
                named_args: args,
                body: Some(body),
            });
        }
    }
    Ok(names)
}

/// Emits a FIR block in default rendering mode.
fn emit_block(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    block: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let mut mode = EmitMode::Default;
    emit_block_with_mode(store, out, options, block, indent, &mut mode)
}

/// Emits every statement in a FIR block under the active rendering mode.
fn emit_block_with_mode(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    block: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return Err(unsupported_node("expected block", block, store));
    };
    for stmt in items {
        emit_stmt(store, out, options, stmt, indent, mode)?;
    }
    Ok(())
}

/// Renders the increment of a non-reverse `ForLoop` in C style
/// (`i = i + step`; the `cpp` backend spells this `i += step`).
fn c_for_loop_step(var: &str, step: &str) -> String {
    format!("{var} = {var} + {step}")
}

/// Renders the increment of a non-reverse `SimpleForLoop` in C style
/// (`i = i + 1`; the `cpp` backend spells this `++i`).
fn c_simple_loop_increment(var: &str) -> String {
    format!("{var} = {var} + 1")
}

/// Emits one FIR statement into generated C text.
///
/// The arms shared with the `cpp` backend live in
/// [`c_family::emit_stmt_common`] — including `Control`/`WhileLoop`, which
/// this backend previously had no arms for and hard-failed on (DRIFT 7 in
/// the C-family plan §2.7). Only the C-specific arms remain here:
/// `AddMetaDeclare` (the C `MetaGlue` interface threads an explicit
/// `m->metaInterface` handle and always passes a zone argument) and `Label`
/// (rendered as a comment; the `cpp` backend drops labels silently).
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    stmt: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    let ctx = c_family::CFamilyStmtCtx {
        syntax: &SYNTAX,
        var_ref: emit_var_ref,
        for_loop_step: c_for_loop_step,
        simple_loop_increment: c_simple_loop_increment,
        render_named_type: &|typ, name| emit_named_type(typ, name, options),
        render_void_call: &|name, args| {
            // In C a sub-container entry point stays a free function whose
            // first parameter is the receiver, so only the `(void)` wrapper
            // has to go.
            if !name.starts_with("instanceInit") && !name.starts_with("fill") {
                return None;
            }
            let rendered: Vec<String> = args
                .iter()
                .map(|arg| emit_value(store, options, *arg))
                .collect::<Result<_, _>>()
                .ok()?;
            Some(format!("{name}({})", rendered.join(", ")))
        },
        render_type: &|typ| emit_type(typ, options),
        render_value: &|value| emit_value(store, options, value),
        emit_block: &|out, block, indent, mode| {
            emit_block_with_mode(store, out, options, block, indent, mode)
        },
        emit_stmt: &|out, stmt, indent, mode| emit_stmt(store, out, options, stmt, indent, mode),
    };
    if let Some(result) = c_family::emit_stmt_common(store, out, &ctx, stmt, indent, mode) {
        return result;
    }
    let tab = "    ".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::AddMetaDeclare { var, key, value } => {
            let zone = if var == "0" {
                "0".to_owned()
            } else {
                format!("&dsp->{var}")
            };
            match mode {
                EmitMode::Ui => {
                    let _ = writeln!(
                        out,
                        "{tab}ui_interface->declare(ui_interface->uiInterface, {zone}, {}, {});",
                        c_string_literal(&key),
                        c_string_literal(&value)
                    );
                }
                EmitMode::Default | EmitMode::Metadata | EmitMode::Compute => {
                    let _ = writeln!(
                        out,
                        "{tab}m->declare(m->metaInterface, {zone}, {}, {});",
                        c_string_literal(&key),
                        c_string_literal(&value)
                    );
                }
            }
            Ok(())
        }
        FirMatch::Label(label) => {
            let _ = writeln!(out, "{tab}// {label}");
            Ok(())
        }
        _ => Err(unsupported_node("statement", stmt, store)),
    }
}

/// Emits one FIR value expression into a C expression string.
///
/// All arms shared with the `cpp` backend live in
/// [`c_family::emit_value_common`] — including `Bitcast`, which this backend
/// previously had no arm for and hard-failed on (DRIFT 2 closure, C-family
/// plan §2.2). It renders as `*((T*)&v)` from the [`SYNTAX`] leaves: the
/// corrected spelling of what upstream C's `BitcastInst` visitor evidently
/// intends — upstream's own `-ftz 2` C output is garbled/uncompilable text
/// (`*((int*(&v ...`, a known-broken TODO in `c_instructions.hh`), so the
/// oracle here is the upstream *C++* form transposed to a C-style pointer
/// cast. This backend has no language-only value arms today.
fn emit_value(store: &FirStore, options: &COptions, value: FirId) -> Result<String, CodegenError> {
    let ctx = c_family::CFamilyValueCtx {
        syntax: &SYNTAX,
        var_ref: emit_var_ref,
        fun_name: emit_c_fun_name,
        render_type: &|typ| emit_type(typ, options),
        recurse: &|nested| emit_value(store, options, nested),
    };
    if let Some(result) = c_family::emit_value_common(store, &ctx, value) {
        return result;
    }
    // A sub-container allocation goes through the generated `new<Sub>()`
    // helper, which wraps `calloc`; `cpp` has the same arm with `new`.
    if let FirMatch::NewDsp { name, .. } = match_fir(store, value) {
        return if options.memory_manager_mode.is_mem0() {
            Ok(format!("new{name}(fClassManager)"))
        } else {
            Ok(format!("new{name}()"))
        };
    }
    Err(unsupported_node("value", value, store))
}

/// Maps bare FIR math names to the C symbol spelling.
///
/// `min_i`/`max_i` become the `faustmini`/`faustmaxi` helper macros, and any
/// `std::` prefix left by shared lowering is stripped (C has no namespaces).
fn emit_c_fun_name(name: &str) -> String {
    match name {
        "min_i" => "faustmini".to_owned(),
        "max_i" => "faustmaxi".to_owned(),
        _ => name.strip_prefix("std::").unwrap_or(name).to_owned(),
    }
}

/// Renders a variable reference according to its storage class.
fn emit_var_ref(name: &str, access: AccessType) -> String {
    match access {
        AccessType::Struct => format!("dsp->{name}"),
        _ => name.to_owned(),
    }
}

/// Renders a FIR type into the current C backend spelling.
///
/// Shared with the `cpp` backend via [`c_family::emit_type`]: the C-specific
/// leaves (`int` for `Bool`, `UIGlue*`/`MetaGlue*`) come from [`SYNTAX`], the
/// configurable `Quad`/`FixedPoint` spellings from `options`.
fn emit_type(typ: &FirType, options: &COptions) -> String {
    c_family::emit_type(
        typ,
        &SYNTAX,
        &options.quad_type_name,
        &options.fixed_type_name,
    )
}

/// Renders a C declarator: `<base type> <name><array suffix>`.
///
/// C array bounds are part of the declarator, not the type prefix (`float
/// buf[8];`, not `float[8] buf;`), so this cannot reuse [`emit_type`]
/// directly for array-typed declarations; it defers to
/// [`emit_type_base_and_suffix`] to peel the bracketed suffix off first.
fn emit_named_type(typ: &FirType, name: &str, options: &COptions) -> String {
    let mut suffix = String::new();
    let base = emit_type_base_and_suffix(typ, options, &mut suffix);
    format!("{base} {name}{suffix}")
}

/// Recursively splits an array type into its element base type and the
/// accumulated `[size]...` declarator suffix, appending to `suffix` for each
/// nested array dimension. Non-array types are rendered directly via
/// [`emit_type`] with an untouched (typically empty) `suffix`.
fn emit_type_base_and_suffix(typ: &FirType, options: &COptions, suffix: &mut String) -> String {
    match typ {
        FirType::Array(inner, size) => {
            suffix.push_str(&format!("[{size}]"));
            emit_type_base_and_suffix(inner, options, suffix)
        }
        _ => emit_type(typ, options),
    }
}

/// Emits `DeclareTable(AccessType::Static)` nodes as `static const` arrays
/// with inline initializers, placed before the struct definition.
///
/// Shared with the `cpp` backend via [`c_family::emit_static_tables`]; the
/// C-specific `static const` keyword order comes from [`SYNTAX`], element
/// values render through this backend's [`emit_value`].
fn emit_static_tables(
    store: &FirStore,
    out: &mut String,
    options: &COptions,
    block: FirId,
) -> Result<(), CodegenError> {
    if options.memory_manager_mode.is_mem0() {
        let FirMatch::Block(items) = match_fir(store, block) else {
            return Ok(());
        };
        for item in items {
            if let FirMatch::DeclareVar {
                name,
                typ: FirType::Array(elem, _),
                access: AccessType::Static,
                init: None,
            } = match_fir(store, item)
            {
                let _ = writeln!(out, "static {}* {name} = NULL;", emit_type(&elem, options));
                continue;
            }
            match match_fir(store, item) {
                FirMatch::DeclareTable {
                    name,
                    elem_type,
                    values,
                    ..
                } => {
                    let rendered = values
                        .iter()
                        .map(|value| emit_value(store, options, *value))
                        .collect::<Result<Vec<_>, _>>()?;
                    let _ = writeln!(
                        out,
                        "static const {} {name}[{}] = {{{}}};",
                        emit_type(&elem_type, options),
                        values.len(),
                        rendered.join(", ")
                    );
                }
                FirMatch::NullStatement => {}
                other => {
                    return Err(CodegenError::new(
                        CodegenErrorCode::InvalidModuleSection,
                        format!("unsupported static declaration in mem0: {other:?}"),
                    ));
                }
            }
        }
        return Ok(());
    }
    c_family::emit_static_tables(
        store,
        out,
        &SYNTAX,
        &options.quad_type_name,
        &options.fixed_type_name,
        block,
        |value| emit_value(store, options, value),
    )
}

/// Decodes the FIR module header expected by the C emitter.
fn decode_module(store: &FirStore, module: FirId) -> Result<ModuleView, CodegenError> {
    if let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(store, module)
    {
        Ok(ModuleView {
            name,
            dsp_struct,
            globals,
            functions,
            num_inputs,
            num_outputs,
            static_decls,
            sub_modules,
        })
    } else {
        Err(CodegenError::new(
            CodegenErrorCode::RootNotModule,
            format!(
                "expected FIR module root, got {:?} at node {}",
                match_fir(store, module),
                module.as_u32()
            ),
        ))
    }
}

/// Builds a stable unsupported-node diagnostic for the C emitter.
fn unsupported_node(kind: &str, node: FirId, store: &FirStore) -> CodegenError {
    CodegenError::new(
        CodegenErrorCode::UnsupportedNode,
        format!(
            "unsupported FIR {kind} node {:?} at {}",
            match_fir(store, node),
            node.as_u32()
        ),
    )
}

/// Escapes a Rust string into a C string literal.
///
/// Shared with the `cpp` backend via [`c_family::string_literal`] (this
/// backend's escape table — `\\`, `"`, `\n`, `\r`, `\t` — was the reference
/// the shared version was unified on).
fn c_string_literal(input: &str) -> String {
    c_family::string_literal(input)
}

#[cfg(test)]
mod tests {
    use super::{COptions, EmitMode, emit_stmt, generate_c_module};
    use crate::fixtures::build_sine_phasor_test_module;
    use crate::memory_layout::MemoryManagerMode;
    use fir::{FirBuilder, FirStore, FirType, NamedType};

    #[test]
    /// DRIFT 7 regression (C-family plan §2.7): `Control` and `WhileLoop`
    /// statements — previously handled only by the `cpp` backend — must
    /// render through the shared statement core instead of hard-failing.
    fn control_and_while_loop_statements_render() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let cond = b.int32(1);
        let inner_value = b.int32(2);
        let inner = b.drop_(inner_value);
        let control = b.control(cond, inner);
        let body = b.block(&[inner]);
        let while_loop = b.while_loop(cond, body);

        let options = COptions::default();
        let mut out = String::new();
        let mut mode = EmitMode::Default;
        emit_stmt(&store, &mut out, &options, control, 1, &mut mode).expect("Control renders");
        assert_eq!(out, "    if (1) {\n        (void)(2);\n    }\n");

        let mut out = String::new();
        emit_stmt(&store, &mut out, &options, while_loop, 1, &mut mode).expect("WhileLoop renders");
        assert_eq!(out, "    while (1) {\n        (void)(2);\n    }\n");
    }

    #[test]
    /// DRIFT 2 regression (C-family plan §2.2): `Bitcast` — previously a
    /// hard error in this backend — renders as `*((T*)&v)`. Upstream C's own
    /// `BitcastInst` visitor emits garbled, uncompilable text (a known-broken
    /// TODO in `c_instructions.hh`), so the oracle is the upstream C++
    /// `-ftz 2` form (`*reinterpret_cast<int*>(&v)`) transposed to a C-style
    /// pointer cast — the spelling the upstream visitor evidently intends.
    fn bitcast_renders_c_pointer_cast_form() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let operand = b.load_var("fTemp0", fir::AccessType::Stack, FirType::Float32);
        let bitcast = b.bitcast(FirType::Int32, operand);

        let options = COptions::default();
        let rendered = super::emit_value(&store, &options, bitcast).expect("Bitcast renders");
        assert_eq!(rendered, "*((int*)&fTemp0)");
    }

    #[test]
    fn emits_c_module_with_dsp_struct_ui_and_compute_loop() {
        let (store, module) = build_sine_phasor_test_module();
        let out = generate_c_module(&store, module, &COptions::default())
            .expect("c module generation should succeed");

        assert!(out.contains("typedef struct {"));
        assert!(out.contains("FAUSTFLOAT fFreq;"));
        assert!(out.contains("FAUSTFLOAT fGain;"));
        assert!(out.contains("double fPhase;"));
        assert!(out.contains("dsp->fFreq = (FAUSTFLOAT)(440.0);"));
        assert!(out.contains("dsp->fGain = (FAUSTFLOAT)(0.2);"));
        assert!(out.contains("void buildUserInterfacemydsp(mydsp* dsp, UIGlue* ui_interface)"));
        assert!(out.contains(
            "ui_interface->addHorizontalSlider(ui_interface->uiInterface, \"freq\", &dsp->fFreq, (FAUSTFLOAT)440.0, (FAUSTFLOAT)20.0, (FAUSTFLOAT)3000.0, (FAUSTFLOAT)1.0);"
        ));
        assert!(out.contains("void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs)"));
        assert!(out.contains("for (int i0 = 0; i0 < count; i0 = i0 + 1)"));
        assert!(out.contains("output0[i0] = "));
        assert!(out.contains("sin("));
        assert!(out.contains("void instanceConstantsmydsp(mydsp* dsp, int sample_rate) {"));
        assert!(out.contains("dsp->fSampleRate = sample_rate;"));
        let instance_init_i = out
            .find("void instanceInitmydsp(mydsp* dsp, int sample_rate) {")
            .expect("instanceInit should be emitted");
        let constants_call_i = out
            .find("instanceConstantsmydsp(dsp, sample_rate);")
            .expect("instanceConstants call should be emitted");
        let reset_call_i = out
            .find("instanceResetUserInterfacemydsp(dsp);")
            .expect("instanceResetUserInterface call should be emitted");
        let clear_call_i = out
            .find("instanceClearmydsp(dsp);")
            .expect("instanceClear call should be emitted");
        assert!(
            instance_init_i < constants_call_i
                && constants_call_i < reset_call_i
                && reset_call_i < clear_call_i,
            "instanceInit should call constants -> resetUI -> clear in order"
        );
    }

    #[test]
    fn ordinary_c_output_has_no_memory_manager_surface() {
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let text = generate_c_module(&store, module, &COptions::default()).unwrap();
        for forbidden in [
            "faust_memory_manager",
            "memoryInfoChecked",
            "fOwnerManager",
            "create",
            "classDestroyChecked",
        ] {
            assert!(!text.contains(forbidden), "unexpected {forbidden}: {text}");
        }
    }

    #[test]
    fn mem0_c_uses_the_effective_single_or_double_sample_width() {
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let single = generate_c_module(
            &store,
            module,
            &COptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                ..COptions::default()
            },
        )
        .unwrap();
        let double = generate_c_module(
            &store,
            module,
            &COptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                double_precision: true,
                ..COptions::default()
            },
        )
        .unwrap();
        assert!(single.contains("#define FAUSTFLOAT float"));
        assert!(single.contains("manager->context, 16, 4"));
        assert!(double.contains("#define FAUSTFLOAT float"));
        assert!(double.contains("manager->context, 32, 8"));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn mem0_generated_c_compiles_and_unwinds_allocation_failures() {
        use std::process::Command;

        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
        if Command::new(&cc).arg("--version").output().is_err() {
            eprintln!("skipping mem0 C smoke test: `{cc}` is unavailable");
            return;
        }
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let generated = generate_c_module(
            &store,
            module,
            &COptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                ..COptions::default()
            },
        )
        .unwrap();
        assert!(generated.contains("FAUSTFLOAT* fDelay;"), "{generated}");
        assert!(generated.contains("void memoryInfomydsp(faust_memory_manager* manager)"));
        assert!(generated.contains("mydsp* createmydsp(faust_memory_manager* manager)"));
        assert!(generated.contains("void destroymydsp(mydsp* dsp)"));

        let prelude = r#"
#include <assert.h>
typedef struct UIGlue UIGlue;
typedef struct MetaGlue {
    void* metaInterface;
    void (*declare)(void*, const char*, const char*);
} MetaGlue;
"#;
        let main = r#"
typedef struct test_manager {
    void* live[16];
    size_t live_count;
    size_t calls;
    size_t fail_at;
    size_t described;
} test_manager;
static void test_begin(void* context, size_t count) { ((test_manager*)context)->described = count; }
static void test_info(void* context, const char* name, faust_memory_type type,
                      size_t count, size_t bytes, size_t alignment,
                      uint64_t reads, uint64_t writes) {
    (void)context; (void)name; (void)type; (void)count; (void)bytes;
    (void)alignment; (void)reads; (void)writes;
}
static void test_end(void* context) { (void)context; }
static void* test_allocate(void* context, size_t bytes, size_t alignment) {
    test_manager* state = (test_manager*)context;
    void* address;
    (void)alignment;
    if (state->calls++ == state->fail_at) return NULL;
    address = malloc(bytes);
    if (address != NULL) state->live[state->live_count++] = address;
    return address;
}
static void test_destroy(void* context, void* address, size_t bytes, size_t alignment) {
    test_manager* state = (test_manager*)context;
    size_t index;
    (void)bytes; (void)alignment;
    for (index = 0; index < state->live_count; ++index) {
        if (state->live[index] == address) {
            state->live[index] = state->live[--state->live_count];
            free(address);
            return;
        }
    }
    assert(0 && "destroy of unowned address");
}
static faust_memory_manager make_manager(test_manager* context) {
    faust_memory_manager manager = {
        FAUST_MEMORY_MANAGER_ABI_VERSION, sizeof(faust_memory_manager), context,
        test_begin, test_info, test_end, test_allocate, test_destroy
    };
    return manager;
}
int main(void) {
    test_manager first = {{0}, 0, 0, (size_t)-1, 0};
    test_manager second = {{0}, 0, 0, (size_t)-1, 0};
    faust_memory_manager first_api = make_manager(&first);
    faust_memory_manager second_api = make_manager(&second);
    mydsp* a;
    mydsp* b;
    FAUSTFLOAT input_a[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    FAUSTFLOAT input_b[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    FAUSTFLOAT output_a[8] = {0};
    FAUSTFLOAT output_b[8] = {0};
    FAUSTFLOAT* inputs_a[1] = {input_a};
    FAUSTFLOAT* inputs_b[1] = {input_b};
    FAUSTFLOAT* outputs_a[1] = {output_a};
    FAUSTFLOAT* outputs_b[1] = {output_b};
    size_t fail_at;
    size_t frame;
    assert(memoryInfoCheckedmydsp(&first_api));
    assert(first.described >= 2);
    for (fail_at = 0; fail_at < 2; ++fail_at) {
        first.calls = 0;
        first.fail_at = fail_at;
        assert(createmydsp(&first_api) == NULL);
        assert(first.live_count == 0);
    }
    first.calls = 0;
    first.fail_at = (size_t)-1;
    a = createmydsp(&first_api);
    b = createmydsp(&second_api);
    assert(a != NULL && b != NULL);
    assert(a->fOwnerManager == &first_api);
    assert(b->fOwnerManager == &second_api);
    initmydsp(a, 48000);
    instanceInitmydsp(b, 48000);
    computemydsp(a, 8, inputs_a, outputs_a);
    computemydsp(b, 8, inputs_b, outputs_b);
    for (frame = 0; frame < 8; ++frame) {
        FAUSTFLOAT expected = frame < 4 ? 0 : (FAUSTFLOAT)(frame - 3);
        assert(output_a[frame] == expected);
        assert(output_b[frame] == expected);
    }
    assert(!classDestroyCheckedmydsp(&first_api));
    destroymydsp(b);
    destroymydsp(a);
    assert(first.live_count == 0 && second.live_count == 0);
    assert(classDestroyCheckedmydsp(&first_api));
    return 0;
}
"#;
        let stem = format!("faust-rs-mem0-c-{}", std::process::id());
        let source = std::env::temp_dir().join(format!("{stem}.c"));
        let binary = std::env::temp_dir().join(if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem
        });
        std::fs::write(&source, format!("{prelude}\n{generated}\n{main}"))
            .expect("write C smoke source");
        let compile = Command::new(&cc)
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .expect("run C compiler");
        assert!(
            compile.status.success(),
            "C compile failed:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&binary).output().expect("run C smoke binary");
        assert!(
            run.status.success(),
            "C runtime failed:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(binary);
    }

    #[test]
    fn rejects_invalid_canonical_metadata_signature() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let body = b.block(&[]);
        let bad_ty = FirType::Fun {
            args: vec![FirType::Int32],
            ret: Box::new(FirType::Void),
        };
        let bad_args = vec![NamedType {
            name: "x".to_string(),
            typ: FirType::Int32,
        }];
        let metadata = b.declare_fun("metadata", bad_ty, &bad_args, Some(body), false);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[metadata]);
        let static_decls = b.block(&[]);
        let module = b.module(
            0,
            0,
            "mydsp",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[],
        );

        let err = generate_c_module(&store, module, &COptions::default())
            .expect_err("invalid canonical metadata signature must fail");
        assert_eq!(err.code(), super::CodegenErrorCode::InvalidModuleSection);
        assert!(
            err.to_string()
                .contains("invalid FIR signature for metadata")
        );
    }

    #[test]
    fn emits_ui_and_metadata_nodes_in_distinct_callbacks() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let group_meta = b.add_meta_declare("0", "tooltip", "hello");
        let open = b.open_box(fir::UiBoxType::Vertical, "group");
        let slider_meta = b.add_meta_declare("fGain", "unit", "dB");
        let slider = b.add_slider(
            fir::SliderType::Horizontal,
            "gain",
            "fGain",
            fir::SliderRange {
                init: 0.5,
                lo: 0.0,
                hi: 1.0,
                step: 0.01,
            },
        );
        let close = b.close_box();
        let ui_body = b.block(&[group_meta, open, slider_meta, slider, close]);
        let build_ui_ty = FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::UI],
            ret: Box::new(FirType::Void),
        };
        let build_ui_args = [
            NamedType {
                name: "dsp".to_string(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "ui_interface".to_string(),
                typ: FirType::UI,
            },
        ];
        let ui = b.declare_fun(
            "buildUserInterface",
            build_ui_ty,
            &build_ui_args,
            Some(ui_body),
            false,
        );
        let module_meta = b.add_meta_declare("0", "author", "faust-rs");
        let metadata_body = b.block(&[module_meta]);
        let metadata_ty = FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Meta],
            ret: Box::new(FirType::Void),
        };
        let metadata_args = [
            NamedType {
                name: "dsp".to_string(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "meta".to_string(),
                typ: FirType::Meta,
            },
        ];
        let metadata = b.declare_fun(
            "metadata",
            metadata_ty,
            &metadata_args,
            Some(metadata_body),
            false,
        );
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[ui, metadata]);
        let static_decls = b.block(&[]);
        let module = b.module(
            0,
            0,
            "mydsp",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[],
        );

        let out = generate_c_module(&store, module, &COptions::default())
            .expect("C UI nodes emit in the correct callback family");

        assert!(out.contains("void buildUserInterfacemydsp(mydsp* dsp, UIGlue* ui_interface)"));
        assert!(out.contains(
            "ui_interface->declare(ui_interface->uiInterface, 0, \"tooltip\", \"hello\");"
        ));
        assert!(out.contains(
            "ui_interface->declare(ui_interface->uiInterface, &dsp->fGain, \"unit\", \"dB\");"
        ));
        assert!(out.contains(
            "ui_interface->addHorizontalSlider(ui_interface->uiInterface, \"gain\", &dsp->fGain, (FAUSTFLOAT)0.5, (FAUSTFLOAT)0.0, (FAUSTFLOAT)1.0, (FAUSTFLOAT)0.01);"
        ));
        assert!(out.contains("void metadatamydsp(MetaGlue* m)"));
        assert!(out.contains("m->declare(m->metaInterface, 0, \"author\", \"faust-rs\");"));
    }

    #[test]
    fn double_literal_format_preserves_grain_prng_scale_precision() {
        assert_eq!(
            crate::backends::c_family::trim_float(1.0 / 2147483647.0),
            "0.0000000004656612875245797",
            "C backend double literals must preserve enough precision for grain/table DSPs"
        );
    }

    #[test]
    /// S4a: `c` emits the sub-container as a struct plus free functions taking
    /// it as their first parameter, with `calloc`/`free` allocation and a
    /// `classInit` that fills the table — the reference shape of plan §5.9.1.
    fn sub_module_is_emitted_as_a_struct_with_a_filling_class_init() {
        let mut store = FirStore::new();
        let module = {
            let mut b = FirBuilder::new(&mut store);
            let obj_ty = FirType::Ptr(Box::new(FirType::Obj));
            let sub = {
                let init_body = b.block(&[]);
                let init = b.declare_fun(
                    "instanceInitmydspSIG0",
                    FirType::Fun {
                        args: vec![obj_ty.clone(), FirType::Int32],
                        ret: Box::new(FirType::Void),
                    },
                    &[
                        NamedType {
                            name: "dsp".into(),
                            typ: obj_ty.clone(),
                        },
                        NamedType {
                            name: "sample_rate".into(),
                            typ: FirType::Int32,
                        },
                    ],
                    Some(init_body),
                    false,
                );
                let fill_body = b.block(&[]);
                let table_ty = FirType::Ptr(Box::new(FirType::Float32));
                let fill = b.declare_fun(
                    "fillmydspSIG0",
                    FirType::Fun {
                        args: vec![obj_ty.clone(), FirType::Int32, table_ty.clone()],
                        ret: Box::new(FirType::Void),
                    },
                    &[
                        NamedType {
                            name: "dsp".into(),
                            typ: obj_ty.clone(),
                        },
                        NamedType {
                            name: "count".into(),
                            typ: FirType::Int32,
                        },
                        NamedType {
                            name: "table".into(),
                            typ: table_ty,
                        },
                    ],
                    Some(fill_body),
                    false,
                );
                let functions = b.block(&[init, fill]);
                let helper_array = b.declare_var(
                    "iVec0",
                    FirType::Array(Box::new(FirType::Int32), 2),
                    fir::AccessType::Struct,
                    None,
                );
                let helper_state = b.block(&[helper_array]);
                let empty = b.block(&[]);
                b.sub_module(
                    "mydspSIG0",
                    FirType::Float32,
                    helper_state,
                    empty,
                    empty,
                    functions,
                    &[],
                )
            };
            let table = b.declare_var(
                "ftbl0mydspSIG0",
                FirType::Array(Box::new(FirType::Float32), 8),
                fir::AccessType::Static,
                None,
            );
            let static_decls = b.block(&[table]);
            let static_init_body = {
                let new_obj = b.new_dsp("mydspSIG0", obj_ty.clone());
                let alloc = b.declare_var(
                    "sig0",
                    obj_ty.clone(),
                    fir::AccessType::Stack,
                    Some(new_obj),
                );
                let obj = b.load_var("sig0", fir::AccessType::Stack, obj_ty.clone());
                let sr = b.load_var("sample_rate", fir::AccessType::FunArgs, FirType::Int32);
                let init_call = b.fun_call("instanceInitmydspSIG0", &[obj, sr], FirType::Void);
                let init_stmt = b.drop_(init_call);
                let obj2 = b.load_var("sig0", fir::AccessType::Stack, obj_ty.clone());
                let count = b.int32(8);
                let tbl = b.load_var(
                    "ftbl0mydspSIG0",
                    fir::AccessType::Static,
                    FirType::Array(Box::new(FirType::Float32), 8),
                );
                let fill_call = b.fun_call("fillmydspSIG0", &[obj2, count, tbl], FirType::Void);
                let fill_stmt = b.drop_(fill_call);
                b.block(&[alloc, init_stmt, fill_stmt])
            };
            let static_init = b.declare_fun(
                "staticInit",
                FirType::Fun {
                    args: vec![obj_ty.clone(), FirType::Int32],
                    ret: Box::new(FirType::Void),
                },
                &[
                    NamedType {
                        name: "dsp".into(),
                        typ: obj_ty.clone(),
                    },
                    NamedType {
                        name: "sample_rate".into(),
                        typ: FirType::Int32,
                    },
                ],
                Some(static_init_body),
                false,
            );
            let instance_constants = b.declare_fun(
                "instanceConstants",
                FirType::Fun {
                    args: vec![obj_ty.clone(), FirType::Int32],
                    ret: Box::new(FirType::Void),
                },
                &[
                    NamedType {
                        name: "dsp".into(),
                        typ: obj_ty.clone(),
                    },
                    NamedType {
                        name: "sample_rate".into(),
                        typ: FirType::Int32,
                    },
                ],
                Some(static_init_body),
                false,
            );
            let compute_body = b.block(&[]);
            let buffers = FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat))));
            let compute = b.declare_fun(
                "compute",
                FirType::Fun {
                    args: vec![
                        obj_ty.clone(),
                        FirType::Int32,
                        buffers.clone(),
                        buffers.clone(),
                    ],
                    ret: Box::new(FirType::Void),
                },
                &[
                    NamedType {
                        name: "dsp".into(),
                        typ: obj_ty,
                    },
                    NamedType {
                        name: "count".into(),
                        typ: FirType::Int32,
                    },
                    NamedType {
                        name: "inputs".into(),
                        typ: buffers.clone(),
                    },
                    NamedType {
                        name: "outputs".into(),
                        typ: buffers,
                    },
                ],
                Some(compute_body),
                false,
            );
            let functions = b.block(&[static_init, instance_constants, compute]);
            let empty = b.block(&[]);
            b.module(0, 1, "mydsp", empty, empty, functions, static_decls, &[sub])
        };

        let text = generate_c_module(&store, module, &COptions::default())
            .expect("sub-module emission must succeed");

        assert!(text.contains("} mydspSIG0;"), "struct missing: {text}");
        assert!(
            text.contains("static mydspSIG0* newmydspSIG0() { return (mydspSIG0*)calloc(1, sizeof(mydspSIG0)); }"),
            "calloc helper missing: {text}"
        );
        assert!(
            text.contains("static void deletemydspSIG0(mydspSIG0* dsp) { free(dsp); }"),
            "free helper missing: {text}"
        );
        assert!(
            text.contains("static void instanceInitmydspSIG0(mydspSIG0* dsp, int sample_rate)"),
            "init signature must take the receiver: {text}"
        );
        assert!(
            text.contains("static float ftbl0mydspSIG0[8];"),
            "uninitialized table declaration missing: {text}"
        );
        for expected in [
            "mydspSIG0* sig0 = newmydspSIG0();",
            "instanceInitmydspSIG0(sig0, sample_rate);",
            "fillmydspSIG0(sig0, 8, ftbl0mydspSIG0);",
            "deletemydspSIG0(sig0);",
        ] {
            assert!(
                text.contains(expected),
                "classInit missing `{expected}`: {text}"
            );
        }
        // Emitted once as the classInit body, never as a function of its own.
        assert!(
            !text.contains("static void staticInit("),
            "staticInit leaked as a function: {text}"
        );

        let mem_text = generate_c_module(
            &store,
            module,
            &COptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                ..COptions::default()
            },
        )
        .expect("mem0 sub-module emission must succeed");
        assert!(mem_text.contains("int iVec0[2];"), "{mem_text}");
        assert!(!mem_text.contains("int* iVec0;"), "{mem_text}");
        assert!(
            mem_text.matches("deletemydspSIG0(sig0);").count() >= 2,
            "class and instance table helpers must both be released: {mem_text}"
        );
    }
}
