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

use fir::{FirBinOp, FirId, FirMatch, FirMathOp, FirStore, FirType, NamedType, match_fir};

use crate::backends::faust_api;

pub const BACKEND_NAME: &str = "cpp";

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

/// Typed backend error for C++ generation.
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
/// Borrowed function declaration view used while stitching the C++ class body.
///
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

/// Rendering mode for statement/expression emission.
///
/// `Compute` enables the subset of formatting rules that are specific to the
/// sample loop and output buffer writes. `Metadata` and `Ui` preserve the C++
/// split between `m->declare(...)` in `metadata()` and
/// `ui_interface->declare(...)` in `buildUserInterface()`.
#[derive(Debug, Clone, Copy)]
enum EmitMode {
    Default,
    Metadata,
    Ui,
    Compute,
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
    emit_dsp_contract_methods(
        &mut out,
        module.num_inputs,
        module.num_outputs,
        class_name,
        &module_name,
        &declared_functions,
        1,
    );
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

/// Emits the standard Faust `dsp` API surface expected from generated C++.
///
/// Methods are synthesized even when the FIR module omitted some sections so
/// that the generated class still satisfies the stable backend ABI. When a
/// section is absent, the emitted method falls back to the same neutral/default
/// behavior as the C++ backend.
fn emit_dsp_contract_methods(
    out: &mut String,
    num_inputs: usize,
    num_outputs: usize,
    class_name: &str,
    module_name: &str,
    declared_functions: &[String],
    indent: usize,
) {
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
fn emit_stmt_with_mode(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    module_name: &str,
    stmt: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    let tab = "    ".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::DeclareVar {
            name,
            typ,
            access: _,
            init,
        } => {
            let _ = write!(out, "{tab}{}", emit_named_type(&typ, &name, options));
            if let Some(init) = init {
                let init = emit_value(store, options, init)?;
                let _ = write!(out, " = {init}");
            }
            let _ = writeln!(out, ";");
            Ok(())
        }
        FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } => {
            let _ = writeln!(
                out,
                "{tab}{} {}[{}];",
                emit_type(&elem_type, options),
                name,
                values.len()
            );
            Ok(())
        }
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
        FirMatch::DeclareStructType { typ } => {
            let _ = writeln!(
                out,
                "{tab}// struct type declaration: {}",
                emit_type(&typ, options)
            );
            Ok(())
        }
        FirMatch::DeclareBufferIterators {
            name1,
            name2,
            channels,
            typ,
            mutable,
            chunk,
        } => {
            let _ = writeln!(
                out,
                "{tab}// buffer iterators: {name1}, {name2}, channels={channels}, type={}, mutable={mutable}, chunk={chunk}",
                emit_type(&typ, options)
            );
            Ok(())
        }
        FirMatch::StoreVar {
            name,
            access: _,
            value,
        } => {
            let value = emit_value(store, options, value)?;
            let _ = writeln!(out, "{tab}{name} = {value};");
            Ok(())
        }
        FirMatch::StoreTable {
            name, index, value, ..
        } => {
            let index = emit_value(store, options, index)?;
            let value = emit_value(store, options, value)?;
            let _ = writeln!(out, "{tab}{name}[{index}] = {value};");
            Ok(())
        }
        FirMatch::ShiftArrayVar {
            name,
            access: _,
            delay,
        } => {
            let _ = writeln!(out, "{tab}// shift array {name} by {delay}");
            Ok(())
        }
        FirMatch::Drop(value) => {
            let value = emit_value(store, options, value)?;
            let _ = mode;
            let _ = writeln!(out, "{tab}(void)({value});");
            Ok(())
        }
        FirMatch::NullStatement => {
            let _ = writeln!(out, "{tab};");
            Ok(())
        }
        FirMatch::Return(value) => {
            if let Some(value) = value {
                let value = emit_value(store, options, value)?;
                let _ = writeln!(out, "{tab}return {value};");
            } else {
                let _ = writeln!(out, "{tab}return;");
            }
            Ok(())
        }
        FirMatch::Block(_) => {
            emit_block_with_mode(store, out, options, module_name, stmt, indent, mode)
        }
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}if ({cond}) {{");
            emit_block_with_mode(
                store,
                out,
                options,
                module_name,
                then_block,
                indent + 1,
                mode,
            )?;
            let _ = writeln!(out, "{tab}}}");
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{tab}else {{");
                emit_block_with_mode(
                    store,
                    out,
                    options,
                    module_name,
                    else_block,
                    indent + 1,
                    mode,
                )?;
                let _ = writeln!(out, "{tab}}}");
            }
            Ok(())
        }
        FirMatch::Control { cond, stmt } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}if ({cond}) {{");
            emit_stmt_with_mode(store, out, options, module_name, stmt, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::ForLoop {
            var,
            init,
            end,
            step,
            body,
            is_reverse,
        } => {
            // init is a DeclareVar(kLoop) per FIR contract; extract its value.
            let init_val =
                if let FirMatch::DeclareVar { init: Some(v), .. } = match_fir(store, init) {
                    emit_value(store, options, v)?
                } else {
                    emit_value(store, options, init)?
                };
            let end = emit_value(store, options, end)?;
            let step = emit_value(store, options, step)?;
            if is_reverse {
                let _ = writeln!(
                    out,
                    "{tab}for (int {var} = {init_val}; {var} > {end}; {var} = {var} + {step}) {{"
                );
            } else {
                let _ = writeln!(
                    out,
                    "{tab}for (int {var} = {init_val}; {var} < {end}; {var} += {step}) {{"
                );
            }
            emit_block_with_mode(store, out, options, module_name, body, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } => {
            let upper = emit_value(store, options, upper)?;
            if is_reverse {
                let _ = writeln!(
                    out,
                    "{tab}for (int {var} = ({upper}) - 1; {var} >= 0; {var} = {var} - 1) {{"
                );
            } else {
                let _ = writeln!(out, "{tab}for (int {var} = 0; {var} < {upper}; ++{var}) {{");
            }
            emit_block_with_mode(store, out, options, module_name, body, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::IteratorForLoop {
            iterators,
            is_reverse: _,
            body,
        } => {
            let joined = iterators.join(", ");
            let _ = writeln!(out, "{tab}// iterator-for over [{joined}]");
            emit_block_with_mode(store, out, options, module_name, body, indent + 1, mode)?;
            Ok(())
        }
        FirMatch::WhileLoop { cond, body } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}while ({cond}) {{");
            emit_block_with_mode(store, out, options, module_name, body, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}switch ({cond}) {{");
            for (value, block) in cases {
                let _ = writeln!(out, "{tab}case {value}: {{");
                emit_block_with_mode(store, out, options, module_name, block, indent + 1, mode)?;
                let _ = writeln!(out, "{tab}    break;");
                let _ = writeln!(out, "{tab}}}");
            }
            if let Some(default) = default {
                let _ = writeln!(out, "{tab}default: {{");
                emit_block_with_mode(store, out, options, module_name, default, indent + 1, mode)?;
                let _ = writeln!(out, "{tab}}}");
            }
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Label(label) => {
            let _ = label;
            Ok(())
        }
        FirMatch::OpenBox { typ, label } => {
            let api = match typ {
                fir::UiBoxType::Vertical => "openVerticalBox",
                fir::UiBoxType::Horizontal => "openHorizontalBox",
                fir::UiBoxType::Tab => "openTabBox",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({});",
                cpp_string_literal(&label)
            );
            Ok(())
        }
        FirMatch::CloseBox => {
            let _ = writeln!(out, "{tab}ui_interface->closeBox();");
            Ok(())
        }
        FirMatch::AddButton { typ, label, var } => {
            let api = match typ {
                fir::ButtonType::Button => "addButton",
                fir::ButtonType::Checkbox => "addCheckButton",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}, &{var});",
                cpp_string_literal(&label)
            );
            Ok(())
        }
        FirMatch::AddSlider {
            typ,
            label,
            var,
            init,
            lo,
            hi,
            step,
        } => {
            let api = match typ {
                fir::SliderType::Horizontal => "addHorizontalSlider",
                fir::SliderType::Vertical => "addVerticalSlider",
                fir::SliderType::NumEntry => "addNumEntry",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}, &{var}, {}, {}, {}, {});",
                cpp_string_literal(&label),
                trim_float(init),
                trim_float(lo),
                trim_float(hi),
                trim_float(step)
            );
            Ok(())
        }
        FirMatch::AddBargraph {
            typ,
            label,
            var,
            lo,
            hi,
        } => {
            let api = match typ {
                fir::BargraphType::Horizontal => "addHorizontalBargraph",
                fir::BargraphType::Vertical => "addVerticalBargraph",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}, &{var}, {}, {});",
                cpp_string_literal(&label),
                trim_float(lo),
                trim_float(hi)
            );
            Ok(())
        }
        FirMatch::AddSoundfile { label, url, var } => {
            let _ = writeln!(
                out,
                "{tab}ui_interface->addSoundfile({}, {}, &{var});",
                cpp_string_literal(&label),
                cpp_string_literal(&url)
            );
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
fn emit_value(
    store: &FirStore,
    options: &CppOptions,
    value: FirId,
) -> Result<String, CodegenError> {
    match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(value.to_string()),
        FirMatch::Int64 { value, .. } => Ok(value.to_string()),
        FirMatch::Float32 { value, .. } => Ok(format_float32(f64::from(value))),
        FirMatch::Float64 { value, .. } => Ok(trim_float(value)),
        FirMatch::Bool { value, .. } => Ok(if value { "true" } else { "false" }.to_owned()),
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
        FirMatch::LoadVar { name, .. } | FirMatch::LoadVarAddress { name, .. } => Ok(name),
        FirMatch::LoadTable { name, index, .. } => {
            let index = emit_value(store, options, index)?;
            Ok(format!("{name}[{index}]"))
        }
        FirMatch::TeeVar {
            name,
            access: _,
            value,
            ..
        } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("({name} = {value})"))
        }
        FirMatch::BinOp { op, lhs, rhs, .. } => {
            let lhs = emit_value(store, options, lhs)?;
            let rhs = emit_value(store, options, rhs)?;
            Ok(emit_binop_expr(op, &lhs, &rhs))
        }
        FirMatch::Neg { value, .. } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("(-{value})"))
        }
        FirMatch::Cast { typ, value } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("(({})({value}))", emit_type(&typ, options)))
        }
        FirMatch::Bitcast { typ, value } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("bitcast<{}>({value})", emit_type(&typ, options)))
        }
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            let cond = emit_value(store, options, cond)?;
            let then_value = emit_value(store, options, then_value)?;
            let else_value = emit_value(store, options, else_value)?;
            Ok(format!("({cond} ? {then_value} : {else_value})"))
        }
        FirMatch::FunCall { name, args, .. } => {
            let mut rendered = Vec::with_capacity(args.len());
            for arg in args {
                rendered.push(emit_value(store, options, arg)?);
            }
            let cpp_name = emit_cpp_fun_name(&name);
            Ok(format!("{cpp_name}({})", rendered.join(", ")))
        }
        FirMatch::NullValue { .. } => Ok("nullptr".to_owned()),
        FirMatch::NewDsp { name, .. } => Ok(format!("new {name}()")),
        FirMatch::LoadSoundfileLength { var, part } => {
            let part = emit_value(store, options, part)?;
            Ok(format!("{var}->fLength[{part}]"))
        }
        FirMatch::LoadSoundfileRate { var, part } => {
            let part = emit_value(store, options, part)?;
            Ok(format!("{var}->fSR[{part}]"))
        }
        FirMatch::LoadSoundfileBuffer {
            var,
            chan,
            part,
            idx,
            ..
        } => {
            let chan = emit_value(store, options, chan)?;
            let part = emit_value(store, options, part)?;
            let idx = emit_value(store, options, idx)?;
            Ok(format!(
                "((FAUSTFLOAT**){var}->fBuffers)[{chan}][{var}->fOffset[{part}] + {idx}]"
            ))
        }
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

/// Maps one FIR binary operator to its C++ token spelling.
fn emit_binop(op: FirBinOp) -> &'static str {
    match op {
        FirBinOp::Add => "+",
        FirBinOp::Sub => "-",
        FirBinOp::Mul => "*",
        FirBinOp::Div => "/",
        FirBinOp::Rem => "%",
        FirBinOp::And => "&",
        FirBinOp::Or => "|",
        FirBinOp::Xor => "^",
        FirBinOp::Lsh => "<<",
        FirBinOp::ARsh => ">>",
        FirBinOp::LRsh => ">>",
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::Lt => "<",
        FirBinOp::Le => "<=",
        FirBinOp::Gt => ">",
        FirBinOp::Ge => ">=",
    }
}

fn emit_binop_expr(op: FirBinOp, lhs: &str, rhs: &str) -> String {
    match op {
        FirBinOp::LRsh => format!("((int32_t)(((uint32_t)({lhs})) >> ({rhs})))"),
        _ => format!("({lhs} {} {rhs})", emit_binop(op)),
    }
}

/// Renders a FIR type into the current C++ backend spelling.
fn emit_type(typ: &FirType, options: &CppOptions) -> String {
    match typ {
        FirType::Int32 => "int".to_owned(),
        FirType::Int64 => "long long".to_owned(),
        FirType::Float32 => "float".to_owned(),
        FirType::Float64 => "double".to_owned(),
        FirType::FaustFloat => "FAUSTFLOAT".to_owned(),
        FirType::Quad => options.quad_type_name.clone(),
        FirType::FixedPoint => options.fixed_type_name.clone(),
        FirType::Bool => "bool".to_owned(),
        FirType::Void => "void".to_owned(),
        FirType::Obj => "void*".to_owned(),
        // FIR handle kinds are already pointer-shaped at the type-model level.
        // `Ptr(UI)` would therefore become `UI**`.
        FirType::Sound => "Soundfile*".to_owned(),
        FirType::UI => "UI*".to_owned(),
        FirType::Meta => "Meta*".to_owned(),
        FirType::Ptr(inner) => format!("{}*", emit_type(inner, options)),
        FirType::Array(inner, size) => format!("{}[{size}]", emit_type(inner, options)),
        FirType::Vector(inner, lanes) => format!("Vec<{},{lanes}>", emit_type(inner, options)),
        FirType::Struct(name, _fields) => name.clone(),
        FirType::Fun { args, ret } => {
            let args = args
                .iter()
                .map(|arg| emit_type(arg, options))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", emit_type(ret, options))
        }
    }
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
fn trim_float(value: f64) -> String {
    if value.is_nan() {
        return "NAN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-INFINITY".to_owned()
        } else {
            "INFINITY".to_owned()
        };
    }
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

fn format_float32(value: f64) -> String {
    if !value.is_finite() {
        return trim_float(value);
    }
    format!("{}f", trim_float(value))
}

/// Renders an initializer-list literal from already-rendered elements.
fn format_array(values: impl Iterator<Item = String>) -> String {
    format!("{{{}}}", values.collect::<Vec<_>>().join(", "))
}

/// Escapes a Rust string into a C++ string literal.
fn cpp_string_literal(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Emits `DeclareTable(AccessType::Static)` nodes as `const static` arrays
/// with inline initializers, placed before the class definition.
fn emit_static_tables(
    store: &FirStore,
    out: &mut String,
    options: &CppOptions,
    block: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(stmts) = match_fir(store, block) else {
        return Ok(());
    };
    for stmt in stmts {
        if let FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } = match_fir(store, stmt)
        {
            let type_str = emit_type(&elem_type, options);
            let n = values.len();
            if n == 0 {
                let _ = writeln!(out, "const static {type_str} {name}[0] = {{}};");
            } else {
                let _ = write!(out, "const static {type_str} {name}[{n}] = {{");
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        let _ = write!(out, ", ");
                    }
                    let rendered = emit_value(store, options, *v)?;
                    let _ = write!(out, "{rendered}");
                }
                let _ = writeln!(out, "}};");
            }
        }
    }
    Ok(())
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
    use fir::FirBuilder;

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
        assert!(
            out.contains(
                "ui_interface->addHorizontalSlider(\"gain\", &fGain, 0.5, 0.0, 1.0, 0.01);"
            )
        );
        assert!(
            out.contains("ui_interface->addHorizontalBargraph(\"level\", &fLevel, -60.0, 6.0);")
        );
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
