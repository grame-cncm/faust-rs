//! Rust backend generation from FIR `Module` roots.
//!
//! # Source provenance (C++)
//! - `compiler/generator/rust/rust_code_container.cpp`
//! - `compiler/generator/rust/rust_instructions.hh`
//!
//! # Current slice and boundary
//! - Module-first emission from FIR `Module`.
//! - Faust-style Rust shell (`pub struct mydsp`, lifecycle methods, `compute`).
//! - The backend is intentionally downstream of `signal_fir`: it does not
//!   reconstruct Signal-IR typing/promotion. It consumes explicit FIR types and
//!   converts them to Rust spellings at statement/value emission time.
//!
//! # Rust runtime shape
//!
//! The generated source follows the same high-level contract as C++ Faust
//! `-lang rust`: it assumes the host has definitions for runtime names such as
//! `Meta`, `UI`, and `Soundfile`. This module is responsible for producing
//! backend text, not for packaging a Rust runtime. The generated unit defines
//! its own `FaustFloat` alias (`f32` by default, `f64` for double precision).
//!
//! # C-semantics preservation
//!
//! FIR is shared with C-family backends, so this emitter must preserve C
//! arithmetic semantics under Rust's stricter rules:
//! - Integer `+`, `-`, `*`, `/`, `%` lower to `wrapping_*` methods so overflow
//!   wraps like the two's-complement C behavior relied on by e.g. noise LCGs.
//! - Implicit C numeric conversions become explicit `as` casts: binary
//!   operands, function-call arguments, store values, and `Select2` branches
//!   are coerced to the FIR result/destination type when their own FIR type
//!   differs.
//! - Table indices are C-style zero-based `i32` values; every generated Rust
//!   indexing boundary appends `as usize`.
//!
//! # Buffer model
//!
//! The canonical FIR `compute(dsp, count, FAUSTFLOAT**, FAUSTFLOAT**)`
//! signature lowers to `&[&[FaustFloat]]` / `&mut [&mut [FaustFloat]]`. FIR
//! channel aliases (`output0 = outputs[0]`) are rewritten to disjoint mutable
//! borrows taken in channel order from `outputs.iter_mut()`, which is the
//! borrow-checker-friendly equivalent of the C pointer aliases.
//!
//! Unsupported FIR nodes fail with `FRS-CGEN-RUST-0003`.

use std::collections::HashMap;
use std::fmt::Write as _;

use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

use crate::backends::faust_api;

pub const BACKEND_NAME: &str = "rust";

/// Rust backend options for module-first emission.
///
/// The backend mirrors the C/C++ convention of defaulting to `mydsp` for
/// deterministic generated type names. `None` means "use the FIR module name",
/// which is useful for fixture-level backend tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustOptions {
    /// Optional Rust DSP struct name override for the FIR module name.
    pub class_name: Option<String>,
    /// Concrete scalar type behind the generated `FaustFloat` alias.
    pub faust_float_type: RustRealType,
}

impl Default for RustOptions {
    fn default() -> Self {
        Self {
            class_name: Some("mydsp".to_owned()),
            faust_float_type: RustRealType::Float32,
        }
    }
}

/// Scalar precision selected for the generated `FaustFloat` alias.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RustRealType {
    /// Single precision, matching Faust default code generation.
    #[default]
    Float32,
    /// Double precision, matching Faust `-double`.
    Float64,
}

impl RustRealType {
    fn rust_name(self) -> &'static str {
        match self {
            Self::Float32 => "f32",
            Self::Float64 => "f64",
        }
    }
}

/// Stable machine-readable error codes for the Rust backend emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// Root FIR node is not a module (`FirMatch::Module`).
    RootNotModule,
    /// One module section is not a FIR block or canonical API signatures are invalid.
    InvalidModuleSection,
    /// The Rust emitter slice does not yet support this FIR node.
    UnsupportedNode,
}

impl CodegenErrorCode {
    /// Returns the stable machine-readable error code string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-RUST-0001",
            Self::InvalidModuleSection => "FRS-CGEN-RUST-0002",
            Self::UnsupportedNode => "FRS-CGEN-RUST-0003",
        }
    }
}

/// Typed backend error returned by the Rust emitter.
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
    /// Creates a typed Rust backend code generation error.
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

/// Decoded `FirMatch::Module` header used by the Rust emitter.
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
/// The Rust backend initializes fields in `new()` and replays explicit reset
/// values in `instance_reset_user_interface` when the FIR module does not
/// provide a canonical reset body. Keeping this view separate preserves FIR
/// declaration order across both places.
#[derive(Debug, Clone)]
struct StructInit {
    name: String,
    typ: FirType,
    init: FirId,
}

/// One mutable state table initializer extracted from `dsp_struct` or `globals`.
///
/// State tables become fixed-size array fields (`[T; N]`) so generated compute
/// code can update elements in place.
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
/// emitted for `metadata` (`m`) or `build_user_interface` (`ui_interface`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitMode {
    Default,
    Metadata,
    Ui,
}

/// Per-function emission context threaded through statement emission.
///
/// Beyond the UI/metadata mode this carries the Rust-specific state needed for
/// C-parity emission:
/// - destination types for `StoreVar`/`StoreTable` coercion (Rust has no
///   implicit conversions),
/// - the `outputs.iter_mut()` cursor used to turn FIR output channel aliases
///   into disjoint mutable borrows.
struct EmitCtx {
    mode: EmitMode,
    /// Known scalar destination types (`self` fields and stack locals).
    var_types: HashMap<String, FirType>,
    /// Known table element types (`self` tables, statics, and channel aliases).
    table_elem_types: HashMap<String, FirType>,
    /// Whether `let mut outputs_iter = outputs.iter_mut();` was emitted.
    outputs_iter_started: bool,
    /// Number of output channels already consumed from `outputs_iter`.
    outputs_taken: usize,
}

impl EmitCtx {
    fn new(mode: EmitMode, base: &StateTypes) -> Self {
        Self {
            mode,
            var_types: base.var_types.clone(),
            table_elem_types: base.table_elem_types.clone(),
            outputs_iter_started: false,
            outputs_taken: 0,
        }
    }
}

/// Module-level state typing collected once before emission.
///
/// Rust requires explicit coercion at store sites, so the emitter records the
/// declared type of every state field and table up front and clones this base
/// into each function's [`EmitCtx`].
#[derive(Default)]
struct StateTypes {
    var_types: HashMap<String, FirType>,
    table_elem_types: HashMap<String, FirType>,
}

/// Body-bearing FIR function declaration normalized for textual emission.
///
/// Prototype-only FIR declarations are intentionally filtered out when this
/// view is collected. Public lifecycle fallbacks should not be displaced by a
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

/// Generates Rust code from a FIR module root.
///
/// The emitter expects a module produced by the transform fast lane. It emits a
/// single Rust source unit containing:
/// - a `FaustFloat` alias plus small math helpers (`remainder_*`),
/// - static tables,
/// - a `pub struct` DSP state container with a `new()` constructor,
/// - Faust lifecycle methods (`init`, `class_init`, `instance_init`, ...),
/// - `build_user_interface` and `compute`,
/// - helper functions.
///
/// The generated unit is structurally aligned with `faust -lang rust` but is
/// not intended to be byte-for-byte identical. Host `Meta`/`UI` traits are
/// expected from the surrounding architecture, mirroring the other module-first
/// backends.
///
/// # Errors
/// Returns [`CodegenError`] if the root is not a FIR module or if the module
/// contains unsupported FIR nodes for the current Rust emitter slice.
pub fn generate_rust_module(
    store: &FirStore,
    module: FirId,
    options: &RustOptions,
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
    let state_types = collect_state_types(
        store,
        &[module.dsp_struct, module.globals, module.static_decls],
    );
    let state_sections = [module.dsp_struct, module.globals];

    let mut out = String::new();
    emit_rust_header(&mut out, options);
    emit_static_tables(store, &mut out, options, module.static_decls)?;
    emit_struct_definition(
        store,
        &mut out,
        &class_name,
        module.dsp_struct,
        module.globals,
    )?;
    emit_rust_api(
        store,
        &mut out,
        RustApiEmitInput {
            options,
            class_name: &class_name,
            num_inputs: module.num_inputs,
            num_outputs: module.num_outputs,
            declared_functions: &functions,
            struct_inits: &struct_inits,
            table_inits: &table_inits,
            state_types: &state_types,
            state_sections,
        },
    )?;
    Ok(out)
}

/// Emits the Rust prologue shared by every generated source unit.
///
/// The `FaustFloat` alias plays the role of the C `FAUSTFLOAT` macro. The
/// `remainder_*` helpers implement the C99 `remainder` semantics (IEEE
/// round-half-to-even quotient), which has no direct `std` equivalent.
fn emit_rust_header(out: &mut String, options: &RustOptions) {
    let real = options.faust_float_type.rust_name();
    let _ = writeln!(
        out,
        "/* ------------------------------------------------------------"
    );
    let _ = writeln!(out, "Code generated with faust-rs");
    let _ = writeln!(out, "Compilation options: -lang rust");
    let _ = writeln!(
        out,
        "------------------------------------------------------------ */"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "pub type FaustFloat = {real};");
    let _ = writeln!(out);
    let _ = writeln!(out, "#[allow(dead_code)]");
    let _ = writeln!(out, "fn remainder_f32(x: f32, y: f32) -> f32 {{");
    let _ = writeln!(out, "    x - y * (x / y).round_ties_even()");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "#[allow(dead_code)]");
    let _ = writeln!(out, "fn remainder_f64(x: f64, y: f64) -> f64 {{");
    let _ = writeln!(out, "    x - y * (x / y).round_ties_even()");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Emits the DSP state `pub struct` definition and derives nothing.
///
/// FIR state is split between `dsp_struct` and `globals`; both sections become
/// fields on the Rust struct because generated single-unit Rust has no direct
/// equivalent of C++ class statics. If the FIR module omits `fSampleRate`, the
/// backend adds it to preserve the canonical Faust lifecycle API.
fn emit_struct_definition(
    store: &FirStore,
    out: &mut String,
    class_name: &str,
    dsp_struct: FirId,
    globals: FirId,
) -> Result<(), CodegenError> {
    let has_sample_rate_field = block_declares_var(store, dsp_struct, "fSampleRate")
        || block_declares_var(store, globals, "fSampleRate");

    let _ = writeln!(
        out,
        "#[allow(non_snake_case, non_camel_case_types, dead_code)]"
    );
    let _ = writeln!(out, "pub struct {class_name} {{");
    emit_struct_fields(store, out, dsp_struct)?;
    emit_struct_fields(store, out, globals)?;
    if !has_sample_rate_field {
        let _ = writeln!(out, "    fSampleRate: i32,");
    }
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits typed Rust fields for one FIR declaration block.
///
/// Scalar variables become `name: Type`; mutable tables become
/// `name: [Type; N]`. Other FIR declaration forms are ignored here because they
/// belong to function/static sections, not DSP state storage.
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
                let _ = writeln!(out, "    {name}: {},", emit_type(&typ));
            }
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => {
                let _ = writeln!(
                    out,
                    "    {name}: [{}; {}],",
                    emit_type(&elem_type),
                    values.len()
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// Aggregated inputs needed to synthesize the public Rust DSP API.
///
/// `generate_rust_module` collects these views once, then passes borrowed
/// slices into the API emitter. This keeps the rendering phase deterministic:
/// canonical FIR functions, scalar resets, and table resets are all replayed in
/// the order discovered from the FIR module rather than re-scanning sections in
/// several independent helpers.
struct RustApiEmitInput<'a> {
    options: &'a RustOptions,
    /// Effective Rust DSP struct name.
    class_name: &'a str,
    /// Propagated input arity reported by the FIR module.
    num_inputs: usize,
    /// Propagated output arity reported by the FIR module.
    num_outputs: usize,
    /// Body-bearing function declarations collected from the module.
    declared_functions: &'a [DeclareFunView],
    /// Scalar state initializers for `new()` and synthesized reset paths.
    struct_inits: &'a [StructInit],
    /// Mutable state table initializers for synthesized reset paths.
    table_inits: &'a [TableInit],
    /// Module-level state typing for store-site coercion.
    state_types: &'a StateTypes,
    /// Raw `dsp_struct`/`globals` section ids re-walked by the constructor.
    state_sections: [FirId; 2],
}

/// Emits the public Faust-style Rust API around the lowered FIR bodies.
///
/// Canonical body-bearing FIR functions are replayed when present. Missing
/// lifecycle callbacks are synthesized using the same ordering as C++ Faust:
/// `instance_constants`, `instance_reset_user_interface`, then
/// `instance_clear` inside `instance_init`, and `class_init` before
/// `instance_init` inside `init`.
///
/// This function is intentionally the only place that knows the Faust public
/// method names and their Rust signatures. Lower-level statement/value helpers
/// only emit generic FIR syntax and do not make lifecycle policy decisions.
fn emit_rust_api(
    store: &FirStore,
    out: &mut String,
    spec: RustApiEmitInput<'_>,
) -> Result<(), CodegenError> {
    let RustApiEmitInput {
        options,
        class_name,
        num_inputs,
        num_outputs,
        declared_functions,
        struct_inits,
        table_inits,
        state_types,
        state_sections,
    } = spec;

    let _ = writeln!(
        out,
        "#[allow(non_snake_case, dead_code, unused_variables, unused_mut, unused_parens, clippy::all)]"
    );
    let _ = writeln!(out, "impl {class_name} {{");

    emit_constructor(store, out, options, class_name, state_sections)?;

    if let Some(f) = declared_functions.iter().find(|f| f.name == "metadata") {
        emit_named_method(store, out, options, state_types, f)?;
    } else {
        let _ = writeln!(out, "    pub fn metadata(&self, m: &mut dyn Meta) {{");
        let _ = writeln!(
            out,
            "        m.declare(\"faust-rs\", \"module-first rust backend prototype\");"
        );
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "    pub fn get_sample_rate(&self) -> i32 {{");
    let _ = writeln!(out, "        self.fSampleRate");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);
    let _ = writeln!(out, "    pub fn get_num_inputs(&self) -> i32 {{");
    let _ = writeln!(out, "        {num_inputs}");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);
    let _ = writeln!(out, "    pub fn get_num_outputs(&self) -> i32 {{");
    let _ = writeln!(out, "        {num_outputs}");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);

    let _ = writeln!(out, "    pub fn class_init(sample_rate: i32) {{");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceConstants")
    {
        emit_named_method(store, out, options, state_types, f)?;
    } else {
        let _ = writeln!(
            out,
            "    pub fn instance_constants(&mut self, sample_rate: i32) {{"
        );
        let _ = writeln!(out, "        self.fSampleRate = sample_rate;");
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceResetUserInterface")
    {
        emit_named_method(store, out, options, state_types, f)?;
    } else {
        let _ = writeln!(
            out,
            "    pub fn instance_reset_user_interface(&mut self) {{"
        );
        for init in struct_inits {
            let value = emit_value(store, options, init.init)?;
            let value = coerce_rendered(store, &init.typ, init.init, &value);
            let _ = writeln!(out, "        self.{} = {value};", init.name);
        }
        for init in table_inits {
            for (index, value_id) in init.values.iter().copied().enumerate() {
                let value = emit_value(store, options, value_id)?;
                let value = coerce_rendered(store, &init.elem_type, value_id, &value);
                let _ = writeln!(out, "        self.{}[{index}] = {value};", init.name);
            }
        }
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "instanceClear")
    {
        emit_named_method(store, out, options, state_types, f)?;
    } else {
        let _ = writeln!(out, "    pub fn instance_clear(&mut self) {{");
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
    }

    let _ = writeln!(
        out,
        "    pub fn instance_init(&mut self, sample_rate: i32) {{"
    );
    let _ = writeln!(out, "        self.instance_constants(sample_rate);");
    let _ = writeln!(out, "        self.instance_reset_user_interface();");
    let _ = writeln!(out, "        self.instance_clear();");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);

    let _ = writeln!(out, "    pub fn init(&mut self, sample_rate: i32) {{");
    let _ = writeln!(out, "        Self::class_init(sample_rate);");
    let _ = writeln!(out, "        self.instance_init(sample_rate);");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);

    if let Some(f) = declared_functions
        .iter()
        .find(|f| f.name == "buildUserInterface")
    {
        emit_named_method(store, out, options, state_types, f)?;
    } else {
        let _ = writeln!(
            out,
            "    pub fn build_user_interface(&mut self, ui_interface: &mut dyn UI<FaustFloat>) {{"
        );
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
    }

    if let Some(f) = declared_functions.iter().find(|f| f.name == "compute") {
        emit_named_method(store, out, options, state_types, f)?;
    } else {
        let _ = writeln!(
            out,
            "    pub fn compute(&mut self, count: i32, inputs: &[&[FaustFloat]], outputs: &mut [&mut [FaustFloat]]) {{"
        );
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "}}");

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
        let _ = writeln!(out);
        emit_helper_function(store, out, options, state_types, f)?;
    }

    Ok(())
}

/// Emits the `new()` constructor with every field explicitly initialized.
///
/// Rust struct literals must name every field, so this constructor covers all
/// scalar/table declarations from both state sections plus the synthesized
/// `fSampleRate` when FIR does not declare it. Explicit FIR initializers are
/// honored; otherwise a typed neutral value is synthesized.
fn emit_constructor(
    store: &FirStore,
    out: &mut String,
    options: &RustOptions,
    class_name: &str,
    state_sections: [FirId; 2],
) -> Result<(), CodegenError> {
    let _ = writeln!(out, "    pub fn new() -> {class_name} {{");
    let _ = writeln!(out, "        {class_name} {{");
    let mut has_sample_rate_field = false;
    // The constructor re-walks the raw state sections (rather than the
    // collected initializer views) so uninitialized fields still receive a
    // typed default in declaration order.
    for section in state_sections {
        let FirMatch::Block(items) = match_fir(store, section) else {
            continue;
        };
        for item in items {
            match match_fir(store, item) {
                FirMatch::DeclareVar {
                    name, typ, init, ..
                } => {
                    if name == "fSampleRate" {
                        has_sample_rate_field = true;
                    }
                    let value = if let Some(init) = init {
                        let rendered = emit_value(store, options, init)?;
                        coerce_rendered(store, &typ, init, &rendered)
                    } else {
                        zero_value(&typ)
                    };
                    let _ = writeln!(out, "            {name}: {value},");
                }
                FirMatch::DeclareTable {
                    name,
                    elem_type,
                    values,
                    ..
                } => {
                    let mut rendered = Vec::with_capacity(values.len());
                    for value in values {
                        let v = emit_value(store, options, value)?;
                        rendered.push(coerce_rendered(store, &elem_type, value, &v));
                    }
                    let _ = writeln!(out, "            {name}: [{}],", rendered.join(", "));
                }
                _ => {}
            }
        }
    }
    if !has_sample_rate_field {
        let _ = writeln!(out, "            fSampleRate: 0,");
    }
    let _ = writeln!(out, "        }}");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits one canonical DSP API function from its FIR body.
///
/// The canonical Faust function signatures are validated before textual
/// emission. This catches corrupted or non-canonical module shapes at the
/// backend boundary rather than producing Rust with a mismatched lifecycle
/// method.
///
/// `instance_constants` has one extra policy hook: if the FIR body does not
/// store `fSampleRate`, the emitter writes `self.fSampleRate = sample_rate;`
/// before replaying the body. This mirrors the C/C++ lifecycle invariant used
/// by the other module-first backends.
fn emit_named_method(
    store: &FirStore,
    out: &mut String,
    options: &RustOptions,
    state_types: &StateTypes,
    decl: &DeclareFunView,
) -> Result<(), CodegenError> {
    faust_api::validate_canonical_dsp_api_signature(&decl.name, &decl.typ, &decl.named_args)
        .map_err(|msg| CodegenError::new(CodegenErrorCode::InvalidModuleSection, msg))?;
    let signature = match decl.name.as_str() {
        "metadata" => "pub fn metadata(&self, m: &mut dyn Meta)".to_owned(),
        "instanceConstants" => {
            "pub fn instance_constants(&mut self, sample_rate: i32)".to_owned()
        }
        "instanceResetUserInterface" => {
            "pub fn instance_reset_user_interface(&mut self)".to_owned()
        }
        "instanceClear" => "pub fn instance_clear(&mut self)".to_owned(),
        "buildUserInterface" => {
            "pub fn build_user_interface(&mut self, ui_interface: &mut dyn UI<FaustFloat>)"
                .to_owned()
        }
        "compute" => {
            "pub fn compute(&mut self, count: i32, inputs: &[&[FaustFloat]], outputs: &mut [&mut [FaustFloat]])"
                .to_owned()
        }
        other => format!("pub fn {other}(&mut self)"),
    };
    let body = decl
        .body
        .expect("emit_named_method called with prototype-only DeclareFunView");
    let _ = writeln!(out, "    {signature} {{");
    if decl.name == "instanceConstants" && !block_stores_var(store, body, "fSampleRate") {
        let _ = writeln!(out, "        self.fSampleRate = sample_rate;");
    }
    let mode = match decl.name.as_str() {
        "metadata" => EmitMode::Metadata,
        "buildUserInterface" => EmitMode::Ui,
        _ => EmitMode::Default,
    };
    let mut ctx = EmitCtx::new(mode, state_types);
    emit_block(store, out, options, body, 2, &mut ctx)?;
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits a non-canonical helper function declared in the FIR functions block.
///
/// Helper functions keep their FIR argument names/types when available. They
/// are emitted after the DSP `impl` block, matching the current C/C++
/// fast-lane structure closely enough for structural parity tests.
fn emit_helper_function(
    store: &FirStore,
    out: &mut String,
    options: &RustOptions,
    state_types: &StateTypes,
    decl: &DeclareFunView,
) -> Result<(), CodegenError> {
    let body = decl
        .body
        .expect("emit_helper_function called with prototype-only DeclareFunView");
    let (params, ret) = match &decl.typ {
        FirType::Fun { args, ret } => {
            let rendered = args
                .iter()
                .enumerate()
                .map(|(index, arg_type)| {
                    let name = decl
                        .named_args
                        .get(index)
                        .map_or_else(|| format!("arg{index}"), |named| named.name.clone());
                    format!("{name}: {}", emit_type(arg_type))
                })
                .collect::<Vec<_>>()
                .join(", ");
            (rendered, ret.as_ref().clone())
        }
        other => (String::new(), other.clone()),
    };
    let _ = writeln!(
        out,
        "#[allow(non_snake_case, dead_code, unused_variables, unused_mut, unused_parens, clippy::all)]"
    );
    if matches!(ret, FirType::Void) {
        let _ = writeln!(out, "fn {}({params}) {{", decl.name);
    } else {
        let _ = writeln!(out, "fn {}({params}) -> {} {{", decl.name, emit_type(&ret));
    }
    let mut ctx = EmitCtx::new(EmitMode::Default, state_types);
    emit_block(store, out, options, body, 1, &mut ctx)?;
    let _ = writeln!(out, "}}");
    Ok(())
}

/// Emits every statement in a FIR block under the active emission context.
///
/// The context is threaded through nested control-flow blocks so UI/metadata
/// statements keep the correct receiver and the compute output-channel cursor
/// stays consistent even when aliases appear under nested blocks.
///
/// The function rejects non-block ids with a backend diagnostic rather than
/// treating them as single statements. FIR lowering is expected to wrap
/// function bodies and branch bodies in explicit `Block` nodes.
fn emit_block(
    store: &FirStore,
    out: &mut String,
    options: &RustOptions,
    block: FirId,
    indent: usize,
    ctx: &mut EmitCtx,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, block) else {
        return Err(unsupported_node("expected block", block, store));
    };
    for stmt in items {
        emit_stmt(store, out, options, stmt, indent, ctx)?;
    }
    Ok(())
}

/// Emits one FIR statement as Rust source.
///
/// Control flow is rendered with native Rust `if`, `while`, `for`, and `match`
/// syntax. General `ForLoop` nodes use a scoped `while` because FIR expresses
/// C-style init/end/step triples, including reverse loops, more directly than
/// Rust ranges. `SimpleForLoop` uses ranges and preserves the zero-based loop
/// variable used by the shared FIR contract.
///
/// FIR channel aliases (`DeclareVar` initialized from `inputs`/`outputs`
/// `FunArgs` tables) are the one Rust-specific rewrite: input aliases copy the
/// shared channel slice, output aliases take the next disjoint mutable borrow
/// from `outputs.iter_mut()`.
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    options: &RustOptions,
    stmt: FirId,
    indent: usize,
    ctx: &mut EmitCtx,
) -> Result<(), CodegenError> {
    let tab = "    ".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::DeclareVar {
            name, typ, init, ..
        } => {
            if let Some(init_id) = init
                && let Some(channel) = decode_io_alias(store, init_id)
            {
                return emit_io_alias(store, out, &tab, &name, channel, ctx);
            }
            if matches!(typ, FirType::Ptr(_)) {
                // Non-channel pointer aliases keep Rust inference; annotating
                // FIR pointer spellings would force a borrow model choice that
                // the initializer already made.
                if let Some(init) = init {
                    let init = emit_value(store, options, init)?;
                    let _ = writeln!(out, "{tab}let mut {name} = {init};");
                } else {
                    return Err(unsupported_node("pointer declaration", stmt, store));
                }
                return Ok(());
            }
            let value = if let Some(init) = init {
                let rendered = emit_value(store, options, init)?;
                coerce_rendered(store, &typ, init, &rendered)
            } else {
                zero_value(&typ)
            };
            ctx.var_types.insert(name.clone(), typ.clone());
            let _ = writeln!(out, "{tab}let mut {name}: {} = {value};", emit_type(&typ));
            Ok(())
        }
        FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } => {
            let mut rendered = Vec::with_capacity(values.len());
            for value in &values {
                let v = emit_value(store, options, *value)?;
                rendered.push(coerce_rendered(store, &elem_type, *value, &v));
            }
            ctx.table_elem_types.insert(name.clone(), elem_type.clone());
            let _ = writeln!(
                out,
                "{tab}let mut {name}: [{}; {}] = [{}];",
                emit_type(&elem_type),
                values.len(),
                rendered.join(", ")
            );
            Ok(())
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => {
            let rendered = emit_value(store, options, value)?;
            let rendered = if let Some(dest) = ctx.var_types.get(&name).cloned() {
                coerce_rendered(store, &dest, value, &rendered)
            } else {
                rendered
            };
            let target = emit_var_ref(&name, access);
            let _ = writeln!(out, "{tab}{target} = {rendered};");
            Ok(())
        }
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => {
            let index = emit_index_expr(store, options, index)?;
            let rendered = emit_value(store, options, value)?;
            let rendered = if let Some(elem) = ctx.table_elem_types.get(&name).cloned() {
                coerce_rendered(store, &elem, value, &rendered)
            } else {
                rendered
            };
            let target = emit_var_ref(&name, access);
            let _ = writeln!(out, "{tab}{target}[{index}] = {rendered};");
            Ok(())
        }
        FirMatch::Drop(value) => {
            let value = emit_value(store, options, value)?;
            let _ = writeln!(out, "{tab}let _ = {value};");
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
        FirMatch::Block(_) => emit_block(store, out, options, stmt, indent, ctx),
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let cond = emit_cond(store, options, cond)?;
            let _ = writeln!(out, "{tab}if {cond} {{");
            emit_block(store, out, options, then_block, indent + 1, ctx)?;
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{tab}}} else {{");
                emit_block(store, out, options, else_block, indent + 1, ctx)?;
            }
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Control { cond, stmt } => {
            let cond = emit_cond(store, options, cond)?;
            let _ = writeln!(out, "{tab}if {cond} {{");
            emit_stmt(store, out, options, stmt, indent + 1, ctx)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Switch {
            cond,
            ref cases,
            default,
        } => {
            let cond = emit_value(store, options, cond)?;
            let _ = writeln!(out, "{tab}match {cond} {{");
            for (value, block) in cases {
                let _ = writeln!(out, "{tab}    {value} => {{");
                emit_block(store, out, options, *block, indent + 2, ctx)?;
                let _ = writeln!(out, "{tab}    }}");
            }
            let _ = writeln!(out, "{tab}    _ => {{");
            if let Some(default) = default {
                emit_block(store, out, options, default, indent + 2, ctx)?;
            }
            let _ = writeln!(out, "{tab}    }}");
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
            let cmp = if is_reverse { ">" } else { "<" };
            let _ = writeln!(out, "{tab}{{");
            let _ = writeln!(out, "{tab}    let mut {var}: i32 = {init_val};");
            let _ = writeln!(out, "{tab}    while {var} {cmp} {end} {{");
            emit_block(store, out, options, body, indent + 2, ctx)?;
            let _ = writeln!(out, "{tab}        {var} = {var}.wrapping_add({step});");
            let _ = writeln!(out, "{tab}    }}");
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
                let _ = writeln!(out, "{tab}for {var} in (0..{upper}).rev() {{");
            } else {
                let _ = writeln!(out, "{tab}for {var} in 0..{upper} {{");
            }
            emit_block(store, out, options, body, indent + 1, ctx)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::WhileLoop { cond, body } => {
            let cond = emit_cond(store, options, cond)?;
            let _ = writeln!(out, "{tab}while {cond} {{");
            emit_block(store, out, options, body, indent + 1, ctx)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::OpenBox { typ, label } => {
            let api = match typ {
                fir::UiBoxType::Vertical => "open_vertical_box",
                fir::UiBoxType::Horizontal => "open_horizontal_box",
                fir::UiBoxType::Tab => "open_tab_box",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface.{api}({});",
                rust_string_literal(&label)
            );
            Ok(())
        }
        FirMatch::CloseBox => {
            let _ = writeln!(out, "{tab}ui_interface.close_box();");
            Ok(())
        }
        FirMatch::AddButton { typ, label, var } => {
            let api = match typ {
                fir::ButtonType::Button => "add_button",
                fir::ButtonType::Checkbox => "add_check_button",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface.{api}({}, &mut self.{var});",
                rust_string_literal(&label)
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
                fir::SliderType::Horizontal => "add_horizontal_slider",
                fir::SliderType::Vertical => "add_vertical_slider",
                fir::SliderType::NumEntry => "add_num_entry",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface.{api}({}, &mut self.{var}, {} as FaustFloat, {} as FaustFloat, {} as FaustFloat, {} as FaustFloat);",
                rust_string_literal(&label),
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
                fir::BargraphType::Horizontal => "add_horizontal_bargraph",
                fir::BargraphType::Vertical => "add_vertical_bargraph",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface.{api}({}, &mut self.{var}, {} as FaustFloat, {} as FaustFloat);",
                rust_string_literal(&label),
                trim_float(lo),
                trim_float(hi)
            );
            Ok(())
        }
        FirMatch::AddMetaDeclare { var, key, value } => {
            match ctx.mode {
                EmitMode::Ui => {
                    let zone = if var == "0" {
                        "None".to_owned()
                    } else {
                        format!("Some(&mut self.{var})")
                    };
                    let _ = writeln!(
                        out,
                        "{tab}ui_interface.declare({zone}, {}, {});",
                        rust_string_literal(&key),
                        rust_string_literal(&value)
                    );
                }
                EmitMode::Default | EmitMode::Metadata => {
                    let _ = writeln!(
                        out,
                        "{tab}m.declare({}, {});",
                        rust_string_literal(&key),
                        rust_string_literal(&value)
                    );
                }
            }
            Ok(())
        }
        FirMatch::AddSoundfile { label, url, var } => {
            let _ = writeln!(
                out,
                "{tab}ui_interface.add_soundfile({}, {}, &mut self.{var});",
                rust_string_literal(&label),
                rust_string_literal(&url)
            );
            Ok(())
        }
        FirMatch::NullStatement => {
            let _ = writeln!(out, "{tab}();");
            Ok(())
        }
        FirMatch::Label(label) => {
            let _ = writeln!(out, "{tab}// {label}");
            Ok(())
        }
        _ => Err(unsupported_node("statement", stmt, store)),
    }
}

/// Decodes a `DeclareVar` initializer that aliases one I/O channel.
///
/// Returns the `(is_output, channel)` pair when `init` is a
/// `LoadTable("inputs"|"outputs", FunArgs, <const chan>)` node, which is the
/// canonical FIR shape for C `input0 = inputs[0];` channel aliases.
fn decode_io_alias(store: &FirStore, init: FirId) -> Option<(bool, usize)> {
    if let FirMatch::LoadTable {
        name,
        access: AccessType::FunArgs,
        index,
        ..
    } = match_fir(store, init)
        && matches!(name.as_str(), "inputs" | "outputs")
        && let FirMatch::Int32 { value, .. } = match_fir(store, index)
        && let Ok(channel) = usize::try_from(value)
    {
        return Some((name == "outputs", channel));
    }
    None
}

/// Emits one I/O channel alias with the Rust borrow model.
///
/// Input aliases copy the shared `&[FaustFloat]` slice. Output aliases must be
/// disjoint mutable borrows, so the first one starts an `outputs.iter_mut()`
/// cursor and each alias consumes the next channel (skipping any FIR channel
/// gaps with `nth`). FIR emits aliases in ascending channel order, matching
/// the C++ reference containers.
fn emit_io_alias(
    store: &FirStore,
    out: &mut String,
    tab: &str,
    name: &str,
    channel: (bool, usize),
    ctx: &mut EmitCtx,
) -> Result<(), CodegenError> {
    let _ = store;
    let (is_output, chan) = channel;
    if is_output {
        if chan < ctx.outputs_taken {
            return Err(CodegenError::new(
                CodegenErrorCode::UnsupportedNode,
                format!("output channel alias {name} repeats channel {chan}"),
            ));
        }
        if !ctx.outputs_iter_started {
            let _ = writeln!(out, "{tab}let mut outputs_iter = outputs.iter_mut();");
            ctx.outputs_iter_started = true;
        }
        let skip = chan - ctx.outputs_taken;
        let _ = writeln!(
            out,
            "{tab}let {name} = outputs_iter.nth({skip}).expect(\"missing output channel\");"
        );
        ctx.outputs_taken = chan + 1;
    } else {
        let _ = writeln!(out, "{tab}let {name} = inputs[{chan}];");
    }
    ctx.table_elem_types
        .insert(name.to_owned(), FirType::FaustFloat);
    Ok(())
}

/// Emits one FIR value expression as a Rust expression.
///
/// FIR math calls are already normalized to backend-agnostic names. This
/// helper applies Rust spelling fixes (associated functions on the concrete
/// float type) and coerces arguments/operands whose FIR type differs from the
/// FIR result type, because Rust has no implicit numeric conversions.
fn emit_value(
    store: &FirStore,
    options: &RustOptions,
    value: FirId,
) -> Result<String, CodegenError> {
    match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(format!("{value}i32")),
        FirMatch::Int64 { value, .. } => Ok(format!("{value}i64")),
        FirMatch::Float32 { value, .. } => Ok(format!("{}f32", trim_float(f64::from(value)))),
        FirMatch::Float64 { value, .. }
        | FirMatch::Quad { value, .. }
        | FirMatch::FixedPoint { value, .. } => Ok(format!("{}f64", trim_float(value))),
        FirMatch::Bool { value, .. } => Ok(if value { "true" } else { "false" }.to_owned()),
        FirMatch::LoadVar { name, access, .. } => Ok(emit_var_ref(&name, access)),
        FirMatch::LoadVarAddress { name, access, .. } => {
            Ok(format!("&mut {}", emit_var_ref(&name, access)))
        }
        FirMatch::LoadTable {
            name,
            access,
            index,
            ..
        } => {
            if matches!(access, AccessType::FunArgs) && name == "outputs" {
                return Err(unsupported_node(
                    "direct outputs load outside a channel alias",
                    value,
                    store,
                ));
            }
            let index = emit_index_expr(store, options, index)?;
            if matches!(access, AccessType::FunArgs) && name == "inputs" {
                return Ok(format!("inputs[{index}]"));
            }
            Ok(format!("{}[{index}]", emit_var_ref(&name, access)))
        }
        FirMatch::TeeVar {
            name,
            access,
            value: inner,
            ..
        } => {
            let inner = emit_value(store, options, inner)?;
            let target = emit_var_ref(&name, access);
            Ok(format!("{{ {target} = {inner}; {target} }}"))
        }
        FirMatch::BinOp { op, lhs, rhs, typ } => {
            emit_binop_expr(store, options, op, lhs, rhs, &typ)
        }
        FirMatch::Neg { value: inner, typ } => {
            let rendered = emit_value(store, options, inner)?;
            let rendered = coerce_rendered(store, &typ, inner, &rendered);
            Ok(format!("(-{rendered})"))
        }
        FirMatch::Cast { typ, value: inner } => {
            let rendered = emit_value(store, options, inner)?;
            Ok(cast_rendered(
                &typ,
                value_type(store, inner).as_ref(),
                &rendered,
            ))
        }
        FirMatch::Bitcast { typ, value: inner } => {
            let rendered = emit_value(store, options, inner)?;
            emit_bitcast(store, &typ, inner, &rendered)
        }
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            typ,
        } => {
            let cond = emit_cond(store, options, cond)?;
            let then_rendered = emit_value(store, options, then_value)?;
            let then_rendered = coerce_rendered(store, &typ, then_value, &then_rendered);
            let else_rendered = emit_value(store, options, else_value)?;
            let else_rendered = coerce_rendered(store, &typ, else_value, &else_rendered);
            Ok(format!(
                "(if {cond} {{ {then_rendered} }} else {{ {else_rendered} }})"
            ))
        }
        FirMatch::FunCall { name, args, typ } => emit_fun_call(store, options, &name, &args, &typ),
        FirMatch::NullValue { .. } => Ok("Default::default()".to_owned()),
        FirMatch::LoadSoundfileLength { var, part } => {
            let part = emit_index_expr(store, options, part)?;
            Ok(format!("self.{var}.fLength[{part}]"))
        }
        FirMatch::LoadSoundfileRate { var, part } => {
            let part = emit_index_expr(store, options, part)?;
            Ok(format!("self.{var}.fSR[{part}]"))
        }
        FirMatch::LoadSoundfileBuffer {
            var,
            chan,
            part,
            idx,
            ..
        } => {
            let chan = emit_index_expr(store, options, chan)?;
            let part = emit_index_expr(store, options, part)?;
            let idx = emit_index_expr(store, options, idx)?;
            Ok(format!(
                "self.{var}.fBuffers[{chan}][(self.{var}.fOffset[{part}] as usize) + {idx}]"
            ))
        }
        _ => Err(unsupported_node("value", value, store)),
    }
}

/// Emits a FIR value as a Rust `bool` condition.
///
/// FIR conditions are C-style: comparison nodes may be typed `Int32` (their C
/// result type), and plain integer values are also valid truth values. Direct
/// comparison nodes are rendered as native Rust booleans; everything else gets
/// an explicit `!= 0` because Rust has no integer truthiness.
fn emit_cond(store: &FirStore, options: &RustOptions, cond: FirId) -> Result<String, CodegenError> {
    if let FirMatch::BinOp { op, lhs, rhs, .. } = match_fir(store, cond)
        && is_comparison(op)
    {
        return emit_comparison(store, options, op, lhs, rhs);
    }
    let rendered = emit_value(store, options, cond)?;
    match value_type(store, cond) {
        Some(FirType::Bool) => Ok(rendered),
        Some(FirType::Float32 | FirType::Float64 | FirType::FaustFloat | FirType::Quad) => {
            Ok(format!("(({rendered}) != 0.0)"))
        }
        _ => Ok(format!("(({rendered}) != 0)")),
    }
}

/// Returns whether one FIR binary operator is a comparison.
fn is_comparison(op: FirBinOp) -> bool {
    matches!(
        op,
        FirBinOp::Eq | FirBinOp::Ne | FirBinOp::Lt | FirBinOp::Le | FirBinOp::Gt | FirBinOp::Ge
    )
}

/// Emits one FIR comparison as a native Rust boolean expression.
fn emit_comparison(
    store: &FirStore,
    options: &RustOptions,
    op: FirBinOp,
    lhs: FirId,
    rhs: FirId,
) -> Result<String, CodegenError> {
    let lhs_rendered = emit_value(store, options, lhs)?;
    let rhs_rendered = emit_value(store, options, rhs)?;
    let (l, r) = coerce_comparison_operands(store, (lhs, &lhs_rendered), (rhs, &rhs_rendered));
    let token = comparison_token(op);
    Ok(format!("({l} {token} {r})"))
}

/// Emits a FIR table index converted to a Rust `usize`.
///
/// The FIR value itself remains a zero-based C-style `i32`. Only the final
/// indexing expression receives `as usize`, keeping arithmetic and loop bounds
/// shared with other backends.
fn emit_index_expr(
    store: &FirStore,
    options: &RustOptions,
    value: FirId,
) -> Result<String, CodegenError> {
    let rendered = emit_value(store, options, value)?;
    Ok(format!("({rendered}) as usize"))
}

/// Emits a complete Rust binary-operation expression with C semantics.
///
/// Integer arithmetic lowers to `wrapping_*` methods to preserve C
/// two's-complement wrapping; logical right shift reinterprets through the
/// unsigned type of the same width. Operands whose FIR type differs from the
/// FIR result type are coerced first; comparisons promote both operands to
/// their common C arithmetic type.
fn emit_binop_expr(
    store: &FirStore,
    options: &RustOptions,
    op: FirBinOp,
    lhs: FirId,
    rhs: FirId,
    typ: &FirType,
) -> Result<String, CodegenError> {
    let lhs_rendered = emit_value(store, options, lhs)?;
    let rhs_rendered = emit_value(store, options, rhs)?;

    if is_comparison(op) {
        // FIR comparisons carry their C result type (`Int32` from the fast
        // lane, `Bool` in hand-written fixtures). The rendered Rust expression
        // must match that type so arithmetic/coercion callers stay sound.
        let rendered = emit_comparison(store, options, op, lhs, rhs)?;
        return Ok(match typ {
            FirType::Bool => rendered,
            _ => format!("(({rendered}) as {})", emit_type(typ)),
        });
    }

    let l = coerce_rendered(store, typ, lhs, &lhs_rendered);
    let is_int = matches!(typ, FirType::Int32 | FirType::Int64);
    match op {
        FirBinOp::Add | FirBinOp::Sub | FirBinOp::Mul | FirBinOp::Div | FirBinOp::Rem if is_int => {
            let r = coerce_rendered(store, typ, rhs, &rhs_rendered);
            let method = match op {
                FirBinOp::Add => "wrapping_add",
                FirBinOp::Sub => "wrapping_sub",
                FirBinOp::Mul => "wrapping_mul",
                FirBinOp::Div => "wrapping_div",
                _ => "wrapping_rem",
            };
            Ok(format!("({l}).{method}({r})"))
        }
        FirBinOp::Add => Ok(format!(
            "({l} + {})",
            coerce_rendered(store, typ, rhs, &rhs_rendered)
        )),
        FirBinOp::Sub => Ok(format!(
            "({l} - {})",
            coerce_rendered(store, typ, rhs, &rhs_rendered)
        )),
        FirBinOp::Mul => Ok(format!(
            "({l} * {})",
            coerce_rendered(store, typ, rhs, &rhs_rendered)
        )),
        FirBinOp::Div => Ok(format!(
            "({l} / {})",
            coerce_rendered(store, typ, rhs, &rhs_rendered)
        )),
        FirBinOp::Rem => Ok(format!(
            "({l} % {})",
            coerce_rendered(store, typ, rhs, &rhs_rendered)
        )),
        FirBinOp::And | FirBinOp::Or | FirBinOp::Xor => {
            let r = if matches!(typ, FirType::Bool) {
                rhs_rendered
            } else {
                coerce_rendered(store, typ, rhs, &rhs_rendered)
            };
            let token = match op {
                FirBinOp::And => "&",
                FirBinOp::Or => "|",
                _ => "^",
            };
            Ok(format!("({l} {token} {r})"))
        }
        FirBinOp::Lsh => Ok(format!("({l}).wrapping_shl(({rhs_rendered}) as u32)")),
        FirBinOp::ARsh => Ok(format!("({l}).wrapping_shr(({rhs_rendered}) as u32)")),
        FirBinOp::LRsh => {
            let (unsigned, signed) = if matches!(typ, FirType::Int64) {
                ("u64", "i64")
            } else {
                ("u32", "i32")
            };
            Ok(format!(
                "(((({l}) as {unsigned}).wrapping_shr(({rhs_rendered}) as u32)) as {signed})"
            ))
        }
        FirBinOp::Eq | FirBinOp::Ne | FirBinOp::Lt | FirBinOp::Le | FirBinOp::Gt | FirBinOp::Ge => {
            unreachable!("comparisons handled above")
        }
    }
}

/// Returns the Rust token for one FIR comparison operator.
fn comparison_token(op: FirBinOp) -> &'static str {
    match op {
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::Lt => "<",
        FirBinOp::Le => "<=",
        FirBinOp::Gt => ">",
        FirBinOp::Ge => ">=",
        _ => unreachable!("not a comparison operator"),
    }
}

/// Coerces both comparison operands to their common C arithmetic type.
///
/// FIR comparisons are typed `Bool`, so the promotion target must be derived
/// from the operands themselves using C usual-arithmetic-conversion ranking.
fn coerce_comparison_operands<'a>(
    store: &FirStore,
    (lhs, lhs_rendered): (FirId, &'a str),
    (rhs, rhs_rendered): (FirId, &'a str),
) -> (String, String) {
    let lhs_type = value_type(store, lhs);
    let rhs_type = value_type(store, rhs);
    let (Some(lt), Some(rt)) = (lhs_type, rhs_type) else {
        return (lhs_rendered.to_owned(), rhs_rendered.to_owned());
    };
    if lt == rt {
        return (lhs_rendered.to_owned(), rhs_rendered.to_owned());
    }
    let target = if numeric_rank(&lt) >= numeric_rank(&rt) {
        lt.clone()
    } else {
        rt.clone()
    };
    (
        cast_rendered(&target, Some(&lt), lhs_rendered),
        cast_rendered(&target, Some(&rt), rhs_rendered),
    )
}

/// Emits one FIR function call with Rust math spellings.
///
/// Names arrive normalized by FIR lowering (`sin`, `fmin`, `fabs`,
/// `remainder`, ...). Calls to unknown names are emitted verbatim so FIR
/// helper functions keep working.
fn emit_fun_call(
    store: &FirStore,
    options: &RustOptions,
    name: &str,
    args: &[FirId],
    typ: &FirType,
) -> Result<String, CodegenError> {
    let base = name.strip_prefix("std::").unwrap_or(name);
    // C float math families spell single-precision variants with an `f`
    // suffix (`sinf`, `acoshf`, `fmodf`, ...). The FIR result type already
    // carries the precision, so the suffix is dropped for known names.
    const KNOWN_FLOAT_FNS: &[&str] = &[
        "pow",
        "fmin",
        "fmax",
        "fabs",
        "sin",
        "cos",
        "tan",
        "asin",
        "acos",
        "atan",
        "atan2",
        "sinh",
        "cosh",
        "tanh",
        "asinh",
        "acosh",
        "atanh",
        "exp",
        "log",
        "log2",
        "log10",
        "sqrt",
        "floor",
        "ceil",
        "round",
        "rint",
        "copysign",
        "fmod",
        "remainder",
        "exp10",
    ];
    let base = match base.strip_suffix('f') {
        Some(stem) if KNOWN_FLOAT_FNS.contains(&stem) => stem,
        _ => base,
    };

    if matches!(base, "min_i" | "max_i") {
        let rendered = render_args(store, options, args, None)?;
        let method = if base == "min_i" { "min" } else { "max" };
        return Ok(format!("i32::{method}({})", rendered.join(", ")));
    }

    // C integer math intrinsics keep their C semantics on Rust integer types.
    if matches!(typ, FirType::Int32 | FirType::Int64) {
        let int_type = if matches!(typ, FirType::Int64) {
            "i64"
        } else {
            "i32"
        };
        match base {
            "abs" | "labs" => {
                let rendered = render_args(store, options, args, Some(typ))?;
                return Ok(format!("(({})).wrapping_abs()", rendered[0]));
            }
            "min" | "fmin" => {
                let rendered = render_args(store, options, args, Some(typ))?;
                return Ok(format!("{int_type}::min({})", rendered.join(", ")));
            }
            "max" | "fmax" => {
                let rendered = render_args(store, options, args, Some(typ))?;
                return Ok(format!("{int_type}::max({})", rendered.join(", ")));
            }
            _ => {}
        }
    }

    let float_type = float_fn_type(typ);
    let method = match base {
        "pow" | "powf" => Some("powf"),
        "fmin" | "min" => Some("min"),
        "fmax" | "max" => Some("max"),
        "fabs" | "abs" => Some("abs"),
        "sin" => Some("sin"),
        "cos" => Some("cos"),
        "tan" => Some("tan"),
        "asin" => Some("asin"),
        "acos" => Some("acos"),
        "atan" => Some("atan"),
        "atan2" => Some("atan2"),
        "sinh" => Some("sinh"),
        "cosh" => Some("cosh"),
        "tanh" => Some("tanh"),
        "asinh" => Some("asinh"),
        "acosh" => Some("acosh"),
        "atanh" => Some("atanh"),
        "exp" => Some("exp"),
        "log" => Some("ln"),
        "log2" => Some("log2"),
        "log10" => Some("log10"),
        "sqrt" => Some("sqrt"),
        "floor" => Some("floor"),
        "ceil" => Some("ceil"),
        "round" => Some("round"),
        "rint" => Some("round_ties_even"),
        "copysign" | "copysignf" => Some("copysign"),
        _ => None,
    };
    if let Some(method) = method {
        let rendered = render_args(store, options, args, Some(typ))?;
        return Ok(format!("{float_type}::{method}({})", rendered.join(", ")));
    }
    match base {
        // C classification macros return int; the FIR result type is Int32.
        "isnan" | "isnanf" | "isnanl" => {
            let rendered = render_args(store, options, args, None)?;
            return Ok(format!("((({}).is_nan()) as i32)", rendered[0]));
        }
        "isinf" | "isinff" | "isinfl" => {
            let rendered = render_args(store, options, args, None)?;
            return Ok(format!("((({}).is_infinite()) as i32)", rendered[0]));
        }
        _ => {}
    }
    match base {
        "fmod" => {
            let rendered = render_args(store, options, args, Some(typ))?;
            Ok(format!("(({}) % ({}))", rendered[0], rendered[1]))
        }
        "remainder" => {
            let rendered = render_args(store, options, args, Some(typ))?;
            let suffix = match resolve_float(typ, options) {
                RustRealType::Float32 => "f32",
                RustRealType::Float64 => "f64",
            };
            Ok(format!(
                "remainder_{suffix}({}, {})",
                rendered[0], rendered[1]
            ))
        }
        "exp10" => {
            let rendered = render_args(store, options, args, Some(typ))?;
            Ok(format!(
                "{float_type}::powf(10.0 as {float_type}, {})",
                rendered[0]
            ))
        }
        _ => {
            let rendered = render_args(store, options, args, None)?;
            Ok(format!("{base}({})", rendered.join(", ")))
        }
    }
}

/// Renders call arguments, optionally coercing each to a target type.
fn render_args(
    store: &FirStore,
    options: &RustOptions,
    args: &[FirId],
    coerce_to: Option<&FirType>,
) -> Result<Vec<String>, CodegenError> {
    let mut rendered = Vec::with_capacity(args.len());
    for arg in args {
        let value = emit_value(store, options, *arg)?;
        let value = if let Some(target) = coerce_to {
            coerce_rendered(store, target, *arg, &value)
        } else {
            value
        };
        rendered.push(value);
    }
    Ok(rendered)
}

/// Emits a FIR bit-level reinterpretation via `to_bits`/`from_bits`.
fn emit_bitcast(
    store: &FirStore,
    target: &FirType,
    value: FirId,
    rendered: &str,
) -> Result<String, CodegenError> {
    let source = value_type(store, value);
    match (source.as_ref(), target) {
        (Some(FirType::Float32 | FirType::FaustFloat), FirType::Int32) => {
            Ok(format!("(f32::to_bits({rendered}) as i32)"))
        }
        (Some(FirType::Int32), FirType::Float32 | FirType::FaustFloat) => {
            Ok(format!("f32::from_bits(({rendered}) as u32)"))
        }
        (Some(FirType::Float64), FirType::Int64) => {
            Ok(format!("(f64::to_bits({rendered}) as i64)"))
        }
        (Some(FirType::Int64), FirType::Float64) => {
            Ok(format!("f64::from_bits(({rendered}) as u64)"))
        }
        _ => Err(unsupported_node("bitcast", value, store)),
    }
}

/// Coerces one rendered value to `target` when its FIR type is known to differ.
///
/// This is the workhorse behind C implicit-conversion parity: stores, binary
/// operands, call arguments, and `Select2` branches all funnel through it.
/// Values with unknown or non-numeric FIR types are passed through unchanged.
fn coerce_rendered(store: &FirStore, target: &FirType, value: FirId, rendered: &str) -> String {
    let source = value_type(store, value);
    if source.as_ref() == Some(target) {
        return rendered.to_owned();
    }
    cast_rendered(target, source.as_ref(), rendered)
}

/// Renders a Rust `as` conversion for one already-rendered expression.
///
/// Non-scalar targets (pointers, handles, structs) pass through: FIR pointer
/// values lower to Rust borrows and casting them would break channel aliases.
/// `bool` conversions have no `as` path in Rust, so they lower through `!= 0`
/// / intermediate `i32` casts instead.
fn cast_rendered(target: &FirType, source: Option<&FirType>, rendered: &str) -> String {
    match target {
        FirType::Ptr(_)
        | FirType::Void
        | FirType::Obj
        | FirType::UI
        | FirType::Meta
        | FirType::Sound
        | FirType::Struct(..)
        | FirType::Fun { .. }
        | FirType::Array(..)
        | FirType::Vector(..) => rendered.to_owned(),
        FirType::Bool => match source {
            Some(FirType::Bool) => rendered.to_owned(),
            Some(FirType::Float32 | FirType::Float64 | FirType::FaustFloat | FirType::Quad) => {
                format!("(({rendered}) != 0.0)")
            }
            _ => format!("(({rendered}) != 0)"),
        },
        _ => {
            let target_name = emit_type(target);
            match source {
                Some(FirType::Bool)
                    if matches!(
                        target,
                        FirType::Float32
                            | FirType::Float64
                            | FirType::FaustFloat
                            | FirType::Quad
                            | FirType::FixedPoint
                    ) =>
                {
                    format!("((({rendered}) as i32) as {target_name})")
                }
                _ => format!("(({rendered}) as {target_name})"),
            }
        }
    }
}

/// Returns the FIR result type of one value node, when it carries one.
fn value_type(store: &FirStore, value: FirId) -> Option<FirType> {
    match match_fir(store, value) {
        FirMatch::Int32 { typ, .. }
        | FirMatch::Int64 { typ, .. }
        | FirMatch::Float32 { typ, .. }
        | FirMatch::Float64 { typ, .. }
        | FirMatch::Bool { typ, .. }
        | FirMatch::Quad { typ, .. }
        | FirMatch::FixedPoint { typ, .. }
        | FirMatch::LoadVar { typ, .. }
        | FirMatch::LoadTable { typ, .. }
        | FirMatch::LoadVarAddress { typ, .. }
        | FirMatch::TeeVar { typ, .. }
        | FirMatch::BinOp { typ, .. }
        | FirMatch::Neg { typ, .. }
        | FirMatch::Cast { typ, .. }
        | FirMatch::Bitcast { typ, .. }
        | FirMatch::Select2 { typ, .. }
        | FirMatch::FunCall { typ, .. }
        | FirMatch::NullValue { typ }
        | FirMatch::LoadSoundfileBuffer { typ, .. } => Some(typ),
        FirMatch::LoadSoundfileLength { .. } | FirMatch::LoadSoundfileRate { .. } => {
            Some(FirType::Int32)
        }
        _ => None,
    }
}

/// C usual-arithmetic-conversion rank used for comparison promotion.
fn numeric_rank(typ: &FirType) -> u8 {
    match typ {
        FirType::Bool => 0,
        FirType::Int32 => 1,
        FirType::Int64 => 2,
        FirType::Float32 => 3,
        FirType::FaustFloat => 4,
        FirType::Float64 | FirType::Quad | FirType::FixedPoint => 5,
        _ => 0,
    }
}

/// Returns the concrete Rust float type used for math associated functions.
fn float_fn_type(typ: &FirType) -> &'static str {
    match typ {
        FirType::Float32 => "f32",
        FirType::FaustFloat => "FaustFloat",
        _ => "f64",
    }
}

/// Resolves a FIR float type to the concrete scalar behind it.
fn resolve_float(typ: &FirType, options: &RustOptions) -> RustRealType {
    match typ {
        FirType::Float32 => RustRealType::Float32,
        FirType::FaustFloat => options.faust_float_type,
        _ => RustRealType::Float64,
    }
}

/// Renders a variable reference according to FIR storage class.
///
/// Struct state is addressed through `self.name`; stack, loop, global, static,
/// and function-argument values keep their local textual name in this backend
/// slice.
fn emit_var_ref(name: &str, access: AccessType) -> String {
    match access {
        AccessType::Struct => format!("self.{name}"),
        _ => name.to_owned(),
    }
}

/// Maps FIR types to Rust type spellings.
///
/// The mapping is a backend representation choice, not a type inference pass:
/// all numeric promotion has already happened before FIR reaches codegen.
///
/// `Quad` and `FixedPoint` currently lower to `f64` because the first Rust
/// backend slice has no dedicated runtime aliases for these extended Faust
/// scalar families.
fn emit_type(typ: &FirType) -> String {
    match typ {
        FirType::Int32 => "i32".to_owned(),
        FirType::Int64 => "i64".to_owned(),
        FirType::Float32 => "f32".to_owned(),
        FirType::Float64 => "f64".to_owned(),
        FirType::FaustFloat => "FaustFloat".to_owned(),
        FirType::Quad => "f64".to_owned(),
        FirType::FixedPoint => "f64".to_owned(),
        FirType::Bool => "bool".to_owned(),
        FirType::Void => "()".to_owned(),
        FirType::Obj => "()".to_owned(),
        FirType::Sound => "Soundfile".to_owned(),
        FirType::UI => "dyn UI<FaustFloat>".to_owned(),
        FirType::Meta => "dyn Meta".to_owned(),
        FirType::Ptr(inner) => match inner.as_ref() {
            FirType::FaustFloat | FirType::Float32 | FirType::Float64 => {
                format!("&mut [{}]", emit_type(inner))
            }
            _ => format!("&mut {}", emit_type(inner)),
        },
        FirType::Array(inner, size) => format!("[{}; {size}]", emit_type(inner)),
        FirType::Vector(inner, lanes) => format!("[{}; {lanes}]", emit_type(inner)),
        FirType::Struct(name, _fields) => name.clone(),
        FirType::Fun { ret, .. } => emit_type(ret),
    }
}

/// Returns a neutral value used for uninitialized fields and locals.
fn zero_value(typ: &FirType) -> String {
    match typ {
        FirType::Bool => "false".to_owned(),
        FirType::Int32 => "0".to_owned(),
        FirType::Int64 => "0i64".to_owned(),
        FirType::Float32 => "0.0f32".to_owned(),
        FirType::Float64 | FirType::Quad | FirType::FixedPoint => "0.0f64".to_owned(),
        FirType::FaustFloat => "0.0 as FaustFloat".to_owned(),
        FirType::Array(inner, size) | FirType::Vector(inner, size) => {
            format!("[{}; {size}]", zero_value(inner))
        }
        _ => "Default::default()".to_owned(),
    }
}

/// Emits immutable static FIR tables before the DSP struct.
///
/// Static tables become `static` arrays. Mutable state tables are handled
/// separately by [`emit_struct_fields`] and [`emit_constructor`].
///
/// An absent or malformed static table section is treated as empty. The module
/// root validation already ensures the section ids are present; this helper is
/// permissive because older/minimal FIR fixtures may not populate static
/// declarations yet.
fn emit_static_tables(
    store: &FirStore,
    out: &mut String,
    options: &RustOptions,
    block: FirId,
) -> Result<(), CodegenError> {
    let FirMatch::Block(stmts) = match_fir(store, block) else {
        return Ok(());
    };
    let mut emitted = false;
    for stmt in &stmts {
        if let FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } = match_fir(store, *stmt)
        {
            let mut rendered = Vec::with_capacity(values.len());
            for value in values {
                let v = emit_value(store, options, value)?;
                rendered.push(coerce_rendered(store, &elem_type, value, &v));
            }
            let _ = writeln!(out, "#[allow(non_upper_case_globals, dead_code)]");
            let _ = writeln!(
                out,
                "static {name}: [{}; {}] = [{}];",
                emit_type(&elem_type),
                rendered.len(),
                rendered.join(", ")
            );
            emitted = true;
        }
    }
    if emitted {
        let _ = writeln!(out);
    }
    Ok(())
}

/// Collects explicit scalar initializers from DSP state sections.
///
/// These initializers are replayed in synthesized reset paths when the FIR
/// module does not provide its own canonical `instanceResetUserInterface`
/// body.
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

/// Collects declared state/table types used for store-site coercion.
fn collect_state_types(store: &FirStore, sections: &[FirId]) -> StateTypes {
    let mut out = StateTypes::default();
    for section in sections {
        let FirMatch::Block(items) = match_fir(store, *section) else {
            continue;
        };
        for item in items {
            match match_fir(store, item) {
                FirMatch::DeclareVar { name, typ, .. } => {
                    out.var_types.insert(name, typ);
                }
                FirMatch::DeclareTable {
                    name, elem_type, ..
                } => {
                    out.table_elem_types.insert(name, elem_type);
                }
                _ => {}
            }
        }
    }
    out
}

/// Collects body-bearing function declarations from the module function block.
///
/// Prototype-only declarations are ignored because they are not executable
/// bodies and should not suppress lifecycle fallback generation.
fn collect_module_functions(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<DeclareFunView>, CodegenError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Err(invalid_section("functions section", functions, store));
    };
    let mut out = Vec::new();
    for item in items {
        if let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body: Some(body),
            ..
        } = match_fir(store, item)
        {
            out.push(DeclareFunView {
                name,
                typ,
                named_args: args,
                body: Some(body),
            });
        }
    }
    Ok(out)
}

/// Decodes and validates the FIR module root expected by this backend.
///
/// Returning a [`ModuleView`] keeps the public entry point small and gives all
/// downstream helpers the exact ids for the seven module sections they need.
/// Non-module roots are rejected with `FRS-CGEN-RUST-0001`.
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
/// Used by `instance_constants` emission to avoid writing `fSampleRate` twice
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

/// Builds a stable diagnostic for FIR nodes outside the current Rust slice.
///
/// Unsupported-node diagnostics are preferred over best-effort partial output:
/// the generated Rust source should either represent the complete FIR body or
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

/// Formats a floating literal without a Rust type suffix.
///
/// Rust's `Debug` formatting emits the shortest round-trippable decimal for
/// finite `f64` values and preserves `.0` for integral-looking floats. That is
/// important for small constants such as Faust's noise LCG scale, where fixed
/// decimal truncation is enough to move impulse samples.
fn trim_float(value: f64) -> String {
    if value.is_nan() {
        return "f64::NAN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "f64::NEG_INFINITY".to_owned()
        } else {
            "f64::INFINITY".to_owned()
        };
    }
    let s = format!("{value:?}");
    if s == "-0.0" { "0.0".to_owned() } else { s }
}

/// Escapes a string into a Rust double-quoted string literal.
///
/// The escape set intentionally mirrors the C/C++ text backends: quotes,
/// backslashes, and common control characters are normalized while all other
/// characters are passed through unchanged.
fn rust_string_literal(input: &str) -> String {
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
    use super::{CodegenErrorCode, RustOptions, RustRealType, generate_rust_module};
    use crate::fixtures::{
        build_gain_bias_ui_meta_test_module, build_passthrough_test_module,
        build_sine_phasor_test_module, build_table_state_delay_test_module,
    };
    use fir::{FirBuilder, FirStore};

    #[test]
    fn emits_rust_module_with_dsp_struct_ui_and_compute_loop() {
        let (store, module) = build_sine_phasor_test_module();
        let out = generate_rust_module(&store, module, &RustOptions::default())
            .expect("rust module generation should succeed");

        assert!(out.contains("pub type FaustFloat = f32;"));
        assert!(out.contains("pub struct mydsp {"));
        assert!(out.contains("fFreq: FaustFloat,"));
        assert!(out.contains("fPhase: f64,"));
        assert!(out.contains("pub fn new() -> mydsp {"));
        assert!(out.contains(
            "pub fn build_user_interface(&mut self, ui_interface: &mut dyn UI<FaustFloat>)"
        ));
        assert!(out.contains("ui_interface.add_horizontal_slider(\"freq\", &mut self.fFreq"));
        assert!(out.contains(
            "pub fn compute(&mut self, count: i32, inputs: &[&[FaustFloat]], outputs: &mut [&mut [FaustFloat]])"
        ));
        assert!(out.contains("let mut outputs_iter = outputs.iter_mut();"));
        assert!(
            out.contains("let output0 = outputs_iter.nth(0).expect(\"missing output channel\");")
        );
        assert!(out.contains("for i0 in 0..count {"));
        assert!(out.contains("output0[(i0) as usize] = "));
        assert!(out.contains("f64::sin("));
    }

    #[test]
    fn lifecycle_conformance_matches_cpp_reference_order() {
        // Required by porting/backend-lifecycle-contract-en.md for every
        // source-emitting backend before it can join impulse/golden gates.
        let (store, module) = build_sine_phasor_test_module();
        let out = generate_rust_module(&store, module, &RustOptions::default())
            .expect("rust module generation should succeed");

        // init must call class_init before instance_init.
        let init_i = out
            .find("pub fn init(&mut self, sample_rate: i32) {")
            .expect("init should be emitted");
        let class_init_call_i = out
            .find("Self::class_init(sample_rate);")
            .expect("class_init call should be emitted");
        let instance_init_call_i = out
            .find("self.instance_init(sample_rate);")
            .expect("instance_init call should be emitted");
        assert!(
            init_i < class_init_call_i && class_init_call_i < instance_init_call_i,
            "init should call class_init before instance_init"
        );

        // instance_init must call constants -> resetUI -> clear in order.
        let instance_init_i = out
            .find("pub fn instance_init(&mut self, sample_rate: i32) {")
            .expect("instance_init should be emitted");
        let constants_i = out
            .find("self.instance_constants(sample_rate);")
            .expect("instance_constants call should be emitted");
        let reset_i = out
            .find("self.instance_reset_user_interface();")
            .expect("instance_reset_user_interface call should be emitted");
        let clear_i = out
            .find("self.instance_clear();")
            .expect("instance_clear call should be emitted");
        assert!(
            instance_init_i < constants_i && constants_i < reset_i && reset_i < clear_i,
            "instance_init should call constants -> resetUI -> clear in order"
        );

        // instance_init must not call class_init.
        let instance_init_body_end = out[instance_init_i..]
            .find("\n    }")
            .map(|end| instance_init_i + end)
            .expect("instance_init body should close");
        let instance_init_body = &out[instance_init_i..instance_init_body_end];
        assert!(
            !instance_init_body.contains("class_init"),
            "instance_init must not call class_init"
        );
    }

    #[test]
    fn passthrough_aliases_io_channels_with_disjoint_borrows() {
        let (store, module) = build_passthrough_test_module();
        let out = generate_rust_module(&store, module, &RustOptions::default())
            .expect("rust module generation should succeed");

        assert!(out.contains("let input0 = inputs[0];"));
        assert!(out.contains("let mut outputs_iter = outputs.iter_mut();"));
        assert!(
            out.contains("let output0 = outputs_iter.nth(0).expect(\"missing output channel\");")
        );
        assert!(out.contains("output0[(i0) as usize] = input0[(i0) as usize];"));
    }

    #[test]
    fn integer_state_arithmetic_uses_wrapping_semantics() {
        let (store, module) = build_table_state_delay_test_module();
        let out = generate_rust_module(&store, module, &RustOptions::default())
            .expect("rust module generation should succeed");

        assert!(
            out.contains(".wrapping_add("),
            "integer additions must preserve C two's-complement wrapping"
        );
        assert!(out.contains("fDelay: [FaustFloat; 4],"));
        assert!(out.contains("self.fDelay[(self.fWriteIdx) as usize]"));
    }

    #[test]
    fn emits_ui_and_metadata_nodes_in_distinct_callbacks() {
        let (store, module) = build_gain_bias_ui_meta_test_module();
        let out = generate_rust_module(&store, module, &RustOptions::default())
            .expect("rust module generation should succeed");

        assert!(out.contains("pub fn metadata(&self, m: &mut dyn Meta) {"));
        assert!(out.contains("m.declare(\"name\", \"gain-bias-ui-meta\");"));
        assert!(out.contains("ui_interface.add_check_button(\"gate\", &mut self.fGate);"));
        assert!(out.contains("ui_interface.add_horizontal_bargraph(\"level\", &mut self.fLevel"));
        assert!(out.contains("pub fn instance_reset_user_interface(&mut self) {"));
    }

    #[test]
    fn emits_float64_faustfloat_alias_when_requested() {
        let (store, module) = build_sine_phasor_test_module();
        let options = RustOptions {
            faust_float_type: RustRealType::Float64,
            ..RustOptions::default()
        };
        let out = generate_rust_module(&store, module, &options)
            .expect("rust module generation should succeed");

        assert!(out.contains("pub type FaustFloat = f64;"));
    }

    #[test]
    fn rejects_non_module_root() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let root = b.int32(1);

        let err = generate_rust_module(&store, root, &RustOptions::default())
            .expect_err("non-module roots must fail");
        assert_eq!(err.code(), CodegenErrorCode::RootNotModule);
        assert!(err.to_string().contains("expected FIR module root"));
    }
}
