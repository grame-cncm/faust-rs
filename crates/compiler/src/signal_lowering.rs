//! Signal-to-FIR lowering context, error types, and per-backend dispatch.
//!
//! Centralises everything between the evaluator/propagate output
//! ([`SignalCompileOutput`]) and the backend emitter:
//!
//! - [`LowerError<E>`] / [`LowerToInterpError`] / [`LowerToFirError`] —
//!   three-stage error envelopes (Transform → Verify → Codegen);
//! - [`SignalLoweringContext`] — lane selection, FIR verify options, real type,
//!   delay parameters, and optional timing sink bundled as one value;
//! - `lower_signals_to_*` — public dispatch entry points (C++, C, Julia, interp, FIR);
//! - `lower_signals_to_*_transform_fastlane` — the actual lowering implementations
//!   shared by all backends;
//! - `maybe_verify_fir_module` / `serialize_factory` / `resolve_module_name` —
//!   shared helpers used across multiple entry points.

use super::*;

use crate::execution::{ExecutionOptionsError, validate_execution_options};

// ─── Signal-to-FIR lower errors ───────────────────────────────────────────────

/// Generic lower-to-backend error for backends that follow the
/// Transform → Verify → Codegen pattern.
///
/// `E` is the backend-specific codegen error type.
/// Specialised as [`LowerToCppError`] and [`LowerToCError`].
#[derive(Debug)]
pub(crate) enum LowerError<E> {
    /// Execution-option request rejected by the capability model before
    /// any parsing or lowering work.
    ExecutionOptions(ExecutionOptionsError),
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify {
        report: FirVerifyReport,
        origins: transform::signal_fir::FirOrigins,
    },
    /// Backend emission failed after successful FIR lowering.
    Codegen {
        error: E,
        origins: transform::signal_fir::FirOrigins,
    },
}

/// Lower error for the C++ backend.
pub(crate) type LowerToCppError = LowerError<CppCodegenError>;
/// Lower error for the C backend.
pub(crate) type LowerToCError = LowerError<CCodegenError>;
/// Lower error for the Julia backend.
pub(crate) type LowerToJuliaError = LowerError<JuliaCodegenError>;
/// Lowering error surface for the Rust backend.
pub(crate) type LowerToRustError = LowerError<RustCodegenError>;
/// Lowering error surface for the AssemblyScript backend.
pub(crate) type LowerToAscError = LowerError<AscCodegenError>;
/// Lowering error surface for the codebox (RNBO) backend.
pub(crate) type LowerToCodeboxError = LowerError<CodeboxCodegenError>;
/// Lowering error surface for the Cmajor backend.
pub(crate) type LowerToCmajorError = LowerError<CmajorCodegenError>;

#[derive(Debug)]
pub(crate) enum LowerToInterpError {
    /// Execution-option request rejected by the capability model.
    ExecutionOptions(ExecutionOptionsError),
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify {
        report: FirVerifyReport,
        origins: transform::signal_fir::FirOrigins,
    },
    /// Interpreter backend emission failed after successful lowering.
    Codegen {
        error: InterpCodegenError,
        origins: transform::signal_fir::FirOrigins,
    },
    /// Serialization of the factory to `.fbc` text failed.
    Serialize(String),
}

#[cfg(not(target_arch = "wasm32"))]
/// Lowering error surface for the Cranelift backend report.
///
/// Not a [`LowerError<CraneliftBackendError>`]: the subset-gap diagnosis and
/// the JIT emission are two fallible backend steps, and both fold into the
/// single `Codegen` variant here.
#[derive(Debug)]
pub(crate) enum LowerToCraneliftError {
    /// Execution-option request rejected by the capability model.
    ExecutionOptions(ExecutionOptionsError),
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify {
        report: FirVerifyReport,
        origins: transform::signal_fir::FirOrigins,
    },
    /// Cranelift JIT emission (or its subset diagnosis) failed.
    Codegen {
        error: CraneliftBackendError,
        origins: transform::signal_fir::FirOrigins,
    },
}

#[derive(Debug)]
pub(crate) enum LowerToFirError {
    /// Execution-option request rejected by the capability model.
    ExecutionOptions(ExecutionOptionsError),
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify {
        report: FirVerifyReport,
        origins: transform::signal_fir::FirOrigins,
    },
}

impl<E> From<ExecutionOptionsError> for LowerError<E> {
    fn from(error: ExecutionOptionsError) -> Self {
        Self::ExecutionOptions(error)
    }
}

impl From<ExecutionOptionsError> for LowerToInterpError {
    fn from(error: ExecutionOptionsError) -> Self {
        Self::ExecutionOptions(error)
    }
}

impl From<ExecutionOptionsError> for LowerToFirError {
    fn from(error: ExecutionOptionsError) -> Self {
        Self::ExecutionOptions(error)
    }
}

/// Runs `f`, optionally recording its wall-clock duration in `timing_sink`.
///
/// When `timing_sink` is `None`, the closure is called directly with zero
/// overhead.  When present, the elapsed time is passed to the sink under
/// `name` so callers can collect per-phase timing without conditional logic
/// at each call site.
pub(crate) fn time_phase_with_sink<T>(
    timing_sink: Option<&TimingSink>,
    name: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    if let Some(sink) = timing_sink {
        let start = Instant::now();
        let result = f();
        sink(name, start.elapsed());
        result
    } else {
        f()
    }
}

// ─── Signal-to-FIR lower functions ───────────────────────────────────────────

/// Shared configuration for all `lower_signals_to_*` entry points.
///
/// Bundles the parameters that are common across every backend so callers
/// construct one value and pass it to the chosen dispatch function.
#[derive(Clone)]
pub(crate) struct SignalLoweringContext {
    /// Which signal→FIR lowering lane the caller selected.
    ///
    /// Never read: the transform fast lane is the only route, so every
    /// dispatcher validates and delegates without consulting it. The field is
    /// kept because the public entry points do take a lane, and recording it
    /// here is what makes a second lane a change to the lowering rather than to
    /// every signature. Before this was explicit, six `let _ = ctx.lane;`
    /// statements scattered through the dispatchers silenced the same warning
    /// and hid the fact.
    #[allow(dead_code)]
    pub(crate) lane: SignalFirLane,
    /// Whether and how strictly to run FIR verification after lowering.
    pub(crate) fir_verify: FirVerifyOptions,
    /// Floating-point precision for the generated DSP core.
    pub(crate) real_type: RealType,
    /// Maximum number of samples a delay may be copy-unrolled before falling
    /// back to a ring-buffer state slot.
    pub(crate) max_copy_delay: u32,
    /// Delay-line count threshold above which the lowerer switches strategy.
    pub(crate) delay_line_threshold: u32,
    /// `compute()` codegen strategy: scalar, or the checked vector pipeline
    /// (`-vec`) that falls back to scalar for shapes it cannot certify.
    pub(crate) compute_mode: ComputeMode,
    /// How generated-table content is produced (`--table-init`).
    pub(crate) table_init_mode: transform::signal_fir::TableInitMode,
    /// Explicit SR used only to fold `ma.SR`-dependent const table generators.
    pub(crate) table_init_sample_rate: Option<i32>,
    /// Signal/loop dependency scheduling policy (`-ss` /
    /// `--scheduling-strategy`) applied to the lowered dependency graph.
    pub(crate) scheduling_strategy: SchedulingStrategy,
    /// Control-rate evaluation scheduling (`-ec`): whether block-rate work is
    /// emitted inline or in a separate `control` entry point. The default
    /// reproduces the classic contract.
    pub(crate) control_rate_mode: ControlRateMode,
    /// Public processing-API shape (`-os`): whether a one-sample `frame` entry
    /// point is emitted alongside an empty canonical `compute`. The default
    /// reproduces the classic contract.
    pub(crate) processing_api: ProcessingApi,
    /// Optional per-phase timing callback; `None` disables timing.
    pub(crate) timing_sink: Option<TimingSink>,
}

/// Lowers signals through the transform fast lane then serializes an interpreter `.fbc`.
///
/// This function reuses `lower_signals_to_fir_transform_fastlane` so that the
/// C, C++, and interp transform paths share one FIR lowering implementation.
pub(crate) fn lower_signals_to_interp_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &InterpOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToInterpError> {
    let module_name = resolve_module_name(options.module_name.as_deref(), source_name);
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerToInterpError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerToInterpError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    match ctx.real_type {
        RealType::Float32 => {
            let factory: FbcDspFactory<f32> =
                time_phase_with_sink(timing_sink, "interp-codegen", || {
                    generate_interp_module(&lowered.store, lowered.module, options)
                })
                .map_err(|error| LowerToInterpError::Codegen {
                    error,
                    origins: lowered.origins.clone(),
                })?;
            time_phase_with_sink(timing_sink, "interp-serialize", || {
                serialize_factory(&factory)
            })
            .map_err(LowerToInterpError::Serialize)
        }
        RealType::Float64 => {
            let factory: FbcDspFactory<f64> =
                time_phase_with_sink(timing_sink, "interp-codegen", || {
                    generate_interp_module(&lowered.store, lowered.module, options)
                })
                .map_err(|error| LowerToInterpError::Codegen {
                    error,
                    origins: lowered.origins.clone(),
                })?;
            time_phase_with_sink(timing_sink, "interp-serialize", || {
                serialize_factory(&factory)
            })
            .map_err(LowerToInterpError::Serialize)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Lowers propagated signals to FIR, JIT-compiles them with the Cranelift
/// backend, and renders the resulting module as a text status report.
///
/// The `JitDspModule` is dropped before returning: the report is a snapshot of
/// its shape (symbol names, entry address, `dsp` struct layout), never a
/// handle to the finalized code. Callers that need to *run* the compiled DSP
/// must own the module themselves and so cannot go through this function —
/// see `crates/cranelift-ffi`.
pub(crate) fn lower_signals_to_cranelift_report(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CraneliftOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToCraneliftError> {
    validate_execution_options(
        "cranelift",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerToCraneliftError::ExecutionOptions)?;
    // Same derivation as `lower_signals_to_fir`, not `resolve_module_name`
    // (which is the class-name path and defaults to "mydsp"): the report names
    // the FIR module, so it must match what the FIR dump would show for the
    // same input.
    let module_name = sanitize_cpp_ident(source_name_to_class(source_name).as_str());
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerToCraneliftError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerToCraneliftError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;

    // A subset gap is a lowering *limitation* to report, not a failure: the
    // module still compiles, with an unlowered compute body. Only a hard
    // backend error stops the report.
    let subset_gap = diagnose_cranelift_compute_subset_gap(&lowered.store, lowered.module)
        .map_err(|error| LowerToCraneliftError::Codegen {
            error,
            origins: lowered.origins.clone(),
        })?;
    let compiled = time_phase_with_sink(timing_sink, "cranelift-codegen", || {
        generate_cranelift_module(&lowered.store, lowered.module, options)
    })
    .map_err(|error| LowerToCraneliftError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })?;
    Ok(render_cranelift_module_report(
        &compiled,
        subset_gap.as_deref(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
/// Renders a compiled Cranelift module as the backend status report.
///
/// Shape and field order are the CLI's long-standing `--lang cranelift`
/// output, kept byte-for-byte so the facade entry points and the CLI cannot
/// drift apart.
pub fn render_cranelift_module_report(compiled: &JitDspModule, subset_gap: Option<&str>) -> String {
    let layout = compiled.struct_layout();
    let mut out = String::new();
    out.push_str("backend: cranelift (experimental)\n");
    out.push_str(&format!("module: {}\n", compiled.module_name()));
    out.push_str(&format!(
        "compute_symbol: {}\n",
        compiled.compute_symbol_name()
    ));
    out.push_str(&format!(
        "compute_entry_addr: 0x{:x}\n",
        compiled.compute_entry_addr()
    ));
    out.push_str(&format!(
        "compute_body_lowered: {}\n",
        compiled.compute_body_lowered()
    ));
    if let Some(reason) = subset_gap {
        out.push_str(&format!("subset_gap: {reason}\n"));
    }
    out.push_str(&format!(
        "dsp_struct_layout: size={} align={} fields={}\n",
        layout.size_bytes(),
        layout.align_bytes(),
        layout.fields().len()
    ));
    for field in layout.fields() {
        let kind = match &field.kind {
            StructFieldKind::Scalar(typ) => format!("scalar:{typ:?}"),
            StructFieldKind::Table { elem_type, len } => {
                format!("table:{elem_type:?}[{len}]")
            }
            StructFieldKind::ExternalTable {
                elem_type,
                len,
                zone_id,
            } => format!("external-table:{elem_type:?}[{len}] zone={}", zone_id.0),
        };
        out.push_str(&format!(
            "  - {} @{} size={} align={} {}\n",
            field.name, field.offset_bytes, field.size_bytes, field.align_bytes, kind
        ));
    }
    out
}

/// Serializes a [`FbcDspFactory`] to `.fbc` text format.
pub(crate) fn serialize_factory<R: FbcReal>(factory: &FbcDspFactory<R>) -> Result<String, String> {
    let mut buf = Vec::new();
    write_fbc(factory, &mut buf, false).map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

/// Shared prologue of every backend dispatch: reject an execution-option
/// request the backend cannot honor, then hand over to the fast-lane
/// implementation.
///
/// `ctx.lane` is deliberately not consulted. The transform fast lane is the
/// only route today, so these dispatchers exist to validate and delegate, not
/// to choose — which is why each backend's wrapper below is three lines.
fn lower_signals_with_validation<O, E>(
    backend: &'static str,
    source_name: &str,
    output: &SignalCompileOutput,
    options: &O,
    ctx: SignalLoweringContext,
    fastlane: fn(&str, &SignalCompileOutput, &O, &SignalLoweringContext) -> Result<String, E>,
) -> Result<String, E>
where
    E: From<ExecutionOptionsError>,
{
    validate_execution_options(
        backend,
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )?;
    fastlane(source_name, output, options, &ctx)
}

/// Dispatches cpp lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_cpp(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CppOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToCppError> {
    lower_signals_with_validation(
        "cpp",
        source_name,
        output,
        options,
        ctx,
        lower_signals_to_cpp_transform_fastlane,
    )
}

/// Dispatches c lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_c(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &COptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToCError> {
    lower_signals_with_validation(
        "c",
        source_name,
        output,
        options,
        ctx,
        lower_signals_to_c_transform_fastlane,
    )
}

/// Dispatches julia lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_julia(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &JuliaOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToJuliaError> {
    lower_signals_with_validation(
        "julia",
        source_name,
        output,
        options,
        ctx,
        lower_signals_to_julia_transform_fastlane,
    )
}

/// Dispatches asc lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_asc(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &AscOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToAscError> {
    lower_signals_with_validation(
        "asc",
        source_name,
        output,
        options,
        ctx,
        lower_signals_to_asc_transform_fastlane,
    )
}

/// Dispatches codebox lowering through the selected signal->FIR lane, after
/// forcing the two execution modes the target imposes.
///
/// Unlike every other dispatcher, this one *overrides* the caller's request
/// instead of merely validating it. RNBO calls the generated code once per
/// sample and sets controls through `@param` identifiers, so external control
/// and the one-sample API are not modes codebox can be put into — they are
/// what codebox is. The capability table records both as `Intrinsic`,
/// meaning "accepted, and output-invariant"; this override is what makes that
/// second half true, for the CLI and for programmatic callers alike.
///
/// Vector mode is the one request that cannot be absorbed this way, so it is
/// rejected by name — see the `vector` column in [`crate::execution`].
/// Validation runs on the *forced* modes, which is why it is called here
/// rather than through `lower_signals_with_validation`.
pub(crate) fn lower_signals_to_codebox(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CodeboxOptions,
    mut ctx: SignalLoweringContext,
) -> Result<String, LowerToCodeboxError> {
    ctx.control_rate_mode = ControlRateMode::External;
    ctx.processing_api = ProcessingApi::OneSample;
    validate_execution_options(
        "codebox",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )?;
    lower_signals_to_codebox_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches Cmajor lowering after forcing its intrinsic execution shape.
///
/// C++ provenance: `compileCmajor` in `compiler/libcode.cpp` unconditionally
/// selects external control and one-sample code. Rust exposes the same
/// output-invariant contract through the capability table: `-ec` and `-os`
/// are accepted aliases, while vector mode is rejected explicitly.
pub(crate) fn lower_signals_to_cmajor(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CmajorOptions,
    mut ctx: SignalLoweringContext,
) -> Result<String, LowerToCmajorError> {
    ctx.control_rate_mode = ControlRateMode::External;
    ctx.processing_api = ProcessingApi::OneSample;
    validate_execution_options(
        "cmajor",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )?;
    lower_signals_to_cmajor_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches rust lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_rust(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &RustOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToRustError> {
    lower_signals_with_validation(
        "rust",
        source_name,
        output,
        options,
        ctx,
        lower_signals_to_rust_transform_fastlane,
    )
}

/// Dispatches interp lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_interp(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &InterpOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToInterpError> {
    lower_signals_with_validation(
        "interp",
        source_name,
        output,
        options,
        ctx,
        lower_signals_to_interp_transform_fastlane,
    )
}

/// Lowers propagated signals to FIR without invoking a backend emitter.
///
/// This is the shared implementation behind FIR dump/verification flows and is
/// also used as the backend-independent boundary for lane comparisons.
///
/// `module_name` of `None` derives the name from `source_name`, which is what
/// the FIR dump and the Cranelift JIT identity want. Callers that must agree
/// with a C/C++ backend's own class-name convention pass an explicit name —
/// see [`json_memory_layout_module_name`].
// The parameters are exactly the facade-owned lowering knobs; bundling them is
// a separate refactor (they also flow individually through the C++/C/Julia
// paths). Kept explicit for now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_signals_to_fir(
    source_name: &str,
    output: &SignalCompileOutput,
    _lane: SignalFirLane,
    fir_verify: FirVerifyOptions,
    real_type: RealType,
    max_copy_delay: u32,
    delay_line_threshold: u32,
    compute_mode: ComputeMode,
    scheduling_strategy: SchedulingStrategy,
    control_rate_mode: ControlRateMode,
    processing_api: ProcessingApi,
    table_init_mode: transform::signal_fir::TableInitMode,
    table_init_sample_rate: Option<i32>,
    module_name: Option<String>,
) -> Result<FirCompileOutput, LowerToFirError> {
    validate_execution_options("fir", control_rate_mode, processing_api, compute_mode)
        .map_err(LowerToFirError::ExecutionOptions)?;
    let module_name = module_name
        .unwrap_or_else(|| sanitize_cpp_ident(source_name_to_class(source_name).as_str()));
    let lowered = lower_signals_to_fir_transform_fastlane(
        output,
        module_name,
        real_type,
        max_copy_delay,
        delay_line_threshold,
        compute_mode,
        scheduling_strategy,
        control_rate_mode,
        processing_api,
        table_init_mode,
        table_init_sample_rate,
    )
    .map_err(LowerToFirError::Transform)?;
    maybe_verify_fir_module(&lowered, fir_verify).map_err(|report| LowerToFirError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    Ok(lowered)
}

/// Resolves a module name from explicit class_name option or from the source name.
pub(crate) fn resolve_module_name(class_name: Option<&str>, _source_name: &str) -> String {
    class_name
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "mydsp".to_owned())
}

/// Module name a `-mem0` JSON `memory_layout` must lower with to describe
/// `flavor` faithfully, or `None` to keep the source-derived default.
///
/// C and C++ name every generated identifier (class, `SIG0` helpers, `ftbl0`
/// static tables) from [`resolve_module_name`] — `-cn`, default `"mydsp"` —
/// so a `memory_layout` claiming to describe one of them must use the same
/// name, or its zones (`"mydspSIG0"`, `"ftbl0mydspSIG0"`, ...) name
/// identifiers the generated source does not contain. Cranelift is
/// deliberately excluded: its module name is a JIT identity derived from the
/// source name and it has no `-cn` convention.
pub(crate) fn json_memory_layout_module_name(
    flavor: Option<MemoryLayoutFlavor>,
    class_name: Option<&str>,
    source_name: &str,
) -> Option<String> {
    matches!(
        flavor,
        Some(MemoryLayoutFlavor::C | MemoryLayoutFlavor::Cpp)
    )
    .then(|| resolve_module_name(class_name, source_name))
}

/// Transform fast-lane FIR lowering used by native backends and FIR dumps.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_signals_to_fir_transform_fastlane(
    output: &SignalCompileOutput,
    module_name: String,
    real_type: RealType,
    max_copy_delay: u32,
    delay_line_threshold: u32,
    compute_mode: ComputeMode,
    scheduling_strategy: SchedulingStrategy,
    control_rate_mode: ControlRateMode,
    processing_api: ProcessingApi,
    table_init_mode: transform::signal_fir::TableInitMode,
    table_init_sample_rate: Option<i32>,
) -> Result<FirCompileOutput, SignalFirError> {
    lower_signals_to_fir_transform_fastlane_with_timing(
        output,
        module_name,
        real_type,
        max_copy_delay,
        delay_line_threshold,
        compute_mode,
        scheduling_strategy,
        control_rate_mode,
        processing_api,
        table_init_mode,
        table_init_sample_rate,
        None,
    )
}

/// Timed variant of [`lower_signals_to_fir_transform_fastlane`].
///
/// The optional callback is forwarded to transform's observation-only stage
/// timing API; it does not participate in FIR construction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_signals_to_fir_transform_fastlane_with_timing(
    output: &SignalCompileOutput,
    module_name: String,
    real_type: RealType,
    max_copy_delay: u32,
    delay_line_threshold: u32,
    compute_mode: ComputeMode,
    scheduling_strategy: SchedulingStrategy,
    control_rate_mode: ControlRateMode,
    processing_api: ProcessingApi,
    table_init_mode: transform::signal_fir::TableInitMode,
    table_init_sample_rate: Option<i32>,
    timing_sink: Option<&TimingSink>,
) -> Result<FirCompileOutput, SignalFirError> {
    let signal_fir_options = SignalFirOptions {
        module_name,
        real_type,
        max_copy_delay,
        delay_line_threshold,
        compute_mode,
        scheduling_strategy,
        control_rate_mode,
        processing_api,
        table_init_mode,
        table_init_sample_rate,
    };
    let lowered =
        transform::signal_fir::compile_signals_to_fir_fastlane_clocked_with_timing_and_origins(
            &output.parse.state.arena,
            &output.signals,
            output.process_arity.inputs,
            output.propagated_output_count(),
            &output.ui,
            &output.clock_domains,
            &signal_fir_options,
            timing_sink.map(|sink| sink.as_ref()),
            Some(&output.signal_origins),
        )?;
    // Canonicalize every FIR artifact before it reaches any backend or FIR
    // dump. This is deliberately independent of `--no-fir-verify`: pure Drop
    // roots are construction scaffolding, not an optional backend optimization.
    let (store, module, mapping) =
        sweep_scaffolding_drop_roots_with_mapping(&lowered.store, lowered.module);
    let mut origins = lowered.origins.remap_pairs(&mapping);
    origins.derive_reachable(&store, module);
    Ok(FirCompileOutput {
        store,
        module,
        origins,
        vector_pipeline_status: lowered.vector_pipeline_status,
        vector_effective_mode: lowered.vector_effective_mode,
        vector_pipeline_detail: lowered.vector_pipeline_detail,
    })
}

/// Builds the one-line JSON description embedded in generated source by
/// backends that expose a `getJSON`-style accessor.
///
/// Source provenance: C++ `CodeContainer::generateJSONAux`, called by the
/// textual backends that carry their own description (Julia, Rust, and the
/// others in that family).
///
/// Two deliberate narrowings against the reference, both recorded in
/// `porting/wasm-julia-maturity-diff-gap-005-analysis-and-plan-2026-08-14-en.md`:
///
/// - `include_pathnames` stays empty. The reference bakes the compiling
///   machine's absolute import roots into the generated source; that is
///   provenance no host reads and it makes generated artifacts machine-specific.
/// - a failure to describe the module is not fatal. The description is a
///   secondary artifact, and the layout step it needs can reject shapes the
///   textual backend itself emits happily; degrading to the previous empty
///   object keeps such a DSP compiling. The degradation is visible in the
///   artifact — `getJSON` returns `"{}"` — rather than silently wrong.
fn embedded_json_description(
    store: &FirStore,
    module: FirId,
    source_name: &str,
    dsp_name: &str,
    output: &SignalCompileOutput,
    compile_options: Option<&str>,
    real_type: RealType,
) -> Option<String> {
    let compile_options = compile_options.map_or_else(
        || compile_options_json_string(None, real_type == RealType::Float64),
        str::to_owned,
    );
    build_strict_json_description(
        store,
        module,
        StrictJsonContext {
            filename: source_name_to_filename(source_name),
            include_pathnames: Vec::new(),
            library_list: library_list_from_signals(output),
            top_level_meta: json_meta_entries_from_snapshot(&output.compilation_metadata),
            compile_options,
            double_precision: real_type == RealType::Float64,
            memory_flavor: None,
        },
    )
    .ok()
    .map(|mut description| {
        // The FIR module carries the *class* identity (`-cn`, default
        // `"mydsp"`), which is a codegen name, not the DSP name. A description
        // built from it would advertise `"name": "mydsp"` next to
        // `"filename": "simple.dsp"`, and would disagree with the `metadata!`
        // callback generated from the same session. The same confusion produced
        // the `-mem0` `memory_layout` zone-naming defect fixed on 2026-08-13,
        // in the other direction.
        description.name = dsp_name.to_owned();
        description.render_flat()
    })
}

/// Lowers signals through the transform fast lane, verifies FIR, then emits C++.
pub(crate) fn lower_signals_to_cpp_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CppOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToCppError> {
    let module_name = resolve_module_name(options.class_name.as_deref(), source_name);
    let mut effective_options = options.clone();
    effective_options
        .metadata_name
        .get_or_insert_with(|| resolve_ui_root_label(source_name, &output.compilation_metadata));
    effective_options
        .metadata_filename
        .get_or_insert_with(|| source_name_to_filename(source_name));
    if effective_options.metadata_entries.is_empty() {
        effective_options.metadata_entries =
            c_family_meta_entries_from_snapshot(source_name, &output.compilation_metadata);
        if let Some(compile_options) = effective_options.compile_options.clone() {
            effective_options
                .metadata_entries
                .push(("compile_options".to_owned(), compile_options));
        }
    }
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    time_phase_with_sink(timing_sink, "cpp-codegen", || {
        effective_options.double_precision = ctx.real_type == crate::RealType::Float64;
        generate_cpp_module(&lowered.store, lowered.module, &effective_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Lowers signals through the transform fast lane, verifies FIR, then emits C.
pub(crate) fn lower_signals_to_c_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &COptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToCError> {
    let module_name = resolve_module_name(options.class_name.as_deref(), source_name);
    let mut effective_options = options.clone();
    effective_options
        .metadata_name
        .get_or_insert_with(|| resolve_ui_root_label(source_name, &output.compilation_metadata));
    effective_options
        .metadata_filename
        .get_or_insert_with(|| source_name_to_filename(source_name));
    if effective_options.metadata_entries.is_empty() {
        effective_options.metadata_entries =
            c_family_meta_entries_from_snapshot(source_name, &output.compilation_metadata);
        if let Some(compile_options) = effective_options.compile_options.clone() {
            effective_options
                .metadata_entries
                .push(("compile_options".to_owned(), compile_options));
        }
    }
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    time_phase_with_sink(timing_sink, "c-codegen", || {
        effective_options.double_precision = ctx.real_type == crate::RealType::Float64;
        generate_c_module(&lowered.store, lowered.module, &effective_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Lowers signals through the transform fast lane, verifies FIR, then emits Julia.
pub(crate) fn lower_signals_to_julia_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &JuliaOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToJuliaError> {
    let module_name = resolve_module_name(options.class_name.as_deref(), source_name);
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    let mut codegen_options = options.clone();
    codegen_options.real_type = match ctx.real_type {
        RealType::Float32 => JuliaRealType::Float32,
        RealType::Float64 => JuliaRealType::Float64,
    };
    codegen_options
        .metadata_name
        .get_or_insert_with(|| resolve_ui_root_label(source_name, &output.compilation_metadata));
    codegen_options
        .metadata_filename
        .get_or_insert_with(|| source_name_to_filename(source_name));
    if codegen_options.metadata_entries.is_empty() {
        codegen_options.metadata_entries =
            c_family_meta_entries_from_snapshot(source_name, &output.compilation_metadata);
        if let Some(compile_options) = codegen_options.compile_options.clone() {
            codegen_options
                .metadata_entries
                .push(("compile_options".to_owned(), compile_options));
        }
    }
    if codegen_options.dsp_json.is_none() {
        let dsp_name = codegen_options
            .metadata_name
            .clone()
            .unwrap_or_else(|| resolve_ui_root_label(source_name, &output.compilation_metadata));
        codegen_options.dsp_json = embedded_json_description(
            &lowered.store,
            lowered.module,
            source_name,
            &dsp_name,
            output,
            codegen_options.compile_options.as_deref(),
            ctx.real_type,
        );
    }
    time_phase_with_sink(timing_sink, "julia-codegen", || {
        generate_julia_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Lowers signals through the transform fast lane then emits Rust source.
///
/// Mirrors [`lower_signals_to_julia_transform_fastlane`]; both share the one
/// FIR lowering implementation.
pub(crate) fn lower_signals_to_rust_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &RustOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToRustError> {
    let module_name = resolve_module_name(options.class_name.as_deref(), source_name);
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    let mut codegen_options = options.clone();
    codegen_options.faust_float_type = match ctx.real_type {
        RealType::Float32 => RustRealType::Float32,
        RealType::Float64 => RustRealType::Float64,
    };
    time_phase_with_sink(timing_sink, "rust-codegen", || {
        generate_rust_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Lowers signals through the transform fast lane then emits AssemblyScript.
///
/// Mirrors [`lower_signals_to_rust_transform_fastlane`]; both share the one FIR
/// lowering implementation.
pub(crate) fn lower_signals_to_asc_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &AscOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToAscError> {
    let module_name = resolve_module_name(options.class_name.as_deref(), source_name);
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    let mut codegen_options = options.clone();
    codegen_options.double_precision = ctx.real_type == RealType::Float64;
    time_phase_with_sink(timing_sink, "asc-codegen", || {
        generate_asc_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Lowers signals to FIR and emits codebox text.
///
/// Takes `ctx` with the modes already forced by [`lower_signals_to_codebox`];
/// there is no `class_name` to resolve, because a codebox file is flat and
/// names no class.
pub(crate) fn lower_signals_to_codebox_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CodeboxOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToCodeboxError> {
    let module_name = resolve_module_name(None, source_name);
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    let mut codegen_options = options.clone();
    codegen_options.double_precision = ctx.real_type == RealType::Float64;
    time_phase_with_sink(timing_sink, "codebox-codegen", || {
        generate_codebox_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Lowers signals to FIR and emits one scalar Cmajor processor.
pub(crate) fn lower_signals_to_cmajor_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CmajorOptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToCmajorError> {
    let module_name = resolve_module_name(Some(&options.class_name), source_name);
    let timing_sink = ctx.timing_sink.as_ref();
    let lowered = time_phase_with_sink(timing_sink, "signal-fir", || {
        lower_signals_to_fir_transform_fastlane_with_timing(
            output,
            module_name,
            ctx.real_type,
            ctx.max_copy_delay,
            ctx.delay_line_threshold,
            ctx.compute_mode,
            ctx.scheduling_strategy,
            ctx.control_rate_mode,
            ctx.processing_api,
            ctx.table_init_mode,
            ctx.table_init_sample_rate,
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(|report| LowerError::Verify {
        report,
        origins: lowered.origins.clone(),
    })?;
    let mut codegen_options = options.clone();
    codegen_options.real_type = match ctx.real_type {
        RealType::Float32 => codegen::backends::cmajor::CmajorRealType::Float32,
        RealType::Float64 => codegen::backends::cmajor::CmajorRealType::Float64,
    };
    time_phase_with_sink(timing_sink, "cmajor-codegen", || {
        generate_cmajor_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(|error| LowerError::Codegen {
        error,
        origins: lowered.origins.clone(),
    })
}

/// Runs optional FIR verification according to the compiler facade policy.
///
/// In strict mode, warnings are promoted to fatal errors to support CI and
/// parity-audit workflows that want a clean FIR module before backend lowering.
pub(crate) fn maybe_verify_fir_module(
    lowered: &FirCompileOutput,
    options: FirVerifyOptions,
) -> Result<(), FirVerifyReport> {
    if !options.enabled {
        return Ok(());
    }
    let report = verify_fir_module(&lowered.store, lowered.module);
    let fatal = report.has_errors() || (options.strict && report.warnings().next().is_some());
    if fatal { Err(report) } else { Ok(()) }
}
