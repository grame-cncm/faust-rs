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

// ─── Signal-to-FIR lower errors ───────────────────────────────────────────────

/// Generic lower-to-backend error for backends that follow the
/// Transform → Verify → Codegen pattern.
///
/// `E` is the backend-specific codegen error type.
/// Specialised as [`LowerToCppError`] and [`LowerToCError`].
#[derive(Debug)]
pub(crate) enum LowerError<E> {
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify(FirVerifyReport),
    /// Backend emission failed after successful FIR lowering.
    Codegen(E),
}

/// Lower error for the C++ backend.
pub(crate) type LowerToCppError = LowerError<CodegenError>;
/// Lower error for the C backend.
pub(crate) type LowerToCError = LowerError<CCodegenError>;
/// Lower error for the Julia backend.
pub(crate) type LowerToJuliaError = LowerError<JuliaCodegenError>;

#[derive(Debug)]
pub(crate) enum LowerToInterpError {
    /// Fast-lane signal-to-FIR lowering failed.
    Transform(SignalFirError),
    /// Optional FIR verification rejected the lowered module.
    Verify(FirVerifyReport),
    /// Interpreter backend emission failed after successful lowering.
    Codegen(InterpCodegenError),
    /// Serialization of the factory to `.fbc` text failed.
    Serialize(String),
}

#[derive(Debug)]
pub(crate) enum LowerToFirError {
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
    /// `compute()` codegen strategy (scalar / vector). Roadmap P6 (V1 plumbing).
    pub(crate) compute_mode: ComputeMode,
    /// Signal/loop dependency scheduling policy (`-ss` /
    /// `--scheduling-strategy`). Vectorization port plan phase P2: plumbing
    /// only, threaded through to [`SignalFirOptions`] without activating
    /// scheduling.
    pub(crate) scheduling_strategy: SchedulingStrategy,
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
    let _ = ctx.lane;
    lower_signals_to_julia_transform_fastlane(source_name, output, options, &ctx)
}

/// Dispatches interpreter lowering through the selected signal->FIR lane.
pub(crate) fn lower_signals_to_interp(
    source_name: &str,
    output: &SignalCompileOutput,
    options: &InterpOptions,
    ctx: SignalLoweringContext,
) -> Result<String, LowerToInterpError> {
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
) -> Result<FirCompileOutput, LowerToFirError> {
    let module_name = sanitize_cpp_ident(source_name_to_class(source_name).as_str());
    let lowered = lower_signals_to_fir_transform_fastlane(
        output,
        module_name,
        real_type,
        max_copy_delay,
        delay_line_threshold,
        compute_mode,
        scheduling_strategy,
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
pub(crate) fn lower_signals_to_fir_transform_fastlane(
    output: &SignalCompileOutput,
    module_name: String,
    real_type: RealType,
    max_copy_delay: u32,
    delay_line_threshold: u32,
    compute_mode: ComputeMode,
    scheduling_strategy: SchedulingStrategy,
) -> Result<FirCompileOutput, SignalFirError> {
    lower_signals_to_fir_transform_fastlane_with_timing(
        output,
        module_name,
        real_type,
        max_copy_delay,
        delay_line_threshold,
        compute_mode,
        scheduling_strategy,
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
    timing_sink: Option<&TimingSink>,
) -> Result<FirCompileOutput, SignalFirError> {
    let signal_fir_options = SignalFirOptions {
        module_name,
        real_type,
        max_copy_delay,
        delay_line_threshold,
        compute_mode,
        scheduling_strategy,
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
    Ok(FirCompileOutput {
        store: lowered.store,
        module: lowered.module,
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
