//! C++ backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! - `compiler/generator/instructions.hh` (`ModuleInst`)
//! - `compiler/generator/cpp/cpp_instructions.hh` (`CPPInstVisitor::visit(ModuleInst*)`)
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
}

impl Default for CppOptions {
    /// Default backend options.
    ///
    /// Uses `class_name = Some("mydsp")` to match the current workspace
    /// convention for deterministic generated type names.
    fn default() -> Self {
        Self {
            namespace: None,
            class_name: Some("mydsp".to_owned()),
            super_class_name: Some("dsp".to_owned()),
            quad_type_name: "quad".to_owned(),
            fixed_type_name: "fixed".to_owned(),
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
}

impl CodegenErrorCode {
    /// Stable textual code used in diagnostics and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-CPP-0001",
            Self::InvalidModuleSection => "FRS-CGEN-CPP-0002",
            Self::UnsupportedNode => "FRS-CGEN-CPP-0003",
        }
    }
}

/// Typed backend error returned by the C++ emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    code: CodegenErrorCode,
    message: String,
}

impl CodegenError {
    /// Creates a typed C++ backend code generation error.
    #[must_use]
    pub fn new(code: CodegenErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Returns the stable backend error code.
    #[must_use]
    pub fn code(&self) -> CodegenErrorCode {
        self.code
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
    let module = decode_module(store, module)?;
    let module_name = module.name.clone();
    let effective_options = options.clone();
    let declared_functions = collect_module_function_names(store, module.functions)?;
    let has_sample_rate_field = block_declares_var(store, module.dsp_struct, "fSampleRate")
        || block_declares_var(store, module.globals, "fSampleRate");
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(module.name.as_str());
    let super_class_name = options.super_class_name.as_deref().unwrap_or("dsp");

    let mut out = String::new();
    emit_cpp_header(&mut out, class_name, &module_name);
    if let Some(namespace) = options.namespace.as_deref() {
        let _ = writeln!(out, "namespace {namespace} {{");
        let _ = writeln!(out);
    }

    // Emit compile-time constant waveform tables at file scope.
    emit_static_tables(store, &mut out, &effective_options, module.static_decls)?;
    let _ = writeln!(out);

    let _ = writeln!(out, "class {class_name} : public {super_class_name} {{");
    let _ = writeln!(out, "private:");
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
        1,
    )?;
    emit_section(
        store,
        &mut out,
        &effective_options,
        &module_name,
        "globals",
        module.globals,
        1,
    )?;
    let _ = writeln!(out, "public:");
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
        1,
    )?;
    let _ = writeln!(out, "}};");

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
    let _ = writeln!(out);
    let _ = writeln!(out, "{tab}virtual int getNumInputs() {{");
    let _ = writeln!(out, "{tab}    return {};", num_inputs);
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}virtual int getNumOutputs() {{");
    let _ = writeln!(out, "{tab}    return {};", num_outputs);
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}static void classInit(int sample_rate) {{");
    let _ = writeln!(out, "{tab}    (void)sample_rate;");
    let _ = writeln!(out, "{tab}}}");
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
    let _ = writeln!(out, "{tab}virtual {class_name}* clone() {{");
    let _ = writeln!(out, "{tab}    return new {class_name}(*this);");
    let _ = writeln!(out, "{tab}}}");
    if !has_metadata {
        let _ = writeln!(out, "{tab}virtual void metadata(Meta* m) {{");
        let _ = writeln!(out, "{tab}    (void)m;");
        let _ = writeln!(
            out,
            "{tab}    m->declare(\"filename\", \"{}.dsp\");",
            module_name
        );
        let _ = writeln!(out, "{tab}    m->declare(\"name\", \"{module_name}\");");
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
    Ok(())
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
fn emit_cpp_header(out: &mut String, class_name: &str, module_name: &str) {
    let _ = writeln!(
        out,
        "/* ------------------------------------------------------------"
    );
    let _ = writeln!(out, "name: {}", cpp_string_literal(module_name));
    let _ = writeln!(out, "Code generated with Faust (https://faust.grame.fr)");
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
    let _ = writeln!(out, "#include <cmath>");
    let _ = writeln!(out, "#include <cstdint>");
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
    _indent: usize,
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
        emit_stmt(store, out, options, module_name, item, _indent)?;
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

/// Emits one FIR statement using the active rendering mode.
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
    let ctx = c_family::CFamilyStmtCtx {
        syntax: &SYNTAX,
        var_ref: emit_var_ref,
        for_loop_step: cpp_for_loop_step,
        simple_loop_increment: cpp_simple_loop_increment,
        render_named_type: &|typ, name| emit_named_type(typ, name, options),
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
    let strip_explicit_dsp_arg = is_dsp_api_method(decl.name)
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
    }
    if let Some(override_params) = params_override {
        params = override_params;
    }
    let is_dsp_api = is_dsp_api_method(decl.name);
    let method_prefix = if is_dsp_api { "virtual " } else { "" };
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
    } else if decl.name == "compute" {
        emit_compute_body(store, out, options, body, indent + 1)?;
    } else if decl.name == "metadata" && is_empty_block(store, body) {
        let _ = writeln!(out, "{tab}    (void)m;");
        let _ = writeln!(
            out,
            "{tab}    m->declare(\"filename\", \"{}.dsp\");",
            module_name
        );
        let _ = writeln!(out, "{tab}    m->declare(\"name\", \"{module_name}\");");
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
        FirMatch::NewDsp { name, .. } => Ok(format!("new {name}()")),
        _ => Err(unsupported_node("value", value, store)),
    }
}

fn emit_named_type(typ: &FirType, name: &str, options: &CppOptions) -> String {
    let mut suffix = String::new();
    let base = emit_type_base_and_suffix(typ, options, &mut suffix);
    format!("{base} {name}{suffix}")
}

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
        } => Ok(ModuleView {
            name,
            dsp_struct,
            globals,
            functions,
            num_inputs,
            num_outputs,
            static_decls,
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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);

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
        assert!(out.contains("Code generated with Faust (https://faust.grame.fr)"));
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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);
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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);
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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);
        let out = generate_cpp_module(&store, module, &CppOptions::default())
            .expect("core statement/value slice should generate");

        assert!(out.contains("int helper(int x)"));
        assert!(out.contains("if ((acc < 16))"));
        assert!(out.contains("for (int i = 0; i < 4; ++i)"));
        assert!(out.contains("while ((acc < 16))"));
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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);

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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);

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
}
