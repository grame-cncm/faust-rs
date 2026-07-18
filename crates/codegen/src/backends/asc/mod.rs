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

use fir::{
    AccessType, FirBinOp, FirId, FirMatch, FirMathOp, FirStore, FirType, NamedType, match_fir,
};

use crate::backends::{faust_api, purity::is_obviously_side_effect_free_value};

pub const BACKEND_NAME: &str = "asc";

/// AssemblyScript backend options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AscOptions {
    /// Optional class name override for the FIR module name.
    pub class_name: Option<String>,
    /// Emit double-precision FaustFloat (`f64`) instead of single (`f32`).
    pub double_precision: bool,
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
            double_precision: false,
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
    let _ = writeln!(
        out,
        "// Code generated with faust-rs (https://faust.grame.fr)"
    );
    let _ = writeln!(out, "// Language: AssemblyScript (experimental)");
    let _ = writeln!(out, "// name: {}", module.name);
    let _ = writeln!(out);
    emit_runtime_helpers(&mut out);
    let _ = writeln!(out);

    // Top-level helper functions (non-DSP-API DeclareFun in the globals section)
    // are emitted as module-level `export function`s before the class.
    emit_toplevel_functions(store, &mut out, options, &class_name, module.globals)?;

    let _ = writeln!(out, "export class {class_name} {{");

    // ---- fields: DSP struct state + globals (non-function) ----
    if !block_declares_var(store, module.dsp_struct, "fSampleRate")
        && !block_declares_var(store, module.globals, "fSampleRate")
    {
        let _ = writeln!(out, "    fSampleRate: i32 = 0;");
    }
    emit_section_fields(store, &mut out, options, &class_name, module.dsp_struct, 1)?;
    emit_section_fields(store, &mut out, options, &class_name, module.globals, 1)?;

    // Compile-time constant tables become `static` class members so the
    // qualified `ClassName.table` references resolve, mirroring the C++ asc
    // backend's class-static table placement (which downstream tooling's
    // static-field hoisting also expects).
    emit_static_tables(store, &mut out, options, module.static_decls)?;
    let _ = writeln!(out);

    // ---- methods, in the C++ produceClass order ----
    // Downstream tooling extracts method bodies by FIRST textual occurrence of
    // `name(`, so every instance* DEFINITION must precede the synthesized
    // instanceInit/init methods that CALL them (mirroring the C++ backend).
    emit_methods_canonical_order(
        store,
        &mut out,
        options,
        &class_name,
        &module,
        &declared_functions,
    )?;

    let _ = writeln!(out, "}}");
    Ok(out)
}

/// Emits small runtime helpers for FIR math names that AssemblyScript does not
/// expose with C/libm spelling.
fn emit_runtime_helpers(out: &mut String) {
    let _ = writeln!(
        out,
        r#"function _fmodf(a: f32, b: f32): f32 {{
  return a % b;
}}

function _remainderf(a: f32, b: f32): f32 {{
  return a - _rintf(a / b) * b;
}}

function _rintf(x: f32): f32 {{
  let floor: f32 = Mathf.floor(x);
  let frac: f32 = x - floor;
  if (frac < 0.5) return floor;
  if (frac > 0.5) return floor + 1.0;
  let i: i32 = <i32>floor;
  return (i & 1) == 0 ? floor : floor + 1.0;
}}

function _exp10f(x: f32): f32 {{
  return Mathf.pow(10.0, x);
}}

function _isnanf(x: f32): i32 {{
  return isNaN<f32>(x) ? 1 : 0;
}}

function _isinff(x: f32): i32 {{
  return isFinite<f32>(x) ? 0 : (isNaN<f32>(x) ? 0 : 1);
}}

function _copysignf(a: f32, b: f32): f32 {{
  let sign: bool = b < 0.0 || (b == 0.0 && 1.0 / b < 0.0);
  return sign ? -Mathf.abs(a) : Mathf.abs(a);
}}

function _fmod(a: f64, b: f64): f64 {{
  return a % b;
}}

function _remainder(a: f64, b: f64): f64 {{
  return a - _rint(a / b) * b;
}}

function _rint(x: f64): f64 {{
  let floor: f64 = Math.floor(x);
  let frac: f64 = x - floor;
  if (frac < 0.5) return floor;
  if (frac > 0.5) return floor + 1.0;
  let i: i64 = <i64>floor;
  return (i & 1) == 0 ? floor : floor + 1.0;
}}

function _exp10(x: f64): f64 {{
  return Math.pow(10.0, x);
}}

function _isnan(x: f64): i32 {{
  return isNaN<f64>(x) ? 1 : 0;
}}

function _isinf(x: f64): i32 {{
  return isFinite<f64>(x) ? 0 : (isNaN<f64>(x) ? 0 : 1);
}}

function _copysign(a: f64, b: f64): f64 {{
  let sign: bool = b < 0.0 || (b == 0.0 && 1.0 / b < 0.0);
  return sign ? -Math.abs(a) : Math.abs(a);
}}

@external("env", "_soundfileLength")
declare function _soundfileLength(slot: i32, part: i32): i32;

@external("env", "_soundfileRate")
declare function _soundfileRate(slot: i32, part: i32): i32;

@external("env", "_soundfileBuffer")
declare function _soundfileBuffer(slot: i32, chan: i32, part: i32, idx: i32): f64;"#
    );
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
            name,
            typ,
            args,
            body,
            ..
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

/// Emits every class method in the C++ `produceClass` order, pulling declared
/// FIR functions when present and synthesizing the rest.
///
/// Order: info getters, getJSON, metadata/buildUserInterface, classInit,
/// instanceResetUserInterface, instanceClear, instanceConstants, instanceInit,
/// init, compute, then any remaining declared functions.
fn emit_methods_canonical_order(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    module: &ModuleView,
    declared: &[String],
) -> Result<(), CodegenError> {
    let has = |n: &str| declared.iter().any(|d| d == n);
    let mut emitted: Vec<&str> = Vec::new();

    let _ = writeln!(out, "    getSampleRate(): i32 {{");
    let _ = writeln!(out, "        return this.fSampleRate;");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    getNumInputs(): i32 {{");
    let _ = writeln!(out, "        return {};", module.num_inputs);
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    getNumOutputs(): i32 {{");
    let _ = writeln!(out, "        return {};", module.num_outputs);
    let _ = writeln!(out, "    }}");

    // JSON snapshot for tooling (UI/param extraction), mirroring the C++ asc
    // backend's getJSON().
    if let Some(json) = options.json.as_deref() {
        let _ = writeln!(out, "    getJSON(): string {{");
        let _ = writeln!(out, "        return \"{}\";", escape_as_string(json));
        let _ = writeln!(out, "    }}");
    }

    for name in ["metadata", "buildUserInterface"] {
        if has(name) {
            emit_declared_method(store, out, options, class_name, module.functions, name)?;
            emitted.push(name);
        }
    }

    let _ = writeln!(out, "    static classInit(sample_rate: i32): void {{");
    let _ = writeln!(out, "    }}");

    for name in [
        "instanceResetUserInterface",
        "instanceClear",
        "instanceConstants",
    ] {
        if has(name) {
            emit_declared_method(store, out, options, class_name, module.functions, name)?;
            emitted.push(name);
        } else if name == "instanceConstants" {
            let _ = writeln!(out, "    instanceConstants(sample_rate: i32): void {{");
            let _ = writeln!(out, "        this.fSampleRate = sample_rate;");
            let _ = writeln!(out, "    }}");
        } else {
            let _ = writeln!(out, "    {name}(): void {{");
            let _ = writeln!(out, "    }}");
        }
    }

    let _ = writeln!(out, "    instanceInit(sample_rate: i32): void {{");
    let _ = writeln!(out, "        this.instanceConstants(sample_rate);");
    let _ = writeln!(out, "        this.instanceResetUserInterface();");
    let _ = writeln!(out, "        this.instanceClear();");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    init(sample_rate: i32): void {{");
    let _ = writeln!(out, "        {class_name}.classInit(sample_rate);");
    let _ = writeln!(out, "        this.instanceInit(sample_rate);");
    let _ = writeln!(out, "    }}");

    if has("compute") {
        emit_declared_method(store, out, options, class_name, module.functions, "compute")?;
        emitted.push("compute");
    } else {
        let real_type = faust_float_type(options);
        let _ = writeln!(
            out,
            "    compute(count: i32, inputs: Array<StaticArray<{real_type}>>, outputs: Array<StaticArray<{real_type}>>): void {{"
        );
        let _ = writeln!(out, "    }}");
    }

    // Remaining declared functions in original order.
    let FirMatch::Block(items) = match_fir(store, module.functions) else {
        return Err(invalid_section("functions", module.functions, store));
    };
    for item in items {
        if let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            ..
        } = match_fir(store, item)
        {
            if emitted.iter().any(|done| *done == name) {
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
                1,
                /* as_method = */ true,
            )?;
        }
    }
    Ok(())
}

/// Emits one declared FIR function (by name) as a class method.
fn emit_declared_method(
    store: &FirStore,
    out: &mut String,
    options: &AscOptions,
    class_name: &str,
    functions: FirId,
    method: &str,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Err(invalid_section("functions", functions, store));
    };
    for item in items {
        if let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            ..
        } = match_fir(store, item)
            && name == method
        {
            return emit_declare_fun(
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
                1,
                /* as_method = */ true,
            );
        }
    }
    Ok(())
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
        FirMatch::DeclareVar {
            name,
            typ,
            access,
            init,
        } => {
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
            }
            Ok(())
        }
        FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            access,
        } => {
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
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            let value = emit_value(store, options, class_name, value)?;
            let lhs = qualify(&name, access, class_name);
            let _ = writeln!(out, "{tab}{lhs} = {value};");
            Ok(())
        }
        FirMatch::StoreTable {
            name,
            index,
            value,
            access,
        } => {
            let index = emit_value(store, options, class_name, index)?;
            let value = emit_value(store, options, class_name, value)?;
            let lhs = qualify(&name, access, class_name);
            let _ = writeln!(out, "{tab}{lhs}[{index}] = {value};");
            Ok(())
        }
        FirMatch::Drop(value) => {
            if is_obviously_side_effect_free_value(store, value) {
                return Ok(());
            }
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
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
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
            emit_stmt(
                store,
                out,
                options,
                class_name,
                stmt,
                indent + 1,
                Phase::Body,
            )?;
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
        FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } => {
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
        FirMatch::IteratorForLoop {
            iterators, body, ..
        } => {
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
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
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
            let _ = writeln!(
                out,
                "{tab}// ui soundfile unsupported in AssemblyScript backend"
            );
            Ok(())
        }
        FirMatch::AddMetaDeclare { key, .. } => {
            let _ = writeln!(out, "{tab}// metadata {key}");
            Ok(())
        }
        FirMatch::Label(_)
        | FirMatch::ShiftArrayVar { .. }
        | FirMatch::DeclareStructType { .. } => Ok(()),
        FirMatch::DeclareBufferIterators {
            name1,
            name2,
            channels,
            ..
        } => {
            // Buffer-iterator hints are a vectorization aid; the scalar asc path
            // does not use them. Emit a comment for parity with the C++ backend.
            let _ = writeln!(
                out,
                "{tab}// buffer iterators: {name1}, {name2}, channels={channels}"
            );
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
        let real_type = faust_float_type(options);
        format!(
            "count: i32, inputs: Array<StaticArray<{real_type}>>, outputs: Array<StaticArray<{real_type}>>"
        )
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
        let _ = writeln!(
            out,
            "{tab}export function {}({params}): {ret} {{",
            decl.name
        );
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
        FirMatch::LoadTable {
            name,
            index,
            access,
            ..
        } => {
            let index = emit_value(store, options, class_name, index)?;
            Ok(format!("{}[{index}]", qualify(&name, access, class_name)))
        }
        FirMatch::LoadSoundfileLength { var, part } => {
            let part = emit_value(store, options, class_name, part)?;
            Ok(format!(
                "_soundfileLength(<i32>({}), {part})",
                qualify(&var, AccessType::Struct, class_name)
            ))
        }
        FirMatch::LoadSoundfileRate { var, part } => {
            let part = emit_value(store, options, class_name, part)?;
            Ok(format!(
                "_soundfileRate(<i32>({}), {part})",
                qualify(&var, AccessType::Struct, class_name)
            ))
        }
        FirMatch::LoadSoundfileBuffer {
            var,
            chan,
            part,
            idx,
            typ,
        } => {
            let chan = emit_value(store, options, class_name, chan)?;
            let part = emit_value(store, options, class_name, part)?;
            let idx = emit_value(store, options, class_name, idx)?;
            Ok(format!(
                "<{}>(_soundfileBuffer(<i32>({}), {chan}, {part}, {idx}))",
                emit_type(&typ, options),
                qualify(&var, AccessType::Struct, class_name)
            ))
        }
        FirMatch::TeeVar {
            name,
            access,
            value,
            ..
        } => {
            let value = emit_value(store, options, class_name, value)?;
            Ok(format!(
                "({} = {value})",
                qualify(&name, access, class_name)
            ))
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
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
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
            Ok(format!(
                "{}({})",
                map_fun_name(&name, options),
                rendered.join(", ")
            ))
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
/// FaustFloat helpers follow `AscOptions::double_precision`; explicit `*f`
/// libm names stay on the `Mathf`/`f32` path.
fn map_fun_name(name: &str, options: &AscOptions) -> String {
    let real_type = faust_float_type(options);
    let math = math_namespace(options);
    let fmod = if options.double_precision {
        "_fmod"
    } else {
        "_fmodf"
    };
    let remainder = if options.double_precision {
        "_remainder"
    } else {
        "_remainderf"
    };
    let rint = if options.double_precision {
        "_rint"
    } else {
        "_rintf"
    };
    let exp10 = if options.double_precision {
        "_exp10"
    } else {
        "_exp10f"
    };
    let isnan = if options.double_precision {
        "_isnan"
    } else {
        "_isnanf"
    };
    let isinf = if options.double_precision {
        "_isinf"
    } else {
        "_isinff"
    };
    let copysign = if options.double_precision {
        "_copysign"
    } else {
        "_copysignf"
    };
    match name {
        "abs" => return "abs<i32>".to_owned(),
        "fabs" => return format!("{math}.abs"),
        "fabsf" => return "Mathf.abs".to_owned(),
        "min_i" => return "min<i32>".to_owned(),
        "max_i" => return "max<i32>".to_owned(),
        "min_f" | "min_" | "fmin" => return format!("min<{real_type}>"),
        "max_f" | "max_" | "fmax" => return format!("max<{real_type}>"),
        "fminf" => return "min<f32>".to_owned(),
        "fmaxf" => return "max<f32>".to_owned(),
        "fmod" => return fmod.to_owned(),
        "fmodf" => return "_fmodf".to_owned(),
        "remainder" => return remainder.to_owned(),
        "remainderf" => return "_remainderf".to_owned(),
        "rint" => return rint.to_owned(),
        "rintf" => return "_rintf".to_owned(),
        "exp10" => return exp10.to_owned(),
        "exp10f" => return "_exp10f".to_owned(),
        "isnan" => return isnan.to_owned(),
        "isnanf" => return "_isnanf".to_owned(),
        "isinf" => return isinf.to_owned(),
        "isinff" => return "_isinff".to_owned(),
        "copysign" => return copysign.to_owned(),
        "copysignf" => return "_copysignf".to_owned(),
        "acosh" => return format!("{math}.acosh"),
        "asinh" => return format!("{math}.asinh"),
        "atanh" => return format!("{math}.atanh"),
        "cosh" => return format!("{math}.cosh"),
        "sinh" => return format!("{math}.sinh"),
        "tanh" => return format!("{math}.tanh"),
        "acoshf" => return "Mathf.acosh".to_owned(),
        "asinhf" => return "Mathf.asinh".to_owned(),
        "atanhf" => return "Mathf.atanh".to_owned(),
        "coshf" => return "Mathf.cosh".to_owned(),
        "sinhf" => return "Mathf.sinh".to_owned(),
        "tanhf" => return "Mathf.tanh".to_owned(),
        _ => {}
    }
    if let Some(stripped) = name.strip_suffix('f')
        && let Some(op) = FirMathOp::from_symbol(stripped)
    {
        return format!("Mathf.{}", op.symbol());
    }
    if let Some(op) = FirMathOp::from_symbol(name) {
        return format!("{math}.{}", op.symbol());
    }
    name.to_owned()
}

fn faust_float_type(options: &AscOptions) -> &'static str {
    if options.double_precision {
        "f64"
    } else {
        "f32"
    }
}

fn math_namespace(options: &AscOptions) -> &'static str {
    if options.double_precision {
        "Math"
    } else {
        "Mathf"
    }
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
        FirType::FaustFloat => faust_float_type(options).to_owned(),
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

/// Emits `DeclareTable(Static)` nodes as `static` class-member StaticArrays.
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
        if let FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } = match_fir(store, stmt)
        {
            let ty = emit_type(&elem_type, options);
            if values.is_empty() {
                let _ = writeln!(
                    out,
                    "    static {name}: StaticArray<{ty}> = new StaticArray<{ty}>(0);"
                );
            } else {
                let mut elems = Vec::with_capacity(values.len());
                for v in &values {
                    elems.push(emit_value(store, options, "", *v)?);
                }
                let _ = writeln!(
                    out,
                    "    static {name}: StaticArray<{ty}> = StaticArray.fromArray<{ty}>([{}]);",
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
    items.iter().any(
        |id| matches!(match_fir(store, *id), FirMatch::DeclareVar { name: v, .. } if v == name),
    )
}

fn block_stores_var(store: &FirStore, block: FirId, name: &str) -> bool {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return false;
    };
    items
        .iter()
        .any(|id| matches!(match_fir(store, *id), FirMatch::StoreVar { name: v, .. } if v == name))
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

/// Formats one `f64` as an AssemblyScript floating literal.
///
/// Special values use the AssemblyScript/TypeScript global spellings (`NaN`,
/// `Infinity`, `-Infinity`); without this special-casing, Rust's `Display`
/// output plus the `.0` suffix produced invalid literals such as `inf.0` or
/// `NaN.0` (the same bug class fixed for c/cpp in commit `2b615948` and never
/// ported here — see the C-family plan §5). Negative zero normalizes to
/// `"0.0"`, matching every other textual backend and upstream constant
/// folding. Deliberately *not* shared with `c_family::trim_float`: the
/// special-value spellings genuinely differ (C `NAN`/`INFINITY` macros vs
/// AssemblyScript globals).
fn trim_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity".to_owned()
        } else {
            "Infinity".to_owned()
        };
    }
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    if text == "-0.0" {
        "0.0".to_owned()
    } else {
        text
    }
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
    use fir::{FirBuilder, FirType};

    #[test]
    fn pure_drop_is_elided_but_foreign_call_drop_is_retained() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let one = b.float32(1.0);
        let pure = b.drop_(one);
        let foreign = b.fun_call("foreign", &[], FirType::Float32);
        let effectful = b.drop_(foreign);
        let mut out = String::new();

        emit_stmt(
            &store,
            &mut out,
            &AscOptions::default(),
            "mydsp",
            pure,
            0,
            Phase::Body,
        )
        .expect("pure drop should emit");
        assert!(out.is_empty());

        emit_stmt(
            &store,
            &mut out,
            &AscOptions::default(),
            "mydsp",
            effectful,
            0,
            Phase::Body,
        )
        .expect("effectful drop should emit");
        assert!(out.contains("foreign()"));
    }

    #[test]
    /// Regression for the C-family plan §5 finding: special values must use
    /// the AssemblyScript global spellings — the previous formatter emitted
    /// invalid literals (`inf.0`, `NaN.0`) for any constant folding to
    /// NaN/infinity, and `-0.0` for negative zero (every other textual
    /// backend normalizes it).
    fn trim_float_spells_assemblyscript_special_values() {
        assert_eq!(trim_float(f64::NAN), "NaN");
        assert_eq!(trim_float(f64::INFINITY), "Infinity");
        assert_eq!(trim_float(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(trim_float(-0.0), "0.0");
        assert_eq!(trim_float(0.5), "0.5");
        assert_eq!(trim_float(3.0), "3.0");
    }

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
        assert!(out.contains("getNumInputs(): i32 {"));
        assert!(out.contains("        return 1;"));
        assert!(out.contains("getNumOutputs(): i32 {"));
        assert!(out.contains("        return 2;"));
        assert!(out.contains(
            "compute(count: i32, inputs: Array<StaticArray<f32>>, outputs: Array<StaticArray<f32>>): void"
        ));
        assert!(out.contains("// Language: AssemblyScript"));
    }

    #[test]
    fn double_precision_uses_f64_compute_buffers() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[]);
        let static_decls = b.block(&[]);
        let module = b.module(1, 1, "mydsp", dsp_struct, globals, functions, static_decls);
        let out = generate_asc_module(
            &store,
            module,
            &AscOptions {
                double_precision: true,
                ..AscOptions::default()
            },
        )
        .expect("module should generate");
        assert!(out.contains(
            "compute(count: i32, inputs: Array<StaticArray<f64>>, outputs: Array<StaticArray<f64>>): void"
        ));
    }

    #[test]
    fn initializes_local_static_array_buffers() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let vbuf = b.declare_var(
            "vbuf0",
            FirType::Array(Box::new(FirType::FaustFloat), 32),
            AccessType::Stack,
            None,
        );
        let body = b.block(&[vbuf]);
        let args = [
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "count".to_owned(),
                typ: FirType::Int32,
            },
            NamedType {
                name: "inputs".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            },
            NamedType {
                name: "outputs".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            },
        ];
        let compute = b.declare_fun(
            "compute",
            FirType::Fun {
                args: args.iter().map(|arg| arg.typ.clone()).collect(),
                ret: Box::new(FirType::Void),
            },
            &args,
            Some(body),
            false,
        );
        let functions = b.block(&[compute]);
        let static_decls = b.block(&[]);
        let module = b.module(0, 0, "mydsp", dsp_struct, globals, functions, static_decls);
        let out = generate_asc_module(&store, module, &AscOptions::default())
            .expect("module should generate");
        assert!(out.contains("let vbuf0: StaticArray<f32> = new StaticArray<f32>(32);"));
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
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "sample_rate".to_owned(),
                typ: FirType::Int32,
            },
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

    #[test]
    fn lowers_soundfile_access_to_host_imports() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let soundfile = b.declare_var("fSound0", FirType::Sound, AccessType::Struct, None);
        let dsp_struct = b.block(&[soundfile]);
        let globals = b.block(&[]);
        let static_decls = b.block(&[]);

        let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
        let args = vec![
            NamedType {
                name: "dsp".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Obj)),
            },
            NamedType {
                name: "count".to_owned(),
                typ: FirType::Int32,
            },
            NamedType {
                name: "inputs".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            },
            NamedType {
                name: "outputs".to_owned(),
                typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            },
        ];
        let zero = b.int32(0);
        let output0_ptr = b.load_table("outputs", AccessType::FunArgs, zero, ptr_ty.clone());
        let output0 = b.declare_var(
            "output0",
            ptr_ty.clone(),
            AccessType::Stack,
            Some(output0_ptr),
        );
        let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
        let chan0 = b.int32(0);
        let part0 = b.int32(0);
        let sample = b.load_soundfile_buffer("fSound0", chan0, part0, i0, FirType::FaustFloat);
        let store_out = b.store_table("output0", AccessType::Stack, i0, sample);
        let len = b.load_soundfile_length("fSound0", part0);
        let rate = b.load_soundfile_rate("fSound0", part0);
        let drop_len = b.drop_(len);
        let drop_rate = b.drop_(rate);
        let loop_body = b.block(&[store_out]);
        let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        let loop_stmt = b.simple_for_loop("i0", count, loop_body, false);
        let body = b.block(&[output0, drop_len, drop_rate, loop_stmt]);
        let compute = b.declare_fun(
            "compute",
            FirType::Fun {
                args: args.iter().map(|arg| arg.typ.clone()).collect(),
                ret: Box::new(FirType::Void),
            },
            &args,
            Some(body),
            false,
        );
        let functions = b.block(&[compute]);
        let module = b.module(0, 1, "mydsp", dsp_struct, globals, functions, static_decls);

        let out = generate_asc_module(&store, module, &AscOptions::default())
            .expect("soundfile access should generate");
        assert!(out.contains("declare function _soundfileLength"));
        assert!(out.contains("_soundfileLength(<i32>(this.fSound0), <i32>(0))"));
        assert!(out.contains("_soundfileRate(<i32>(this.fSound0), <i32>(0))"));
        assert!(
            out.contains("<f32>(_soundfileBuffer(<i32>(this.fSound0), <i32>(0), <i32>(0), i0))")
        );
    }
}
