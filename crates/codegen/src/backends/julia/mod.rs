//! Julia backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! - `compiler/generator/julia/julia_code_container.cpp`
//! - `compiler/generator/julia/julia_instructions.hh`
//!
//! # Current slice and boundary
//! - Module-first emission from FIR `Module`.
//! - Faust-style Julia shell (`mutable struct mydsp{T} <: dsp`, lifecycle
//!   methods, `compute!`).
//! - Julia arrays are one-based, while lowered FIR loop variables and tape/table
//!   offsets remain Faust/C-style zero-based. Table accesses therefore add one at
//!   every generated Julia indexing boundary.
//! - The backend is intentionally downstream of `signal_fir`: it does not
//!   reconstruct Signal-IR typing/promotion. It consumes explicit FIR types and
//!   converts them to Julia spellings at statement/value emission time.
//!
//! # Julia runtime shape
//!
//! The generated source follows the same high-level contract as C++ Faust
//! `-lang julia`: it assumes the host has definitions for runtime names such as
//! `dsp`, `UI`, `FMeta`, `FAUSTFLOAT`, and the `*!` UI callbacks. This module is
//! responsible for producing backend text, not for packaging a Julia runtime.
//!
//! # Indexing and pointer values
//!
//! FIR keeps compute loops and table offsets zero-based because the same FIR is
//! shared by C/C++/WASM/interpreter style backends. Julia indexing is one-based,
//! so this emitter adds one only at final `LoadTable`/`StoreTable` boundaries.
//! This keeps loop variables and arithmetic structurally close to the C++
//! reference while producing valid Julia array accesses.
//!
//! FIR pointer loads for `inputs`/`outputs` lower to Julia `@view` slices, not
//! raw addresses. Consequently `emit_cast` deliberately preserves
//! `FirType::Ptr(_)` values rather than wrapping views in scalar constructors.
//!
//! Unsupported FIR nodes fail with `FRS-CGEN-JULIA-0003`.

use std::fmt::Write as _;

use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

use crate::backends::{faust_api, purity::is_obviously_side_effect_free_value};

pub const BACKEND_NAME: &str = "julia";

/// Julia backend options for module-first emission.
///
/// The first backend slice mirrors the C/C++ backend convention of defaulting to
/// `mydsp` for deterministic generated type names. `None` means "use the FIR
/// module name", which is useful for fixture-level backend tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JuliaOptions {
    /// Optional Julia DSP struct name override for the FIR module name.
    pub class_name: Option<String>,
    /// Julia real scalar type used by the generated `REAL` alias.
    pub real_type: JuliaRealType,
}

impl Default for JuliaOptions {
    fn default() -> Self {
        Self {
            class_name: Some("mydsp".to_owned()),
            real_type: JuliaRealType::Float32,
        }
    }
}

/// Scalar precision selected for the Julia backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JuliaRealType {
    /// Single precision, matching Faust default code generation.
    #[default]
    Float32,
    /// Double precision, matching Faust `-double`.
    Float64,
}

impl JuliaRealType {
    fn julia_name(self) -> &'static str {
        match self {
            Self::Float32 => "Float32",
            Self::Float64 => "Float64",
        }
    }
}

/// Stable machine-readable error codes for the Julia backend emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// Root FIR node is not a module (`FirMatch::Module`).
    RootNotModule,
    /// One module section is not a FIR block or canonical API signatures are invalid.
    InvalidModuleSection,
    /// The Julia emitter slice does not yet support this FIR node.
    UnsupportedNode,
}

impl CodegenErrorCode {
    /// Returns the stable machine-readable error code string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-JULIA-0001",
            Self::InvalidModuleSection => "FRS-CGEN-JULIA-0002",
            Self::UnsupportedNode => "FRS-CGEN-JULIA-0003",
        }
    }
}

/// Typed backend error returned by the Julia emitter.
///
/// Codegen errors are intentionally lightweight and stable: they carry one
/// machine-readable [`CodegenErrorCode`] plus a human message that may include
/// the offending FIR node id. This mirrors the C/C++ backend error surface
/// without forcing callers to depend on private emitter details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    code: CodegenErrorCode,
    message: String,
}

impl CodegenError {
    /// Creates a typed Julia backend code generation error.
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

/// Decoded `FirMatch::Module` header used by the Julia emitter.
///
/// This is not a second IR. It is just a borrowed/id-based view that prevents
/// each emission phase from re-matching the module root. All ids still point
/// back into the original [`FirStore`] and are decoded at the point where their
/// section is emitted.
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

/// One scalar state initializer extracted from `dsp_struct` or `globals`.
///
/// Julia emits typed fields first, then initializes them in the inner struct
/// constructor and replays explicit reset values in
/// `instanceResetUserInterface!` when the FIR module does not provide a
/// canonical reset body. Keeping this view separate preserves FIR declaration
/// order across both places.
#[derive(Debug, Clone)]
struct StructInit {
    name: String,
    typ: FirType,
    init: FirId,
}

/// One mutable state table initializer extracted from `dsp_struct` or `globals`.
///
/// State tables are represented as `MVector` fields so generated compute code
/// can update elements in place while retaining the compact StaticArrays shape
/// used by the C++ Julia backend family.
#[derive(Clone, Debug)]
struct TableInit {
    name: String,
    elem_type: FirType,
    values: Vec<FirId>,
}

/// Context-dependent statement emission mode.
///
/// The same FIR UI/metadata statements can appear in different canonical API
/// callbacks. `AddMetaDeclare` therefore needs to know whether it is being
/// emitted for `metadata!` (`m`) or `buildUserInterface!` (`ui_interface`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitMode {
    Default,
    Metadata,
    Ui,
}

/// Body-bearing FIR function declaration normalized for textual emission.
///
/// Prototype-only FIR declarations are intentionally filtered out when this view
/// is collected. Public lifecycle fallbacks should not be displaced by a
/// declaration that has no body.
#[derive(Debug, Clone)]
struct DeclareFunView {
    name: String,
    typ: FirType,
    named_args: Vec<NamedType>,
    body: Option<FirId>,
}

#[must_use]
/// Returns the stable backend identifier.
pub fn backend_id() -> &'static str {
    BACKEND_NAME
}

/// Generates Julia code from a FIR module root.
///
/// The emitter expects a module produced by the transform fast lane. It emits a
/// single Julia source unit containing:
/// - helper aliases (`REAL`, `pow`, `rint`, `remainder`),
/// - static tables,
/// - a mutable DSP state struct,
/// - Faust lifecycle callbacks,
/// - helper functions and `compute!`.
///
/// The generated unit is structurally aligned with `faust -lang julia` but is
/// not intended to be byte-for-byte identical. See
/// `porting/julia-backend-plan-2026-05-13-en.md` for the staged parity plan.
///
/// # Errors
/// Returns [`CodegenError`] if the root is not a FIR module or if the module
/// contains unsupported FIR nodes for the current Julia emitter slice.
pub fn generate_julia_module(
    store: &FirStore,
    module: FirId,
    options: &JuliaOptions,
) -> Result<String, CodegenError> {
    let module = decode_module(store, module)?;
    let class_name = options
        .class_name
        .as_deref()
        .unwrap_or(module.name.as_str())
        .to_owned();
    let functions = collect_module_functions(store, module.functions)?;
    let struct_inits = collect_struct_initializers(store, module.dsp_struct, module.globals)?;
    let table_inits = collect_table_initializers(store, module.dsp_struct, module.globals)?;

    let mut out = String::new();
    emit_julia_header(&mut out, options.real_type);
    emit_static_tables(store, &mut out, module.static_decls)?;
    emit_struct_definition(
        store,
        &mut out,
        &class_name,
        module.dsp_struct,
        module.globals,
    )?;
    emit_julia_api(
        store,
        &mut out,
        JuliaApiEmitInput {
            class_name: &class_name,
            num_inputs: module.num_inputs,
            num_outputs: module.num_outputs,
            declared_functions: &functions,
            struct_inits: &struct_inits,
            table_inits: &table_inits,
        },
    )?;
    Ok(out)
}

/// Emits the Julia prologue shared by every generated source unit.
///
/// The helper aliases are deliberately close to the C++ Faust Julia backend
/// output so downstream structural tests and user expectations see the same
/// runtime vocabulary.
fn emit_julia_header(out: &mut String, real_type: JuliaRealType) {
    let _ = writeln!(out, "#=");
    let _ = writeln!(out, "Code generated with faust-rs");
    let _ = writeln!(out, "Compilation options: -lang julia");
    let _ = writeln!(out, "=#");
    let _ = writeln!(out);
    let _ = writeln!(out, "using StaticArrays");
    let _ = writeln!(out);
    let _ = writeln!(out, "const REAL = {}", real_type.julia_name());
    let _ = writeln!(out, "pow(x, y) = x ^ y");
    let _ = writeln!(out, "rint(x) = round(x, Base.Rounding.RoundNearest)");
    let _ = writeln!(out, "fmod(x, y) = rem(x, y)");
    let _ = writeln!(out, "atan2(y, x) = atan(y, x)");
    let _ = writeln!(
        out,
        "faust_wrap_int32(x) = reinterpret(Int32, UInt32(mod(trunc(Int64, x), 0x100000000)))"
    );
    let _ = writeln!(
        out,
        "remainder(x, y) = rem(x, y, Base.Rounding.RoundNearest)"
    );
    let _ = writeln!(out);
}

/// Emits the mutable DSP state container and its inner constructor.
///
/// FIR state is split between `dsp_struct` and `globals`; both sections become
/// fields on the Julia struct because Julia has no direct equivalent of C++
/// class statics in this generated single-unit shape. If the FIR module omits
/// `fSampleRate`, the backend adds it to preserve the canonical Faust lifecycle
/// API.
fn emit_struct_definition(
    store: &FirStore,
    out: &mut String,
    class_name: &str,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<(), CodegenError> {
    let has_sample_rate_field = block_declares_var(store, dsp_struct, "fSampleRate")
        || block_declares_var(store, globals, "fSampleRate");

    let _ = writeln!(out, "mutable struct {class_name}{{T}} <: dsp");
    emit_struct_fields(store, out, dsp_struct)?;
    emit_struct_fields(store, out, globals)?;
    if !has_sample_rate_field {
        let _ = writeln!(out, "\tfSampleRate::Int32");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "\tfunction {class_name}{{T}}() where {{T}}");
    let _ = writeln!(out, "\t\tdsp = new{{T}}()");
    emit_struct_default_initializers(store, out, dsp_struct)?;
    emit_struct_default_initializers(store, out, globals)?;
    if !has_sample_rate_field {
        let _ = writeln!(out, "\t\tdsp.fSampleRate = Int32(0)");
    }
    let _ = writeln!(out, "\t\treturn dsp");
    let _ = writeln!(out, "\tend");
    let _ = writeln!(out, "end");
    let _ = writeln!(out);
    Ok(())
}

/// Emits typed Julia fields for one FIR declaration block.
///
/// Scalar variables become `name::Type`; mutable tables become
/// `name::MVector{N, Type}`. Other FIR declaration forms are ignored here
/// because they belong to function/static sections, not DSP state storage.
fn emit_struct_fields(
    store: &FirStore,
    out: &mut String,
    block_id: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block_id) else {
        return Err(invalid_section("struct section", block_id, store));
    };
    for item in items {
        match match_fir(store, item) {
            FirMatch::DeclareVar { name, typ, .. } => {
                let _ = writeln!(out, "\t{name}::{}", emit_type(&typ));
            }
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => {
                let _ = writeln!(
                    out,
                    "\t{name}::MVector{{{}, {}}}",
                    values.len(),
                    emit_type(&elem_type)
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// Emits constructor-side default assignments for DSP fields.
///
/// Julia inner constructors allocate with `new{T}()` and must assign every field
/// before returning. Explicit FIR initializers are honored; otherwise a typed
/// neutral value is synthesized.
fn emit_struct_default_initializers(
    store: &FirStore,
    out: &mut String,
    block_id: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block_id) else {
        return Err(invalid_section("struct section", block_id, store));
    };
    for item in items {
        match match_fir(store, item) {
            FirMatch::DeclareVar {
                name, typ, init, ..
            } => {
                let init = if let Some(init) = init {
                    emit_value(store, init)?
                } else {
                    zero_value(&typ)
                };
                let _ = writeln!(out, "\t\tdsp.{name} = {}", emit_cast(&typ, &init));
            }
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => {
                if values.is_empty() {
                    let _ = writeln!(
                        out,
                        "\t\tdsp.{name} = MVector{{0, {}}}()",
                        emit_type(&elem_type)
                    );
                } else {
                    let mut rendered = Vec::with_capacity(values.len());
                    for value in values {
                        let value = emit_value(store, value)?;
                        rendered.push(emit_cast(&elem_type, &value));
                    }
                    let _ = writeln!(
                        out,
                        "\t\tdsp.{name} = MVector{{{}, {}}}({})",
                        rendered.len(),
                        emit_type(&elem_type),
                        rendered.join(", ")
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Aggregated inputs needed to synthesize the public Julia DSP API.
///
/// `generate_julia_module` collects these views once, then passes borrowed
/// slices into the API emitter. This keeps the rendering phase deterministic:
/// canonical FIR functions, scalar resets, and table resets are all replayed in
/// the order discovered from the FIR module rather than re-scanning sections in
/// several independent helpers.
struct JuliaApiEmitInput<'a> {
    /// Effective Julia DSP struct name.
    class_name: &'a str,
    /// Propagated input arity reported by the FIR module.
    num_inputs: usize,
    /// Propagated output arity reported by the FIR module.
    num_outputs: usize,
    /// Body-bearing function declarations collected from the module.
    declared_functions: &'a [DeclareFunView],
    /// Scalar state initializers for synthesized reset paths.
    struct_inits: &'a [StructInit],
    /// Mutable state table initializers for synthesized reset paths.
    table_inits: &'a [TableInit],
}

/// Emits the public Faust-style Julia API around the lowered FIR bodies.
///
/// Canonical body-bearing FIR functions are replayed when present. Missing
/// lifecycle callbacks are synthesized using the same ordering as C++ Faust:
/// `instanceConstants!`, `instanceResetUserInterface!`, then `instanceClear!`
/// inside `instanceInit!`.
///
/// This function is intentionally the only place that knows the Faust public
/// method names and their Julia signatures. Lower-level statement/value helpers
/// only emit generic FIR syntax and do not make lifecycle policy decisions.
fn emit_julia_api(
    store: &FirStore,
    out: &mut String,
    spec: JuliaApiEmitInput<'_>,
) -> Result<(), CodegenError> {
    let JuliaApiEmitInput {
        class_name,
        num_inputs,
        num_outputs,
        declared_functions,
        struct_inits,
        table_inits,
    } = spec;

    emit_metadata(store, out, class_name, declared_functions)?;

    let _ = writeln!(
        out,
        "getSampleRate(dsp::{class_name}{{T}}) where {{T}} = dsp.fSampleRate"
    );
    let _ = writeln!(
        out,
        "getNumInputs(dsp::{class_name}{{T}}) where {{T}} = Int32({num_inputs})"
    );
    let _ = writeln!(
        out,
        "getNumOutputs(dsp::{class_name}{{T}}) where {{T}} = Int32({num_outputs})"
    );
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "function classInit!(dsp::{class_name}{{T}}, sample_rate::Int32) where {{T}}"
    );
    let _ = writeln!(out, "\tnothing");
    let _ = writeln!(out, "end");
    let _ = writeln!(out);

    emit_lifecycle_or_fallback(
        store,
        out,
        class_name,
        declared_functions,
        "instanceConstants",
        "function instanceConstants!(dsp::{class_name}{T}, sample_rate::Int32) where {T}",
        |out| {
            let _ = writeln!(out, "\tdsp.fSampleRate = sample_rate");
            Ok(())
        },
    )?;

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceResetUserInterface")
    {
        emit_named_fun(store, out, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "function instanceResetUserInterface!(dsp::{class_name}{{T}}) where {{T}}"
        );
        if struct_inits.is_empty() && table_inits.is_empty() {
            let _ = writeln!(out, "\tnothing");
        } else {
            for init in struct_inits {
                let value = emit_value(store, init.init)?;
                let _ = writeln!(
                    out,
                    "\tdsp.{} = {}",
                    init.name,
                    emit_cast(&init.typ, &value)
                );
            }
            for init in table_inits {
                for (index, value_id) in init.values.iter().copied().enumerate() {
                    let value = emit_value(store, value_id)?;
                    let _ = writeln!(
                        out,
                        "\tdsp.{}[{}] = {}",
                        init.name,
                        index + 1,
                        emit_cast(&init.elem_type, &value)
                    );
                }
            }
        }
        let _ = writeln!(out, "end");
        let _ = writeln!(out);
    }

    emit_lifecycle_or_fallback(
        store,
        out,
        class_name,
        declared_functions,
        "instanceClear",
        "function instanceClear!(dsp::{class_name}{T}) where {T}",
        |out| {
            let _ = writeln!(out, "\tnothing");
            Ok(())
        },
    )?;

    let _ = writeln!(
        out,
        "function instanceInit!(dsp::{class_name}{{T}}, sample_rate::Int32) where {{T}}"
    );
    let _ = writeln!(out, "\tinstanceConstants!(dsp, sample_rate)");
    let _ = writeln!(out, "\tinstanceResetUserInterface!(dsp)");
    let _ = writeln!(out, "\tinstanceClear!(dsp)");
    let _ = writeln!(out, "end");
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "function init!(dsp::{class_name}{{T}}, sample_rate::Int32) where {{T}}"
    );
    let _ = writeln!(out, "\tclassInit!(dsp, sample_rate)");
    let _ = writeln!(out, "\tinstanceInit!(dsp, sample_rate)");
    let _ = writeln!(out, "end");
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "getJSON(dsp::{class_name}{{T}}) where {{T}} = \"{{}}\""
    );
    let _ = writeln!(out);

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "buildUserInterface")
    {
        emit_named_fun(store, out, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "function buildUserInterface!(dsp::{class_name}{{T}}, ui_interface::UI) where {{T}}"
        );
        let _ = writeln!(out, "\tnothing");
        let _ = writeln!(out, "end");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions.iter().find(|f| f.name == "compute") {
        emit_named_fun(store, out, class_name, f)?;
    } else {
        let _ = writeln!(
            out,
            "function compute!(dsp::{class_name}{{T}}, count::Int32, inputs::AbstractMatrix{{FAUSTFLOAT}}, outputs::AbstractMatrix{{FAUSTFLOAT}}) where {{T}}"
        );
        let _ = writeln!(out, "\tnothing");
        let _ = writeln!(out, "end");
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
        emit_helper_function(store, out, f)?;
    }

    Ok(())
}

/// Emits one lifecycle function from FIR when present, otherwise a fallback.
///
/// This helper is used for callbacks whose absence has a simple default body.
/// More complex fallbacks, such as `instanceResetUserInterface!`, are emitted
/// directly because they replay collected scalar/table initializers.
///
/// The `signature` template uses `{class_name}` as a small local placeholder so
/// callers do not need to allocate each fallback signature before it is known
/// that the FIR function is absent.
fn emit_lifecycle_or_fallback<F>(
    store: &FirStore,
    out: &mut String,
    class_name: &str,
    functions: &[DeclareFunView],
    name: &str,
    signature: &str,
    fallback: F,
) -> Result<(), CodegenError>
where
    F: FnOnce(&mut String) -> Result<(), CodegenError>,
{
    if let Some(f) = functions.iter().find(|f| f.name == name) {
        emit_named_fun(store, out, class_name, f)
    } else {
        let signature = signature.replace("{class_name}", class_name);
        let _ = writeln!(out, "{signature}");
        fallback(out)?;
        let _ = writeln!(out, "end");
        let _ = writeln!(out);
        Ok(())
    }
}

/// Emits `metadata!` or a small default metadata callback.
///
/// A FIR `metadata` function is preferred because it preserves parser/evaluator
/// metadata order. The fallback exists so even minimal modules expose the
/// expected Julia API surface.
///
/// The fallback does not attempt to synthesize full JSON or UI metadata; strict
/// JSON generation remains owned by the JSON backend path.
fn emit_metadata(
    store: &FirStore,
    out: &mut String,
    class_name: &str,
    functions: &[DeclareFunView],
) -> Result<(), CodegenError> {
    if let Some(f) = functions.iter().find(|f| f.name == "metadata") {
        emit_named_fun(store, out, class_name, f)
    } else {
        let _ = writeln!(
            out,
            "function metadata!(dsp::{class_name}{{T}}, m::FMeta) where {{T}}"
        );
        let _ = writeln!(
            out,
            "\tdeclare!(m, \"faust-rs\", \"module-first julia backend prototype\")"
        );
        let _ = writeln!(out, "end");
        let _ = writeln!(out);
        Ok(())
    }
}

/// Emits one canonical DSP API function from its FIR body.
///
/// The canonical Faust function signatures are validated before textual
/// emission. This catches corrupted or non-canonical module shapes at the
/// backend boundary rather than producing Julia with a mismatched lifecycle
/// method.
///
/// `instanceConstants!` has one extra policy hook: if the FIR body does not
/// store `fSampleRate`, the emitter writes `dsp.fSampleRate = sample_rate`
/// before replaying the body. This mirrors the C/C++ lifecycle invariant used by
/// the other module-first backends.
fn emit_named_fun(
    store: &FirStore,
    out: &mut String,
    class_name: &str,
    decl: &DeclareFunView,
) -> Result<(), CodegenError> {
    faust_api::validate_canonical_dsp_api_signature(&decl.name, &decl.typ, &decl.named_args)
        .map_err(|msg| CodegenError::new(CodegenErrorCode::InvalidModuleSection, msg))?;
    let signature = match decl.name.as_str() {
        "metadata" => format!("function metadata!(dsp::{class_name}{{T}}, m::FMeta) where {{T}}"),
        "instanceConstants" => {
            format!(
                "function instanceConstants!(dsp::{class_name}{{T}}, sample_rate::Int32) where {{T}}"
            )
        }
        "instanceResetUserInterface" => {
            format!("function instanceResetUserInterface!(dsp::{class_name}{{T}}) where {{T}}")
        }
        "instanceClear" => format!("function instanceClear!(dsp::{class_name}{{T}}) where {{T}}"),
        "buildUserInterface" => {
            format!(
                "function buildUserInterface!(dsp::{class_name}{{T}}, ui_interface::UI) where {{T}}"
            )
        }
        "compute" => format!(
            "function compute!(dsp::{class_name}{{T}}, count::Int32, inputs::AbstractMatrix{{FAUSTFLOAT}}, outputs::AbstractMatrix{{FAUSTFLOAT}}) where {{T}}"
        ),
        _ => format!(
            "function {}!(dsp::{class_name}{{T}}) where {{T}}",
            decl.name
        ),
    };
    let body = decl
        .body
        .expect("emit_named_fun called with prototype-only DeclareFunView");
    let _ = writeln!(out, "{signature}");
    if decl.name == "instanceConstants" && !block_stores_var(store, body, "fSampleRate") {
        let _ = writeln!(out, "\tdsp.fSampleRate = sample_rate");
    }
    let mut mode = match decl.name.as_str() {
        "metadata" => EmitMode::Metadata,
        "buildUserInterface" => EmitMode::Ui,
        _ => EmitMode::Default,
    };
    emit_block_with_mode(store, out, body, 1, &mut mode)?;
    let _ = writeln!(out, "end");
    let _ = writeln!(out);
    Ok(())
}

/// Emits a non-canonical helper function declared in the FIR functions block.
///
/// Helper functions keep their FIR argument names/types when available. They are
/// emitted after the public API, matching the current C/C++ fast-lane structure
/// closely enough for structural parity tests.
///
/// Helpers are not given the `!` suffix unless the FIR function name already
/// includes it. The suffix policy is reserved for canonical DSP methods where
/// mutating `dsp`/UI state is part of the public Faust-style Julia surface.
fn emit_helper_function(
    store: &FirStore,
    out: &mut String,
    decl: &DeclareFunView,
) -> Result<(), CodegenError> {
    let body = decl
        .body
        .expect("emit_helper_function called with prototype-only DeclareFunView");
    let params = match &decl.typ {
        FirType::Fun { args, .. } => args
            .iter()
            .enumerate()
            .map(|(index, arg_type)| {
                let name = decl
                    .named_args
                    .get(index)
                    .map_or_else(|| format!("arg{index}"), |named| named.name.clone());
                format!("{name}::{}", emit_type(arg_type))
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    let _ = writeln!(out, "function {}({params})", decl.name);
    let mut mode = EmitMode::Default;
    emit_block_with_mode(store, out, body, 1, &mut mode)?;
    let _ = writeln!(out, "end");
    let _ = writeln!(out);
    Ok(())
}

/// Emits every statement in a FIR block under the active API emission mode.
///
/// `mode` is threaded through nested control-flow blocks so UI/metadata
/// statements keep the correct receiver even when they are nested under `if` or
/// loop constructs.
///
/// The function rejects non-block ids with a backend diagnostic rather than
/// treating them as single statements. FIR lowering is expected to wrap function
/// bodies and branch bodies in explicit `Block` nodes.
fn emit_block_with_mode(
    store: &FirStore,
    out: &mut String,
    block: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return Err(unsupported_node("expected block", block, store));
    };
    for stmt in items {
        emit_stmt(store, out, stmt, indent, mode)?;
    }
    Ok(())
}

/// Emits one FIR statement as Julia source.
///
/// Control flow is rendered with native Julia `if`, `while`, and `for` syntax.
/// General `ForLoop` nodes use `while` because FIR expresses C-style
/// init/end/step triples, including reverse loops, more directly than Julia
/// ranges. `SimpleForLoop` uses Julia ranges and preserves the zero-based loop
/// variable used by the shared FIR contract.
///
/// UI statements are rendered as Julia mutating callbacks (`openVerticalBox!`,
/// `addHorizontalSlider!`, etc.). The generated callback names match the
/// expected Julia runtime vocabulary; this module does not define those runtime
/// functions itself.
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    stmt: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Result<(), CodegenError> {
    let tab = "\t".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::DeclareVar {
            name, typ, init, ..
        } => {
            let init = if let Some(init) = init {
                emit_value(store, init)?
            } else {
                zero_value(&typ)
            };
            let _ = writeln!(out, "{tab}{name} = {}", emit_cast(&typ, &init));
            Ok(())
        }
        FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } => {
            let mut rendered = Vec::with_capacity(values.len());
            for value in values {
                let value = emit_value(store, value)?;
                rendered.push(emit_cast(&elem_type, &value));
            }
            let _ = writeln!(out, "{tab}{name} = [{}]", rendered.join(", "));
            Ok(())
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            let value = emit_value(store, value)?;
            let target = emit_var_ref(&name, access);
            let _ = writeln!(out, "{tab}{target} = {value}");
            Ok(())
        }
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => {
            let index = emit_index_expr(store, index)?;
            let value = emit_value(store, value)?;
            let target = emit_var_ref(&name, access);
            let _ = writeln!(out, "{tab}{target}[{index}] = {value}");
            Ok(())
        }
        FirMatch::Drop(value) => {
            if is_obviously_side_effect_free_value(store, value) {
                return Ok(());
            }
            let value = emit_value(store, value)?;
            let _ = writeln!(out, "{tab}_ = {value}");
            Ok(())
        }
        FirMatch::Return(value) => {
            if let Some(value) = value {
                let value = emit_value(store, value)?;
                let _ = writeln!(out, "{tab}return {value}");
            } else {
                let _ = writeln!(out, "{tab}return");
            }
            Ok(())
        }
        FirMatch::Block(_) => emit_block_with_mode(store, out, stmt, indent, mode),
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let cond = emit_value(store, cond)?;
            let _ = writeln!(out, "{tab}if {cond}");
            emit_block_with_mode(store, out, then_block, indent + 1, mode)?;
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{tab}else");
                emit_block_with_mode(store, out, else_block, indent + 1, mode)?;
            }
            let _ = writeln!(out, "{tab}end");
            Ok(())
        }
        FirMatch::Control { cond, stmt } => {
            let cond = emit_value(store, cond)?;
            let _ = writeln!(out, "{tab}if {cond}");
            emit_stmt(store, out, stmt, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}end");
            Ok(())
        }
        FirMatch::Switch {
            cond,
            ref cases,
            default,
        } => {
            let cond = emit_value(store, cond)?;
            for (i, (value, block)) in cases.iter().enumerate() {
                if i == 0 {
                    let _ = writeln!(out, "{tab}if {cond} == {value}");
                } else {
                    let _ = writeln!(out, "{tab}elseif {cond} == {value}");
                }
                emit_block_with_mode(store, out, *block, indent + 1, mode)?;
            }
            if let Some(default) = default {
                let _ = writeln!(out, "{tab}else");
                emit_block_with_mode(store, out, default, indent + 1, mode)?;
            }
            let _ = writeln!(out, "{tab}end");
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
                    emit_value(store, v)?
                } else {
                    emit_value(store, init)?
                };
            let end = emit_value(store, end)?;
            let step = emit_value(store, step)?;
            let cmp = if is_reverse { ">" } else { "<" };
            let _ = writeln!(out, "{tab}{var} = {init_val}");
            let _ = writeln!(out, "{tab}while {var} {cmp} {end}");
            emit_block_with_mode(store, out, body, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}\t{var} = {var} + {step}");
            let _ = writeln!(out, "{tab}end");
            Ok(())
        }
        FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } => {
            let upper = emit_value(store, upper)?;
            if is_reverse {
                let _ = writeln!(out, "{tab}for {var} in (({upper}) - 1):-1:0");
            } else {
                let _ = writeln!(out, "{tab}for {var} in 0:(({upper}) - 1)");
            }
            emit_block_with_mode(store, out, body, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}end");
            Ok(())
        }
        FirMatch::WhileLoop { cond, body } => {
            let cond = emit_value(store, cond)?;
            let _ = writeln!(out, "{tab}while {cond}");
            emit_block_with_mode(store, out, body, indent + 1, mode)?;
            let _ = writeln!(out, "{tab}end");
            Ok(())
        }
        FirMatch::OpenBox { typ, label } => {
            let api = match typ {
                fir::UiBoxType::Vertical => "openVerticalBox!",
                fir::UiBoxType::Horizontal => "openHorizontalBox!",
                fir::UiBoxType::Tab => "openTabBox!",
            };
            let _ = writeln!(
                out,
                "{tab}{api}(ui_interface, {})",
                julia_string_literal(&label)
            );
            Ok(())
        }
        FirMatch::CloseBox => {
            let _ = writeln!(out, "{tab}closeBox!(ui_interface)");
            Ok(())
        }
        FirMatch::AddButton { typ, label, var } => {
            let api = match typ {
                fir::ButtonType::Button => "addButton!",
                fir::ButtonType::Checkbox => "addCheckButton!",
            };
            let _ = writeln!(
                out,
                "{tab}{api}(ui_interface, {}, :{var})",
                julia_string_literal(&label)
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
                fir::SliderType::Horizontal => "addHorizontalSlider!",
                fir::SliderType::Vertical => "addVerticalSlider!",
                fir::SliderType::NumEntry => "addNumEntry!",
            };
            let _ = writeln!(
                out,
                "{tab}{api}(ui_interface, {}, :{var}, FAUSTFLOAT({}), FAUSTFLOAT({}), FAUSTFLOAT({}), FAUSTFLOAT({}))",
                julia_string_literal(&label),
                trim_float(init),
                trim_float(lo),
                trim_float(hi),
                trim_float(step),
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
                fir::BargraphType::Horizontal => "addHorizontalBargraph!",
                fir::BargraphType::Vertical => "addVerticalBargraph!",
            };
            let _ = writeln!(
                out,
                "{tab}{api}(ui_interface, {}, :{var}, FAUSTFLOAT({}), FAUSTFLOAT({}))",
                julia_string_literal(&label),
                trim_float(lo),
                trim_float(hi)
            );
            Ok(())
        }
        FirMatch::AddMetaDeclare { var, key, value } => {
            match mode {
                EmitMode::Ui => {
                    let zone = if var == "0" {
                        ":dummy".to_owned()
                    } else {
                        format!(":{var}")
                    };
                    let _ = writeln!(
                        out,
                        "{tab}declare!(ui_interface, {zone}, {}, {})",
                        julia_string_literal(&key),
                        julia_string_literal(&value)
                    );
                }
                EmitMode::Default | EmitMode::Metadata => {
                    let _ = writeln!(
                        out,
                        "{tab}declare!(m, {}, {})",
                        julia_string_literal(&key),
                        julia_string_literal(&value)
                    );
                }
            }
            Ok(())
        }
        FirMatch::AddSoundfile { label, url, var } => {
            let _ = writeln!(
                out,
                "{tab}addSoundfile!(ui_interface, {}, {}, :{var})",
                julia_string_literal(&label),
                julia_string_literal(&url)
            );
            Ok(())
        }
        FirMatch::NullStatement => {
            let _ = writeln!(out, "{tab}nothing");
            Ok(())
        }
        FirMatch::Label(label) => {
            let _ = writeln!(out, "{tab}# {label}");
            Ok(())
        }
        _ => Err(unsupported_node("statement", stmt, store)),
    }
}

/// Emits one FIR value expression as a Julia expression.
///
/// `LoadTable` is the main Julia-specific boundary:
/// - normal table indices are converted with [`emit_index_expr`],
/// - `inputs`/`outputs` function arguments become `@view matrix[:, channel]`
///   slices so local channel aliases remain mutable arrays in `compute!`.
///
/// FIR math calls are already normalized to backend-agnostic names. This helper
/// only applies Julia spelling fixes for common C/C++ names such as `std::fmin`,
/// `std::fmax`, and `fabs`.
fn emit_value(store: &FirStore, value: FirId) -> Result<String, CodegenError> {
    match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(value.to_string()),
        FirMatch::Int64 { value, .. } => Ok(value.to_string()),
        FirMatch::Float32 { value, .. } => Ok(format!("REAL({})", trim_float(f64::from(value)))),
        FirMatch::Float64 { value, .. } => Ok(trim_float(value)),
        FirMatch::Bool { value, .. } => Ok(if value { "true" } else { "false" }.to_owned()),
        FirMatch::LoadVar { name, access, .. } | FirMatch::LoadVarAddress { name, access, .. } => {
            Ok(emit_var_ref(&name, access))
        }
        FirMatch::LoadTable {
            name,
            access,
            index,
            ..
        } => {
            if matches!(access, AccessType::FunArgs)
                && matches!(name.as_str(), "inputs" | "outputs")
            {
                let channel = emit_index_expr(store, index)?;
                return Ok(format!("(@view {name}[:, {channel}])"));
            }
            let index = emit_index_expr(store, index)?;
            Ok(format!("{}[{index}]", emit_var_ref(&name, access)))
        }
        FirMatch::TeeVar {
            name,
            access,
            value,
            ..
        } => {
            let value = emit_value(store, value)?;
            Ok(format!("({} = {value})", emit_var_ref(&name, access)))
        }
        FirMatch::BinOp { op, lhs, rhs, .. } => {
            let lhs = emit_value(store, lhs)?;
            let rhs = emit_value(store, rhs)?;
            Ok(emit_binop_expr(op, &lhs, &rhs))
        }
        FirMatch::Neg { value, .. } => {
            let value = emit_value(store, value)?;
            Ok(format!("(-{value})"))
        }
        FirMatch::Cast { typ, value } | FirMatch::Bitcast { typ, value } => {
            let value = emit_value(store, value)?;
            Ok(emit_cast(&typ, &value))
        }
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            let cond = emit_value(store, cond)?;
            let then_value = emit_value(store, then_value)?;
            let else_value = emit_value(store, else_value)?;
            Ok(format!("ifelse(({cond}) != 0, {then_value}, {else_value})"))
        }
        FirMatch::FunCall { name, args, .. } => {
            let mut rendered = Vec::with_capacity(args.len());
            for arg in args {
                rendered.push(emit_value(store, arg)?);
            }
            let jl_name = match name.as_str() {
                "min_i" | "fmin" | "std::fmin" => "min",
                "max_i" | "fmax" | "std::fmax" => "max",
                "std::fabs" | "fabs" => "abs",
                _ => name.strip_prefix("std::").unwrap_or(name.as_str()),
            };
            Ok(format!("{jl_name}({})", rendered.join(", ")))
        }
        FirMatch::NullValue { .. } => Ok("nothing".to_owned()),
        FirMatch::LoadSoundfileLength { var, part } => {
            let part = emit_index_expr(store, part)?;
            Ok(format!("dsp.{var}.fLength[{part}]"))
        }
        FirMatch::LoadSoundfileRate { var, part } => {
            let part = emit_index_expr(store, part)?;
            Ok(format!("dsp.{var}.fSR[{part}]"))
        }
        FirMatch::LoadSoundfileBuffer {
            var,
            chan,
            part,
            idx,
            ..
        } => {
            let chan = emit_index_expr(store, chan)?;
            let part = emit_index_expr(store, part)?;
            let idx = emit_index_expr(store, idx)?;
            Ok(format!(
                "dsp.{var}.fBuffers[{chan}][dsp.{var}.fOffset[{part}] + {idx}]"
            ))
        }
        _ => Err(unsupported_node("value", value, store)),
    }
}

/// Emits a FIR table index adjusted for Julia one-based indexing.
///
/// The FIR value itself remains zero-based. Only the final indexing expression
/// receives `+ 1`, keeping arithmetic and loop bounds shared with other
/// backends.
///
/// Because this helper wraps the rendered value in parentheses, callers can pass
/// arbitrary FIR index expressions without depending on precedence details.
fn emit_index_expr(store: &FirStore, value: FirId) -> Result<String, CodegenError> {
    let value = emit_value(store, value)?;
    Ok(format!("({value}) + 1"))
}

/// Emits a complete Julia binary-operation expression.
///
/// Most FIR operations map to infix Julia operators. Logical right shift and
/// XOR need special spellings to preserve C-style integer semantics.
///
/// Comparisons are left as Julia booleans. Numeric contexts that need an integer
/// selector are handled by the FIR shape that consumes them, for example
/// [`emit_value`] rendering `Select2` with `ifelse((cond) != 0, ...)`.
fn emit_binop_expr(op: FirBinOp, lhs: &str, rhs: &str) -> String {
    match op {
        FirBinOp::LRsh => format!("Int32(UInt32({lhs}) >> ({rhs}))"),
        FirBinOp::Xor => format!("xor({lhs}, {rhs})"),
        _ => format!("({lhs} {} {rhs})", emit_binop(op)),
    }
}

/// Returns the Julia infix token for simple FIR binary operators.
///
/// Operations with non-token rendering are handled by [`emit_binop_expr`].
///
/// `FirBinOp::Xor` is still listed here for completeness, but callers should
/// use [`emit_binop_expr`] so XOR is rendered as `xor(lhs, rhs)`.
fn emit_binop(op: FirBinOp) -> &'static str {
    match op {
        FirBinOp::Add => "+",
        FirBinOp::Sub => "-",
        FirBinOp::Mul => "*",
        FirBinOp::Div => "/",
        FirBinOp::Rem => "%",
        FirBinOp::And => "&",
        FirBinOp::Or => "|",
        FirBinOp::Xor => "xor",
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

/// Renders a variable reference according to FIR storage class.
///
/// Struct state is addressed through `dsp.name`; stack, loop, global, static,
/// and function-argument values keep their local textual name in this backend
/// slice.
///
/// This mirrors the current module-first FIR convention where global/static
/// declarations are emitted as textual names in the generated unit, while
/// `AccessType::Struct` is the only storage class requiring a DSP receiver.
fn emit_var_ref(name: &str, access: AccessType) -> String {
    match access {
        AccessType::Struct => format!("dsp.{name}"),
        _ => name.to_owned(),
    }
}

/// Emits a Julia constructor-style cast for a FIR type.
///
/// `Ptr` is intentionally not rendered as a scalar cast. In this backend, FIR
/// pointer values are used for Julia references/views such as `@view inputs[:,1]`;
/// wrapping such a value in `FAUSTFLOAT(...)` would break channel aliases in
/// `compute!`.
fn emit_cast(typ: &FirType, value: &str) -> String {
    match typ {
        // FIR pointer values lower to Julia references/views. Casting them to
        // the pointee scalar type would incorrectly turn `@view inputs[:, n]`
        // into a scalar conversion instead of preserving the array view.
        FirType::Ptr(_) => value.to_owned(),
        FirType::Void => value.to_owned(),
        FirType::Int32 => format!("faust_wrap_int32({value})"),
        FirType::Int64 => format!("trunc(Int64, {value})"),
        FirType::Float32 | FirType::Float64 => format!("T({value})"),
        _ => format!("{}({value})", emit_type(typ)),
    }
}

/// Emits a Julia cast outside the `mydsp{T}` method scope.
///
/// Top-level static declarations cannot refer to `T`, so real-valued static
/// table constants keep concrete Julia constructors.
fn emit_top_level_cast(typ: &FirType, value: &str) -> String {
    match typ {
        FirType::Float32 => format!("Float32({value})"),
        FirType::Float64 => format!("Float64({value})"),
        _ => emit_cast(typ, value),
    }
}

/// Maps FIR types to Julia type spellings.
///
/// The mapping is a backend representation choice, not a type inference pass:
/// all numeric promotion has already happened before FIR reaches codegen.
/// Pointers collapse to their inner type because Julia values are references by
/// convention in the generated API surface.
///
/// `Quad` and `FixedPoint` currently lower to `Float64` because the first Julia
/// backend slice has no dedicated Julia runtime aliases for these extended
/// Faust scalar families.
fn emit_type(typ: &FirType) -> String {
    match typ {
        FirType::Int32 => "Int32".to_owned(),
        FirType::Int64 => "Int64".to_owned(),
        FirType::Float32 => "Float32".to_owned(),
        FirType::Float64 => "Float64".to_owned(),
        FirType::FaustFloat => "FAUSTFLOAT".to_owned(),
        FirType::Quad => "Float64".to_owned(),
        FirType::FixedPoint => "Float64".to_owned(),
        FirType::Bool => "Bool".to_owned(),
        FirType::Void => "Nothing".to_owned(),
        FirType::Obj => "Any".to_owned(),
        FirType::Sound => "Soundfile".to_owned(),
        FirType::UI => "UI".to_owned(),
        FirType::Meta => "FMeta".to_owned(),
        FirType::Ptr(inner) => emit_type(inner),
        FirType::Array(inner, size) => format!("MVector{{{size}, {}}}", emit_type(inner)),
        FirType::Vector(inner, lanes) => format!("SVector{{{lanes}, {}}}", emit_type(inner)),
        FirType::Struct(name, _fields) => name.clone(),
        FirType::Fun { ret, .. } => emit_type(ret),
    }
}

/// Returns a neutral constructor value used for uninitialized struct fields.
///
/// Numeric values return plain `0` and are then wrapped by [`emit_cast`] at the
/// assignment site. This avoids nested casts such as `Int32(Int32(0))`.
fn zero_value(typ: &FirType) -> String {
    match typ {
        FirType::Bool => "false".to_owned(),
        FirType::Void | FirType::Obj | FirType::Sound | FirType::UI | FirType::Meta => {
            "nothing".to_owned()
        }
        FirType::Array(inner, size) => {
            format!("MVector{{{size}, {}}}(undef)", emit_type(inner))
        }
        _ => "0".to_owned(),
    }
}

/// Emits immutable static FIR tables before the DSP struct.
///
/// Static tables become `@SVector` constants. Mutable state tables are handled
/// separately by [`emit_struct_fields`] and [`emit_struct_default_initializers`].
///
/// An absent or malformed static table section is treated as empty. The module
/// root validation already ensures the section ids are present; this helper is
/// permissive because older/minimal FIR fixtures may not populate static
/// declarations yet.
fn emit_static_tables(
    store: &FirStore,
    out: &mut String,
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
            let mut rendered = Vec::with_capacity(values.len());
            for value in values {
                let value = emit_value(store, value)?;
                rendered.push(emit_top_level_cast(&elem_type, &value));
            }
            let _ = writeln!(out, "const {name} = @SVector [{}]", rendered.join(", "));
        }
    }
    if !matches!(match_fir(store, block), FirMatch::Block(ref items) if items.is_empty()) {
        let _ = writeln!(out);
    }
    Ok(())
}

/// Collects explicit scalar initializers from DSP state sections.
///
/// These initializers are replayed in synthesized reset paths when the FIR
/// module does not provide its own canonical `instanceResetUserInterface` body.
///
/// Only `DeclareVar` entries with `init: Some(_)` are collected. Uninitialized
/// fields are still assigned in the Julia constructor through [`zero_value`],
/// but they do not need a reset replay entry.
fn collect_struct_initializers(
    store: &FirStore,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<Vec<StructInit>, CodegenError> {
    let mut out = Vec::new();
    for section in [dsp_struct, globals] {
        let FirMatch::Block(items) = match_fir(store, section) else {
            return Err(invalid_section("struct section", section, store));
        };
        for item in items {
            if let FirMatch::DeclareVar {
                name,
                typ,
                init: Some(init),
                ..
            } = match_fir(store, item)
            {
                out.push(StructInit { name, typ, init });
            }
        }
    }
    Ok(out)
}

/// Collects mutable table initializers from DSP state sections.
///
/// Each collected table keeps FIR value ids so the final emission path can reuse
/// the normal value renderer and type casts.
///
/// Tables from both `dsp_struct` and `globals` are treated as mutable DSP state
/// in the Julia backend. Static declarations are emitted separately by
/// [`emit_static_tables`].
fn collect_table_initializers(
    store: &FirStore,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<Vec<TableInit>, CodegenError> {
    let mut out = Vec::new();
    for section in [dsp_struct, globals] {
        let FirMatch::Block(items) = match_fir(store, section) else {
            return Err(invalid_section("struct section", section, store));
        };
        for item in items {
            if let FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } = match_fir(store, item)
            {
                out.push(TableInit {
                    name,
                    elem_type,
                    values,
                });
            }
        }
    }
    Ok(out)
}

/// Collects body-bearing function declarations from the module function block.
///
/// Prototype-only declarations are ignored because they are not executable
/// bodies and should not suppress lifecycle fallback generation.
///
/// The resulting vector preserves FIR declaration order. Public API methods are
/// selected by name later, and any remaining helper definitions are emitted in
/// this stable order.
fn collect_module_functions(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<DeclareFunView>, CodegenError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Err(invalid_section("functions section", functions, store));
    };
    let mut functions = Vec::new();
    for item in items {
        if let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body: Some(body),
            ..
        } = match_fir(store, item)
        {
            functions.push(DeclareFunView {
                name,
                typ,
                named_args: args,
                body: Some(body),
            });
        }
    }
    Ok(functions)
}

/// Decodes and validates the FIR module root expected by this backend.
///
/// Returning a [`ModuleView`] keeps the public entry point small and gives all
/// downstream helpers the exact ids for the seven module sections they need.
/// Non-module roots are rejected with `FRS-CGEN-JULIA-0001`.
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

/// Returns whether a FIR block declares a variable with `name`.
///
/// Used to decide whether the backend must synthesize `fSampleRate` in the Julia
/// struct. Non-block inputs return `false` because the caller will validate
/// section shape separately when emitting the fields.
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

/// Returns whether a FIR block contains a direct store to `name`.
///
/// Used by `instanceConstants!` emission to avoid writing `fSampleRate` twice
/// when FIR already has an explicit store in the canonical function body.
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

/// Builds a stable diagnostic for malformed module sections.
///
/// This is the Julia-specific companion of the C/C++ backend section checks.
/// The diagnostic includes both the logical section name and the concrete FIR
/// node id to make fixture/debug output actionable.
fn invalid_section(kind: &str, node: FirId, store: &FirStore) -> CodegenError {
    CodegenError::new(
        CodegenErrorCode::InvalidModuleSection,
        format!(
            "{kind} must be a FIR block, got {:?} at node {}",
            match_fir(store, node),
            node.as_u32()
        ),
    )
}

/// Builds a stable diagnostic for FIR nodes outside the current Julia slice.
///
/// Unsupported-node diagnostics are preferred over best-effort partial output:
/// the generated Julia source should either represent the complete FIR body or
/// fail before writing misleading code.
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

/// Formats a floating literal in Julia syntax.
///
/// Rust's `Debug` formatting emits the shortest round-trippable decimal for
/// finite `f64` values and preserves `.0` for integral-looking floats. That is
/// important for small constants such as Faust's noise LCG scale, where fixed
/// decimal truncation is enough to move impulse samples.
fn trim_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Inf".to_owned()
        } else {
            "Inf".to_owned()
        };
    }
    let s = format!("{value:?}");
    if s == "-0.0" { "0.0".to_owned() } else { s }
}

/// Escapes a Rust string into a Julia double-quoted string literal.
///
/// The escape set intentionally mirrors the C/C++ text backends: quotes,
/// backslashes, and common control characters are normalized while all other
/// characters are passed through unchanged.
fn julia_string_literal(input: &str) -> String {
    let escaped = input
        .chars()
        .flat_map(|c| match c {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            _ => vec![c],
        })
        .collect::<String>();
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::{
        CodegenErrorCode, EmitMode, JuliaOptions, JuliaRealType, emit_cast, emit_stmt,
        generate_julia_module,
    };
    use crate::fixtures::build_sine_phasor_test_module;
    use fir::{FirBuilder, FirStore, FirType};

    #[test]
    fn pure_drop_is_elided_but_foreign_call_drop_is_retained() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let one = b.float32(1.0);
        let pure = b.drop_(one);
        let foreign = b.fun_call("foreign", &[], FirType::Float32);
        let effectful = b.drop_(foreign);
        let mut out = String::new();
        let mut mode = EmitMode::Default;

        emit_stmt(&store, &mut out, pure, 0, &mut mode).expect("pure drop should emit");
        assert!(out.is_empty());

        emit_stmt(&store, &mut out, effectful, 0, &mut mode).expect("effectful drop should emit");
        assert!(out.contains("_ = foreign()"));
    }

    #[test]
    fn emits_julia_module_with_dsp_struct_ui_and_compute_loop() {
        let (store, module) = build_sine_phasor_test_module();
        let out = generate_julia_module(&store, module, &JuliaOptions::default())
            .expect("julia module generation should succeed");

        assert!(out.contains("using StaticArrays"));
        assert!(out.contains("fmod(x, y) = rem(x, y)"));
        assert!(out.contains("atan2(y, x) = atan(y, x)"));
        assert!(out.contains("faust_wrap_int32(x) = reinterpret(Int32"));
        assert!(out.contains("mutable struct mydsp{T} <: dsp"));
        assert!(out.contains("fFreq::FAUSTFLOAT"));
        assert!(
            out.contains("function buildUserInterface!(dsp::mydsp{T}, ui_interface::UI) where {T}")
        );
        assert!(out.contains("addHorizontalSlider!(ui_interface, \"freq\", :fFreq"));
        assert!(out.contains("function compute!(dsp::mydsp{T}, count::Int32, inputs::AbstractMatrix{FAUSTFLOAT}, outputs::AbstractMatrix{FAUSTFLOAT}) where {T}"));
        assert!(out.contains("for i0 in 0:((count) - 1)"));
        assert!(out.contains("output0[(i0) + 1] = "));
        assert!(out.contains("sin("));
        assert!(out.contains("instanceConstants!(dsp, sample_rate)"));
        assert!(out.contains("instanceResetUserInterface!(dsp)"));
        assert!(out.contains("instanceClear!(dsp)"));
    }

    #[test]
    fn emits_float64_real_alias_when_requested() {
        let (store, module) = build_sine_phasor_test_module();
        let options = JuliaOptions {
            real_type: JuliaRealType::Float64,
            ..JuliaOptions::default()
        };
        let out = generate_julia_module(&store, module, &options)
            .expect("julia module generation should succeed");

        assert!(out.contains("const REAL = Float64"));
    }

    #[test]
    fn integer_casts_truncate_float_values_like_faust() {
        assert_eq!(
            emit_cast(&fir::FirType::Int32, "0.6000000000000001"),
            "faust_wrap_int32(0.6000000000000001)"
        );
        assert_eq!(
            emit_cast(&fir::FirType::Int64, "-2.9"),
            "trunc(Int64, -2.9)"
        );
    }

    #[test]
    fn real_casts_use_internal_type_parameter() {
        assert_eq!(
            emit_cast(&fir::FirType::Float32, "dsp.fVslider0"),
            "T(dsp.fVslider0)"
        );
        assert_eq!(
            emit_cast(&fir::FirType::Float64, "dsp.fVslider0"),
            "T(dsp.fVslider0)"
        );
    }

    #[test]
    fn rejects_non_module_root() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let root = b.int32(1);

        let err = generate_julia_module(&store, root, &JuliaOptions::default())
            .expect_err("non-module roots must fail");
        assert_eq!(err.code(), CodegenErrorCode::RootNotModule);
        assert!(err.to_string().contains("expected FIR module root"));
    }
}
