//! C++ backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! - `compiler/generator/instructions.hh` (`ModuleInst`)
//! - `compiler/generator/cpp/cpp_instructions.hh` (`CPPInstVisitor::visit(ModuleInst*)`)
//! - `compiler/generator/cpp/cpp_code_container.cpp` (`memoryInfo`,
//!   `memoryCreate`, `memoryDestroy`, `create`, `destroy`, `classDestroy`)
//! - `compiler/generator/fir_to_fir.hh` (`ArrayToPointer`)
//! - `compiler/generator/text_instructions.hh`
//!
//! # Current slice
//! This backend follows a module-first contract:
//! input must be a FIR module node and code generation walks FIR through
//! `match_fir` only.
//!
//! # Output contract
//! - Emits `class <name> : public <super-class>`.
//! - Emits Faust `dsp` lifecycle/API methods (`init`, `instance*`,
//!   `buildUserInterface`, `compute`, `getNumInputs/Outputs`, `metadata`).
//! - Emits `compute(int count, FAUSTFLOAT** RESTRICT, FAUSTFLOAT** RESTRICT)`
//!   with a per-sample loop and channel writes.
//!
//! # Limitations
//! Unsupported FIR nodes currently fail fast with `FRS-CGEN-CPP-0003`.

use std::fmt::Write as _;

use fir::{FirId, FirMatch, FirMathOp, FirStore, FirType, NamedType, match_fir};

use crate::backends::c_family::{self, CFamilySyntax, EmitMode};
use crate::backends::faust_api;
use crate::memory_layout::{
    AllocationPhase, Mem0Analysis, Mem0AnalysisOptions, MemoryLayoutFlavor, MemoryManagerMode,
    MemoryRole, MemoryScope, MemoryZone, analyze_effective_mem0,
};

pub const BACKEND_NAME: &str = "cpp";

/// C++ spellings for the shared C-family emission core.
const SYNTAX: CFamilySyntax = CFamilySyntax {
    bool_type: "bool",
    ui_type: "UI*",
    meta_type: "Meta*",
    static_table_keywords: "const static",
    bool_true: "true",
    bool_false: "false",
    null_value: "nullptr",
    ui_glue_arg: "",
    ui_glue_solo: "",
    faustfloat_cast_open: "FAUSTFLOAT(",
    faustfloat_cast_close: ")",
    switch_default_break: false,
    bitcast_open: "*reinterpret_cast<",
    bitcast_mid: "*>(&",
    bitcast_close: ")",
};

/// C++ backend options for module-first emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CppOptions {
    /// Custom memory-manager layout selected for generated state.
    ///
    /// Source provenance: Faust C++ `global::gMemoryManager` and
    /// `CPPCodeContainer`. Passing it explicitly is an `adapted` replacement
    /// for the reference compiler's process-global option.
    pub memory_manager_mode: MemoryManagerMode,
    /// Effective `FAUSTFLOAT` width used by target-layout analysis.
    pub double_precision: bool,
    /// Optional namespace wrapping generated code.
    pub namespace: Option<String>,
    /// Optional class name override for the FIR module name.
    pub class_name: Option<String>,
    /// Optional superclass override for the generated DSP class.
    ///
    /// Mirrors Faust `-scn/--super-class-name` and defaults to `dsp`.
    pub super_class_name: Option<String>,
    /// C++ spelling used for FIR `Quad` values.
    ///
    /// C++ uses target-dependent `quad` spellings; Rust backend keeps this
    /// configurable to preserve parity when exact 1:1 naming is unavailable.
    pub quad_type_name: String,
    /// C++ spelling used for FIR `FixedPoint` values.
    ///
    /// C++ fixed-point support may be backend-specific; Rust backend keeps this
    /// configurable to document/adapt non-1:1 mappings explicitly.
    pub fixed_type_name: String,
    /// Compilation options string printed in the generated-file header.
    ///
    /// Mirrors C++ Faust's `Compilation options: ...` header line. `None`
    /// falls back to a minimal `-lang cpp` line for callers (mostly tests)
    /// that do not thread the real CLI flags through.
    pub compile_options: Option<String>,
    /// Source-level DSP name reported in the generated banner and metadata
    /// callback. This is independent from [`Self::class_name`].
    pub metadata_name: Option<String>,
    /// Source basename reported by the generated metadata callback.
    pub metadata_filename: Option<String>,
    /// Non-identity compilation metadata replayed by `metadata()`.
    pub metadata_entries: Vec<(String, String)>,
}

impl Default for CppOptions {
    /// Default backend options.
    ///
    /// Uses `class_name = Some("mydsp")` to match the current workspace
    /// convention for deterministic generated type names.
    fn default() -> Self {
        Self {
            memory_manager_mode: MemoryManagerMode::None,
            double_precision: false,
            namespace: None,
            class_name: Some("mydsp".to_owned()),
            super_class_name: Some("dsp".to_owned()),
            quad_type_name: "quad".to_owned(),
            fixed_type_name: "fixed".to_owned(),
            compile_options: None,
            metadata_name: None,
            metadata_filename: None,
            metadata_entries: Vec::new(),
        }
    }
}

/// Stable backend error codes for C++ code generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// Root FIR node is not a module (`FirMatch::Module`).
    RootNotModule,
    /// Module section is not a FIR block shape.
    InvalidModuleSection,
    /// One FIR node is not yet supported by the C++ emitter slice.
    UnsupportedNode,
    /// Canonical memory analysis rejected an unsafe/unrepresentable layout.
    MemoryLayout,
}

impl CodegenErrorCode {
    /// Stable textual code used in diagnostics and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-CPP-0001",
            Self::InvalidModuleSection => "FRS-CGEN-CPP-0002",
            Self::UnsupportedNode => "FRS-CGEN-CPP-0003",
            Self::MemoryLayout => "FRS-CGEN-CPP-0004",
        }
    }
}

/// Typed backend error returned by the C++ emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    code: CodegenErrorCode,
    message: String,
    fir_node: Option<FirId>,
}

impl CodegenError {
    /// Creates a typed C++ backend code generation error.
    #[must_use]
    pub fn new(code: CodegenErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fir_node: None,
        }
    }

    /// Associates the error with the FIR node rejected by the emitter.
    #[must_use]
    pub fn at_node(mut self, node: FirId) -> Self {
        self.fir_node = Some(node);
        self
    }

    /// Returns the offending FIR node when the emitter retained it.
    #[must_use]
    pub const fn fir_node(&self) -> Option<FirId> {
        self.fir_node
    }

    /// Returns the stable backend error code.
    #[must_use]
    pub fn code(&self) -> CodegenErrorCode {
        self.code
    }

    /// Returns the backend-specific message without the bracketed code.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CodegenError {}

/// Decoded FIR module header used by the text emitter.
///
/// Like the C backend equivalent, this is a short-lived view whose ids still
/// point back into the FIR store for actual section emission.
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

/// Borrowed function declaration view used while stitching the C++ class body.
///
/// The emitter only needs structural information here: name, type, arguments,
/// optional body, and whether the FIR declaration requested inline emission.
/// This avoids repeated FIR decoding while preserving access to borrowed names
/// and signature components.
struct DeclareFunView<'a> {
    name: &'a str,
    typ: &'a FirType,
    named_args: &'a [NamedType],
    /// `None` when this is a prototype-only declaration (no body).
    body: Option<FirId>,
    is_inline: bool,
}

/// Generates C++ code from a FIR module root.
///
/// # C++ parity mapping
/// This is the Rust module-first entrypoint corresponding to C++ `ModuleInst`
/// visitor-based emission.
///
/// # Options behavior
/// - `class_name`: overrides FIR module name.
/// - `super_class_name`: overrides the generated DSP superclass.
/// - `namespace`: wraps the generated class in `namespace <name>`.
/// - input/output arity is taken from FIR module metadata.
///
/// # Errors
/// Returns [`CodegenError`] with code `FRS-CGEN-CPP-0001` when `module`
/// does not decode to `FirMatch::Module`.
pub fn generate_cpp_module(
    store: &FirStore,
    module: FirId,
    options: &CppOptions,
) -> Result<String, CodegenError> {
    let module_id = module;
    let module = decode_module(store, module_id)?;
    let module_name = module.name.clone();
    let effective_options = options.clone();
    let metadata_name = options.metadata_name.as_deref().unwrap_or(&module_name);
    let declared_functions = collect_module_function_names(store, module.functions)?;
    let has_sample_rate_field = block_declares_var(store, module.dsp_struct, "fSampleRate")
        || block_declares_var(store, module.globals, "fSampleRate");
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(module.name.as_str());
    let super_class_name = options.super_class_name.as_deref().unwrap_or("dsp");
    let mem0 = if options.memory_manager_mode.is_mem0() {
        Some(
            analyze_effective_mem0(
                store,
                module_id,
                &Mem0AnalysisOptions::native(MemoryLayoutFlavor::Cpp, options.double_precision),
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

    let mut out = String::new();
    emit_cpp_header(
        &mut out,
        class_name,
        metadata_name,
        options.compile_options.as_deref(),
    );
    if let Some(namespace) = options.namespace.as_deref() {
        let _ = writeln!(out, "namespace {namespace} {{");
        let _ = writeln!(out);
    }

    // Emit compile-time constant waveform tables at file scope.
    emit_static_tables(store, &mut out, &effective_options, module.static_decls)?;
    let _ = writeln!(out);

    // Generated-table sub-containers come before the DSP class that fills
    // them, deepest-first so a nested generator's class precedes its user.
    emit_sub_modules(
        store,
        &mut out,
        &effective_options,
        &module_name,
        module.sub_modules,
    )?;

    let _ = writeln!(out, "class {class_name} : public {super_class_name} {{");
    let _ = writeln!(out, "private:");
    if mem0.is_some() {
        let _ = writeln!(out, "    dsp_memory_manager* fOwnerManager;");
    }
    if !has_sample_rate_field {
        let _ = writeln!(out, "    int fSampleRate;");
    }
    emit_section(
        store,
        &mut out,
        &effective_options,
        &module_name,
        "dsp_struct",
        module.dsp_struct,
        true,
    )?;
    emit_section(
        store,
        &mut out,
        &effective_options,
        &module_name,
        "globals",
        module.globals,
        true,
    )?;
    let _ = writeln!(out, "public:");
    if mem0.is_some() {
        let _ = writeln!(out, "    static dsp_memory_manager* fManager;");
        let _ = writeln!(out, "private:");
        let _ = writeln!(out, "    static dsp_memory_manager* fClassManager;");
        let _ = writeln!(out, "    static int fClassSampleRate;");
        let _ = writeln!(out, "    static size_t fLiveInstances;");
        let _ = writeln!(out, "public:");
    }
    let struct_inits = c_family::collect_struct_initializers(
        store,
        module.dsp_struct,
        module.globals,
        |section| invalid_struct_section(store, section),
    )?;
    let table_inits = c_family::collect_table_initializers(
        store,
        module.dsp_struct,
        module.globals,
        |section| invalid_struct_section(store, section),
    )?;
    emit_dsp_contract_methods(
        store,
        &mut out,
        DspContractEmitInput {
            options: &effective_options,
            num_inputs: module.num_inputs,
            num_outputs: module.num_outputs,
            class_name,
            module_name: &module_name,
            declared_functions: &declared_functions,
            struct_inits: &struct_inits,
            table_inits: &table_inits,
            static_init_body: find_function_body(store, module.functions, "staticInit"),
            mem0: mem0.as_ref(),
            indent: 1,
        },
    )?;
    emit_section(
        store,
        &mut out,
        &effective_options,
        &module_name,
        "functions",
        module.functions,
        true,
    )?;
    let _ = writeln!(out, "}};");

    if mem0.is_some() {
        let _ = writeln!(out);
        let _ = writeln!(out, "dsp_memory_manager* {class_name}::fManager = nullptr;");
        let _ = writeln!(
            out,
            "dsp_memory_manager* {class_name}::fClassManager = nullptr;"
        );
        let _ = writeln!(out, "int {class_name}::fClassSampleRate = 0;");
        let _ = writeln!(out, "size_t {class_name}::fLiveInstances = 0;");
    }

    if let Some(namespace) = options.namespace.as_deref() {
        let _ = writeln!(out);
        let _ = writeln!(out, "}} // namespace {namespace}");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "#endif");
    Ok(out)
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

/// Inputs for [`emit_dsp_contract_methods`], grouped like the C backend's
/// `CApiEmitInput` to keep the emission signature flat.
struct DspContractEmitInput<'a> {
    options: &'a CppOptions,
    num_inputs: usize,
    num_outputs: usize,
    class_name: &'a str,
    module_name: &'a str,
    declared_functions: &'a [String],
    /// Scalar state initializers replayed by the synthesized
    /// `instanceResetUserInterface` fallback (DRIFT 6 closure, C-family plan
    /// §2.6 — `c` and `julia` already replayed these; `cpp` left the fallback
    /// body empty, so UI-bound state stayed zeroed instead of taking its
    /// declared init value).
    struct_inits: &'a [c_family::StructInit],
    /// Table initializers replayed by the same fallback.
    table_inits: &'a [c_family::TableInit],
    /// Body of the FIR `staticInit` function, when the module declares one.
    /// Rendered as the `classInit` body.
    static_init_body: Option<FirId>,
    /// Canonical snapshot retained by the emitter; every manager-facing method
    /// below is generated from these exact zones.
    mem0: Option<&'a Mem0Analysis>,
    indent: usize,
}

/// Emits the standard Faust `dsp` API surface expected from generated C++.
///
/// Methods are synthesized even when the FIR module omitted some sections so
/// that the generated class still satisfies the stable backend ABI. When a
/// section is absent, the emitted method falls back to the same neutral/default
/// behavior as the C++ backend.
fn emit_dsp_contract_methods(
    store: &FirStore,
    out: &mut String,
    spec: DspContractEmitInput<'_>,
) -> Result<(), CodegenError> {
    let DspContractEmitInput {
        options,
        num_inputs,
        num_outputs,
        class_name,
        module_name,
        declared_functions,
        struct_inits,
        table_inits,
        static_init_body,
        mem0,
        indent,
    } = spec;
    let tab = "    ".repeat(indent);
    let has_build_ui = declared_functions
        .iter()
        .any(|name| name == "buildUserInterface");
    let has_metadata = declared_functions.iter().any(|name| name == "metadata");
    let has_instance_constants = declared_functions
        .iter()
        .any(|name| name == "instanceConstants");
    let has_instance_reset_ui = declared_functions
        .iter()
        .any(|name| name == "instanceResetUserInterface");
    let has_instance_clear = declared_functions
        .iter()
        .any(|name| name == "instanceClear");
    let has_compute = declared_functions.iter().any(|name| name == "compute");

    if let Some(analysis) = mem0 {
        emit_mem0_constructors(out, class_name, analysis, indent);
    } else {
        let _ = writeln!(out, "{tab}{class_name}() {{");
        let _ = writeln!(out, "{tab}}}");
        let _ = writeln!(out);
        let _ = writeln!(out, "{tab}{class_name}(const {class_name}&) = default;");
        let _ = writeln!(out);
        let _ = writeln!(out, "{tab}virtual ~{class_name}() = default;");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{tab}{class_name}& operator=(const {class_name}&) = default;"
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{tab}virtual int getNumInputs() {{");
    let _ = writeln!(out, "{tab}    return {};", num_inputs);
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}virtual int getNumOutputs() {{");
    let _ = writeln!(out, "{tab}    return {};", num_outputs);
    let _ = writeln!(out, "{tab}}}");
    // `classInit` is the backend rendering of the FIR `staticInit` body: the
    // fills of file-scope generated tables, shared by every instance. Without
    // a `staticInit` there is nothing to initialize and the method stays empty.
    if let Some(mem0) = mem0 {
        let _ = writeln!(out, "{tab}static bool classInitChecked(int sample_rate) {{");
        let _ = writeln!(out, "{tab}    if (fManager == nullptr) return false;");
        let _ = writeln!(out, "{tab}    if (fClassManager != nullptr) {{");
        let _ = writeln!(
            out,
            "{tab}        return fClassManager == fManager && fClassSampleRate == sample_rate;"
        );
        let _ = writeln!(out, "{tab}    }}");
        let _ = writeln!(out, "{tab}    fClassManager = fManager;");
        emit_mem0_class_allocations(out, mem0, indent + 1);
        let _ = writeln!(out, "{tab}    try {{");
        if let Some(static_init_body) = static_init_body {
            emit_block(
                store,
                out,
                options,
                module_name,
                static_init_body,
                indent + 2,
            )?;
            for (var, sub) in allocated_sub_containers(store, static_init_body) {
                let _ = writeln!(out, "{tab}        delete{sub}({var});");
            }
        } else {
            let _ = writeln!(out, "{tab}        (void)sample_rate;");
        }
        let _ = writeln!(out, "{tab}    }} catch (...) {{");
        for zone in mem0_class_tables(mem0).iter().rev() {
            let _ = writeln!(out, "{tab}        if ({} != nullptr) {{", zone.name);
            let _ = writeln!(
                out,
                "{tab}            fClassManager->destroy({});",
                zone.name
            );
            let _ = writeln!(out, "{tab}            {} = nullptr;", zone.name);
            let _ = writeln!(out, "{tab}        }}");
        }
        let _ = writeln!(out, "{tab}        fClassManager = nullptr;");
        let _ = writeln!(out, "{tab}        return false;");
        let _ = writeln!(out, "{tab}    }}");
        let _ = writeln!(out, "{tab}    fClassSampleRate = sample_rate;");
        let _ = writeln!(out, "{tab}    return true;");
        let _ = writeln!(out, "{tab}}}");
        let _ = writeln!(out, "{tab}static void classInit(int sample_rate) {{");
        let _ = writeln!(
            out,
            "{tab}    if (!classInitChecked(sample_rate)) std::terminate();"
        );
        let _ = writeln!(out, "{tab}}}");
    } else {
        let _ = writeln!(out, "{tab}static void classInit(int sample_rate) {{");
        if let Some(static_init_body) = static_init_body {
            emit_block(
                store,
                out,
                options,
                module_name,
                static_init_body,
                indent + 1,
            )?;
            for (var, sub) in allocated_sub_containers(store, static_init_body) {
                let _ = writeln!(out, "{tab}    delete{sub}({var});");
            }
        } else {
            let _ = writeln!(out, "{tab}    (void)sample_rate;");
        }
        let _ = writeln!(out, "{tab}}}");
    }
    let _ = writeln!(out, "{tab}virtual int getSampleRate() {{");
    let _ = writeln!(out, "{tab}    return fSampleRate;");
    let _ = writeln!(out, "{tab}}}");
    if !has_instance_constants {
        let _ = writeln!(
            out,
            "{tab}virtual void instanceConstants(int sample_rate) {{"
        );
        let _ = writeln!(out, "{tab}    fSampleRate = sample_rate;");
        let _ = writeln!(out, "{tab}}}");
    }
    if !has_instance_reset_ui {
        let _ = writeln!(out, "{tab}virtual void instanceResetUserInterface() {{");
        // DRIFT 6 closure (C-family plan §2.6): replay declared state
        // initializers so UI-bound fields regain their default values on
        // reset, matching the `c` backend's fallback shape (`dsp->` prefix
        // aside).
        for init in struct_inits {
            let value = emit_value(store, options, init.init)?;
            let _ = writeln!(
                out,
                "{tab}    {} = ({})({value});",
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
                    "{tab}    {table_ref}[{index}] = ({})({value});",
                    emit_type(&init.elem_type, options)
                );
            }
        }
        let _ = writeln!(out, "{tab}}}");
    }
    if !has_instance_clear {
        let _ = writeln!(out, "{tab}virtual void instanceClear() {{");
        let _ = writeln!(out, "{tab}}}");
    }
    let _ = writeln!(out, "{tab}virtual void init(int sample_rate) {{");
    let _ = writeln!(out, "{tab}    classInit(sample_rate);");
    let _ = writeln!(out, "{tab}    instanceInit(sample_rate);");
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}virtual void instanceInit(int sample_rate) {{");
    let _ = writeln!(out, "{tab}    instanceConstants(sample_rate);");
    let _ = writeln!(out, "{tab}    instanceResetUserInterface();");
    let _ = writeln!(out, "{tab}    instanceClear();");
    let _ = writeln!(out, "{tab}}}");
    if let Some(analysis) = mem0 {
        emit_mem0_clone(out, class_name, analysis, indent);
    } else {
        let _ = writeln!(out, "{tab}virtual {class_name}* clone() {{");
        let _ = writeln!(out, "{tab}    return new {class_name}(*this);");
        let _ = writeln!(out, "{tab}}}");
    }
    if !has_metadata {
        let _ = writeln!(out, "{tab}virtual void metadata(Meta* m) {{");
        let _ = writeln!(out, "{tab}    (void)m;");
        emit_compilation_metadata(out, options, module_name, indent + 1);
        let _ = writeln!(out, "{tab}}}");
    }
    if !has_build_ui {
        let _ = writeln!(
            out,
            "{tab}virtual void buildUserInterface(UI* ui_interface) {{"
        );
        let _ = writeln!(
            out,
            "{tab}    ui_interface->openVerticalBox({});",
            cpp_string_literal(module_name)
        );
        let _ = writeln!(out, "{tab}    ui_interface->closeBox();");
        let _ = writeln!(out, "{tab}}}");
    }
    if !has_compute {
        let _ = writeln!(
            out,
            "{tab}virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {{"
        );
        let _ = writeln!(out, "{tab}    (void)count;");
        let _ = writeln!(out, "{tab}    (void)inputs;");
        let _ = writeln!(out, "{tab}    (void)outputs;");
        let _ = writeln!(out, "{tab}}}");
    }
    if let Some(analysis) = mem0 {
        emit_mem0_manager_methods(out, class_name, analysis, indent);
    }
    Ok(())
}

/// Emits the manager-aware constructors required by `ArrayToPointer`-style
/// state. Every external pointer starts null so partial creation can unwind
/// safely; copy construction is disabled because a bytewise pointer copy would
/// violate clone independence.
fn emit_mem0_constructors(
    out: &mut String,
    class_name: &str,
    analysis: &Mem0Analysis,
    indent: usize,
) {
    let tab = "    ".repeat(indent);
    let buffers = mem0_instance_buffers(analysis);
    let _ = writeln!(out, "{tab}{class_name}() : fOwnerManager(nullptr) {{");
    for zone in &buffers {
        let _ = writeln!(out, "{tab}    {} = nullptr;", zone.name);
    }
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(
        out,
        "{tab}explicit {class_name}(dsp_memory_manager* manager)"
    );
    let _ = writeln!(out, "{tab}    : fOwnerManager(manager) {{");
    for zone in &buffers {
        let _ = writeln!(out, "{tab}    {} = nullptr;", zone.name);
    }
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}{class_name}(const {class_name}&) = delete;");
    let _ = writeln!(
        out,
        "{tab}{class_name}& operator=(const {class_name}&) = delete;"
    );
    let _ = writeln!(out, "{tab}virtual ~{class_name}() = default;");
}

/// Emits class-table allocation before semantic `staticInit`. Each completed
/// allocation is released in reverse order on failure; a failed attempt never
/// publishes `fClassManager`.
fn emit_mem0_class_allocations(out: &mut String, analysis: &Mem0Analysis, indent: usize) {
    let tab = "    ".repeat(indent);
    let zones = mem0_class_tables(analysis);
    for (index, zone) in zones.iter().enumerate() {
        let _ = writeln!(
            out,
            "{tab}{} = static_cast<decltype({})>(fClassManager->allocate({}));",
            zone.name, zone.name, zone.size_bytes
        );
        let _ = writeln!(
            out,
            "{tab}if ({0} == nullptr || (reinterpret_cast<uintptr_t>({0}) % {1}) != 0) {{",
            zone.name, zone.alignment
        );
        let _ = writeln!(out, "{tab}    if ({} != nullptr) {{", zone.name);
        let _ = writeln!(out, "{tab}        fClassManager->destroy({});", zone.name);
        let _ = writeln!(out, "{tab}        {} = nullptr;", zone.name);
        let _ = writeln!(out, "{tab}    }}");
        for prior in zones[..index].iter().rev() {
            let _ = writeln!(out, "{tab}    fClassManager->destroy({});", prior.name);
            let _ = writeln!(out, "{tab}    {} = nullptr;", prior.name);
        }
        let _ = writeln!(out, "{tab}    fClassManager = nullptr;");
        let _ = writeln!(out, "{tab}    return false;");
        let _ = writeln!(out, "{tab}}}");
    }
}

/// Emits deep clone from the same captured allocator. Embedded scalar fields
/// are copied by name; every external buffer receives independent storage and
/// payload bytes. Class/static tables remain shared by design.
fn emit_mem0_clone(out: &mut String, class_name: &str, analysis: &Mem0Analysis, indent: usize) {
    let tab = "    ".repeat(indent);
    let _ = writeln!(out, "{tab}virtual {class_name}* clone() {{");
    let _ = writeln!(
        out,
        "{tab}    {class_name}* copy = createChecked(fOwnerManager);"
    );
    let _ = writeln!(out, "{tab}    if (copy == nullptr) return nullptr;");
    let _ = writeln!(out, "{tab}    copy->fSampleRate = fSampleRate;");
    for zone in analysis
        .memory_layout
        .zones
        .iter()
        .filter(|zone| zone.role == MemoryRole::EmbeddedScalar && zone.name != "fSampleRate")
    {
        let _ = writeln!(out, "{tab}    copy->{0} = {0};", zone.name);
    }
    for zone in mem0_instance_buffers(analysis) {
        let _ = writeln!(
            out,
            "{tab}    std::memcpy(copy->{0}, {0}, {1});",
            zone.name, zone.size_bytes
        );
    }
    let _ = writeln!(out, "{tab}    return copy;");
    let _ = writeln!(out, "{tab}}}");
}

/// Emits the source-compatible C++ manager surface plus additive checked
/// methods. The legacy void entry points terminate on contract violations;
/// hosts that need failure recovery use the `*Checked` variants.
fn emit_mem0_manager_methods(
    out: &mut String,
    class_name: &str,
    analysis: &Mem0Analysis,
    indent: usize,
) {
    let tab = "    ".repeat(indent);
    let buffers = mem0_instance_buffers(analysis);
    let class_tables = mem0_class_tables(analysis);
    let runtime_zones: Vec<_> = analysis
        .memory_layout
        .zones
        .iter()
        .filter(|zone| zone.runtime_allocated)
        .collect();

    let _ = writeln!(
        out,
        "{tab}static bool memoryInfoChecked(dsp_memory_manager* manager) {{"
    );
    let _ = writeln!(out, "{tab}    if (manager == nullptr) return false;");
    let _ = writeln!(out, "{tab}    manager->begin({});", runtime_zones.len());
    for zone in &runtime_zones {
        let size_bytes = match zone.role {
            MemoryRole::DspObject => format!("sizeof({class_name})"),
            MemoryRole::Subcontainer => format!("sizeof({})", zone.name),
            _ => zone.size_bytes.to_string(),
        };
        let _ = writeln!(
            out,
            "{tab}    manager->info({}, dsp_memory_manager::{}, {}, {size_bytes}, {}, {});",
            cpp_string_literal(&zone.name),
            zone.memory_type.legacy_name(),
            zone.element_count,
            zone.reads,
            zone.writes
        );
    }
    let _ = writeln!(out, "{tab}    manager->end();");
    let _ = writeln!(out, "{tab}    return true;");
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}static void memoryInfo() {{");
    let _ = writeln!(
        out,
        "{tab}    if (!memoryInfoChecked(fManager)) std::terminate();"
    );
    let _ = writeln!(out, "{tab}}}");

    let _ = writeln!(out, "{tab}bool memoryCreate() {{");
    let _ = writeln!(out, "{tab}    if (fOwnerManager == nullptr) return false;");
    for (index, zone) in buffers.iter().enumerate() {
        let _ = writeln!(
            out,
            "{tab}    {0} = static_cast<decltype({0})>(fOwnerManager->allocate({1}));",
            zone.name, zone.size_bytes
        );
        let _ = writeln!(
            out,
            "{tab}    if ({0} == nullptr || (reinterpret_cast<uintptr_t>({0}) % {1}) != 0) {{",
            zone.name, zone.alignment
        );
        let _ = writeln!(out, "{tab}        if ({} != nullptr) {{", zone.name);
        let _ = writeln!(
            out,
            "{tab}            fOwnerManager->destroy({});",
            zone.name
        );
        let _ = writeln!(out, "{tab}            {} = nullptr;", zone.name);
        let _ = writeln!(out, "{tab}        }}");
        for prior in buffers[..index].iter().rev() {
            let _ = writeln!(out, "{tab}        fOwnerManager->destroy({});", prior.name);
            let _ = writeln!(out, "{tab}        {} = nullptr;", prior.name);
        }
        let _ = writeln!(out, "{tab}        return false;");
        let _ = writeln!(out, "{tab}    }}");
    }
    let _ = writeln!(out, "{tab}    return true;");
    let _ = writeln!(out, "{tab}}}");

    let _ = writeln!(out, "{tab}void memoryDestroy() {{");
    for zone in buffers.iter().rev() {
        let _ = writeln!(out, "{tab}    if ({} != nullptr) {{", zone.name);
        let _ = writeln!(out, "{tab}        fOwnerManager->destroy({});", zone.name);
        let _ = writeln!(out, "{tab}        {} = nullptr;", zone.name);
        let _ = writeln!(out, "{tab}    }}");
    }
    let _ = writeln!(out, "{tab}}}");

    let _ = writeln!(
        out,
        "{tab}static {class_name}* createChecked(dsp_memory_manager* manager) {{"
    );
    let _ = writeln!(out, "{tab}    if (manager == nullptr) return nullptr;");
    let _ = writeln!(
        out,
        "{tab}    void* storage = manager->allocate(sizeof({class_name}));"
    );
    let _ = writeln!(
        out,
        "{tab}    if (storage == nullptr || (reinterpret_cast<uintptr_t>(storage) % alignof({class_name})) != 0) {{"
    );
    let _ = writeln!(
        out,
        "{tab}        if (storage != nullptr) manager->destroy(storage);"
    );
    let _ = writeln!(out, "{tab}        return nullptr;");
    let _ = writeln!(out, "{tab}    }}");
    let _ = writeln!(
        out,
        "{tab}    {class_name}* dsp = new (storage) {class_name}(manager);"
    );
    let _ = writeln!(out, "{tab}    if (!dsp->memoryCreate()) {{");
    let _ = writeln!(out, "{tab}        dsp->~{class_name}();");
    let _ = writeln!(out, "{tab}        manager->destroy(storage);");
    let _ = writeln!(out, "{tab}        return nullptr;");
    let _ = writeln!(out, "{tab}    }}");
    let _ = writeln!(out, "{tab}    ++fLiveInstances;");
    let _ = writeln!(out, "{tab}    return dsp;");
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}static {class_name}* create() {{");
    let _ = writeln!(out, "{tab}    return createChecked(fManager);");
    let _ = writeln!(out, "{tab}}}");

    let _ = writeln!(out, "{tab}static void destroy(dsp* instance) {{");
    let _ = writeln!(out, "{tab}    if (instance == nullptr) return;");
    let _ = writeln!(
        out,
        "{tab}    {class_name}* typed = static_cast<{class_name}*>(instance);"
    );
    let _ = writeln!(
        out,
        "{tab}    dsp_memory_manager* owner = typed->fOwnerManager;"
    );
    let _ = writeln!(out, "{tab}    typed->memoryDestroy();");
    let _ = writeln!(out, "{tab}    typed->~{class_name}();");
    let _ = writeln!(out, "{tab}    owner->destroy(typed);");
    let _ = writeln!(out, "{tab}    --fLiveInstances;");
    let _ = writeln!(out, "{tab}}}");

    let _ = writeln!(out, "{tab}static bool classDestroyChecked() {{");
    let _ = writeln!(out, "{tab}    if (fLiveInstances != 0) return false;");
    let _ = writeln!(out, "{tab}    if (fClassManager == nullptr) return true;");
    for zone in class_tables.iter().rev() {
        let _ = writeln!(out, "{tab}    if ({} != nullptr) {{", zone.name);
        let _ = writeln!(out, "{tab}        fClassManager->destroy({});", zone.name);
        let _ = writeln!(out, "{tab}        {} = nullptr;", zone.name);
        let _ = writeln!(out, "{tab}    }}");
    }
    let _ = writeln!(out, "{tab}    fClassManager = nullptr;");
    let _ = writeln!(out, "{tab}    fClassSampleRate = 0;");
    let _ = writeln!(out, "{tab}    return true;");
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}static void classDestroy() {{");
    let _ = writeln!(
        out,
        "{tab}    if (!classDestroyChecked()) std::terminate();"
    );
    let _ = writeln!(out, "{tab}}}");
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

/// Collects declared function names to decide which DSP API stubs to synthesize.
fn collect_module_function_names(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<String>, CodegenError> {
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
        if let FirMatch::DeclareFun { name, .. } = match_fir(store, item) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Emits the generated-file prologue and platform macros.
fn emit_cpp_header(
    out: &mut String,
    class_name: &str,
    module_name: &str,
    compile_options: Option<&str>,
) {
    let _ = writeln!(
        out,
        "/* ------------------------------------------------------------"
    );
    let _ = writeln!(out, "name: {}", cpp_string_literal(module_name));
    let _ = writeln!(
        out,
        "Code generated with Faust {} (https://faust.grame.fr)",
        crate::VERSION
    );
    let _ = writeln!(
        out,
        "Compilation options: {}",
        compile_options.unwrap_or("-lang cpp")
    );
    let _ = writeln!(
        out,
        "------------------------------------------------------------ */"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifndef  __{class_name}_H__");
    let _ = writeln!(out, "#define  __{class_name}_H__");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifndef FAUSTFLOAT");
    let _ = writeln!(out, "#define FAUSTFLOAT float");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#include <algorithm>");
    let _ = writeln!(out, "#include <cstddef>");
    let _ = writeln!(out, "#include <cmath>");
    let _ = writeln!(out, "#include <cstdint>");
    let _ = writeln!(out, "#include <cstring>");
    let _ = writeln!(out, "#include <exception>");
    let _ = writeln!(out, "#include <new>");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifndef FAUSTCLASS");
    let _ = writeln!(out, "#define FAUSTCLASS {class_name}");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifdef __APPLE__");
    let _ = writeln!(out, "#define exp10f __exp10f");
    let _ = writeln!(out, "#define exp10 __exp10");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
    let _ = writeln!(out, "#if defined(_WIN32)");
    let _ = writeln!(out, "#define RESTRICT __restrict");
    let _ = writeln!(out, "#else");
    let _ = writeln!(out, "#define RESTRICT __restrict__");
    let _ = writeln!(out, "#endif");
    let _ = writeln!(out);
}

/// Emits one FIR module section (`dsp_struct`, `globals`, or `functions`).
fn emit_section(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    section_name: &str,
    section_id: FirId,
    externalize_mem0_arrays: bool,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, section_id) else {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidModuleSection,
            format!(
                "section '{section_name}' must be a FIR block, got {:?} at node {}",
                match_fir(store, section_id),
                section_id.as_u32()
            ),
        ));
    };

    for item in items {
        if section_name == "globals"
            && matches!(match_fir(store, item), FirMatch::DeclareFun { .. })
        {
            continue;
        }
        // `staticInit` is rendered as the body of `classInit`, not as a method
        // of its own.
        if section_name == "functions"
            && matches!(match_fir(store, item), FirMatch::DeclareFun { ref name, .. } if name == "staticInit")
        {
            continue;
        }
        if options.memory_manager_mode.is_mem0()
            && !externalize_mem0_arrays
            && matches!(
                match_fir(store, item),
                FirMatch::DeclareVar {
                    typ: FirType::Array(_, _) | FirType::Vector(_, _),
                    access: fir::AccessType::Struct,
                    ..
                } | FirMatch::DeclareTable {
                    access: fir::AccessType::Struct,
                    ..
                }
            )
        {
            // Faust mode zero externalizes the main DSP arrays, while
            // generated-table helper arrays remain embedded in the temporary
            // helper object covered by its single manager allocation.
            let mut embedded_options = options.clone();
            embedded_options.memory_manager_mode = MemoryManagerMode::None;
            emit_stmt(store, out, &embedded_options, module_name, item, 1)?;
        } else {
            emit_stmt(store, out, options, module_name, item, 1)?;
        }
    }
    Ok(())
}

/// Emits one FIR statement in default rendering mode.
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    stmt: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let mut mode = EmitMode::Default;
    emit_stmt_with_mode(store, out, options, module_name, stmt, indent, &mut mode)
}

/// Renders the increment of a non-reverse `ForLoop` in C++ style
/// (`i += step`; the `c` backend spells this `i = i + step`).
fn cpp_for_loop_step(var: &str, step: &str) -> String {
    format!("{var} += {step}")
}

/// Renders the increment of a non-reverse `SimpleForLoop` in C++ style
/// (`++i`; the `c` backend spells this `i = i + 1`).
fn cpp_simple_loop_increment(var: &str) -> String {
    format!("++{var}")
}

/// Emits one FIR statement into generated C++ text.
///
/// The arms shared with the `c` backend live in
/// [`c_family::emit_stmt_common`]; only the C++-specific arms remain here:
/// `DeclareFun` (methods nest inside the class body) and `AddMetaDeclare`
/// (C++'s `Meta`/`UI` interfaces take no glue handle and omit the zone
/// argument for module-level declares). `Label` is deliberately silent in
/// this backend. The former `DeclareStructType`/`DeclareBufferIterators`/
/// `ShiftArrayVar`/`IteratorForLoop` comment stubs were removed (plan §4
/// Phase 4 single-owner decision): both backends now fail loudly on these
/// unproduced FIR nodes, per the `backends` module contract, instead of C++
/// silently emitting placeholder comments (`IteratorForLoop` even unrolled
/// its body once — wrong code that compiled).
fn emit_stmt_with_mode(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    stmt: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    if options.memory_manager_mode.is_mem0() {
        let tab = "    ".repeat(indent);
        match match_fir(store, stmt) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(elem, _),
                access: fir::AccessType::Struct,
                ..
            }
            | FirMatch::DeclareVar {
                name,
                typ: FirType::Vector(elem, _),
                access: fir::AccessType::Struct,
                ..
            } => {
                let _ = writeln!(out, "{tab}{}* {name};", emit_type(&elem, options));
                return Ok(());
            }
            FirMatch::DeclareTable {
                name,
                access: fir::AccessType::Struct,
                elem_type,
                ..
            } => {
                let _ = writeln!(out, "{tab}{}* {name};", emit_type(&elem_type, options));
                return Ok(());
            }
            _ => {}
        }
    }
    let ctx = c_family::CFamilyStmtCtx {
        syntax: &SYNTAX,
        var_ref: emit_var_ref,
        for_loop_step: cpp_for_loop_step,
        simple_loop_increment: cpp_simple_loop_increment,
        render_named_type: &|typ, name| emit_named_type(typ, name, options),
        render_void_call: &|name, args| {
            // A sub-container entry point is a method: the first argument is
            // the receiver, and C++ spells the call `sig0->fill…(rest)`.
            if !is_sub_module_method(name) {
                return None;
            }
            let (receiver, rest) = args.split_first()?;
            let receiver = emit_value(store, options, *receiver).ok()?;
            let rendered: Vec<String> = rest
                .iter()
                .map(|arg| emit_value(store, options, *arg))
                .collect::<Result<_, _>>()
                .ok()?;
            Some(format!("{receiver}->{name}({})", rendered.join(", ")))
        },
        render_type: &|typ| emit_type(typ, options),
        render_value: &|value| emit_value(store, options, value),
        emit_block: &|out, block, indent, mode| {
            emit_block_with_mode(store, out, options, module_name, block, indent, mode)
        },
        emit_stmt: &|out, stmt, indent, mode| {
            emit_stmt_with_mode(store, out, options, module_name, stmt, indent, mode)
        },
    };
    if let Some(result) = c_family::emit_stmt_common(store, out, &ctx, stmt, indent, mode) {
        return result;
    }
    let tab = "    ".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            is_inline,
        } => emit_declare_fun(
            store,
            out,
            options,
            module_name,
            DeclareFunView {
                name: &name,
                typ: &typ,
                named_args: &args,
                body,
                is_inline,
            },
            indent,
        ),
        FirMatch::Label(label) => {
            let _ = label;
            Ok(())
        }
        FirMatch::AddMetaDeclare { var, key, value } => {
            match mode {
                EmitMode::Ui => {
                    let zone = if var == "0" {
                        "0".to_owned()
                    } else {
                        format!("&{var}")
                    };
                    let _ = writeln!(
                        out,
                        "{tab}ui_interface->declare({zone}, {}, {});",
                        cpp_string_literal(&key),
                        cpp_string_literal(&value)
                    );
                }
                EmitMode::Default | EmitMode::Metadata | EmitMode::Compute => {
                    if var == "0" {
                        let _ = writeln!(
                            out,
                            "{tab}m->declare({}, {});",
                            cpp_string_literal(&key),
                            cpp_string_literal(&value)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "{tab}m->declare(&{var}, {}, {});",
                            cpp_string_literal(&key),
                            cpp_string_literal(&value)
                        );
                    }
                }
            }
            Ok(())
        }
        _ => Err(unsupported_node("statement", stmt, store)),
    }
}

/// Emits a FIR block in default rendering mode.
fn emit_block(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    block: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let mut mode = EmitMode::Default;
    emit_block_with_mode(store, out, options, module_name, block, indent, &mut mode)
}

/// Emits every statement in a FIR block under the active rendering mode.
fn emit_block_with_mode(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    block: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return Err(unsupported_node("expected block", block, store));
    };
    for stmt in items {
        emit_stmt_with_mode(store, out, options, module_name, stmt, indent, mode)?;
    }
    Ok(())
}

/// Returns `true` when `block` declares a variable named `name`.
fn block_declares_var(store: &FirStore, block: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return false;
    };
    items.iter().any(|id| {
        matches!(
            match_fir(store, *id),
            FirMatch::DeclareVar { name: var_name, .. } if var_name == name
        )
    })
}

/// Returns `true` when `block` stores to a variable named `name`.
fn block_stores_var(store: &FirStore, block: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return false;
    };
    items.iter().any(|id| {
        matches!(
            match_fir(store, *id),
            FirMatch::StoreVar { name: var_name, .. } if var_name == name
        )
    })
}

/// Returns `true` for a generated sub-container entry point.
///
/// These are emitted as methods of the sub-container class, so their explicit
/// `dsp` receiver argument is stripped exactly as for the DSP API methods.
fn is_sub_module_method(name: &str) -> bool {
    name.starts_with("instanceInit") && name != "instanceInit"
        || name.starts_with("fill") && name != "fill"
}

/// Returns the `(variable, sub-module)` pairs allocated inside one block.
///
/// The FIR carries the allocation but not the release: freeing is bound to how
/// each language allocates, and upstream itself skips it for Rust, Julia and
/// AssemblyScript. C++ emits `delete<Sub>(sigN)` once the fill has run.
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

/// Returns the body of the named function declared by a module, if any.
fn find_function_body(store: &FirStore, functions: FirId, wanted: &str) -> Option<FirId> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return None;
    };
    items
        .into_iter()
        .find_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun { name, body, .. } if name == wanted => body,
            _ => None,
        })
}

/// Emits every generated-table sub-container as a nested class.
///
/// C++ parity: `generateSigGen` / `generateStaticSigGen` produce a
/// `CodeContainer` per `SIGGEN`, which `produceInternal` emits as a class with
/// its own state, `instanceInit<Sub>` and `fill<Sub>`, plus `new`/`delete`
/// helpers. Keeping the nested form — rather than inlining the generator into
/// `classInit` — is what lets `classInit` stay `static`: the sub-container is
/// a local of that function, so it needs no instance to live in.
///
/// Sub-modules are emitted deepest-first: a generator that reads another
/// generated table calls its class, which must already be declared.
fn emit_sub_modules(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
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

        emit_sub_modules(store, out, options, module_name, nested)?;
        emit_static_tables(store, out, options, static_decls)?;

        let _ = writeln!(out, "class {name} {{");
        let _ = writeln!(out);
        let _ = writeln!(out, "  private:");
        let _ = writeln!(out);
        if options.memory_manager_mode.is_mem0() {
            let _ = writeln!(out, "    dsp_memory_manager* fClassManager;");
        }
        emit_section(
            store,
            out,
            options,
            module_name,
            "dsp_struct",
            dsp_struct,
            false,
        )?;
        emit_section(store, out, options, module_name, "globals", globals, false)?;
        let _ = writeln!(out);
        let _ = writeln!(out, "  public:");
        let _ = writeln!(out);
        if options.memory_manager_mode.is_mem0() {
            let _ = writeln!(
                out,
                "    explicit {name}(dsp_memory_manager* manager) : fClassManager(manager) {{}}"
            );
            let _ = writeln!(
                out,
                "    dsp_memory_manager* ownerManager() const {{ return fClassManager; }}"
            );
        }
        // Arity getters exist for reference parity; a generator is always
        // 0-input / 1-output by construction, so they are derived rather than
        // read from FIR.
        let _ = writeln!(out, "    int getNumInputs{name}() {{");
        let _ = writeln!(out, "        return 0;");
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out, "    int getNumOutputs{name}() {{");
        let _ = writeln!(out, "        return 1;");
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
        emit_section(
            store,
            out,
            options,
            module_name,
            "functions",
            functions,
            false,
        )?;
        let _ = writeln!(out, "}};");
        let _ = writeln!(out);
        if options.memory_manager_mode.is_mem0() {
            let _ = writeln!(
                out,
                "static {name}* new{name}(dsp_memory_manager* manager) {{"
            );
            let _ = writeln!(
                out,
                "    void* storage = manager->allocate(sizeof({name}));"
            );
            let _ = writeln!(
                out,
                "    if (storage == nullptr || (reinterpret_cast<uintptr_t>(storage) % alignof({name})) != 0) {{"
            );
            let _ = writeln!(
                out,
                "        if (storage != nullptr) manager->destroy(storage);"
            );
            let _ = writeln!(out, "        throw std::bad_alloc();");
            let _ = writeln!(out, "    }}");
            let _ = writeln!(out, "    return new (storage) {name}(manager);");
            let _ = writeln!(out, "}}");
            let _ = writeln!(out, "static void delete{name}({name}* dsp) {{");
            let _ = writeln!(out, "    if (dsp == nullptr) return;");
            let _ = writeln!(out, "    dsp_memory_manager* owner = dsp->ownerManager();");
            let _ = writeln!(out, "    dsp->~{name}();");
            let _ = writeln!(out, "    owner->destroy(dsp);");
            let _ = writeln!(out, "}}");
        } else {
            let _ = writeln!(
                out,
                "static {name}* new{name}() {{ return ({name}*)new {name}(); }}"
            );
            let _ = writeln!(
                out,
                "static void delete{name}({name}* dsp) {{ delete dsp; }}"
            );
        }
        let _ = writeln!(out);
    }
    Ok(())
}

/// Emits one FIR function declaration or method definition into the generated class.
fn emit_declare_fun(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    decl: DeclareFunView<'_>,
    indent: usize,
) -> Result<(), CodegenError> {
    faust_api::validate_canonical_dsp_api_signature(decl.name, decl.typ, decl.named_args)
        .map_err(|msg| CodegenError::new(CodegenErrorCode::InvalidModuleSection, msg))?;
    let tab = "    ".repeat(indent);
    let mut params_override: Option<String> = None;
    let strip_explicit_dsp_arg = (is_dsp_api_method(decl.name) || is_sub_module_method(decl.name))
        && matches!(decl.named_args.first(), Some(named) if named.name == "dsp")
        && matches!(
            decl.typ,
            FirType::Fun { args, .. }
                if matches!(args.first(), Some(FirType::Ptr(inner)) if matches!(inner.as_ref(), FirType::Obj))
        );
    let (ret, mut params) = match decl.typ {
        FirType::Fun {
            args: typed_args,
            ret,
        } => {
            let ret = emit_type(ret, options);
            let skip = usize::from(strip_explicit_dsp_arg);
            let render_args = &typed_args[skip..];
            let mut rendered = Vec::with_capacity(render_args.len());
            for (index, arg_type) in render_args.iter().enumerate() {
                let named_index = index + skip;
                let name = decl
                    .named_args
                    .get(named_index)
                    .map_or_else(|| format!("arg{named_index}"), |named| named.name.clone());
                rendered.push(emit_named_type(arg_type, &name, options));
            }
            (ret, rendered.join(", "))
        }
        other => (emit_type(other, options), String::new()),
    };
    if decl.name == "buildUserInterface" && params.is_empty() {
        params_override = Some("UI* ui_interface".to_owned());
    } else if decl.name == "metadata" && params.is_empty() {
        params_override = Some("Meta* m".to_owned());
    } else if decl.name == "compute"
        && (params.is_empty() || faust_api::is_canonical_compute_signature(decl.typ))
    {
        params_override = Some(
            "int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs".to_owned(),
        );
    } else if decl.name == "frame" {
        params_override =
            Some("FAUSTFLOAT* RESTRICT inputs, FAUSTFLOAT* RESTRICT outputs".to_owned());
    }
    if let Some(override_params) = params_override {
        params = override_params;
    }
    let is_dsp_api = is_dsp_api_method(decl.name);
    // The pinned reference emits `void control()` as a plain method but
    // `frame`/`compute` as virtual (cpp_code_container.cpp at 8eebea429).
    let method_prefix = if is_dsp_api && decl.name != "control" {
        "virtual "
    } else {
        ""
    };
    let inline = if decl.is_inline { "inline " } else { "" };
    // Prototype-only (no body): emit a forward declaration / pure-virtual signature.
    let Some(body) = decl.body else {
        let _ = writeln!(
            out,
            "{tab}{inline}{method_prefix}{ret} {}({params});",
            decl.name
        );
        return Ok(());
    };
    let _ = writeln!(
        out,
        "{tab}{inline}{method_prefix}{ret} {}({params}) {{",
        decl.name
    );
    if decl.name == "instanceConstants" {
        if !block_stores_var(store, body, "fSampleRate") {
            let _ = writeln!(out, "{tab}    fSampleRate = sample_rate;");
        }
        emit_block(store, out, options, module_name, body, indent + 1)?;
        for (var, sub) in allocated_sub_containers(store, body) {
            let _ = writeln!(out, "{tab}    delete{sub}({var});");
        }
    } else if decl.name == "compute" {
        emit_compute_body(store, out, options, body, indent + 1)?;
    } else if decl.name == "metadata" && is_empty_block(store, body) {
        let _ = writeln!(out, "{tab}    (void)m;");
        emit_compilation_metadata(out, options, module_name, indent + 1);
    } else if decl.name == "buildUserInterface" && is_empty_block(store, body) {
        let _ = writeln!(
            out,
            "{tab}    ui_interface->openVerticalBox({});",
            cpp_string_literal(module_name)
        );
        let _ = writeln!(out, "{tab}    ui_interface->closeBox();");
    } else {
        let mut mode = match decl.name {
            "metadata" => EmitMode::Metadata,
            "buildUserInterface" => EmitMode::Ui,
            _ => EmitMode::Default,
        };
        emit_block_with_mode(
            store,
            out,
            options,
            module_name,
            body,
            indent + 1,
            &mut mode,
        )?;
    }
    let _ = writeln!(out, "{tab}}}");
    Ok(())
}

fn emit_compilation_metadata(
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    indent: usize,
) {
    let filename = options
        .metadata_filename
        .clone()
        .unwrap_or_else(|| format!("{module_name}.dsp"));
    let name = options
        .metadata_name
        .clone()
        .unwrap_or_else(|| module_name.to_owned());
    let tab = "    ".repeat(indent);
    for (key, value) in
        c_family::ordered_compilation_metadata(&options.metadata_entries, filename, name)
    {
        let _ = writeln!(
            out,
            "{tab}m->declare({}, {});",
            cpp_string_literal(&key),
            cpp_string_literal(&value)
        );
    }
}

/// Emits the FIR `compute` body as-is.
///
/// The fast-lane now emits an explicit FIR sample loop (`SimpleForLoop/ForLoop`)
/// inside `compute`, so the C++ backend must not synthesize an extra `i0` loop.
fn emit_compute_body(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    body: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let mut mode = EmitMode::Compute;
    emit_block_with_mode(store, out, options, "", body, indent, &mut mode)
}

/// Returns `true` when `name` belongs to the canonical Faust DSP API surface.
fn is_dsp_api_method(name: &str) -> bool {
    matches!(
        name,
        "metadata"
            | "instanceConstants"
            | "instanceResetUserInterface"
            | "instanceClear"
            | "buildUserInterface"
            | "compute"
            | "control"
            | "frame"
    )
}

/// Returns `true` when `body` is an empty FIR block.
fn is_empty_block(store: &FirStore, body: FirId) -> bool {
    match match_fir(store, body) {
        FirMatch::Block(items) => items.is_empty(),
        _ => false,
    }
}

/// Emits one FIR value expression into a C++ expression string.
/// Renders a variable reference in C++ method context: bare `name`, because
/// struct state is reachable through the implicit `this`.
fn emit_var_ref(name: &str, _access: fir::AccessType) -> String {
    name.to_owned()
}

/// Emits one FIR value expression into a C++ expression string.
///
/// The arms shared with the `c` backend live in
/// [`c_family::emit_value_common`] — including `Bitcast`, which now renders
/// as `*reinterpret_cast<T*>(&v)` from the [`SYNTAX`] leaves, matching
/// upstream `-ftz 2` output (DRIFT 2 closure, C-family plan §2.2; the former
/// `bitcast<T>(v)` spelling named a helper neither this backend nor upstream
/// defines). Only the C++-specific arms (`Quad`/`FixedPoint`/array literals,
/// `NewDsp`) remain here.
fn emit_value(
    store: &FirStore,
    options: &CppOptions,
    value: FirId,
) -> Result<String, CodegenError> {
    let ctx = c_family::CFamilyValueCtx {
        syntax: &SYNTAX,
        var_ref: emit_var_ref,
        fun_name: emit_cpp_fun_name,
        render_type: &|typ| emit_type(typ, options),
        recurse: &|nested| emit_value(store, options, nested),
    };
    if let Some(result) = c_family::emit_value_common(store, &ctx, value) {
        return result;
    }
    match match_fir(store, value) {
        FirMatch::Quad { value, .. } => Ok(trim_float(value)),
        FirMatch::FixedPoint { value, .. } => Ok(trim_float(value)),
        FirMatch::ValueArray { values, .. } => {
            let mut out = String::from("{");
            for (index, item) in values.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&emit_value(store, options, *item)?);
            }
            out.push('}');
            Ok(out)
        }
        FirMatch::Int32Array { values, .. } => {
            Ok(format_array(values.iter().map(ToString::to_string)))
        }
        FirMatch::Float32Array { values, .. } => Ok(format_array(
            values.iter().map(|v| format_float32(f64::from(*v))),
        )),
        FirMatch::Float64Array { values, .. }
        | FirMatch::QuadArray { values, .. }
        | FirMatch::FixedPointArray { values, .. } => {
            Ok(format_array(values.iter().map(|v| trim_float(*v))))
        }
        FirMatch::NewDsp { name, .. } => {
            if options.memory_manager_mode.is_mem0() {
                Ok(format!("new{name}(fClassManager)"))
            } else {
                Ok(format!("new{name}()"))
            }
        }
        _ => Err(unsupported_node("value", value, store)),
    }
}

/// Renders a C++ declarator: `<base type> <name><array suffix>`.
///
/// C array bounds are part of the declarator, not the type prefix (`float
/// buf[8];`, not `float[8] buf;`), so this cannot reuse [`emit_type`]
/// directly for array-typed declarations; it defers to
/// [`emit_type_base_and_suffix`] to peel the bracketed suffix off first.
fn emit_named_type(typ: &FirType, name: &str, options: &CppOptions) -> String {
    let mut suffix = String::new();
    let base = emit_type_base_and_suffix(typ, options, &mut suffix);
    format!("{base} {name}{suffix}")
}

/// Recursively splits an array type into its element base type and the
/// accumulated `[size]...` declarator suffix, appending to `suffix` for each
/// nested array dimension. Non-array types are rendered directly via
/// [`emit_type`] with an untouched (typically empty) `suffix`.
fn emit_type_base_and_suffix(typ: &FirType, options: &CppOptions, suffix: &mut String) -> String {
    match typ {
        FirType::Array(inner, size) => {
            suffix.push_str(&format!("[{size}]"));
            emit_type_base_and_suffix(inner, options, suffix)
        }
        _ => emit_type(typ, options),
    }
}

/// Maps bare FIR math names to the appropriate C++ symbol spelling.
fn emit_cpp_fun_name(name: &str) -> String {
    if name.contains("::") {
        return name.to_owned();
    }
    match name {
        "abs" => return "std::abs".to_owned(),
        "min_i" => return "std::min<int>".to_owned(),
        "max_i" => return "std::max<int>".to_owned(),
        _ => {}
    }
    match FirMathOp::from_symbol(name) {
        Some(FirMathOp::Exp10) => "exp10".to_owned(),
        Some(op) => format!("std::{}", op.symbol()),
        None => name.to_owned(),
    }
}

/// Renders a FIR type into the current C++ backend spelling.
///
/// Shared with the `c` backend via [`c_family::emit_type`]: the C++-specific
/// leaves (`bool`/`UI*`/`Meta*`) come from [`SYNTAX`], the configurable
/// `Quad`/`FixedPoint` spellings from `options`.
fn emit_type(typ: &FirType, options: &CppOptions) -> String {
    c_family::emit_type(
        typ,
        &SYNTAX,
        &options.quad_type_name,
        &options.fixed_type_name,
    )
}

/// Builds a stable unsupported-node diagnostic for the C++ emitter.
fn unsupported_node(kind: &str, node: FirId, store: &FirStore) -> CodegenError {
    CodegenError::new(
        CodegenErrorCode::UnsupportedNode,
        format!(
            "unsupported FIR {kind} node {:?} at {}",
            match_fir(store, node),
            node.as_u32()
        ),
    )
    .at_node(node)
}

/// Formats a floating-point literal with stable C++ syntax.
///
/// Shared with the `c` backend via [`c_family::trim_float`]. Phase 2 of the
/// C-family plan fixed the `cpp` drift here: `-0.0` now normalizes to `0.0`,
/// matching `c`, `julia`, and the upstream C++ compiler.
fn trim_float(value: f64) -> String {
    c_family::trim_float(value)
}

/// Formats one single-precision literal (`{value}f`), shared via
/// [`c_family::format_float32`].
fn format_float32(value: f64) -> String {
    c_family::format_float32(value)
}

/// Renders an initializer-list literal from already-rendered elements.
fn format_array(values: impl Iterator<Item = String>) -> String {
    format!("{{{}}}", values.collect::<Vec<_>>().join(", "))
}

/// Escapes a Rust string into a C++ string literal.
///
/// Shared with the `c` backend via [`c_family::string_literal`]. Phase 2 of
/// the C-family plan fixed the `cpp` drift here: `\r`/`\t` are now escaped
/// instead of emitted as raw bytes, matching `c` and `julia`.
fn cpp_string_literal(value: &str) -> String {
    c_family::string_literal(value)
}

/// Emits `DeclareTable(AccessType::Static)` nodes as `const static` arrays
/// with inline initializers, placed before the class definition.
///
/// Shared with the `c` backend via [`c_family::emit_static_tables`]; the
/// C++-specific `const static` keyword order comes from [`SYNTAX`], element
/// values render through this backend's [`emit_value`].
fn emit_static_tables(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
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
                access: fir::AccessType::Static,
                init: None,
            } = match_fir(store, item)
            {
                let _ = writeln!(
                    out,
                    "static {}* {name} = nullptr;",
                    emit_type(&elem, options)
                );
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
                        "const static {} {name}[{}] = {{{}}};",
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
                    )
                    .at_node(item));
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

/// Decodes the FIR module header expected by the C++ emitter.
fn decode_module(store: &FirStore, module: FirId) -> Result<ModuleView, CodegenError> {
    match match_fir(store, module) {
        FirMatch::Module {
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
        } => Ok(ModuleView {
            name,
            dsp_struct,
            globals,
            functions,
            num_inputs,
            num_outputs,
            static_decls,
            sub_modules,
        }),
        _ => Err(CodegenError::new(
            CodegenErrorCode::RootNotModule,
            format!(
                "expected FIR module root, got {:?} at node {}",
                match_fir(store, module),
                module.as_u32()
            ),
        )),
    }
}

#[must_use]
/// Returns the stable backend identifier (`"cpp"`).
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use fir::{FirBinOp, FirBuilder};

    #[test]
    /// DRIFT 3 regression (C-family plan §2.3): a constant folded to `-0.0`
    /// must emit `0.0`, matching the `c`/`julia` backends and the upstream
    /// C++ compiler (which emits `0.0f` for `process = -0.0;`).
    fn trim_float_normalizes_negative_zero_like_c_and_upstream() {
        assert_eq!(trim_float(-0.0), "0.0");
        assert_eq!(format_float32(-0.0), "0.0f");
    }

    #[test]
    /// DRIFT 4 regression (C-family plan §2.4): tabs/carriage returns in
    /// user-authored strings (UI labels, metadata) must be escaped in the
    /// emitted C++ literal, matching the `c`/`julia` backends, instead of
    /// being copied through as raw bytes.
    fn string_literal_escapes_tab_and_carriage_return() {
        assert_eq!(cpp_string_literal("a\tb"), "\"a\\tb\"");
        assert_eq!(cpp_string_literal("a\rb"), "\"a\\rb\"");
    }

    #[test]
    /// DRIFT 1 regression (C-family plan §2.1): a function-local
    /// (`AccessType::Stack`) `DeclareTable` carrying literal values must emit
    /// its initializer list — this backend previously sized the array from
    /// `values.len()` but silently dropped the values themselves, producing
    /// C++ that compiled but read zero-filled storage. Struct-access
    /// declarations (class fields) stay bare, as before.
    fn local_declare_table_emits_initializer_values() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let v0 = b.int32(3);
        let v1 = b.int32(7);
        let local = b.declare_table("tbl", fir::AccessType::Stack, FirType::Int32, &[v0, v1]);
        let field = b.declare_table(
            "fVec0",
            fir::AccessType::Struct,
            FirType::Float32,
            &[v0, v1],
        );

        let options = CppOptions::default();
        let mut out = String::new();
        let mut mode = EmitMode::Default;
        emit_stmt_with_mode(&store, &mut out, &options, "mydsp", local, 1, &mut mode)
            .expect("local table emits");
        assert_eq!(out, "    int tbl[2] = {3, 7};\n");

        let mut out = String::new();
        emit_stmt_with_mode(&store, &mut out, &options, "mydsp", field, 1, &mut mode)
            .expect("struct field emits");
        assert_eq!(out, "    float fVec0[2];\n");
    }

    #[test]
    /// Plan §4 Phase 4 single-owner decision: FIR nodes with no producer
    /// (`IteratorForLoop`, `DeclareStructType`, …) fail loudly in both
    /// C-family backends instead of C++ emitting placeholder comments
    /// (`IteratorForLoop` even unrolled its body once — wrong code that
    /// compiled).
    fn unproduced_statement_nodes_fail_loudly() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let body = b.block(&[]);
        let loop_stmt = b.iterator_for_loop(&["it0"], false, body);

        let options = CppOptions::default();
        let mut out = String::new();
        let mut mode = EmitMode::Default;
        let err = emit_stmt_with_mode(&store, &mut out, &options, "mydsp", loop_stmt, 1, &mut mode)
            .expect_err("IteratorForLoop must be rejected");
        assert_eq!(err.code(), CodegenErrorCode::UnsupportedNode);
    }

    #[test]
    /// DRIFT 6 regression (C-family plan §2.6): when the FIR module supplies
    /// no explicit `instanceResetUserInterface`, the synthesized fallback
    /// must replay declared state initializers — matching the `c` backend,
    /// which emits `dsp->fFreq = (FAUSTFLOAT)(440.0);` for the same fixture —
    /// instead of leaving the body empty (UI-bound state stuck at zero).
    fn synthesized_reset_ui_replays_declared_state_initializers() {
        let (store, module) = crate::fixtures::build_sine_phasor_test_module();
        let out =
            generate_cpp_module(&store, module, &CppOptions::default()).expect("fixture generates");
        let reset_body = out
            .split("virtual void instanceResetUserInterface() {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("synthesized reset method present");
        assert!(reset_body.contains("fFreq = (FAUSTFLOAT)(440.0);"));
        assert!(reset_body.contains("fGain = (FAUSTFLOAT)(0.2);"));
        assert!(reset_body.contains("fPhase = (double)(0.0);"));
    }

    #[test]
    /// DRIFT 2 regression (C-family plan §2.2): `Bitcast` renders as
    /// `*reinterpret_cast<T*>(&v)`, byte-matching the upstream C++ compiler's
    /// `-ftz 2` output (`*reinterpret_cast<int*>(&fTemp0SE)`); the former
    /// `bitcast<T>(v)` spelling named a template neither this backend's
    /// header nor upstream defines, so it could not even compile if reached.
    fn bitcast_renders_upstream_reinterpret_cast_form() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let operand = b.load_var("fTemp0", fir::AccessType::Stack, FirType::Float32);
        let bitcast = b.bitcast(FirType::Int32, operand);

        let options = CppOptions::default();
        let rendered = emit_value(&store, &options, bitcast).expect("Bitcast renders");
        assert_eq!(rendered, "*reinterpret_cast<int*>(&fTemp0)");
    }

    #[test]
    /// Verifies the backend rejects non-module FIR roots with the stable error code.
    fn rejects_non_module_root() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let not_module = b.int32(7);
        let err = generate_cpp_module(&store, not_module, &CppOptions::default())
            .expect_err("non-module root must fail");
        assert_eq!(err.code(), CodegenErrorCode::RootNotModule);
        assert!(err.to_string().contains("FRS-CGEN-CPP-0001"));
    }

    #[test]
    /// Verifies a minimal FIR module emits the expected C++ shell.
    fn accepts_module_root() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
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

        let out = generate_cpp_module(&store, module, &CppOptions::default())
            .expect("module root should generate");
        assert!(out.contains("#define FAUSTCLASS mydsp"));
        assert!(out.contains("class mydsp : public dsp"));
        assert!(out.contains("virtual int getNumInputs()"));
        assert!(out.contains("virtual int getNumOutputs()"));
        assert!(out.contains("virtual void buildUserInterface(UI* ui_interface)"));
        assert!(out.contains(
            "virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs)"
        ));
        assert!(out.contains("#ifndef  __mydsp_H__"));
        assert!(out.contains("#include <cmath>"));
        assert!(out.contains(&format!(
            "Code generated with Faust {} (https://faust.grame.fr)",
            crate::VERSION
        )));
        assert!(out.contains("Compilation options: -lang cpp"));
        assert!(out.contains("\n#endif\n"));
    }

    #[test]
    fn custom_super_class_name_overrides_public_base() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
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
        let options = CppOptions {
            super_class_name: Some("faust_dsp".to_owned()),
            ..CppOptions::default()
        };

        let out =
            generate_cpp_module(&store, module, &options).expect("module root should generate");
        assert!(out.contains("class mydsp : public faust_dsp"));
        assert!(!out.contains("class mydsp : public dsp"));
    }

    #[test]
    /// Verifies malformed module sections are rejected before emission.
    fn rejects_non_block_module_section() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let dsp_struct = b.int32(1);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
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
        let err = generate_cpp_module(&store, module, &CppOptions::default())
            .expect_err("non-block section must fail");
        assert_eq!(err.code(), CodegenErrorCode::InvalidModuleSection);
        assert!(err.to_string().contains("FRS-CGEN-CPP-0002"));
    }

    #[test]
    /// Verifies the current statement/value slice emits the expected control constructs.
    fn emits_core_statement_and_value_slice() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);

        let one = b.int32(1);
        let two = b.int32(2);
        let sum = b.binop(FirBinOp::Add, one, two, FirType::Int32);
        let dec = b.declare_var("acc", FirType::Int32, fir::AccessType::Stack, Some(sum));
        let acc = b.load_var("acc", fir::AccessType::Stack, FirType::Int32);
        let sixteen = b.int32(16);
        let cond = b.binop(FirBinOp::Lt, acc, sixteen, FirType::Bool);
        let neg_acc = b.neg(acc, FirType::Int32);
        let then_store = b.store_var("acc", fir::AccessType::Stack, neg_acc);
        let then_block = b.block(&[then_store]);
        let branch = b.if_(cond, then_block, None);
        let loop_drop = b.drop_(acc);
        let loop_body = b.block(&[loop_drop]);
        let four = b.int32(4);
        let loop_ = b.simple_for_loop("i", four, loop_body, false);
        let while_drop = b.drop_(acc);
        let while_body = b.block(&[while_drop]);
        let while_ = b.while_loop(cond, while_body);
        let switch_drop = b.drop_(acc);
        let switch_case = b.block(&[switch_drop]);
        let switch_default = b.block(&[]);
        let switch_ = b.switch(acc, &[(0, switch_case)], Some(switch_default));
        let ret = b.ret(Some(acc));

        let body = b.block(&[dec, branch, loop_, while_, switch_, ret]);
        let fun_ty = FirType::Fun {
            args: vec![FirType::Int32],
            ret: Box::new(FirType::Int32),
        };
        let args = vec![NamedType {
            name: "x".to_owned(),
            typ: FirType::Int32,
        }];
        let fun = b.declare_fun("helper", fun_ty, &args, Some(body), false);

        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[fun]);
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
        let out = generate_cpp_module(&store, module, &CppOptions::default())
            .expect("core statement/value slice should generate");

        assert!(out.contains("int helper(int x)"));
        assert!(out.contains("if (acc < 16)"));
        assert!(out.contains("for (int i = 0; i < 4; ++i)"));
        assert!(out.contains("while (acc < 16)"));
        assert!(out.contains("switch (acc)"));
        assert!(out.contains("return acc;"));
    }

    #[test]
    /// Verifies canonical `buildUserInterface` signature checking stays enforced.
    fn rejects_invalid_canonical_build_ui_signature() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let body = b.block(&[]);
        let bad_ty = FirType::Fun {
            args: vec![FirType::Int32],
            ret: Box::new(FirType::Void),
        };
        let bad_args = vec![NamedType {
            name: "x".to_owned(),
            typ: FirType::Int32,
        }];
        let build_ui = b.declare_fun("buildUserInterface", bad_ty, &bad_args, Some(body), false);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[build_ui]);
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

        let err = generate_cpp_module(&store, module, &CppOptions::default())
            .expect_err("invalid canonical buildUserInterface signature must fail");
        assert_eq!(err.code(), CodegenErrorCode::InvalidModuleSection);
        assert!(
            err.to_string()
                .contains("invalid FIR signature for buildUserInterface")
        );
    }

    #[test]
    /// Verifies UI and metadata FIR nodes lower to the correct C++ callback
    /// families for `buildUserInterface` and `metadata`.
    fn emits_ui_and_metadata_nodes() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let group_meta = b.add_meta_declare("0", "tooltip", "hello");
        let open = b.open_box(fir::UiBoxType::Vertical, "group");
        let button = b.add_button(fir::ButtonType::Button, "gate", "fGate");
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
        let bargraph = b.add_bargraph(fir::BargraphType::Horizontal, "level", "fLevel", -60.0, 6.0);
        let soundfile = b.add_soundfile_with_url("sample", "samples/piano.wav", "fSample");
        let close = b.close_box();
        let body = b.block(&[
            group_meta,
            open,
            button,
            slider_meta,
            slider,
            bargraph,
            soundfile,
            close,
        ]);
        let build_ui_ty = FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::UI],
            ret: Box::new(FirType::Void),
        };
        let build_ui_args = [
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "ui_interface".to_owned(),
                typ: FirType::UI,
            },
        ];
        let ui = b.declare_fun(
            "buildUserInterface",
            build_ui_ty,
            &build_ui_args,
            Some(body),
            false,
        );
        let module_meta = b.add_meta_declare("0", "author", "faust-rs");
        let meta_body = b.block(&[module_meta]);
        let metadata_ty = FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Meta],
            ret: Box::new(FirType::Void),
        };
        let metadata_args = [
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "m".to_owned(),
                typ: FirType::Meta,
            },
        ];
        let metadata = b.declare_fun(
            "metadata",
            metadata_ty,
            &metadata_args,
            Some(meta_body),
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

        let out =
            generate_cpp_module(&store, module, &CppOptions::default()).expect("UI nodes emit");
        assert!(out.contains("virtual void buildUserInterface(UI* ui_interface)"));
        assert!(out.contains("ui_interface->declare(0, \"tooltip\", \"hello\");"));
        assert!(out.contains("ui_interface->openVerticalBox(\"group\");"));
        assert!(out.contains("ui_interface->addButton(\"gate\", &fGate);"));
        assert!(out.contains("ui_interface->declare(&fGain, \"unit\", \"dB\");"));
        // DRIFT 5 closure (C-family plan §2.5): slider/bargraph numeric
        // arguments are wrapped in FAUSTFLOAT(...), matching the upstream C++
        // compiler's `cast2FAUSTFLOAT` (cpp_instructions.hh:44).
        assert!(out.contains(
            "ui_interface->addHorizontalSlider(\"gain\", &fGain, FAUSTFLOAT(0.5), FAUSTFLOAT(0.0), FAUSTFLOAT(1.0), FAUSTFLOAT(0.01));"
        ));
        assert!(out.contains(
            "ui_interface->addHorizontalBargraph(\"level\", &fLevel, FAUSTFLOAT(-60.0), FAUSTFLOAT(6.0));"
        ));
        assert!(
            out.contains(
                "ui_interface->addSoundfile(\"sample\", \"samples/piano.wav\", &fSample);"
            )
        );
        assert!(out.contains("ui_interface->closeBox();"));
        assert!(out.contains("virtual void metadata(Meta* m)"));
        assert!(out.contains("m->declare(\"author\", \"faust-rs\");"));
    }

    #[test]
    /// Verifies type rendering covers the currently supported compound forms.
    fn type_mapping_covers_pointer_array_vector_and_function_forms() {
        let options = CppOptions::default();
        assert_eq!(
            emit_type(&FirType::Ptr(Box::new(FirType::Int32)), &options),
            "int*"
        );
        assert_eq!(
            emit_type(&FirType::Array(Box::new(FirType::Float32), 8), &options),
            "float[8]"
        );
        assert_eq!(emit_type(&FirType::FaustFloat, &options), "FAUSTFLOAT");
        assert_eq!(
            emit_type(&FirType::Vector(Box::new(FirType::Float64), 4), &options),
            "Vec<double,4>"
        );
        assert_eq!(
            emit_type(
                &FirType::Fun {
                    args: vec![FirType::Int32, FirType::Ptr(Box::new(FirType::Float32))],
                    ret: Box::new(FirType::Float64),
                },
                &options,
            ),
            "double(int, float*)"
        );
    }

    #[test]
    /// Verifies target spelling overrides are used for `Quad` and `FixedPoint`.
    fn type_mapping_supports_quad_and_fixed_spelling_overrides() {
        let options = CppOptions {
            quad_type_name: "long double".to_owned(),
            fixed_type_name: "faustfixed".to_owned(),
            ..CppOptions::default()
        };
        assert_eq!(emit_type(&FirType::Quad, &options), "long double");
        assert_eq!(emit_type(&FirType::FixedPoint, &options), "faustfixed");
    }

    #[test]
    /// S4a: `cpp` emits the nested sub-container class, its `new`/`delete`
    /// helpers, and a `classInit` that allocates, initializes, fills and
    /// releases it — the reference shape frozen in plan §5.9.1.
    fn sub_module_is_emitted_as_a_nested_class_with_a_filling_class_init() {
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

        let text = generate_cpp_module(&store, module, &CppOptions::default())
            .expect("sub-module emission must succeed");

        assert!(text.contains("class mydspSIG0 {"), "{text}");
        assert!(
            text.contains("static mydspSIG0* newmydspSIG0()"),
            "allocation helper missing: {text}"
        );
        assert!(
            text.contains("static void deletemydspSIG0(mydspSIG0* dsp)"),
            "release helper missing: {text}"
        );
        // The table is declared with its size, mutable and uninitialized: its
        // content is computed by classInit, not by the compiler.
        assert!(
            text.contains("static float ftbl0mydspSIG0[8];"),
            "uninitialized table declaration missing: {text}"
        );
        assert!(
            !text.contains("const static float ftbl0mydspSIG0"),
            "a runtime-filled table must not be const: {text}"
        );
        for expected in [
            "mydspSIG0* sig0 = newmydspSIG0();",
            "sig0->instanceInitmydspSIG0(sample_rate);",
            "sig0->fillmydspSIG0(8, ftbl0mydspSIG0);",
            "deletemydspSIG0(sig0);",
        ] {
            assert!(
                text.contains(expected),
                "classInit missing `{expected}`: {text}"
            );
        }
        // `staticInit` is the classInit body, never a method of its own.
        assert!(
            !text.contains("void staticInit("),
            "staticInit leaked as a method: {text}"
        );

        let mem_text = generate_cpp_module(
            &store,
            module,
            &CppOptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                ..CppOptions::default()
            },
        )
        .expect("mem0 sub-module emission must succeed");
        assert!(
            mem_text.contains("static float* ftbl0mydspSIG0 = nullptr;"),
            "{mem_text}"
        );
        assert!(
            mem_text.contains("newmydspSIG0(fClassManager)"),
            "{mem_text}"
        );
        assert!(mem_text.contains("int iVec0[2];"), "{mem_text}");
        assert!(!mem_text.contains("int* iVec0;"), "{mem_text}");
        assert!(
            mem_text.contains("fClassManager->allocate(32)"),
            "{mem_text}"
        );
        assert!(mem_text.contains("owner->destroy(dsp);"), "{mem_text}");
        assert!(
            mem_text.matches("deletemydspSIG0(sig0);").count() >= 2,
            "class and instance table helpers must both be released: {mem_text}"
        );
    }

    #[test]
    fn mem0_externalizes_buffers_and_emits_checked_lifecycle() {
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let options = CppOptions {
            memory_manager_mode: MemoryManagerMode::Mem0,
            ..CppOptions::default()
        };
        let text = generate_cpp_module(&store, module, &options).unwrap();

        assert!(text.contains("FAUSTFLOAT* fDelay;"), "{text}");
        assert!(!text.contains("FAUSTFLOAT fDelay[4];"), "{text}");
        assert!(text.contains("static bool memoryInfoChecked"), "{text}");
        assert!(text.contains("manager->info(\"fDelay\""), "{text}");
        assert!(text.contains("static mydsp* createChecked"), "{text}");
        assert!(text.contains("dsp_memory_manager* owner = typed->fOwnerManager"));
        assert!(text.contains("typed->memoryDestroy();"));
        assert!(text.contains("copy->fWriteIdx = fWriteIdx;"));
        assert!(text.contains("std::memcpy(copy->fDelay, fDelay, 16);"));
        assert!(text.contains("if (fLiveInstances != 0) return false;"));

        let init = text
            .split("virtual void init(int sample_rate) {")
            .nth(1)
            .and_then(|tail| tail.split('}').next())
            .unwrap();
        assert!(
            init.find("classInit(sample_rate)") < init.find("instanceInit(sample_rate)"),
            "{init}"
        );
    }

    #[test]
    fn mem0_cpp_uses_the_effective_single_or_double_sample_width() {
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let text = generate_cpp_module(
            &store,
            module,
            &CppOptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                double_precision: true,
                ..CppOptions::default()
            },
        )
        .unwrap();
        assert!(text.contains("#define FAUSTFLOAT float"));
        assert!(text.contains("fOwnerManager->allocate(32)"));
        assert!(text.contains("std::memcpy(copy->fDelay, fDelay, 32)"));
    }

    #[test]
    fn ordinary_cpp_output_has_no_memory_manager_surface() {
        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let text = generate_cpp_module(&store, module, &CppOptions::default()).unwrap();
        assert!(text.contains("FAUSTFLOAT fDelay[4];"));
        for forbidden in [
            "fOwnerManager",
            "memoryInfoChecked",
            "createChecked",
            "classDestroyChecked",
        ] {
            assert!(!text.contains(forbidden), "unexpected {forbidden}: {text}");
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn mem0_generated_cpp_compiles_and_clone_is_independent() {
        use std::process::Command;

        let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
        if Command::new(&cxx).arg("--version").output().is_err() {
            eprintln!("skipping mem0 C++ smoke test: `{cxx}` is unavailable");
            return;
        }

        let (store, module) = crate::fixtures::build_table_state_delay_test_module();
        let generated = generate_cpp_module(
            &store,
            module,
            &CppOptions {
                memory_manager_mode: MemoryManagerMode::Mem0,
                ..CppOptions::default()
            },
        )
        .unwrap();
        let prelude = r#"
#include <cassert>
#include <cstddef>
#include <limits>
#include <new>
#include <unordered_set>
struct UI { void openVerticalBox(const char*) {} void closeBox() {} };
struct Meta { void declare(const char*, const char*) {} };
struct Soundfile {};
struct dsp { virtual ~dsp() = default; };
struct dsp_memory_manager {
    enum MemType { kInt32, kInt32_ptr, kFloat, kFloat_ptr, kDouble,
        kDouble_ptr, kQuad, kQuad_ptr, kFixedPoint, kFixedPoint_ptr,
        kObj, kObj_ptr, kSound, kSound_ptr, kInt64, kInt64_ptr,
        kBool, kBool_ptr };
    virtual ~dsp_memory_manager() = default;
    virtual void begin(size_t) {}
    virtual void info(const char*, MemType, size_t, size_t, size_t, size_t) {}
    virtual void end() {}
    virtual void* allocate(size_t size) = 0;
    virtual void destroy(void* ptr) = 0;
};
struct manager final : dsp_memory_manager {
    std::unordered_set<void*> live;
    size_t described = 0;
    size_t calls = 0;
    size_t fail_at = std::numeric_limits<size_t>::max();
    void begin(size_t count) override { described = count; }
    void* allocate(size_t size) override {
        if (calls++ == fail_at) return nullptr;
        void* ptr = ::operator new(size, std::nothrow);
        if (ptr) live.insert(ptr);
        return ptr;
    }
    void destroy(void* ptr) override {
        assert(live.erase(ptr) == 1);
        ::operator delete(ptr);
    }
};
"#;
        let main = r#"
int main() {
    manager mem;
    mydsp::fManager = &mem;
    assert(mydsp::memoryInfoChecked(&mem));
    assert(mem.described == 2);
    for (size_t fail_at = 0; fail_at < 2; ++fail_at) {
        mem.calls = 0;
        mem.fail_at = fail_at;
        assert(mydsp::create() == nullptr);
        assert(mem.live.empty());
    }
    mem.calls = 0;
    mem.fail_at = std::numeric_limits<size_t>::max();
    mydsp* original = mydsp::create();
    assert(original != nullptr);
    original->init(48000);
    float first_in[4] = {1, 2, 3, 4};
    float first_out[4] = {};
    float* in[] = {first_in};
    float* out[] = {first_out};
    original->compute(4, in, out);
    mydsp* copy = original->clone();
    assert(copy != nullptr);
    float copy_in[4] = {9, 10, 11, 12};
    float copy_out[4] = {};
    float* copy_inputs[] = {copy_in};
    float* copy_outputs[] = {copy_out};
    copy->compute(4, copy_inputs, copy_outputs);
    float zero_in[4] = {};
    float original_out[4] = {};
    float* zero_inputs[] = {zero_in};
    float* original_outputs[] = {original_out};
    original->compute(4, zero_inputs, original_outputs);
    for (int i = 0; i < 4; ++i) assert(original_out[i] == float(i + 1));
    mydsp::destroy(copy);
    mydsp::destroy(original);
    assert(mydsp::classDestroyChecked());
    assert(mem.live.empty());
}
"#;

        let stem = format!("faust-rs-mem0-cpp-{}", std::process::id());
        let source = std::env::temp_dir().join(format!("{stem}.cpp"));
        let binary = std::env::temp_dir().join(if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem
        });
        std::fs::write(&source, format!("{prelude}\n{generated}\n{main}"))
            .expect("write C++ smoke source");
        let compile = Command::new(&cxx)
            .args(["-std=c++17", "-Wall", "-Wextra", "-Werror"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .expect("run C++ compiler");
        assert!(
            compile.status.success(),
            "C++ compile failed:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&binary)
            .output()
            .expect("run C++ smoke binary");
        assert!(
            run.status.success(),
            "C++ runtime failed:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(binary);
    }
}
