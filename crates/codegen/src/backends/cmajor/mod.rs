//! Cmajor source backend.
//!
//! This module translates a canonical Faust FIR module into one Cmajor
//! `processor`. Cmajor executes processors one sample at a time, so callers
//! must lower with external control and the one-sample processing API before
//! calling [`generate_cmajor_module`]. The compiler facade owns that
//! policy; this crate validates and renders the FIR it receives.
//!
//! # C++ source provenance
//!
//! The parity reference is Faust C++ commit `8eebea429`:
//!
//! - `compiler/libcode.cpp::compileCmajor` selects external control, internal
//!   real values, and one-sample I/O;
//! - `compiler/generator/cmajor/cmajor_code_container.cpp` assembles the
//!   processor, lifecycle, `control`, and forever-running `main` loop;
//! - `compiler/generator/cmajor/cmajor_instructions.hh` defines statement,
//!   value, stream, array-index, and math spelling;
//! - `compiler/generator/type_manager.hh::CmajorStringTypeManager` defines the
//!   target type names.
//!
//! Mapping status: `adapted`. The generated Cmajor contract follows the C++
//! backend, but Rust consumes canonical FIR instead of the C++ instruction
//! hierarchy and reports typed errors instead of assertions. Lifecycle order
//! deliberately follows `porting/backend-lifecycle-contract-en.md`:
//! `init = classInit -> instanceInit`, while `instanceInit` contains only
//! `instanceConstants -> instanceResetUserInterface -> instanceClear`.
//!
//! # Supported first-gate contract
//!
//! - scalar `float32` and `float64` processors;
//! - one input/output stream per channel;
//! - scalar state, arrays, loops, conditions, delays, and math calls;
//! - input-event UI controls with short names, metadata, and dirty-control
//!   handlers;
//! - output-event bargraphs rate-limited to approximately 50 Hz;
//! - generated lifecycle, separated `control`, and one-sample `main`;
//! - stable rejection of vector types, fixed/quad values, bitcasts,
//!   soundfiles, and malformed FIR.
//!
//! # Table representation adaptation
//!
//! The C++ backend runs `CmajorTableTypeVisitor` and
//! `CmajorTableVisitor` because its generic instruction functions may still
//! carry placeholder table types. Canonical Rust FIR already co-locates the
//! concrete element type and length in [`FirType::Array`] and
//! [`FirMatch::DeclareTable`]. The emitter therefore renders those owned types
//! directly: no name-indexed specialization side table is needed. Integration
//! tests cover read-only, writable, waveform, and generated tables at multiple
//! sizes, plus repeated generation to guard request-local determinism.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::backends::codegen_error::{BackendError, CodegenErrorCode as BackendErrorCode};
use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

use super::textual::{OperandSide, c_like_fir_operator, infix_operand_needs_parentheses};

/// Stable backend identifier used by CLI, diagnostics, and capability tables.
pub const BACKEND_NAME: &str = "cmajor";

/// Cmajor scalar precision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CmajorRealType {
    /// Cmajor `float32`, matching Faust `-single`.
    #[default]
    Float32,
    /// Cmajor `float64`, matching Faust `-double`.
    Float64,
}

impl CmajorRealType {
    /// Returns the Cmajor scalar type spelling.
    #[must_use]
    pub const fn cmajor_name(self) -> &'static str {
        match self {
            Self::Float32 => "float32",
            Self::Float64 => "float64",
        }
    }
}

/// Options controlling one Cmajor source artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CmajorOptions {
    /// Public Cmajor processor name selected by Faust `-cn`.
    pub class_name: String,
    /// Scalar precision used for `FAUSTFLOAT` and generated real values.
    pub real_type: CmajorRealType,
    /// Compilation options string printed in the generated-file header.
    ///
    /// `None` falls back to a minimal `-lang cmajor` line derived from
    /// [`Self::real_type`], for callers (mostly tests) that do not thread the
    /// real CLI flags through.
    pub compile_options: Option<String>,
}

impl Default for CmajorOptions {
    fn default() -> Self {
        Self {
            class_name: "mydsp".to_owned(),
            real_type: CmajorRealType::Float32,
            compile_options: None,
        }
    }
}

/// Stable machine-readable Cmajor code-generation error classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenErrorCode {
    /// The supplied root does not decode as a FIR module.
    RootNotModule,
    /// The FIR contains a construct Cmajor cannot express.
    Unsupported,
    /// A processor or generated endpoint name is not a Cmajor identifier.
    InvalidIdentifier,
    /// Required module sections or DSP entry points are inconsistent.
    InvalidStructure,
    /// A concrete table signature cannot be derived safely.
    TableSpecialization,
}

impl CodegenErrorCode {
    /// Returns the stable diagnostic code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RootNotModule => "FRS-CGEN-CMAJ-0001",
            Self::Unsupported => "FRS-CGEN-CMAJ-0002",
            Self::InvalidIdentifier => "FRS-CGEN-CMAJ-0003",
            Self::InvalidStructure => "FRS-CGEN-CMAJ-0004",
            Self::TableSpecialization => "FRS-CGEN-CMAJ-0005",
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

/// Decoded FIR module sections needed by the Cmajor container.
struct ModuleView {
    dsp_struct: FirId,
    globals: FirId,
    functions: FirId,
    static_decls: FirId,
    sub_modules: FirId,
    num_inputs: usize,
    num_outputs: usize,
}

/// Borrowed helper-function declaration passed through the emitter as one
/// semantic unit.
struct FunctionView<'a> {
    name: &'a str,
    typ: &'a FirType,
    args: &'a [NamedType],
    body: Option<FirId>,
}

/// One UI endpoint reconstructed from `buildUserInterface`.
///
/// The endpoint owns all strings and metadata so naming remains request-local;
/// unlike the C++ backend, this emitter has no `gGlobal` fresh-name state.
struct UiWidget {
    kind: UiWidgetKind,
    label: String,
    short_name: String,
    group: String,
    zone: String,
    metadata: Vec<(String, String)>,
}

/// Cmajor-relevant widget payload.
enum UiWidgetKind {
    Button(fir::ButtonType),
    Slider {
        init: f64,
        lo: f64,
        hi: f64,
        step: f64,
    },
    Bargraph {
        lo: f64,
        hi: f64,
    },
}

/// Fully resolved UI plan shared by declarations, handlers, and scheduling.
struct UiPlan {
    widgets: Vec<UiWidget>,
    bargraph_zones: BTreeSet<String>,
}

impl UiPlan {
    fn has_bargraphs(&self) -> bool {
        !self.bargraph_zones.is_empty()
    }
}

/// Context controlling declaration and statement spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmitContext {
    /// Persistent processor fields.
    Field,
    /// Lifecycle initialization body, where Cmajor forbids endpoint access.
    Init,
    /// Local function body.
    Body,
}

/// Returns the stable Cmajor backend identifier.
#[must_use]
pub const fn backend_id() -> &'static str {
    BACKEND_NAME
}

/// Generates a complete Cmajor processor from a canonical FIR module.
///
/// The caller must provide FIR lowered for external control and one-sample
/// processing. This function remains side-effect free: it does not invoke the
/// Cmajor SDK and stores no request-global naming or table state.
///
/// # Errors
///
/// - [`CodegenErrorCode::RootNotModule`] if `module` is not a FIR module;
/// - [`CodegenErrorCode::InvalidIdentifier`] for an invalid processor name;
/// - [`CodegenErrorCode::InvalidStructure`] when the one-sample `frame` entry
///   point is missing;
/// - [`CodegenErrorCode::Unsupported`] for a FIR node or type outside the
///   scalar Cmajor contract.
pub fn generate_cmajor_module(
    store: &FirStore,
    module: FirId,
    options: &CmajorOptions,
) -> Result<String, CodegenError> {
    validate_identifier(&options.class_name, "processor")?;
    // Cmajor has no shared static storage, so a generated table is a processor
    // field — the adaptation this backend's own plan documents at §4.5. The
    // sub-module itself stays nested: Cmajor has structs and functions taking
    // an explicit receiver, so the reference emits the generator as a struct.
    let promoted = fir::subcontainer::has_sub_modules(store, module)
        .then(|| {
            let mut owned = FirStore::new();
            let root = owned.import_from(store, module);
            fir::subcontainer::promote_static_tables_to_struct(&mut owned, root)
                .and_then(|root| {
                    // Cmajor addresses struct members only through the
                    // receiver: a bare `fConst0` inside `void f (Sub& this, …)`
                    // is "Cannot find symbol". Rewriting the references once
                    // here keeps the emitter free of any notion of where it is.
                    fir::subcontainer::qualify_sub_module_bodies(&mut owned, root, "this")
                })
                .map(|root| (owned, root))
                .map_err(|err| {
                    CodegenError::new(
                        CodegenErrorCode::Unsupported,
                        format!("promoting generated tables failed: {err}"),
                    )
                })
        })
        .transpose()?;
    let (store, module) = promoted.as_ref().map_or((store, module), |(s, m)| (s, *m));

    let view = decode_module(store, module)?;
    if find_function_body(store, view.functions, "frame").is_none()
        && find_function_body(store, view.functions, "compute").is_none()
    {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidStructure,
            "Cmajor requires a one-sample `frame` entry point",
        ));
    }
    let ui = collect_ui(store, view.functions)?;

    let mut out = String::new();
    let _ = writeln!(out, "/* Code generated with faust-rs {}", crate::VERSION);
    let _ = writeln!(
        out,
        "   Compilation options: {} */",
        options
            .compile_options
            .as_deref()
            .unwrap_or(match options.real_type {
                CmajorRealType::Float32 => "-lang cmajor -single",
                CmajorRealType::Float64 => "-lang cmajor -double",
            })
    );
    let _ = writeln!(out, "namespace faust");
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "\tprocessor {}", options.class_name);
    let _ = writeln!(out, "\t{{");

    emit_streams(&mut out, &view, options);
    emit_ui_declarations(&mut out, &ui, options)?;
    emit_field_block(store, &mut out, view.dsp_struct, options, 2)?;
    emit_field_block(store, &mut out, view.static_decls, options, 2)?;
    emit_field_block(store, &mut out, view.globals, options, 2)?;
    let table_sizes = collect_table_sizes(store, &[view.functions, view.sub_modules]);
    emit_sub_modules(store, &mut out, options, view.sub_modules, &table_sizes)?;
    let _ = writeln!(out, "\t\tbool fUpdated;");
    if ui.has_bargraphs() {
        let _ = writeln!(out, "\t\tint fControlSlice;");
    }
    let _ = writeln!(out);

    emit_ui_handlers(&mut out, &ui, options);
    emit_non_api_functions(store, &mut out, &view, options)?;
    emit_math_helpers(&mut out, options);
    emit_arity_helpers(&mut out, &view);
    emit_lifecycle(store, &mut out, &view, options, ui.has_bargraphs())?;
    emit_control(store, &mut out, &view, options)?;
    emit_main(store, &mut out, &view, options, ui.has_bargraphs())?;

    let _ = writeln!(out, "\t}}");
    let _ = writeln!(out, "}}");
    Ok(out)
}

/// Emits every generated-table sub-container as a nested struct with its
/// functions.
///
/// Cmajor has structs and functions taking an explicit receiver, so the
/// reference emits the generator as a `struct` plus `void f (Sub& this, …)`
/// functions rather than inlining it.
///
/// Two Cmajor-specific shapes, both from this backend's plan §4.5: array
/// parameters carry their concrete length, so the fill function's name is
/// suffixed with the table size (`fillmydspSIG0_65536`) to keep one name per
/// instantiated length; and `new<Sub>` returns a value while `delete<Sub>` is
/// empty, the language having no manual deallocation.
fn emit_sub_modules(
    store: &FirStore,
    out: &mut String,
    options: &CmajorOptions,
    sub_modules: FirId,
    table_sizes: &BTreeMap<String, usize>,
) -> Result<(), CodegenError> {
    let FirMatch::Block(items) = match_fir(store, sub_modules) else {
        return Ok(());
    };
    for item in items {
        let FirMatch::SubModule {
            name,
            elem_type,
            dsp_struct,
            static_decls,
            globals,
            functions,
            sub_modules: nested,
        } = match_fir(store, item)
        else {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidStructure,
                format!("sub_modules holds a non-SubModule node {}", item.as_u32()),
            ));
        };

        emit_sub_modules(store, out, options, nested, table_sizes)?;

        // A generator's own constants — chiefly the `const int32[N] …Wave0`
        // arrays a `waveform` generator reads from — are shared, not per
        // instance, so the reference hoists them to processor scope alongside
        // the struct rather than making them fields. Dropping them left the
        // fill body referencing an undeclared symbol.
        emit_field_block(store, out, static_decls, options, 2)?;
        emit_field_block(store, out, globals, options, 2)?;

        let _ = writeln!(out, "\t\tstruct {name}");
        let _ = writeln!(out, "\t\t{{");
        emit_field_block(store, out, dsp_struct, options, 3)?;
        let _ = writeln!(out, "\t\t}}");
        let _ = writeln!(out);

        // No known size means no fill call site was found for this generator.
        // Emitting `elem[0]& table` would produce a definition that no call
        // site matches, so fail loudly instead.
        let size = table_sizes.get(&name).copied().ok_or_else(|| {
            CodegenError::new(
                CodegenErrorCode::InvalidStructure,
                format!("no fill call site found for sub-module `{name}`, cannot type its table"),
            )
        })?;
        let elem = emit_type(&elem_type, options)?;
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
                let emitted_name = if fun_name.starts_with("fill") {
                    format!("{fun_name}_{size}")
                } else {
                    fun_name.clone()
                };
                let mut params = vec![format!("{name}& this")];
                for arg in args.iter().skip(1) {
                    params.push(match &arg.typ {
                        FirType::Ptr(_) => format!("{elem}[{size}]& {}", arg.name),
                        other => format!("{} {}", emit_type(other, options)?, arg.name),
                    });
                }
                let _ = writeln!(out, "\t\tvoid {emitted_name} ({})", params.join(", "));
                let _ = writeln!(out, "\t\t{{");
                emit_block_items(store, out, options, body, 3, EmitContext::Init)?;
                let _ = writeln!(out, "\t\t}}");
                let _ = writeln!(out);
            }
        }
        let _ = writeln!(out, "\t\t{name} new{name}() {{ {name} obj; return obj; }}");
        let _ = writeln!(out, "\t\tvoid delete{name} ({name}& this) {{}}");
        let _ = writeln!(out);
    }
    Ok(())
}

/// Maps each sub-module to the length of the table it fills.
///
/// Cmajor array parameters are typed by length, so the fill signature needs the
/// concrete size. The FIR keeps the generator length-agnostic — exactly as the
/// C++ `fill(count, table)` does — so the size is read back from the call site.
///
/// The scan covers the whole module, not just `staticInit`: a read-only table
/// is filled from `staticInit`, but a writable one (`rwtable`) is filled from
/// `instanceConstants`, and a nested generator's fill call sits inside its
/// parent sub-module's `instanceInit`. Scanning only `staticInit` left every
/// `rwtable` generator with no known size.
fn collect_table_sizes(store: &FirStore, roots: &[FirId]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<FirId> = roots.to_vec();
    while let Some(id) = stack.pop() {
        if let FirMatch::FunCall { name, args, .. } = match_fir(store, id)
            && let Some(sub) = name.strip_prefix("fill")
            && let Some(count) = args.get(1)
            && let FirMatch::Int32 { value, .. } = match_fir(store, *count)
            && let Ok(size) = usize::try_from(value)
        {
            out.insert(sub.to_owned(), size);
        }
        stack.extend(fir::fir_match_children(store, id));
    }
    out
}

/// Emits Cmajor streams in the ordering of the pinned C++ `sortIO` reference.
fn emit_streams(out: &mut String, view: &ModuleView, options: &CmajorOptions) {
    let real = options.real_type.cmajor_name();
    for channel in (0..view.num_outputs).rev() {
        let _ = writeln!(out, "\t\toutput stream {real} output{channel};");
    }
    for channel in (0..view.num_inputs).rev() {
        let _ = writeln!(out, "\t\tinput stream {real} input{channel};");
    }
}

/// Reconstructs endpoint names and annotations from `buildUserInterface`.
///
/// C++ parity: the first `ShortnameInstVisitor` pass observes every address,
/// then the second pass emits endpoints. Rust materializes that two-pass state
/// explicitly. Repeated display addresses are legal and receive the same short
/// name; endpoint uniqueness is enforced independently from each widget zone.
fn collect_ui(store: &FirStore, functions: FirId) -> Result<UiPlan, CodegenError> {
    let Some(body) = find_function_body(store, functions, "buildUserInterface") else {
        return Ok(UiPlan {
            widgets: Vec::new(),
            bargraph_zones: BTreeSet::new(),
        });
    };

    let mut path = Vec::new();
    let mut pending_metadata = Vec::new();
    let mut collected = Vec::new();
    collect_ui_in(
        store,
        body,
        &mut path,
        &mut pending_metadata,
        &mut collected,
    )?;

    let addresses: Vec<String> = collected
        .iter()
        .map(|(address, _)| address.clone())
        .collect();
    let short_names = crate::shortname::compute_short_names(&addresses);
    let mut bargraph_zones = BTreeSet::new();
    let mut endpoint_names = BTreeSet::new();
    let mut widgets = Vec::with_capacity(collected.len());
    for (address, mut widget) in collected {
        widget.short_name = short_names
            .get(&address)
            .cloned()
            .unwrap_or_else(|| build_label(&widget.label));
        let endpoint = format!("event{}", widget.zone);
        validate_identifier(&endpoint, "UI endpoint")?;
        if !endpoint_names.insert(endpoint.clone()) {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidStructure,
                format!("two UI widgets emit the endpoint `{endpoint}`"),
            ));
        }
        if matches!(widget.kind, UiWidgetKind::Bargraph { .. }) {
            bargraph_zones.insert(widget.zone.clone());
        }
        widgets.push(widget);
    }
    Ok(UiPlan {
        widgets,
        bargraph_zones,
    })
}

/// Recursive UI collector retaining C++'s sequential metadata semantics.
fn collect_ui_in(
    store: &FirStore,
    block: FirId,
    path: &mut Vec<String>,
    pending_metadata: &mut Vec<(String, String)>,
    out: &mut Vec<(String, UiWidget)>,
) -> Result<(), CodegenError> {
    for stmt in block_items(store, block) {
        match match_fir(store, stmt) {
            FirMatch::OpenBox { label, .. } => {
                path.push(label);
                pending_metadata.clear();
            }
            FirMatch::CloseBox => {
                path.pop();
                pending_metadata.clear();
            }
            FirMatch::AddMetaDeclare { key, value, .. } => {
                pending_metadata.push((key, value));
            }
            FirMatch::AddButton { typ, label, var } => push_ui_widget(
                path,
                pending_metadata,
                out,
                label,
                var,
                UiWidgetKind::Button(typ),
            ),
            FirMatch::AddSlider {
                label,
                var,
                init,
                lo,
                hi,
                step,
                ..
            } => push_ui_widget(
                path,
                pending_metadata,
                out,
                label,
                var,
                UiWidgetKind::Slider { init, lo, hi, step },
            ),
            FirMatch::AddBargraph {
                label, var, lo, hi, ..
            } => push_ui_widget(
                path,
                pending_metadata,
                out,
                label,
                var,
                UiWidgetKind::Bargraph { lo, hi },
            ),
            FirMatch::AddSoundfile { label, .. } => {
                return Err(CodegenError::new(
                    CodegenErrorCode::Unsupported,
                    format!("soundfile `{label}` is not supported by Cmajor"),
                ));
            }
            FirMatch::Block(_) => {
                collect_ui_in(store, stmt, path, pending_metadata, out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Finalizes one UI record and consumes metadata attached to it.
fn push_ui_widget(
    path: &[String],
    pending_metadata: &mut Vec<(String, String)>,
    out: &mut Vec<(String, UiWidget)>,
    label: String,
    zone: String,
    kind: UiWidgetKind,
) {
    let address = ui_path(path, &label);
    out.push((
        address.clone(),
        UiWidget {
            kind,
            label,
            short_name: String::new(),
            group: address,
            zone,
            metadata: std::mem::take(pending_metadata),
        },
    ));
}

/// Emits public Cmajor event endpoints with host-visible UI annotations.
fn emit_ui_declarations(
    out: &mut String,
    ui: &UiPlan,
    options: &CmajorOptions,
) -> Result<(), CodegenError> {
    let real = options.real_type.cmajor_name();
    let mut metadata_ids: BTreeMap<&str, usize> = BTreeMap::new();
    for widget in &ui.widgets {
        let direction = if matches!(widget.kind, UiWidgetKind::Bargraph { .. }) {
            "output"
        } else {
            "input"
        };
        let endpoint = format!("event{}", widget.zone);
        validate_identifier(&endpoint, "UI endpoint")?;
        let display_name = match widget.kind {
            UiWidgetKind::Slider { .. } => &widget.label,
            UiWidgetKind::Button(_) | UiWidgetKind::Bargraph { .. } => &widget.short_name,
        };
        let _ = write!(
            out,
            "\t\t{direction} event {real} {endpoint} [[ name: {}, group: {}",
            cmajor_string(display_name),
            cmajor_string(&build_ui_path(&widget.group))
        );
        match widget.kind {
            UiWidgetKind::Button(typ) => {
                if typ == fir::ButtonType::Checkbox {
                    let _ = write!(out, ", latching");
                }
                let _ = write!(out, ", text: \"off|on\", boolean");
            }
            UiWidgetKind::Slider { init, lo, hi, step } => {
                let _ = write!(
                    out,
                    ", min: {}, max: {}, init: {}, step: {}",
                    float_literal(lo, options.real_type),
                    float_literal(hi, options.real_type),
                    float_literal(init, options.real_type),
                    float_literal(step, options.real_type)
                );
            }
            UiWidgetKind::Bargraph { lo, hi } => {
                let _ = write!(
                    out,
                    ", min: {}, max: {}",
                    float_literal(lo, options.real_type),
                    float_literal(hi, options.real_type)
                );
            }
        }
        for (key, value) in &widget.metadata {
            if key.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                continue;
            }
            let next = metadata_ids.entry(key).or_default();
            let metadata_name = format!("meta_{key}{next}");
            *next += 1;
            validate_identifier(&metadata_name, "Cmajor metadata annotation")?;
            let _ = write!(out, ", {metadata_name}: {}", cmajor_string(value));
        }
        let _ = writeln!(out, " ]];");
    }
    if !ui.widgets.is_empty() {
        let _ = writeln!(out);
    }
    Ok(())
}

/// Emits input-event handlers; output bargraphs need declarations only.
fn emit_ui_handlers(out: &mut String, ui: &UiPlan, options: &CmajorOptions) {
    let real = options.real_type.cmajor_name();
    for widget in &ui.widgets {
        if matches!(widget.kind, UiWidgetKind::Bargraph { .. }) {
            continue;
        }
        let _ = writeln!(out, "\t\t// {}", widget.short_name);
        let _ = writeln!(
            out,
            "\t\tevent event{}({real} val) {{ fUpdated ||= ({} != val); {} = val; }}",
            widget.zone, widget.zone, widget.zone
        );
    }
    if ui
        .widgets
        .iter()
        .any(|widget| !matches!(widget.kind, UiWidgetKind::Bargraph { .. }))
    {
        let _ = writeln!(out);
    }
}

/// Builds the full UI path used by the short-name pass.
fn ui_path(path: &[String], label: &str) -> String {
    let mut result = String::new();
    for segment in path {
        result.push('/');
        result.push_str(segment);
    }
    result.push('/');
    result.push_str(label);
    result
}

/// Applies the Cmajor backend's narrower path-label normalization.
fn build_ui_path(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(build_label)
        .fold(String::new(), |mut result, part| {
            result.push('/');
            result.push_str(&part);
            result
        })
}

/// C++ `buildLabel`: replaces punctuation Cmajor treats specially.
fn build_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| match ch {
            ' ' | '(' | ')' | '\\' | '/' | '.' | '-' => '_',
            other => other,
        })
        .collect()
}

/// Quotes a Faust label or metadata value as a Cmajor string literal.
fn cmajor_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

/// Emits persistent declarations from one FIR module section.
fn emit_field_block(
    store: &FirStore,
    out: &mut String,
    block: FirId,
    options: &CmajorOptions,
    indent: usize,
) -> Result<(), CodegenError> {
    for stmt in block_items(store, block) {
        match match_fir(store, stmt) {
            FirMatch::DeclareVar { ref name, .. }
                if name.starts_with("input") || name.starts_with("output") => {}
            FirMatch::DeclareVar { .. }
            | FirMatch::DeclareTable { .. }
            | FirMatch::DeclareStructType { .. } => {
                emit_stmt(store, out, options, stmt, indent, EmitContext::Field)?;
            }
            FirMatch::DeclareFun { .. }
            | FirMatch::Label(_)
            | FirMatch::NullStatement
            | FirMatch::Block(_) => {}
            other => {
                return Err(CodegenError::new(
                    CodegenErrorCode::InvalidStructure,
                    format!("unexpected processor-field node {other:?}"),
                ));
            }
        }
    }
    Ok(())
}

/// Emits helper functions that are not reshaped DSP API entry points.
fn emit_non_api_functions(
    store: &FirStore,
    out: &mut String,
    view: &ModuleView,
    options: &CmajorOptions,
) -> Result<(), CodegenError> {
    for section in [view.globals, view.functions] {
        for stmt in block_items(store, section) {
            let FirMatch::DeclareFun {
                name,
                typ,
                args,
                body,
                ..
            } = match_fir(store, stmt)
            else {
                continue;
            };
            // `is_dsp_api_method` covers this backend's own entry points;
            // `is_lifecycle_function` covers the shared list, which is where
            // `staticInit` lives — it is rendered as the `classInit` body, so
            // emitting it here too produced a second, never-called copy.
            if is_dsp_api_method(&name) || crate::backends::is_lifecycle_function(&name) {
                continue;
            }
            emit_function(
                store,
                out,
                options,
                FunctionView {
                    name: &name,
                    typ: &typ,
                    args: &args,
                    body,
                },
                2,
            )?;
        }
    }
    Ok(())
}

/// Emits the two Cmajor helpers missing from the target math library contract.
fn emit_math_helpers(out: &mut String, options: &CmajorOptions) {
    match options.real_type {
        CmajorRealType::Float32 => {
            let _ = writeln!(
                out,
                "\t\tfloat32 copysign(float32 x, float32 y) {{ return abs(x) * ((y < 0.0f) ? -1.0f : 1.0f); }}"
            );
            let _ = writeln!(
                out,
                "\t\tfloat32 round(float32 x) {{ return float32(roundToInt(x)); }}"
            );
        }
        CmajorRealType::Float64 => {
            let _ = writeln!(
                out,
                "\t\tfloat64 copysign(float64 x, float64 y) {{ return abs(x) * ((y < 0.0) ? -1.0 : 1.0); }}"
            );
            let _ = writeln!(
                out,
                "\t\tfloat64 round(float64 x) {{ return float64(roundToInt(x)); }}"
            );
        }
    }
    let _ = writeln!(out);
}

/// Emits the public arity helpers, including the upstream spelling typo.
fn emit_arity_helpers(out: &mut String, view: &ModuleView) {
    let _ = writeln!(
        out,
        "\t\tint getNumInputs() {{ return {}; }}",
        view.num_inputs
    );
    let _ = writeln!(
        out,
        "\t\tint getNumOuputs() {{ return {}; }}",
        view.num_outputs
    );
    let _ = writeln!(out);
}

/// Emits lifecycle methods with the repository-wide authoritative ordering.
fn emit_lifecycle(
    store: &FirStore,
    out: &mut String,
    view: &ModuleView,
    options: &CmajorOptions,
    has_bargraphs: bool,
) -> Result<(), CodegenError> {
    emit_named_body(
        store,
        out,
        options,
        view.functions,
        // `staticInit` is the FIR name for what the DSP API calls `classInit`;
        // Cmajor calls it from `instanceInit` because its tables are
        // per-processor (§4.6 of the cmajor plan).
        "staticInit",
        "void classInit(int sample_rate)",
        false,
    )?;
    emit_named_body(
        store,
        out,
        options,
        view.functions,
        "instanceConstants",
        "void instanceConstants(int sample_rate)",
        false,
    )?;
    emit_named_body(
        store,
        out,
        options,
        view.functions,
        "instanceResetUserInterface",
        "void instanceResetUserInterface()",
        true,
    )?;
    emit_named_body(
        store,
        out,
        options,
        view.functions,
        "instanceClear",
        "void instanceClear()",
        false,
    )?;

    let _ = writeln!(out, "\t\tvoid instanceInit(int sample_rate)");
    let _ = writeln!(out, "\t\t{{");
    let _ = writeln!(out, "\t\t\tinstanceConstants(sample_rate);");
    let _ = writeln!(out, "\t\t\tinstanceResetUserInterface();");
    let _ = writeln!(out, "\t\t\tinstanceClear();");
    let _ = writeln!(out, "\t\t}}");
    let _ = writeln!(out);

    let _ = writeln!(out, "\t\tvoid init()");
    let _ = writeln!(out, "\t\t{{");
    let _ = writeln!(out, "\t\t\tlet sample_rate = int(processor.frequency);");
    if has_bargraphs {
        let _ = writeln!(out, "\t\t\tfControlSlice = int(processor.frequency) / 50;");
    }
    let _ = writeln!(out, "\t\t\tclassInit(sample_rate);");
    let _ = writeln!(out, "\t\t\tinstanceInit(sample_rate);");
    let _ = writeln!(out, "\t\t}}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits one lifecycle method from its canonical FIR body or an empty fallback.
fn emit_named_body(
    store: &FirStore,
    out: &mut String,
    options: &CmajorOptions,
    functions: FirId,
    name: &str,
    signature: &str,
    mark_control_dirty: bool,
) -> Result<(), CodegenError> {
    let _ = writeln!(out, "\t\t{signature}");
    let _ = writeln!(out, "\t\t{{");
    if mark_control_dirty {
        let _ = writeln!(out, "\t\t\tfUpdated = true;");
    }
    if let Some(body) = find_function_body(store, functions, name) {
        emit_block_items(store, out, options, body, 3, EmitContext::Init)?;
    }
    let _ = writeln!(out, "\t\t}}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits the separated control-rate function.
fn emit_control(
    store: &FirStore,
    out: &mut String,
    view: &ModuleView,
    options: &CmajorOptions,
) -> Result<(), CodegenError> {
    let _ = writeln!(out, "\t\tvoid control()");
    let _ = writeln!(out, "\t\t{{");
    if let Some(body) = find_function_body(store, view.functions, "control") {
        emit_block_items(store, out, options, body, 3, EmitContext::Body)?;
    }
    let _ = writeln!(out, "\t\t}}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits Cmajor's forever-running one-sample processor entry point.
fn emit_main(
    store: &FirStore,
    out: &mut String,
    view: &ModuleView,
    options: &CmajorOptions,
    has_bargraphs: bool,
) -> Result<(), CodegenError> {
    let body = find_function_body(store, view.functions, "frame")
        .or_else(|| find_function_body(store, view.functions, "compute"))
        .ok_or_else(|| {
            CodegenError::new(
                CodegenErrorCode::InvalidStructure,
                "Cmajor one-sample body is missing",
            )
        })?;
    let _ = writeln!(out, "\t\tvoid main()");
    let _ = writeln!(out, "\t\t{{");
    let _ = writeln!(out, "\t\t\tloop");
    let _ = writeln!(out, "\t\t\t{{");
    let _ = writeln!(
        out,
        "\t\t\t\tif (fUpdated) {{ fUpdated = false; control(); }}"
    );
    emit_block_items(store, out, options, body, 4, EmitContext::Body)?;
    if has_bargraphs {
        let _ = writeln!(
            out,
            "\t\t\t\tif (fControlSlice-- == 0) {{ fControlSlice = int(processor.frequency) / 50; }}"
        );
    }
    let _ = writeln!(out, "\t\t\t\tadvance();");
    let _ = writeln!(out, "\t\t\t}}");
    let _ = writeln!(out, "\t\t}}");
    Ok(())
}

/// Emits all statements held by a FIR block.
fn emit_block_items(
    store: &FirStore,
    out: &mut String,
    options: &CmajorOptions,
    block: FirId,
    indent: usize,
    context: EmitContext,
) -> Result<(), CodegenError> {
    match match_fir(store, block) {
        FirMatch::Block(items) => {
            for stmt in items {
                emit_stmt(store, out, options, stmt, indent, context)?;
            }
            Ok(())
        }
        _ => emit_stmt(store, out, options, block, indent, context),
    }
}

/// Returns the `const ` storage qualifier for a `DeclareVar`/`DeclareTable`
/// with `AccessType::Static`, or `""` otherwise. Shared by the `DeclareVar`
/// and `DeclareTable` arms of `emit_stmt`.
///
/// In [`EmitContext::Field`] position specifically, `Static` must not be
/// translated to `const`: Cmajor has no class-static storage, so the C++
/// backend emits these declarations as ordinary per-processor-instance
/// fields that may still be initialized at runtime.
fn static_qualifier(context: EmitContext, access: AccessType) -> &'static str {
    if context != EmitContext::Field && access == AccessType::Static {
        "const "
    } else {
        ""
    }
}

/// Emits one Cmajor statement.
fn emit_stmt(
    store: &FirStore,
    out: &mut String,
    options: &CmajorOptions,
    stmt: FirId,
    indent: usize,
    context: EmitContext,
) -> Result<(), CodegenError> {
    let tab = "\t".repeat(indent);
    match match_fir(store, stmt) {
        FirMatch::Label(label) => {
            let _ = writeln!(out, "{tab}// {label}");
            Ok(())
        }
        FirMatch::NullStatement => Ok(()),
        FirMatch::DeclareVar {
            name,
            typ,
            access,
            init,
        } => {
            if name.starts_with("input") || name.starts_with("output") {
                return Ok(());
            }
            validate_identifier(&name, "variable")?;
            // A sub-container handle is a value of the generator struct, not a
            // runtime pointer: Cmajor has no `Obj` handle type, and its
            // functions take `Sub& this`.
            let type_name = match init.map(|id| match_fir(store, id)) {
                Some(FirMatch::NewDsp { name: sub, .. }) => sub,
                _ => emit_type(&typ, options)?,
            };
            let qualifier = static_qualifier(context, access);
            let _ = write!(out, "{tab}{qualifier}{type_name} {name}");
            if let Some(init) = init {
                let init = emit_value(store, options, init)?;
                let _ = write!(out, " = {init}");
            }
            let _ = writeln!(out, ";");
            Ok(())
        }
        FirMatch::DeclareTable {
            name,
            access,
            elem_type,
            values,
        } => {
            validate_identifier(&name, "table")?;
            let elem = emit_type(&elem_type, options)?;
            let qualifier = static_qualifier(context, access);
            let _ = write!(out, "{tab}{qualifier}{elem}[{}] {name}", values.len());
            if !values.is_empty() {
                let mut rendered = Vec::with_capacity(values.len());
                for value in values {
                    rendered.push(emit_value(store, options, value)?);
                }
                let _ = write!(out, " = ({})", rendered.join(", "));
            }
            let _ = writeln!(out, ";");
            Ok(())
        }
        FirMatch::DeclareStructType { typ } => emit_struct_type(out, options, &typ, indent),
        FirMatch::StoreVar { name, value, .. } => {
            let value = emit_value(store, options, value)?;
            if let Some(channel) = io_channel(&name, "output") {
                let _ = writeln!(out, "{tab}output{channel} <- {value};");
            } else if is_bargraph_zone(&name) && context != EmitContext::Init {
                let _ = writeln!(out, "{tab}{name} = {value};");
                let _ = writeln!(
                    out,
                    "{tab}if (fControlSlice == 0) {{ event{name} <- {name}; }}"
                );
            } else {
                let _ = writeln!(out, "{tab}{name} = {value};");
            }
            Ok(())
        }
        FirMatch::StoreTable {
            name, index, value, ..
        } => {
            let value = emit_value(store, options, value)?;
            if name == "outputs" {
                let channel = constant_index(store, index).ok_or_else(|| {
                    CodegenError::new(
                        CodegenErrorCode::InvalidStructure,
                        "one-sample Cmajor output channel must be constant",
                    )
                })?;
                let _ = writeln!(out, "{tab}output{channel} <- {value};");
            } else {
                let target = emit_indexed(store, options, &name, index)?;
                let _ = writeln!(out, "{tab}{target} = {value};");
            }
            Ok(())
        }
        FirMatch::ShiftArrayVar { name, delay, .. } => {
            let counter = format!("shift_{}", sanitize_identifier(&name));
            let _ = writeln!(
                out,
                "{tab}for (int32 {counter} = {delay}; {counter} > 0; {counter} = {counter} - 1) {{"
            );
            let _ = writeln!(
                out,
                "{tab}\t{name}.at({counter}) = {name}.at({counter} - 1);"
            );
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Drop(value) => {
            if matches!(match_fir(store, value), FirMatch::NullValue { .. }) {
                return Ok(());
            }
            let value = emit_value(store, options, value)?;
            let _ = writeln!(out, "{tab}{value};");
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
        FirMatch::Block(_) => emit_block_items(store, out, options, stmt, indent, context),
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let cond = emit_condition(store, options, cond)?;
            let _ = writeln!(out, "{tab}if {cond} {{");
            emit_block_items(store, out, options, then_block, indent + 1, context)?;
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{tab}}} else {{");
                emit_block_items(store, out, options, else_block, indent + 1, context)?;
            }
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Control { cond, stmt } => {
            let cond = emit_condition(store, options, cond)?;
            let _ = writeln!(out, "{tab}if {cond} {{");
            emit_stmt(store, out, options, stmt, indent + 1, context)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            let cond = emit_value(store, options, cond)?;
            for (position, (value, block)) in cases.into_iter().enumerate() {
                let keyword = if position == 0 { "if" } else { "else if" };
                let _ = writeln!(out, "{tab}{keyword} (bool({cond} == {value})) {{");
                emit_block_items(store, out, options, block, indent + 1, context)?;
                let _ = writeln!(out, "{tab}}}");
            }
            if let Some(default) = default {
                let _ = writeln!(out, "{tab}else {{");
                emit_block_items(store, out, options, default, indent + 1, context)?;
                let _ = writeln!(out, "{tab}}}");
            }
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
            let start = match match_fir(store, init) {
                FirMatch::DeclareVar {
                    init: Some(value), ..
                } => emit_value(store, options, value)?,
                _ => emit_value(store, options, init)?,
            };
            let end = emit_value(store, options, end)?;
            let step = emit_value(store, options, step)?;
            let comparison = if is_reverse { ">" } else { "<" };
            let _ = writeln!(
                out,
                "{tab}for (int32 {var} = {start}; {var} {comparison} {end}; {var} = {var} + {step}) {{"
            );
            emit_block_items(store, out, options, body, indent + 1, context)?;
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
                    "{tab}for (int32 {var} = {upper} - 1; {var} >= 0; {var} = {var} - 1) {{"
                );
            } else {
                let _ = writeln!(
                    out,
                    "{tab}for (int32 {var} = 0; {var} < {upper}; {var} = {var} + 1) {{"
                );
            }
            emit_block_items(store, out, options, body, indent + 1, context)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::WhileLoop { cond, body } => {
            let cond = emit_condition(store, options, cond)?;
            let _ = writeln!(out, "{tab}while {cond} {{");
            emit_block_items(store, out, options, body, indent + 1, context)?;
            let _ = writeln!(out, "{tab}}}");
            Ok(())
        }
        FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            ..
        } => emit_function(
            store,
            out,
            options,
            FunctionView {
                name: &name,
                typ: &typ,
                args: &args,
                body,
            },
            indent,
        ),
        FirMatch::OpenBox { .. }
        | FirMatch::CloseBox
        | FirMatch::AddButton { .. }
        | FirMatch::AddSlider { .. }
        | FirMatch::AddBargraph { .. }
        | FirMatch::AddMetaDeclare { .. } => Ok(()),
        FirMatch::AddSoundfile { label, .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("soundfile `{label}` is not supported by Cmajor"),
        )),
        FirMatch::IteratorForLoop { .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            "iterator loops are not supported by the scalar Cmajor backend",
        )),
        other => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("statement {other:?} is not supported by the Cmajor backend"),
        )),
    }
}

/// Emits an ordinary helper function from FIR.
fn emit_function(
    store: &FirStore,
    out: &mut String,
    options: &CmajorOptions,
    function: FunctionView<'_>,
    indent: usize,
) -> Result<(), CodegenError> {
    let Some(body) = function.body else {
        return Ok(());
    };
    validate_identifier(function.name, "function")?;
    let ret = match function.typ {
        FirType::Fun { ret, .. } => emit_type(ret, options)?,
        _ => "void".to_owned(),
    };
    let mut rendered = Vec::new();
    for arg in function.args {
        if arg.name == "dsp" {
            continue;
        }
        validate_identifier(&arg.name, "function argument")?;
        rendered.push(format!(
            "{} {}",
            emit_argument_type(&arg.typ, options)?,
            arg.name
        ));
    }
    let tab = "\t".repeat(indent);
    let _ = writeln!(out, "{tab}{ret} {}({})", function.name, rendered.join(", "));
    let _ = writeln!(out, "{tab}{{");
    emit_block_items(store, out, options, body, indent + 1, EmitContext::Body)?;
    let _ = writeln!(out, "{tab}}}");
    let _ = writeln!(out);
    Ok(())
}

/// Emits a FIR value expression in Cmajor syntax.
fn emit_value(
    store: &FirStore,
    options: &CmajorOptions,
    value: FirId,
) -> Result<String, CodegenError> {
    match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(value.to_string()),
        FirMatch::Int64 { value, .. } => Ok(format!("{value}L")),
        FirMatch::Float32 { value, .. } => Ok(float_literal(f64::from(value), options.real_type)),
        FirMatch::Float64 { value, .. } => Ok(float_literal(value, options.real_type)),
        FirMatch::Bool { value, .. } => Ok(if value { "true" } else { "false" }.to_owned()),
        FirMatch::Quad { .. } | FirMatch::FixedPoint { .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            "quad and fixed-point values are not supported by Cmajor",
        )),
        FirMatch::ValueArray { values, .. } => {
            let mut rendered = Vec::with_capacity(values.len());
            for value in values {
                rendered.push(emit_value(store, options, value)?);
            }
            Ok(format!("({})", rendered.join(", ")))
        }
        FirMatch::Int32Array { values, .. } => Ok(format!(
            "({})",
            values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        FirMatch::Float32Array { values, .. } => Ok(format!(
            "({})",
            values
                .iter()
                .map(|value| float_literal(f64::from(*value), options.real_type))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        FirMatch::Float64Array { values, .. } => Ok(format!(
            "({})",
            values
                .iter()
                .map(|value| float_literal(*value, options.real_type))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        FirMatch::QuadArray { .. } | FirMatch::FixedPointArray { .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            "quad and fixed-point arrays are not supported by Cmajor",
        )),
        FirMatch::LoadVar { name, .. } | FirMatch::LoadVarAddress { name, .. } => Ok(name),
        FirMatch::LoadTable { name, index, .. } => {
            if name == "inputs" {
                let channel = constant_index(store, index).ok_or_else(|| {
                    CodegenError::new(
                        CodegenErrorCode::InvalidStructure,
                        "one-sample Cmajor input channel must be constant",
                    )
                })?;
                Ok(format!("input{channel}"))
            } else {
                emit_indexed(store, options, &name, index)
            }
        }
        FirMatch::TeeVar { name, value, .. } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("({name} = {value})"))
        }
        FirMatch::BinOp { op, lhs, rhs, typ } => {
            let lhs = emit_infix_operand(store, options, op, lhs, OperandSide::Left)?;
            let rhs = emit_infix_operand(store, options, op, rhs, OperandSide::Right)?;
            let expression = format!("{lhs} {} {rhs}", emit_binop(op));
            // Canonical Faust FIR gives comparisons the language-level int32
            // result type, whereas Cmajor comparisons produce `bool`. Preserve
            // the FIR contract explicitly; Cmajor rejects an implicit bool to
            // int32 store even though C/C++ accept it.
            if is_comparison(op) && typ == FirType::Int32 {
                Ok(format!("int32({expression})"))
            } else {
                Ok(expression)
            }
        }
        FirMatch::Neg { value, .. } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("(-{value})"))
        }
        FirMatch::Cast { typ, value } => {
            let value = emit_value(store, options, value)?;
            Ok(format!("{}({value})", emit_type(&typ, options)?))
        }
        FirMatch::Bitcast { .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            "bitcast is not supported by Cmajor",
        )),
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            let cond = emit_condition(store, options, cond)?;
            let then_value = emit_value(store, options, then_value)?;
            let else_value = emit_value(store, options, else_value)?;
            Ok(format!("({cond} ? {then_value} : {else_value})"))
        }
        FirMatch::FunCall { name, args, .. } => {
            let mut rendered = Vec::with_capacity(args.len());
            for arg in &args {
                rendered.push(emit_value(store, options, *arg)?);
            }
            // A fill call names the size-specialized function; the size is the
            // literal `count` argument the producer emitted.
            let name = match args.get(1).map(|id| match_fir(store, *id)) {
                Some(FirMatch::Int32 { value, .. }) if name.starts_with("fill") => {
                    format!("{name}_{value}")
                }
                _ => map_math_name(&name).to_string(),
            };
            Ok(format!("{name}({})", rendered.join(", ")))
        }
        FirMatch::NullValue { .. } => Ok("0".to_owned()),
        FirMatch::NewDsp { name, .. } => Ok(format!("new{name}()")),
        FirMatch::LoadSoundfileLength { .. }
        | FirMatch::LoadSoundfileRate { .. }
        | FirMatch::LoadSoundfileBuffer { .. } => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            "soundfile values are not supported by Cmajor",
        )),
        other => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("value {other:?} is not supported by the Cmajor backend"),
        )),
    }
}

/// Emits one child of a Cmajor infix expression with only required grouping.
///
/// The shared textual policy owns precedence and associativity. This adapter
/// supplies FIR operator identity and treats an `int32` comparison as atomic:
/// [`emit_value`] wraps that comparison in a Cmajor conversion, so its infix
/// expression is no longer exposed directly to the parent operator.
fn emit_infix_operand(
    store: &FirStore,
    options: &CmajorOptions,
    parent_op: FirBinOp,
    operand: FirId,
    side: OperandSide,
) -> Result<String, CodegenError> {
    let rendered = emit_value(store, options, operand)?;
    let FirMatch::BinOp {
        op: child_op, typ, ..
    } = match_fir(store, operand)
    else {
        return Ok(rendered);
    };
    if is_comparison(child_op) && typ == FirType::Int32 {
        return Ok(rendered);
    }
    let needs_parentheses = infix_operand_needs_parentheses(
        c_like_fir_operator(parent_op),
        c_like_fir_operator(child_op),
        side,
        parent_op == child_op,
    );
    Ok(if needs_parentheses {
        format!("({rendered})")
    } else {
        rendered
    })
}

/// Emits an explicit Cmajor boolean conversion for a control condition.
fn emit_condition(
    store: &FirStore,
    options: &CmajorOptions,
    condition: FirId,
) -> Result<String, CodegenError> {
    Ok(format!(
        "(bool({}))",
        emit_value(store, options, condition)?
    ))
}

/// Emits a constant `[index]` or dynamic `.at(index)` array access.
fn emit_indexed(
    store: &FirStore,
    options: &CmajorOptions,
    name: &str,
    index: FirId,
) -> Result<String, CodegenError> {
    if let Some(index) = constant_index(store, index) {
        Ok(format!("{name}[{index}]"))
    } else {
        Ok(format!("{name}.at({})", emit_value(store, options, index)?))
    }
}

/// Returns a non-negative literal index when the FIR node is constant.
fn constant_index(store: &FirStore, index: FirId) -> Option<usize> {
    match match_fir(store, index) {
        FirMatch::Int32 { value, .. } => usize::try_from(value).ok(),
        FirMatch::Int64 { value, .. } => usize::try_from(value).ok(),
        _ => None,
    }
}

/// Emits a Cmajor scalar, array, or struct type.
fn emit_type(typ: &FirType, options: &CmajorOptions) -> Result<String, CodegenError> {
    match typ {
        FirType::Int32 => Ok("int32".to_owned()),
        FirType::Int64 => Ok("int64".to_owned()),
        FirType::Float32 => Ok("float32".to_owned()),
        FirType::Float64 => Ok("float64".to_owned()),
        FirType::FaustFloat => Ok(options.real_type.cmajor_name().to_owned()),
        FirType::Bool => Ok("bool".to_owned()),
        FirType::Void => Ok("void".to_owned()),
        FirType::Array(elem, size) => Ok(format!("{}[{size}]", emit_type(elem, options)?)),
        FirType::Struct(name, _) => Ok(name.clone()),
        FirType::Ptr(inner) => Ok(format!("{}&", emit_type(inner, options)?)),
        FirType::Fun { ret, .. } => emit_type(ret, options),
        FirType::Quad | FirType::FixedPoint | FirType::Vector(_, _) => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("type {typ:?} is not supported by scalar Cmajor"),
        )),
        FirType::Obj | FirType::Sound | FirType::UI | FirType::Meta => Err(CodegenError::new(
            CodegenErrorCode::Unsupported,
            format!("runtime handle type {typ:?} is not supported in Cmajor source"),
        )),
    }
}

/// Emits a by-reference function argument when Cmajor requires one.
fn emit_argument_type(typ: &FirType, options: &CmajorOptions) -> Result<String, CodegenError> {
    match typ {
        FirType::Array(_, _) | FirType::Struct(_, _) => {
            Ok(format!("{}&", emit_type(typ, options)?))
        }
        FirType::Ptr(inner) => Ok(format!("{}&", emit_type(inner, options)?)),
        _ => emit_type(typ, options),
    }
}

/// Emits a struct declaration using deterministic field names.
fn emit_struct_type(
    out: &mut String,
    options: &CmajorOptions,
    typ: &FirType,
    indent: usize,
) -> Result<(), CodegenError> {
    let FirType::Struct(name, fields) = typ else {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidStructure,
            format!("DeclareStructType contains {typ:?}"),
        ));
    };
    validate_identifier(name, "struct")?;
    let tab = "\t".repeat(indent);
    let _ = writeln!(out, "{tab}struct {name}");
    let _ = writeln!(out, "{tab}{{");
    for (index, field) in fields.iter().enumerate() {
        let _ = writeln!(out, "{tab}\t{} field{index};", emit_type(field, options)?);
    }
    let _ = writeln!(out, "{tab}}}");
    Ok(())
}

/// Maps FIR operators to Cmajor infix spelling.
const fn emit_binop(op: FirBinOp) -> &'static str {
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
        FirBinOp::ARsh | FirBinOp::LRsh => ">>",
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::Lt => "<",
        FirBinOp::Le => "<=",
        FirBinOp::Gt => ">",
        FirBinOp::Ge => ">=",
    }
}

/// Whether a FIR binary operator produces Cmajor's native `bool` type.
const fn is_comparison(op: FirBinOp) -> bool {
    matches!(
        op,
        FirBinOp::Eq | FirBinOp::Ne | FirBinOp::Lt | FirBinOp::Le | FirBinOp::Gt | FirBinOp::Ge
    )
}

/// Maps C/C++ precision-specific math names to Cmajor built-ins.
fn map_math_name(name: &str) -> &str {
    let name = name.strip_prefix("std::").unwrap_or(name);
    match name {
        "abs" | "fabs" | "fabsf" => "abs",
        "max_i" | "max_f" | "max_" | "fmax" | "fmaxf" => "max",
        "min_i" | "min_f" | "min_" | "fmin" | "fminf" => "min",
        "acosf" => "acos",
        "asinf" => "asin",
        "atanf" => "atan",
        "atan2f" => "atan2",
        "ceilf" => "ceil",
        "cosf" => "cos",
        "expf" => "exp",
        "exp2f" => "exp2",
        "exp10f" => "exp10",
        "floorf" => "floor",
        "fmodf" => "fmod",
        "logf" => "log",
        "log2f" => "log2",
        "log10f" => "log10",
        "powf" => "pow",
        "remainderf" => "remainder",
        "rintf" => "rint",
        "roundf" => "round",
        "sinf" => "sin",
        "sqrtf" => "sqrt",
        "tanf" => "tan",
        "acoshf" => "acosh",
        "asinhf" => "asinh",
        "atanhf" => "atanh",
        "coshf" => "cosh",
        "sinhf" => "sinh",
        "tanhf" => "tanh",
        "isnanf" => "isnan",
        "isinff" => "isinf",
        "copysignf" => "copysign",
        other => other,
    }
}

/// Formats a real literal in the selected Cmajor precision.
fn float_literal(value: f64, real_type: CmajorRealType) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value == f64::INFINITY {
        return "inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".to_owned();
    }
    match real_type {
        CmajorRealType::Float32 => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "single-precision Cmajor literals intentionally narrow to f32"
            )]
            let value = value as f32;
            let text = if value == value.trunc() {
                format!("{value:.1}")
            } else {
                value.to_string()
            };
            format!("{text}f")
        }
        CmajorRealType::Float64 => {
            if value == value.trunc() {
                format!("{value:.1}")
            } else {
                value.to_string()
            }
        }
    }
}

/// Checks the conservative identifier grammar shared by generated names.
fn validate_identifier(name: &str, kind: &str) -> Result<(), CodegenError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidIdentifier,
            format!("{kind} name is empty"),
        ));
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return Err(CodegenError::new(
            CodegenErrorCode::InvalidIdentifier,
            format!("{kind} `{name}` is not a valid Cmajor identifier"),
        ));
    }
    Ok(())
}

/// Produces a valid suffix for generated loop counters.
fn sanitize_identifier(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch == '_' || ch.is_ascii_alphanumeric() {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Extracts a numeric suffix from `prefixN`.
fn io_channel(name: &str, prefix: &str) -> Option<usize> {
    name.strip_prefix(prefix)?.parse().ok()
}

/// Recognizes the canonical FIR zones assigned to bargraph widgets.
fn is_bargraph_zone(name: &str) -> bool {
    name.starts_with("fHbargraph") || name.starts_with("fVbargraph")
}

/// Returns all items held by a FIR block.
fn block_items(store: &FirStore, block: FirId) -> Vec<FirId> {
    match match_fir(store, block) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

/// Finds the body of a canonical DSP method.
fn find_function_body(store: &FirStore, functions: FirId, wanted: &str) -> Option<FirId> {
    block_items(store, functions)
        .into_iter()
        .find_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == wanted => Some(body),
            _ => None,
        })
}

/// Whether a function is reshaped into the Cmajor processor contract.
fn is_dsp_api_method(name: &str) -> bool {
    const METHODS: [&str; 14] = [
        "metadata",
        "getNumInputs",
        "getNumOutputs",
        "getSampleRate",
        "classInit",
        "instanceConstants",
        "instanceResetUserInterface",
        "instanceClear",
        "instanceInit",
        "init",
        "buildUserInterface",
        "control",
        "frame",
        "compute",
    ];
    METHODS.contains(&name)
}

/// Decodes the root module without reaching into raw tree tags.
fn decode_module(store: &FirStore, module: FirId) -> Result<ModuleView, CodegenError> {
    match match_fir(store, module) {
        FirMatch::Module {
            num_inputs,
            num_outputs,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
            ..
        } => Ok(ModuleView {
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
            num_inputs,
            num_outputs,
        }),
        other => Err(CodegenError::new(
            CodegenErrorCode::RootNotModule,
            format!("expected FIR module root, found {other:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fir::{FirBuilder, FirStore};

    /// Builds the smallest one-sample FIR module accepted by Cmajor.
    fn one_sample_passthrough() -> (FirStore, FirId) {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let input_index = b.int32(0);
        let input = b.load_table("inputs", AccessType::FunArgs, input_index, FirType::Float32);
        let output_index = b.int32(0);
        let output = b.store_table("outputs", AccessType::FunArgs, output_index, input);
        let frame_body = b.block(&[output]);
        let frame_type = FirType::Fun {
            args: vec![],
            ret: Box::new(FirType::Void),
        };
        let frame = b.declare_fun("frame", frame_type, &[], Some(frame_body), false);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let functions = b.block(&[frame]);
        let static_decls = b.block(&[]);
        let module = b.module(
            1,
            1,
            "mydsp",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[],
        );
        (store, module)
    }

    #[test]
    fn emits_one_sample_processor_shell() {
        let (store, module) = one_sample_passthrough();
        let text = generate_cmajor_module(&store, module, &CmajorOptions::default())
            .expect("Cmajor generation should succeed");
        assert!(text.contains("processor mydsp"), "{text}");
        assert!(text.contains("input stream float32 input0;"), "{text}");
        assert!(text.contains("output stream float32 output0;"), "{text}");
        assert!(text.contains("output0 <- input0;"), "{text}");
        assert_eq!(text.matches("advance();").count(), 1, "{text}");
    }

    #[test]
    fn comparison_preserves_fir_int32_result_type() {
        let mut store = FirStore::new();
        let comparison = {
            let mut b = FirBuilder::new(&mut store);
            let lhs = b.int32(1);
            let rhs = b.int32(2);
            b.binop(FirBinOp::Lt, lhs, rhs, FirType::Int32)
        };
        let text = emit_value(&store, &CmajorOptions::default(), comparison)
            .expect("comparison emission should succeed");
        assert_eq!(text, "int32(1 < 2)");
    }

    #[test]
    fn infix_emission_flattens_only_safe_c_like_shapes() {
        let mut store = FirStore::new();
        let (right_add, right_sub, grouped_mul) = {
            let mut b = FirBuilder::new(&mut store);
            let a = b.load_var("a", AccessType::Stack, FirType::Float64);
            let b_value = b.load_var("b", AccessType::Stack, FirType::Float64);
            let c = b.load_var("c", AccessType::Stack, FirType::Float64);
            let b_plus_c = b.binop(FirBinOp::Add, b_value, c, FirType::Float64);
            let right_add = b.binop(FirBinOp::Add, a, b_plus_c, FirType::Float64);
            let b_minus_c = b.binop(FirBinOp::Sub, b_value, c, FirType::Float64);
            let right_sub = b.binop(FirBinOp::Sub, a, b_minus_c, FirType::Float64);
            let a_plus_b = b.binop(FirBinOp::Add, a, b_value, FirType::Float64);
            let grouped_mul = b.binop(FirBinOp::Mul, a_plus_b, c, FirType::Float64);
            (right_add, right_sub, grouped_mul)
        };
        let options = CmajorOptions::default();
        assert_eq!(
            emit_value(&store, &options, right_add).expect("right-add emission"),
            "a + b + c"
        );
        assert_eq!(
            emit_value(&store, &options, right_sub).expect("right-sub emission"),
            "a - (b - c)"
        );
        assert_eq!(
            emit_value(&store, &options, grouped_mul).expect("grouped-mul emission"),
            "(a + b) * c"
        );
    }

    #[test]
    fn deep_associative_addition_has_bounded_rendered_nesting() {
        const TERMS: usize = 128;
        let mut store = FirStore::new();
        let expression = {
            let mut b = FirBuilder::new(&mut store);
            let mut expression = b.load_var(
                format!("v{}", TERMS - 1),
                AccessType::Stack,
                FirType::Float64,
            );
            for index in (0..TERMS - 1).rev() {
                let term = b.load_var(format!("v{index}"), AccessType::Stack, FirType::Float64);
                expression = b.binop(FirBinOp::Add, term, expression, FirType::Float64);
            }
            expression
        };
        let text = emit_value(&store, &CmajorOptions::default(), expression)
            .expect("deep addition emission should succeed");
        assert_eq!(text.matches(" + ").count(), TERMS - 1, "{text}");
        assert!(!text.contains(['(', ')']), "{text}");
    }

    #[test]
    fn lifecycle_matches_the_shared_backend_contract() {
        let (store, module) = one_sample_passthrough();
        let text = generate_cmajor_module(&store, module, &CmajorOptions::default())
            .expect("Cmajor generation should succeed");
        let init = text
            .split("void init()")
            .nth(1)
            .and_then(|tail| tail.split("void main()").next())
            .expect("init section");
        assert!(
            init.find("classInit(sample_rate)") < init.find("instanceInit(sample_rate)"),
            "{text}"
        );
        let instance = text
            .split("void instanceInit(int sample_rate)")
            .nth(1)
            .and_then(|tail| tail.split("void init()").next())
            .expect("instanceInit section");
        let constants = instance.find("instanceConstants").expect("constants");
        let reset = instance.find("instanceResetUserInterface").expect("reset");
        let clear = instance.find("instanceClear").expect("clear");
        assert!(constants < reset && reset < clear, "{text}");
        assert!(!instance.contains("classInit"), "{text}");
    }

    #[test]
    fn rejects_digit_initial_processor_name() {
        let (store, module) = one_sample_passthrough();
        let options = CmajorOptions {
            class_name: "1bad".to_owned(),
            ..CmajorOptions::default()
        };
        let error = generate_cmajor_module(&store, module, &options)
            .expect_err("invalid processor name must fail");
        assert_eq!(error.code(), CodegenErrorCode::InvalidIdentifier);
        assert_eq!(error.code().as_str(), "FRS-CGEN-CMAJ-0003");
    }
}
