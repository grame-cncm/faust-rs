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
    Verify(FirVerifyReport),
    /// Backend emission failed after successful FIR lowering.
    Codegen(E),
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

#[derive(Debug)]
pub(crate) enum LowerToInterpError {
    /// Execution-option request rejected by the capability model.
    ExecutionOptions(ExecutionOptionsError),
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify(FirVerifyReport),
    /// Interpreter backend emission failed after successful lowering.
    Codegen(InterpCodegenError),
    /// Serialization of the factory to `.fbc` text failed.
    Serialize(String),
}

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
    Verify(FirVerifyReport),
    /// Cranelift JIT emission (or its subset diagnosis) failed.
    Codegen(CraneliftBackendError),
}

#[derive(Debug)]
pub(crate) enum LowerToFirError {
    /// Execution-option request rejected by the capability model.
    ExecutionOptions(ExecutionOptionsError),
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify(FirVerifyReport),
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
    /// Which signal→FIR lowering lane to use (currently only transform fast-lane).
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

/// Dispatches C++ lowering through the selected signal->FIR lane.
///
/// The backend itself always consumes FIR; the lane choice controls only how
/// the intermediate FIR module is produced from the propagated signal list.
pub(crate) fn lower_signals_to_cpp(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &CppOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToCppError> {
    validate_execution_options(
        "cpp",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerError::ExecutionOptions)?;
    let _ = ctx.lane;
    lower_signals_to_cpp_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches C lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_c(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &COptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToCError> {
    validate_execution_options(
        "c",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerError::ExecutionOptions)?;
    let _ = ctx.lane;
    lower_signals_to_c_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches Julia lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_julia(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &JuliaOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToJuliaError> {
    validate_execution_options(
        "julia",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerError::ExecutionOptions)?;
    let _ = ctx.lane;
    lower_signals_to_julia_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches AssemblyScript lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_asc(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &AscOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToAscError> {
    validate_execution_options(
        "asc",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerError::ExecutionOptions)?;
    let _ = ctx.lane;
    lower_signals_to_asc_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches Rust lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_rust(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &RustOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToRustError> {
    validate_execution_options(
        "rust",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerError::ExecutionOptions)?;
    let _ = ctx.lane;
    lower_signals_to_rust_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches interpreter lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_interp(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &InterpOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToInterpError> {
    validate_execution_options(
        "interp",
        ctx.control_rate_mode,
        ctx.processing_api,
        ctx.compute_mode,
    )
    .map_err(LowerToInterpError::ExecutionOptions)?;
    let _ = ctx.lane;
    lower_signals_to_interp_transform_fastlane(source_name, output, options, &ctx)
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
            timing_sink,
        )
    })
    .map_err(LowerToInterpError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerToInterpError::Verify)?;
    match ctx.real_type {
        RealType::Float32 => {
            let factory: FbcDspFactory<f32> =
                time_phase_with_sink(timing_sink, "interp-codegen", || {
                    generate_interp_module(&lowered.store, lowered.module, options)
                })
                .map_err(LowerToInterpError::Codegen)?;
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
                .map_err(LowerToInterpError::Codegen)?;
            time_phase_with_sink(timing_sink, "interp-serialize", || {
                serialize_factory(&factory)
            })
            .map_err(LowerToInterpError::Serialize)
        }
    }
}

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
            timing_sink,
        )
    })
    .map_err(LowerToCraneliftError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerToCraneliftError::Verify)?;

    // A subset gap is a lowering *limitation* to report, not a failure: the
    // module still compiles, with an unlowered compute body. Only a hard
    // backend error stops the report.
    let subset_gap = diagnose_cranelift_compute_subset_gap(&lowered.store, lowered.module)
        .map_err(LowerToCraneliftError::Codegen)?;
    let compiled = time_phase_with_sink(timing_sink, "cranelift-codegen", || {
        generate_cranelift_module(&lowered.store, lowered.module, options)
    })
    .map_err(LowerToCraneliftError::Codegen)?;
    Ok(render_cranelift_module_report(
        &compiled,
        subset_gap.as_deref(),
    ))
}

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

/// Lowers propagated signals to FIR without invoking a backend emitter.
///
/// This is the shared implementation behind FIR dump/verification flows and is
/// also used as the backend-independent boundary for lane comparisons.
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
) -> Result<FirCompileOutput, LowerToFirError> {
    validate_execution_options("fir", control_rate_mode, processing_api, compute_mode)
        .map_err(LowerToFirError::ExecutionOptions)?;
    let module_name = sanitize_cpp_ident(source_name_to_class(source_name).as_str());
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
    )
    .map_err(LowerToFirError::Transform)?;
    maybe_verify_fir_module(&lowered, fir_verify).map_err(LowerToFirError::Verify)?;
    Ok(lowered)
}

/// Resolves a module name from explicit class_name option or from the source name.
pub(crate) fn resolve_module_name(class_name: Option<&str>, _source_name: &str) -> String {
    class_name
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "mydsp".to_owned())
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
    };
    let lowered = transform::signal_fir::compile_signals_to_fir_fastlane_clocked_with_timing(
        &output.parse.state.arena,
        &output.signals,
        output.process_arity.inputs,
        output.propagated_output_count(),
        &output.ui,
        &output.clock_domains,
        &signal_fir_options,
        timing_sink.map(|sink| sink.as_ref()),
    )?;
    // Canonicalize every FIR artifact before it reaches any backend or FIR
    // dump. This is deliberately independent of `--no-fir-verify`: pure Drop
    // roots are construction scaffolding, not an optional backend optimization.
    let (store, module) = sweep_scaffolding_drop_roots(&lowered.store, lowered.module);
    Ok(FirCompileOutput {
        store,
        module,
        vector_pipeline_status: lowered.vector_pipeline_status,
        vector_effective_mode: lowered.vector_effective_mode,
        vector_pipeline_detail: lowered.vector_pipeline_detail,
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
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerError::Verify)?;
    time_phase_with_sink(timing_sink, "cpp-codegen", || {
        generate_cpp_module(&lowered.store, lowered.module, options)
    })
    .map_err(LowerError::Codegen)
}

/// Lowers signals through the transform fast lane, verifies FIR, then emits C.
pub(crate) fn lower_signals_to_c_transform_fastlane(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &COptions,
    ctx: &SignalLoweringContext,
) -> Result<String, LowerToCError> {
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
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerError::Verify)?;
    time_phase_with_sink(timing_sink, "c-codegen", || {
        generate_c_module(&lowered.store, lowered.module, options)
    })
    .map_err(LowerError::Codegen)
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
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerError::Verify)?;
    let mut codegen_options = options.clone();
    codegen_options.real_type = match ctx.real_type {
        RealType::Float32 => JuliaRealType::Float32,
        RealType::Float64 => JuliaRealType::Float64,
    };
    time_phase_with_sink(timing_sink, "julia-codegen", || {
        generate_julia_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(LowerError::Codegen)
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
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerError::Verify)?;
    let mut codegen_options = options.clone();
    codegen_options.faust_float_type = match ctx.real_type {
        RealType::Float32 => RustRealType::Float32,
        RealType::Float64 => RustRealType::Float64,
    };
    time_phase_with_sink(timing_sink, "rust-codegen", || {
        generate_rust_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(LowerError::Codegen)
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
            timing_sink,
        )
    })
    .map_err(LowerError::Transform)?;
    time_phase_with_sink(timing_sink, "fir-verify", || {
        maybe_verify_fir_module(&lowered, ctx.fir_verify)
    })
    .map_err(LowerError::Verify)?;
    let mut codegen_options = options.clone();
    codegen_options.double_precision = ctx.real_type == RealType::Float64;
    time_phase_with_sink(timing_sink, "asc-codegen", || {
        generate_asc_module(&lowered.store, lowered.module, &codegen_options)
    })
    .map_err(LowerError::Codegen)
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
