//! AssemblyScript (`asc`) backend generation from FIR `Module` roots.
//!
//! # Source provenance
//! Mirrors the C++ Faust AssemblyScript backend:
//! - `compiler/generator/assemblyscript/assemblyscript_instructions.hh`
//! - `compiler/generator/assemblyscript/assemblyscript_code_container.cpp`
//!
//! and follows the same module-first structure as this workspace's `cpp`
//! backend (`crates/codegen/src/backends/cpp/mod.rs`).
//!
//! # Output contract
//! - Emits `export class <name> { ... }`.
//! - Instance state is addressed as `this.<field>`; static struct fields as
//!   `<ClassName>.<field>`.
//! - Numeric literals are wrapped (`<i32>(n)`, `<f32>(n)`, `<f64>(n)`).
//! - Arrays are `StaticArray<T>`.
//! - Standard math maps to `Math.*` / `Mathf.*`.
//! - UI / soundfile nodes are emitted as comments (parity with the C++ asc
//!   backend, which lowers them to `// ui ...`).
//!
//! # Limitations
//! Scalar code path only. Unsupported FIR nodes fail fast with
//! `FRS-CGEN-ASC-0003`.

use std::fmt::Write as _;

use fir::{AccessType, FirBinOp, FirId, FirMatch, FirMathOp, FirStore, FirType, NamedType, match_fir};

use crate::backends::faust_api;

pub const BACKEND_NAME: &str = "asc";

/// AssemblyScript backend options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AscOptions {
    /// Optional class name override for the FIR module name.
    pub class_name: Option<String>,
    /// AssemblyScript spelling used for FIR `Quad` values (unsupported; kept for parity).
    pub quad_type_name: String,
    /// AssemblyScript spelling used for FIR `FixedPoint` values.
    pub fixed_type_name: String,
    /// Optional JSON description embedded as a `getJSON(): string` method.
    ///
    /// Mirrors the C++ asc backend, whose `getJSON()` snapshot is what
    /// downstream tooling (UI/param extraction, impulse runners) parses for
    /// inputs/outputs and the UI tree.
    pub json: Option<String>,
}

impl Default for AscOptions {
    fn default() -> Self {
        Self {
            class_name: Some("mydsp".to_owned()),
            quad_type_name: "f64".to_owned(),
            fixed_type_name: "f32".to_owned(),
            json: None,
        }
    }
}

/// Stable backend error codes for AssemblyScript code generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// Root FIR node is not a module (`FirMatch::Module`).
    RootNotModule,
    /// Module section is not a FIR block shape.
    InvalidModuleSection,
    /// One FIR node is not yet supported by the asc emitter.
    UnsupportedNode,
}

impl CodegenErrorCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-ASC-0001",
            Self::InvalidModuleSection => "FRS-CGEN-ASC-0002",
            Self::UnsupportedNode => "FRS-CGEN-ASC-0003",
        }
    }
}

/// Typed backend error returned by the asc emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    code: CodegenErrorCode,
    message: String,
}

impl CodegenError {
    #[must_use]
    pub fn new(code: CodegenErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

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

/// Borrowed function declaration view used while stitching the class body.
struct DeclareFunView<'a> {
    name: &'a str,
    typ: &'a FirType,
    named_args: &'a [NamedType],
    body: Option<FirId>,
}

/// Where a declaration is being emitted. Controls `let` vs class-field form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Inside the class field section: emit `name: type` members.
    Fields,
    /// Inside a method/function body: emit `let name: type` locals.
    Body,
}

/// Float type used for `FaustFloat` (the asc backend is single-precision).
const FAUST_FLOAT: &str = "f32";

/// Generates AssemblyScript code from a FIR module root.
///
/// # Errors
/// Returns [`CodegenError`] with code `FRS-CGEN-ASC-0001` when `module`
/// does not decode to `FirMatch::Module`.
pub fn generate_asc_module(
    store: &FirStore,
    module: FirId,
    options: &AscOptions,
) -> Result<String, CodegenError> {
    let module = decode_module(store, module)?;
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(module.name.as_str())
        .to_owned();
    let declared_functions = collect_module_function_names(store, module.functions)?;

    let mut out = String::new();
    let _ = writeln!(out, "// Code generated with faust-rs (https://faust.grame.fr)");
    let _ = writeln!(out, "// Language: AssemblyScript (experimental)");
    let _ = writeln!(out, "// name: {}", module.name);
    let _ = writeln!(out);

    // Top-level helper functions (non-DSP-API DeclareFun in the globals section)
    // are emitted as module-level `export function`s before the class.
    emit_toplevel_functions(store, &mut out, options, &class_name, module.globals)?;

    // Compile-time constant tables become module-level `const`s.
    emit_static_tables(store, &mut out, options, module.static_decls)?;

    let _ = writeln!(out, "export class {class_name} {{");

    // ---- fields: DSP struct state + globals (non-function) ----
    if !block_declares_var(store, module.dsp_struct, "fSampleRate")
        && !block_declares_var(store, module.globals, "fSampleRate")
    {
        let _ = writeln!(out, "    fSampleRate: i32 = 0;");
    }
    emit_section_fields(store, &mut out, options, &class_name, module.dsp_struct, 1)?;
    emit_section_fields(store, &mut out, options, &class_name, module.globals, 1)?;
    let _ = writeln!(out);

    // ---- DSP API methods ----
    emit_dsp_contract_methods(
        &mut out,
        module.num_inputs,
        module.num_outputs,
        &class_name,
        &module.name,
        &declared_functions,
        1,
    );

    // JSON snapshot for tooling (UI/param extraction), mirroring the C++ asc
    // backend's getJSON().
    if let Some(json) = options.json.as_deref() {
        let _ = writeln!(out, "    getJSON(): string {{");
        let _ = writeln!(out, "        return \"{}\";", escape_as_string(json));
        let _ = writeln!(out, "    }}");
    }

    // ---- declared functions (compute, instance*, etc.) as class methods ----
    emit_section_methods(store, &mut out, options, &class_name, module.functions, 1)?;

    let _ = writeln!(out, "}}");
    Ok(out)
}

/// Emits module-level helper functions found in the globals section.
fn emit_toplevel_functions(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    globals: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, globals) else {
        return Ok(());
    };
    let mut any = false;
    for item in items {
        if let FirMatch::DeclareFun {
            name, typ, args, body, ..
        } = match_fir(store, item)
        {
            // Only real definitions (with a body) become top-level functions.
            if body.is_none() {
                continue;
            }
            emit_declare_fun(
                store,
                out,
                options,
                class_name,
                DeclareFunView {
                    name: &name,
                    typ: &typ,
                    named_args: &args,
                    body,
                },
                0,
                /* as_method = */ false,
            )?;
            any = true;
        }
    }
    if any {
        let _ = writeln!(out);
    }
    Ok(())
}

/// Emits the standard Faust DSP API surface in AssemblyScript form, synthesizing
/// any methods the FIR module did not declare itself.
fn emit_dsp_contract_methods(
    out: &mut String,
    num_inputs: usize,
    num_outputs: usize,
    class_name: &str,
    module_name: &str,
    declared: &[String],
    indent: usize,
) {
    let tab = "    ".repeat(indent);
    let has = |n: &str| declared.iter().any(|d| d == n);

    let _ = writeln!(out, "{tab}getNumInputs(): i32 {{ return {num_inputs}; }}");
    let _ = writeln!(out, "{tab}getNumOutputs(): i32 {{ return {num_outputs}; }}");
    let _ = writeln!(out, "{tab}getSampleRate(): i32 {{ return this.fSampleRate; }}");
    let _ = writeln!(out, "{tab}static classInit(sample_rate: i32): void {{}}");

    if !has("instanceConstants") {
        let _ = writeln!(out, "{tab}instanceConstants(sample_rate: i32): void {{");
        let _ = writeln!(out, "{tab}    this.fSampleRate = sample_rate;");
        let _ = writeln!(out, "{tab}}}");
    }
    if !has("instanceResetUserInterface") {
        let _ = writeln!(out, "{tab}instanceResetUserInterface(): void {{}}");
    }
    if !has("instanceClear") {
        let _ = writeln!(out, "{tab}instanceClear(): void {{}}");
    }
    let _ = writeln!(out, "{tab}instanceInit(sample_rate: i32): void {{");
    let _ = writeln!(out, "{tab}    this.instanceConstants(sample_rate);");
    let _ = writeln!(out, "{tab}    this.instanceResetUserInterface();");
    let _ = writeln!(out, "{tab}    this.instanceClear();");
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out, "{tab}init(sample_rate: i32): void {{");
    let _ = writeln!(out, "{tab}    {class_name}.classInit(sample_rate);");
    let _ = writeln!(out, "{tab}    this.instanceInit(sample_rate);");
    let _ = writeln!(out, "{tab}}}");
    if !has("metadata") {
        let _ = writeln!(out, "{tab}// metadata: name {module_name}");
    }
    if !has("buildUserInterface") {
        let _ = writeln!(out, "{tab}// ui openbox {module_name}");
    }
    if !has("compute") {
        let _ = writeln!(
            out,
            "{tab}compute(count: i32, inputs: Array<Array<{FAUST_FLOAT}>>, outputs: Array<Array<{FAUST_FLOAT}>>): void {{}}"
        );
    }
}

/// Collects declared function names to decide which DSP API stubs to synthesize.
fn collect_module_function_names(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<String>, CodegenError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Err(invalid_section("functions", functions, store));
    };
    let mut names = Vec::new();
    for item in items {
        if let FirMatch::DeclareFun { name, .. } = match_fir(store, item) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Emits a module section as class fields (`DeclareVar` only).
fn emit_section_fields(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    section_id: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, section_id) else {
        return Err(invalid_section("fields", section_id, store));
    };
    for item in items {
        match match_fir(store, item) {
            FirMatch::DeclareVar { .. } | FirMatch::DeclareTable { .. } => {
                emit_stmt(store, out, options, class_name, item, indent, Phase::Fields)?;
            }
            // functions / struct-type decls are emitted elsewhere or ignored here
            _ => {}
        }
    }
    Ok(())
}

/// Emits a module section as class methods (`DeclareFun` only).
fn emit_section_methods(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    section_id: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, section_id) else {
        return Err(invalid_section("functions", section_id, store));
    };
    for item in items {
        if let FirMatch::DeclareFun {
            name, typ, args, body, ..
        } = match_fir(store, item)
        {
            emit_declare_fun(
                store,
                out,
                options,
                class_name,
                DeclareFunView {
                    name: &name,
                    typ: &typ,
                    named_args: &args,
                    body,
                },
                indent,
                /* as_method = */ true,
            )?;
        }
    }
    Ok(())
}

/// Emits one FIR statement.
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    stmt: FirId,
    indent: usize,
    phase: Phase,
) -> Result<(), CodegenError> {
    let tab = "    ".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::DeclareVar { name, typ, access, init } => {
            if phase == Phase::Fields {
                if access == AccessType::Static {
                    let _ = write!(out, "{tab}static ");
                } else {
                    let _ = write!(out, "{tab}");
                }
                let _ = write!(out, "{}", emit_member_decl(&typ, &name, options));
                if let Some(init) = init {
                    let init = emit_value(store, options, class_name, init)?;
                    let _ = write!(out, " = {init}");
                } else if let FirType::Array(inner, size) = &typ
                    && *size > 0
                {
                    let _ = write!(
                        out,
                        " = new StaticArray<{}>({size})",
                        emit_type(inner, options)
                    );
                }
                let _ = writeln!(out, ";");
            } else {
                let _ = write!(out, "{tab}let {}: {}", name, emit_type(&typ, options));
                if let Some(init) = init {
                    let init = emit_value(store, options, class_name, init)?;
                    let _ = write!(out, " = {init}");
                }
                let _ = writeln!(out, ";");
            }
            Ok(())
        }
        FirMatch::DeclareTable { name, elem_type, values, access } => {
            let prefix = if phase == Phase::Fields { "" } else { "let " };
            let static_kw = if phase == Phase::Fields && access == AccessType::Static {
                "static "
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "{tab}{static_kw}{prefix}{name}: StaticArray<{}> = new StaticArray<{}>({});",
                emit_type(&elem_type, options),
                emit_type(&elem_type, options),
                values.len()
            );
            Ok(())
        }
        FirMatch::StoreVar { name, access, value } => {
            let value = emit_value(store, options, class_name, value)?;
            let lhs = qualify(&name, access, class_name);
            let _ = writeln!(out, "{tab}{lhs} = {value};");
            Ok(())
        }
        FirMatch::StoreTable { name, index, value, access } => {
            let index = emit_value(store, options, class_name, index)?;
            let value = emit_value(store, options, class_name, value)?;
            let lhs = qualify(&name, access, class_name);
            let _ = writeln!(out, "{tab}{lhs}[{index}] = {value};");
            Ok(())
        }
        FirMatch::Drop(value) => {
            let value = emit_value(store, options, class_name, value)?;
            let _ = writeln!(out, "{tab}{value};");
            Ok(())
        }
        FirMatch::NullStatement => Ok(()),
        FirMatch::Return(value) => {
            if let Some(value) = value {
                let value = emit_value(store, options, class_name, value)?;
                let _ = writeln!(out, "{tab}return {value};");
            } else {
                let _ = writeln!(out, "{tab}return;");
            }
            Ok(())
        }
        FirMatch::Block(_) => emit_block(store, out, options, class_name, stmt, indent),
        FirMatch::If { cond, then_block, else_block } => {
            let cond = emit_value(store, options, class_name, cond)?;
            let _ = writeln!(out, "{tab}if ({cond}) {{");
            emit_block(store, out, options, class_name, then_block, indent + 1)?;
            let _ = writeln!(out, "{tab}}}");
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{tab}else {{");
                emit_block(store, out, options, class_name, else_block, indent + 1)?;
                let _ = writeln!(out, "{tab}}}");
            }
            Ok(())
        }
        FirMatch::Control { cond, stmt } => {
            let cond = emit_value(store, options, class_name, cond)?;
            let _ = writeln!(out, "{tab}if ({cond}) {{");
            emit_stmt(store, out, options, class_name, stmt, indent + 1, Phase::Body)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::ForLoop { var, init, end, step, body, is_reverse } => {
            let init_val =
                if let FirMatch::DeclareVar { init: Some(v), .. } = match_fir(store, init) {
                    emit_value(store, options, class_name, v)?
                } else {
                    emit_value(store, options, class_name, init)?
                };
            let end = emit_value(store, options, class_name, end)?;
            let step = emit_value(store, options, class_name, step)?;
            if is_reverse {
                let _ = writeln!(
                    out,
                    "{tab}for (let {var}: i32 = {init_val}; {var} > {end}; {var} = {var} + {step}) {{"
                );
            } else {
                let _ = writeln!(
                    out,
                    "{tab}for (let {var}: i32 = {init_val}; {var} < {end}; {var} = {var} + {step}) {{"
                );
            }
            emit_block(store, out, options, class_name, body, indent + 1)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::SimpleForLoop { var, upper, body, is_reverse } => {
            let upper = emit_value(store, options, class_name, upper)?;
            if is_reverse {
                let _ = writeln!(
                    out,
                    "{tab}for (let {var}: i32 = ({upper}) - 1; {var} >= 0; {var} = {var} - 1) {{"
                );
            } else {
                let _ = writeln!(
                    out,
                    "{tab}for (let {var}: i32 = 0; {var} < {upper}; {var} = {var} + 1) {{"
                );
            }
            emit_block(store, out, options, class_name, body, indent + 1)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::IteratorForLoop { iterators, body, .. } => {
            // The scalar asc path doesn't lower vectorized iterator loops; emit
            // the body inline (parity with the C++ backend's comment + body).
            let _ = writeln!(out, "{tab}// iterator-for over [{}]", iterators.join(", "));
            emit_block(store, out, options, class_name, body, indent)?;
            Ok(())
        }
        FirMatch::WhileLoop { cond, body } => {
            let cond = emit_value(store, options, class_name, cond)?;
            let _ = writeln!(out, "{tab}while ({cond}) {{");
            emit_block(store, out, options, class_name, body, indent + 1)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Switch { cond, cases, default } => {
            let cond = emit_value(store, options, class_name, cond)?;
            let _ = writeln!(out, "{tab}switch ({cond}) {{");
            for (value, block) in cases {
                let _ = writeln!(out, "{tab}case {value}: {{");
                emit_block(store, out, options, class_name, block, indent + 1)?;
                let _ = writeln!(out, "{tab}    break;");
                let _ = writeln!(out, "{tab}}}");
            }
            if let Some(default) = default {
                let _ = writeln!(out, "{tab}default: {{");
                emit_block(store, out, options, class_name, default, indent + 1)?;
                let _ = writeln!(out, "{tab}}}");
            }
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        // UI / metadata / soundfile lower to comments (parity with C++ asc backend).
        FirMatch::OpenBox { label, .. } => {
            let _ = writeln!(out, "{tab}// ui openbox {label}");
            Ok(())
        }
        FirMatch::CloseBox => {
            let _ = writeln!(out, "{tab}// ui closebox");
            Ok(())
        }
        FirMatch::AddButton { label, .. } => {
            let _ = writeln!(out, "{tab}// ui button {label}");
            Ok(())
        }
        FirMatch::AddSlider { label, .. } => {
            let _ = writeln!(out, "{tab}// ui slider {label}");
            Ok(())
        }
        FirMatch::AddBargraph { label, .. } => {
            let _ = writeln!(out, "{tab}// ui bargraph {label}");
            Ok(())
        }
        FirMatch::AddSoundfile { .. } => {
            let _ = writeln!(out, "{tab}// ui soundfile unsupported in AssemblyScript backend");
            Ok(())
        }
        FirMatch::AddMetaDeclare { key, .. } => {
            let _ = writeln!(out, "{tab}// metadata {key}");
            Ok(())
        }
        FirMatch::Label(_) | FirMatch::ShiftArrayVar { .. } | FirMatch::DeclareStructType { .. } => {
            Ok(())
        }
        FirMatch::DeclareBufferIterators {
            name1, name2, channels, ..
        } => {
            // Buffer-iterator hints are a vectorization aid; the scalar asc path
            // does not use them. Emit a comment for parity with the C++ backend.
            let _ = writeln!(out, "{tab}// buffer iterators: {name1}, {name2}, channels={channels}");
            Ok(())
        }
        _ => Err(unsupported_node("statement", stmt, store)),
    }
}

/// Emits every statement in a FIR block (body phase).
fn emit_block(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    block: FirId,
    indent: usize,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return Err(unsupported_node("expected block", block, store));
    };
    for stmt in items {
        emit_stmt(store, out, options, class_name, stmt, indent, Phase::Body)?;
    }
    Ok(())
}

/// Emits one FIR function as a class method or a top-level function.
fn emit_declare_fun(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    decl: DeclareFunView<'_>,
    indent: usize,
    as_method: bool,
) -> Result<(), CodegenError> {
    faust_api::validate_canonical_dsp_api_signature(decl.name, decl.typ, decl.named_args)
        .map_err(|msg| CodegenError::new(CodegenErrorCode::InvalidModuleSection, msg))?;
    let tab = "    ".repeat(indent);

    // For DSP API methods, the FIR carries an explicit leading `dsp` arg; strip it.
    let strip_dsp_arg = is_dsp_api_method(decl.name)
        && matches!(decl.named_args.first(), Some(n) if n.name == "dsp");

    let (ret, params) = match decl.typ {
        FirType::Fun { args, ret } => {
            let ret = emit_type(ret, options);
            let skip = usize::from(strip_dsp_arg);
            let render = &args[skip.min(args.len())..];
            let mut rendered = Vec::with_capacity(render.len());
            for (i, arg_ty) in render.iter().enumerate() {
                let name = decl
                    .named_args
                    .get(i + skip)
                    .map_or_else(|| format!("arg{i}"), |n| n.name.clone());
                rendered.push(format!("{name}: {}", emit_type(arg_ty, options)));
            }
            (ret, rendered.join(", "))
        }
        other => (emit_type(other, options), String::new()),
    };

    // Canonical compute signature override (nested channel arrays).
    let params = if decl.name == "compute" {
        format!("count: i32, inputs: Array<Array<{FAUST_FLOAT}>>, outputs: Array<Array<{FAUST_FLOAT}>>")
    } else {
        params
    };

    let Some(body) = decl.body else {
        // Prototype-only: AssemblyScript has no C-style prototypes, so skip.
        return Ok(());
    };

    if as_method {
        let _ = writeln!(out, "{tab}{}({params}): {ret} {{", decl.name);
    } else {
        let _ = writeln!(out, "{tab}export function {}({params}): {ret} {{", decl.name);
    }
    if decl.name == "instanceConstants" && !block_stores_var(store, body, "fSampleRate") {
        let _ = writeln!(out, "{tab}    this.fSampleRate = sample_rate;");
    }
    emit_block(store, out, options, class_name, body, indent + 1)?;
    let _ = writeln!(out, "{tab}}}");
    Ok(())
}

/// Emits one FIR value expression into an AssemblyScript expression string.
fn emit_value(
    store: &FirStore,
    options: &AscOptions,
    class_name: &str,
    value: FirId,
) -> Result<String, CodegenError> {
    match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(format!("<i32>({value})")),
        FirMatch::Int64 { value, .. } => Ok(format!("<i64>({value})")),
        // Float literals are emitted as bare/generic numbers (not `<f64>(..)`),
        // so they adapt to the assignment/operand context. AssemblyScript has no
        // implicit f64->f32 narrowing, and Faust FIR carries signal constants as
        // Float64 even for single-precision (FaustFloat=f32) DSPs; a forced
        // `<f64>(..)` would fail to assign to an f32 field.
        FirMatch::Float32 { value, .. } => Ok(trim_float(f64::from(value))),
        FirMatch::Float64 { value, .. } => Ok(trim_float(value)),
        FirMatch::Bool { value, .. } => Ok(if value { "true" } else { "false" }.to_owned()),
        FirMatch::Quad { value, .. } => Ok(trim_float(value)),
        FirMatch::FixedPoint { value, .. } => Ok(trim_float(value)),
        FirMatch::Int32Array { values, .. } => {
            Ok(format_array(values.iter().map(ToString::to_string)))
        }
        FirMatch::Float32Array { values, .. } => Ok(format_array(
            values.iter().map(|v| trim_float(f64::from(*v))),
        )),
        FirMatch::Float64Array { values, .. }
        | FirMatch::QuadArray { values, .. }
        | FirMatch::FixedPointArray { values, .. } => {
            Ok(format_array(values.iter().map(|v| trim_float(*v))))
        }
        FirMatch::ValueArray { values, .. } => {
            let mut rendered = Vec::with_capacity(values.len());
            for item in values {
                rendered.push(emit_value(store, options, class_name, item)?);
            }
            Ok(format_array(rendered.into_iter()))
        }
        FirMatch::LoadVar { name, access, .. } | FirMatch::LoadVarAddress { name, access, .. } => {
            Ok(qualify(&name, access, class_name))
        }
        FirMatch::LoadTable { name, index, access, .. } => {
            let index = emit_value(store, options, class_name, index)?;
            Ok(format!("{}[{index}]", qualify(&name, access, class_name)))
        }
        FirMatch::TeeVar { name, access, value, .. } => {
            let value = emit_value(store, options, class_name, value)?;
            Ok(format!("({} = {value})", qualify(&name, access, class_name)))
        }
        FirMatch::BinOp { op, lhs, rhs, .. } => {
            let lhs = emit_value(store, options, class_name, lhs)?;
            let rhs = emit_value(store, options, class_name, rhs)?;
            Ok(format!("({lhs} {} {rhs})", emit_binop(op)))
        }
        FirMatch::Neg { value, .. } => {
            let value = emit_value(store, options, class_name, value)?;
            Ok(format!("(-{value})"))
        }
        FirMatch::Cast { typ, value } | FirMatch::Bitcast { typ, value } => {
            let value = emit_value(store, options, class_name, value)?;
            Ok(format!("<{}>({value})", emit_type(&typ, options)))
        }
        FirMatch::Select2 { cond, then_value, else_value, .. } => {
            let cond = emit_value(store, options, class_name, cond)?;
            let then_value = emit_value(store, options, class_name, then_value)?;
            let else_value = emit_value(store, options, class_name, else_value)?;
            Ok(format!("({cond} ? {then_value} : {else_value})"))
        }
        FirMatch::FunCall { name, args, .. } => {
            let mut rendered = Vec::with_capacity(args.len());
            for arg in args {
                rendered.push(emit_value(store, options, class_name, arg)?);
            }
            Ok(format!("{}({})", map_fun_name(&name), rendered.join(", ")))
        }
        FirMatch::NullValue { .. } => Ok("null".to_owned()),
        FirMatch::NewDsp { name, .. } => Ok(format!("new {name}()")),
        _ => Err(unsupported_node("value", value, store)),
    }
}

/// Qualifies a name with `this.` / `<ClassName>.` based on FIR access kind.
fn qualify(name: &str, access: AccessType, class_name: &str) -> String {
    match access {
        AccessType::Struct => format!("this.{name}"),
        AccessType::Static => format!("{class_name}.{name}"),
        _ => name.to_owned(),
    }
}

/// Emits a class field declaration `name: type` (arrays become `StaticArray<T>`).
fn emit_member_decl(typ: &FirType, name: &str, options: &AscOptions) -> String {
    format!("{name}: {}", emit_type(typ, options))
}

/// Maps a bare FIR math/function name to the AssemblyScript spelling.
///
/// Mirrors the C++ asc backend's `gMathLibTable`: 32-bit (`*f`) intrinsics map to
/// `Mathf.*`, double versions to `Math.*`, `abs`/`fabs` to `Math.abs`, and the
/// min/max helpers to AssemblyScript's generic `min<T>`/`max<T>`.
fn map_fun_name(name: &str) -> String {
    match name {
        "abs" | "fabs" => return "Math.abs".to_owned(),
        "fabsf" => return "Mathf.abs".to_owned(),
        "min_i" => return "min<i32>".to_owned(),
        "max_i" => return "max<i32>".to_owned(),
        // single-precision min/max (Faust `*f` spelling) -> generic f32 helpers
        "min_f" | "fminf" => return "min<f32>".to_owned(),
        "max_f" | "fmaxf" => return "max<f32>".to_owned(),
        // double-precision min/max -> generic f64 helpers
        "min_" | "fmin" => return "min<f64>".to_owned(),
        "max_" | "fmax" => return "max<f64>".to_owned(),
        // fmod has no Math helper; AssemblyScript uses the `%` operator instead,
        // but as a call site we fall back to the f64/f32 remainder helpers.
        "fmod" | "fmodf" => return "_fmod".to_owned(),
        _ => {}
    }
    // float (32-bit) intrinsics -> Mathf.*, double -> Math.*
    if let Some(stripped) = name.strip_suffix('f')
        && let Some(op) = FirMathOp::from_symbol(stripped)
    {
        return format!("Mathf.{}", op.symbol());
    }
    if let Some(op) = FirMathOp::from_symbol(name) {
        return format!("Math.{}", op.symbol());
    }
    name.to_owned()
}

/// Maps one FIR binary operator to its AssemblyScript token spelling.
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
        FirBinOp::LRsh => ">>>",
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::Lt => "<",
        FirBinOp::Le => "<=",
        FirBinOp::Gt => ">",
        FirBinOp::Ge => ">=",
    }
}

/// Renders a FIR type into AssemblyScript spelling.
fn emit_type(typ: &FirType, options: &AscOptions) -> String {
    match typ {
        FirType::Int32 => "i32".to_owned(),
        FirType::Int64 => "i64".to_owned(),
        FirType::Float32 => "f32".to_owned(),
        FirType::Float64 => "f64".to_owned(),
        FirType::FaustFloat => FAUST_FLOAT.to_owned(),
        FirType::Quad => options.quad_type_name.clone(),
        FirType::FixedPoint => options.fixed_type_name.clone(),
        FirType::Bool => "bool".to_owned(),
        FirType::Void => "void".to_owned(),
        FirType::Obj => "usize".to_owned(),
        FirType::Sound => "usize".to_owned(),
        FirType::UI => "usize".to_owned(),
        FirType::Meta => "usize".to_owned(),
        FirType::Ptr(inner) => format!("StaticArray<{}>", emit_type(inner, options)),
        FirType::Array(inner, _size) => format!("StaticArray<{}>", emit_type(inner, options)),
        FirType::Vector(inner, _lanes) => format!("StaticArray<{}>", emit_type(inner, options)),
        FirType::Struct(name, _fields) => name.clone(),
        FirType::Fun { .. } => "usize".to_owned(),
    }
}

/// Emits `DeclareTable(Static)` nodes as module-level `const` StaticArrays.
fn emit_static_tables(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    block: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(stmts) = match_fir(store, block) else {
        return Ok(());
    };
    let mut any = false;
    for stmt in stmts {
        if let FirMatch::DeclareTable { name, elem_type, values, .. } = match_fir(store, stmt) {
            let ty = emit_type(&elem_type, options);
            if values.is_empty() {
                let _ = writeln!(
                    out,
                    "const {name}: StaticArray<{ty}> = new StaticArray<{ty}>(0);"
                );
            } else {
                let mut elems = Vec::with_capacity(values.len());
                for v in &values {
                    elems.push(emit_value(store, options, "", *v)?);
                }
                let _ = writeln!(
                    out,
                    "const {name}: StaticArray<{ty}> = StaticArray.fromArray<{ty}>([{}]);",
                    elems.join(", ")
                );
            }
            any = true;
        }
    }
    if any {
        let _ = writeln!(out);
    }
    Ok(())
}

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

fn block_declares_var(store: &FirStore, block: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return false;
    };
    items.iter().any(|id| {
        matches!(match_fir(store, *id), FirMatch::DeclareVar { name: v, .. } if v == name)
    })
}

fn block_stores_var(store: &FirStore, block: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return false;
    };
    items.iter().any(|id| {
        matches!(match_fir(store, *id), FirMatch::StoreVar { name: v, .. } if v == name)
    })
}

/// Escapes arbitrary text for embedding inside an AssemblyScript double-quoted
/// string literal (used by the `getJSON()` snapshot).
fn escape_as_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 16);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

fn trim_float(value: f64) -> String {
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

fn format_array(values: impl Iterator<Item = String>) -> String {
    format!("[{}]", values.collect::<Vec<_>>().join(", "))
}

fn invalid_section(section: &str, id: FirId, store: &FirStore) -> CodegenError {
    CodegenError::new(
        CodegenErrorCode::InvalidModuleSection,
        format!(
            "section '{section}' must be a FIR block, got {:?} at node {}",
            match_fir(store, id),
            id.as_u32()
        ),
    )
}

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

#[must_use]
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use fir::FirBuilder;

    #[test]
    fn rejects_non_module_root() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let not_module = b.int32(7);
        let err = generate_asc_module(&store, not_module, &AscOptions::default())
            .expect_err("non-module root must fail");
        assert_eq!(err.code(), CodegenErrorCode::RootNotModule);
        assert!(err.to_string().contains("FRS-CGEN-ASC-0001"));
    }

    #[test]
    fn accepts_minimal_module() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
        let static_decls = b.block(&[]);
        let module = b.module(1, 2, "mydsp", dsp_struct, globals, functions, static_decls);
        let out = generate_asc_module(&store, module, &AscOptions::default())
            .expect("module should generate");
        assert!(out.contains("export class mydsp {"));
        assert!(out.contains("getNumInputs(): i32 { return 1; }"));
        assert!(out.contains("getNumOutputs(): i32 { return 2; }"));
        assert!(out.contains("compute(count: i32, inputs: Array<Array<f32>>, outputs: Array<Array<f32>>): void"));
        assert!(out.contains("// Language: AssemblyScript"));
    }

    #[test]
    fn emits_this_prefix_for_struct_access() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let val = b.float32(0.5);
        let store_field = b.store_var("fHslider0", AccessType::Struct, val);
        let body = b.block(&[store_field]);
        let fun_ty = FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Int32],
            ret: Box::new(FirType::Void),
        };
        let args = vec![
            NamedType { name: "dsp".to_owned(), typ: FirType::Ptr(Box::new(FirType::Obj)) },
            NamedType { name: "sample_rate".to_owned(), typ: FirType::Int32 },
        ];
        let fun = b.declare_fun("instanceConstants", fun_ty, &args, Some(body), false);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[fun]);
        let static_decls = b.block(&[]);
        let module = b.module(0, 1, "mydsp", dsp_struct, globals, functions, static_decls);
        let out = generate_asc_module(&store, module, &AscOptions::default())
            .expect("module should generate");
        assert!(out.contains("this.fHslider0 = 0.5;"));
        assert!(out.contains("instanceConstants(sample_rate: i32): void {"));
    }
}
