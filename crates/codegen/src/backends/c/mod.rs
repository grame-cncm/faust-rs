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

use fir::{AccessType, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

use crate::backends::c_family::{self, CFamilySyntax, EmitMode, StructInit, TableInit};
use crate::backends::faust_api;

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
}

impl Default for COptions {
    /// Default backend options.
    ///
    /// Uses `class_name = Some("mydsp")` to match the current workspace
    /// convention for deterministic generated type names.
    fn default() -> Self {
        Self {
            class_name: Some("mydsp".to_owned()),
            quad_type_name: "quad".to_owned(),
            fixed_type_name: "fixed".to_owned(),
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
}

impl CodegenErrorCode {
    /// Stable textual code used in diagnostics and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-C-0001",
            Self::InvalidModuleSection => "FRS-CGEN-C-0002",
            Self::UnsupportedNode => "FRS-CGEN-C-0003",
        }
    }
}

/// Typed backend error returned by the C emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    code: CodegenErrorCode,
    message: String,
}

impl CodegenError {
    /// Creates a typed C backend code generation error.
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
    /// Formats the typed error as `[CODE] message`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CodegenError {}

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
    let module = decode_module(store, module)?;
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(module.name.as_str())
        .to_owned();
    let effective_options = options.clone();

    let declared_functions = collect_module_functions(store, module.functions)?;
    let struct_inits = collect_struct_initializers(store, module.dsp_struct, module.globals)?;
    let table_inits = collect_table_initializers(store, module.dsp_struct, module.globals)?;
    let mut out = String::new();
    emit_c_header(&mut out, &class_name);
    emit_static_tables(store, &mut out, &effective_options, module.static_decls)?;
    let _ = writeln!(out);
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
        },
    )?;
    emit_c_footer(&mut out);
    Ok(out)
}

/// Emits the prologue/header guard and platform macros for the generated unit.
fn emit_c_header(out: &mut String, class_name: &str) {
    let guard = format!("__{}_H__", class_name);
    let _ = writeln!(out, "#ifndef  {guard}");
    let _ = writeln!(out, "#define  {guard}");
    let _ = writeln!(out);
    let _ = writeln!(out, "#ifndef FAUSTFLOAT");
    let _ = writeln!(out, "#define FAUSTFLOAT float");
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
    let _ = writeln!(
        out,
        "static inline int faustmini(int a, int b) {{ return (a < b) ? a : b; }}"
    );
    let _ = writeln!(
        out,
        "static inline int faustmaxi(int a, int b) {{ return (a > b) ? a : b; }}"
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
    emit_struct_fields(store, out, options, dsp_struct)?;
    emit_struct_fields(store, out, options, globals)?;
    if !has_sample_rate_field {
        let _ = writeln!(out, "    int fSampleRate;");
    }
    let _ = writeln!(out, "}} {class_name};");
    let _ = writeln!(out);
    Ok(())
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
                let _ = write!(out, "    {}", emit_named_type(&typ, &name, options));
                let _ = writeln!(out, ";");
            }
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => {
                let _ = writeln!(
                    out,
                    "    {} {}[{}];",
                    emit_type(&elem_type, options),
                    name,
                    values.len()
                );
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
    } = spec;
    let names: Vec<&str> = declared_functions.iter().map(|f| f.name.as_str()).collect();

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

    let _ = writeln!(out, "void classInit{class_name}(int sample_rate) {{");
    let _ = writeln!(out, "    (void)sample_rate;");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

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
    let _ = writeln!(out, "    classInit{class_name}(sample_rate);");
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
        if matches!(
            f.name.as_str(),
            "metadata"
                | "instanceConstants"
                | "instanceResetUserInterface"
                | "instanceClear"
                | "buildUserInterface"
                | "compute"
        ) {
            continue;
        }
        if names.contains(&f.name.as_str()) {
            emit_helper_function(store, out, options, f)?;
        }
    }

    Ok(())
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
    } else {
        let mut mode = match decl.name.as_str() {
            "metadata" => EmitMode::Metadata,
            "buildUserInterface" => EmitMode::Ui,
            _ => EmitMode::Default,
        };
        emit_block_with_mode(store, out, options, body, 1, &mut mode)?;
    }
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    Ok(())
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

/// Emits one FIR statement into C syntax.
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

fn emit_named_type(typ: &FirType, name: &str, options: &COptions) -> String {
    let mut suffix = String::new();
    let base = emit_type_base_and_suffix(typ, options, &mut suffix);
    format!("{base} {name}{suffix}")
}

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
        assert_eq!(out, "    if (1) {\n    }\n");

        let mut out = String::new();
        emit_stmt(&store, &mut out, &options, while_loop, 1, &mut mode).expect("WhileLoop renders");
        assert_eq!(out, "    while (1) {\n    }\n");
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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);

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
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);

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
}
