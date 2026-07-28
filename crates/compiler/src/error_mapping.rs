//! Backend-specific lower-error to `CompilerError` converters.
//!
//! Each `lower_*_error_to_compiler` function maps the three-variant
//! `LowerError<E>` type (Transform / Verify / Codegen) for a specific backend
//! (C++, C, Julia, interpreter, FIR) into the unified `CompilerError` enum
//! consumed by all public `Compiler` methods.
//!
//! Also contains `enrich_diagnostic_with_node` — attaches source-span context
//! to a diagnostic when the error carries an offending box or signal node —
//! and `make_propagate_compiler_error`, the propagate-error-to-`CompilerError`
//! adapter.

use super::*;
use codegen::backend_error::{BackendCodegenError, BackendFailureKind};

// ─── Helpers: error mapping ───────────────────────────────────────────────────

/// Maps a `LowerToCppError` into a `CompilerError`, attaching the source name.
///
/// This keeps the backend-specific lower pipeline internal while exposing one
/// stable facade error surface to callers.
/// Maps a capability-model rejection into the facade error surface with a
/// stable `FRS-EXEC-*` diagnostic bundle.
pub(crate) fn execution_error_to_compiler(
    source: &str,
    backend: &str,
    error: crate::execution::ExecutionOptionsError,
) -> CompilerError {
    CompilerError::ExecutionOptions {
        source: source.into(),
        diagnostics: CompilerError::codegen_diagnostics(
            source,
            backend,
            error.code(),
            &error.to_string(),
            DiagnosticCategory::InvalidOptions,
        ),
        error,
    }
}

/// Maps a `LowerError<E>` into a [`CompilerError`], attaching the source name.
///
/// Three of the four arms are identical for every backend, so they live here
/// once. Only the `Codegen` arm needs backend knowledge, and only twice:
/// `backend` names the emission in the diagnostic bundle, and `wrap` picks the
/// matching `CompilerError::Codegen*` variant — which stays per-backend so
/// callers can still match on a concrete backend error type.
///
/// The [`BackendCodegenError`] bound is what makes this generic possible: it
/// hides whether a backend reports its code and message through methods or
/// through public fields.
fn lower_error_to_compiler<E: BackendCodegenError>(
    source: &str,
    output: &SignalCompileOutput,
    backend: &'static str,
    error: LowerError<E>,
    wrap: impl FnOnce(Box<str>, E, DiagnosticBundle) -> CompilerError,
) -> CompilerError {
    let error = match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, backend, error),
        LowerError::Transform(error) => transform_error_to_compiler(source, output, error),
        LowerError::Verify { report, origins } => {
            fir_verify_error_to_compiler(source, output, report, &origins)
        }
        LowerError::Codegen { error, origins } => {
            let diagnostics = enrich_backend_bundle(
                CompilerError::codegen_diagnostics(
                    source,
                    backend,
                    error.code_str(),
                    error.message_str(),
                    match error.kind() {
                        BackendFailureKind::UnsupportedFeature => {
                            DiagnosticCategory::UnsupportedFeature
                        }
                        BackendFailureKind::CompilerInvariant => DiagnosticCategory::CompilerBug,
                    },
                ),
                output,
                &origins,
                error.fir_node(),
            );
            wrap(source.into(), error, diagnostics)
        }
    };
    error.with_source_map(output.parse.diagnostics.source_map().clone())
}

/// Maps a `LowerToCppError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_cpp_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToCppError,
) -> CompilerError {
    lower_error_to_compiler(
        source,
        output,
        "cpp",
        error,
        |source, error, diagnostics| CompilerError::CodegenCpp {
            source,
            error,
            diagnostics,
        },
    )
}

/// Maps a `LowerToCError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_c_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToCError,
) -> CompilerError {
    lower_error_to_compiler(source, output, "c", error, |source, error, diagnostics| {
        CompilerError::CodegenC {
            source,
            error,
            diagnostics,
        }
    })
}

/// Maps a `LowerToJuliaError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_julia_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToJuliaError,
) -> CompilerError {
    lower_error_to_compiler(
        source,
        output,
        "julia",
        error,
        |source, error, diagnostics| CompilerError::CodegenJulia {
            source,
            error,
            diagnostics,
        },
    )
}

/// Maps a `LowerToAscError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_asc_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToAscError,
) -> CompilerError {
    lower_error_to_compiler(
        source,
        output,
        "asc",
        error,
        |source, error, diagnostics| CompilerError::CodegenAsc {
            source,
            error,
            diagnostics,
        },
    )
}

/// Maps a `LowerToCodeboxError` into a [`CompilerError`], attaching the source
/// name.
pub(crate) fn lower_codebox_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToCodeboxError,
) -> CompilerError {
    lower_error_to_compiler(
        source,
        output,
        "codebox",
        error,
        |source, error, diagnostics| CompilerError::CodegenCodebox {
            source,
            error,
            diagnostics,
        },
    )
}

/// Maps a `LowerToRustError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_rust_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToRustError,
) -> CompilerError {
    lower_error_to_compiler(
        source,
        output,
        "rust",
        error,
        |source, error, diagnostics| CompilerError::CodegenRust {
            source,
            error,
            diagnostics,
        },
    )
}

#[cfg(not(target_arch = "wasm32"))]
/// Maps a `LowerToCraneliftError` into a [`CompilerError`], attaching the
/// source name.
///
/// Not routed through `lower_error_to_compiler`: this envelope is not a
/// `LowerError<E>`, because the subset-gap diagnosis and the JIT emission are
/// two fallible backend steps folded into one `Codegen` variant.
pub(crate) fn lower_cranelift_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToCraneliftError,
) -> CompilerError {
    let error = match error {
        LowerToCraneliftError::ExecutionOptions(error) => {
            execution_error_to_compiler(source, "cranelift", error)
        }
        LowerToCraneliftError::Transform(error) => {
            transform_error_to_compiler(source, output, error)
        }
        LowerToCraneliftError::Verify { report, origins } => {
            fir_verify_error_to_compiler(source, output, report, &origins)
        }
        LowerToCraneliftError::Codegen { error, origins } => CompilerError::CodegenCranelift {
            source: source.into(),
            diagnostics: enrich_backend_bundle(
                CompilerError::codegen_diagnostics(
                    source,
                    "cranelift",
                    error.code.as_str(),
                    &error.message,
                    DiagnosticCategory::UnsupportedFeature,
                ),
                output,
                &origins,
                error.fir_node(),
            ),
            error,
        },
    };
    error.with_source_map(output.parse.diagnostics.source_map().clone())
}

/// Maps a `LowerToInterpError` into a `CompilerError`, attaching the source name.
///
/// The serialization failure arm is normalized into the interpreter backend
/// error surface so CLI and library callers do not need a fourth dedicated
/// interpreter-specific error branch.
pub(crate) fn lower_interp_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToInterpError,
) -> CompilerError {
    let error = match error {
        LowerToInterpError::ExecutionOptions(error) => {
            execution_error_to_compiler(source, "interp", error)
        }
        LowerToInterpError::Transform(error) => transform_error_to_compiler(source, output, error),
        LowerToInterpError::Verify { report, origins } => {
            fir_verify_error_to_compiler(source, output, report, &origins)
        }
        LowerToInterpError::Codegen { error, origins } => CompilerError::CodegenInterp {
            source: source.into(),
            diagnostics: enrich_backend_bundle(
                CompilerError::codegen_diagnostics(
                    source,
                    "interp",
                    error.code.as_str(),
                    &error.message,
                    DiagnosticCategory::UnsupportedFeature,
                ),
                output,
                &origins,
                error.fir_node(),
            ),
            error,
        },
        LowerToInterpError::Serialize(message) => CompilerError::CodegenInterp {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "interp",
                InterpCodegenErrorCode::CompilationFailed.as_str(),
                &message,
                DiagnosticCategory::UnsupportedFeature,
            ),
            error: InterpCodegenError {
                code: InterpCodegenErrorCode::CompilationFailed,
                message,
            },
        },
    };
    error.with_source_map(output.parse.diagnostics.source_map().clone())
}

/// Maps a `LowerToFirError` into a `CompilerError`, attaching the source name.
pub(crate) fn lower_fir_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: LowerToFirError,
) -> CompilerError {
    let error = match error {
        LowerToFirError::ExecutionOptions(error) => {
            execution_error_to_compiler(source, "fir", error)
        }
        LowerToFirError::Transform(error) => transform_error_to_compiler(source, output, error),
        LowerToFirError::Verify { report, origins } => {
            fir_verify_error_to_compiler(source, output, report, &origins)
        }
    };
    error.with_source_map(output.parse.diagnostics.source_map().clone())
}

/// Maps a WASM/strict-JSON backend failure with the canonical FIR provenance.
///
/// The current WASM error type does not yet retain a precise `FirId`, so the
/// module root is used as a conservative trace anchor. Its derived origins are
/// bounded and deterministic and still identify the contributing Faust
/// construct(s).
pub(crate) fn wasm_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    lowered: &FirCompileOutput,
    error: WasmBackendError,
) -> CompilerError {
    let category = match error.kind() {
        BackendFailureKind::UnsupportedFeature => DiagnosticCategory::UnsupportedFeature,
        BackendFailureKind::CompilerInvariant => DiagnosticCategory::CompilerBug,
    };
    CompilerError::CodegenWasm {
        source: source.into(),
        diagnostics: enrich_backend_bundle(
            CompilerError::codegen_diagnostics(
                source,
                "wasm",
                error.code().as_str(),
                error.message(),
                category,
            ),
            output,
            &lowered.origins,
            Some(lowered.module),
        ),
        error,
    }
    .with_source_map(output.parse.diagnostics.source_map().clone())
}

/// Wraps a `SignalFirError` into a `CompilerError::Transform` with one diagnostic.
///
/// The diagnostic bundle is built by [`signal_fir_diagnostic`] which extracts
/// source location and note information from the transform error.
pub(crate) fn transform_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    error: SignalFirError,
) -> CompilerError {
    let mut diagnostic = signal_fir_diagnostic(&error);
    if let Some(signal) = error.signal() {
        diagnostic = diagnostic.with_debug_fact("signal_id", u64::from(signal.as_u32()));
    }
    diagnostic = add_box_origin_labels(diagnostic, error.box_origins(), output);
    CompilerError::Transform {
        source: source.into(),
        diagnostics: bundle_from_diagnostic(diagnostic),
        error: Box::new(error),
    }
}

/// Wraps a FIR verifier report into the facade error surface.
///
/// `strict` is recorded only for the warning-only case promoted to a failure by
/// compiler policy. Reports containing real verifier errors are always fatal,
/// independent from the strictness flag.
pub(crate) fn fir_verify_error_to_compiler(
    source: &str,
    output: &SignalCompileOutput,
    report: FirVerifyReport,
    origins: &transform::signal_fir::FirOrigins,
) -> CompilerError {
    let strict = report.warnings().next().is_some() && !report.has_errors();
    let diagnostics = fir_verify_bundle_from_report(&report)
        .as_slice()
        .iter()
        .zip(&report.diagnostics)
        .map(|(diagnostic, fir_diagnostic)| {
            let diagnostic =
                add_fir_trace(diagnostic.clone(), fir_diagnostic.node, origins, output);
            if fir_diagnostic.severity == FirVerifySeverity::Error {
                diagnostic
                    .with_category(DiagnosticCategory::CompilerBug)
                    .with_help(
                        "this is an internal FIR invariant failure; report a minimal reproducer",
                    )
            } else {
                diagnostic
            }
        })
        .collect::<Vec<_>>()
        .into();
    CompilerError::FirVerify {
        source: source.into(),
        strict,
        diagnostics,
    }
}

fn enrich_backend_bundle(
    bundle: DiagnosticBundle,
    output: &SignalCompileOutput,
    origins: &transform::signal_fir::FirOrigins,
    fir_node: Option<FirId>,
) -> DiagnosticBundle {
    let Some(fir_node) = fir_node else {
        return bundle;
    };
    bundle
        .as_slice()
        .iter()
        .cloned()
        .map(|diagnostic| add_fir_trace(diagnostic, fir_node, origins, output))
        .collect::<Vec<_>>()
        .into()
}

fn add_fir_trace(
    mut diagnostic: Diagnostic,
    fir_node: FirId,
    origins: &transform::signal_fir::FirOrigins,
    output: &SignalCompileOutput,
) -> Diagnostic {
    diagnostic = diagnostic.with_debug_fact("fir_node_id", u64::from(fir_node.as_u32()));
    if let Some(origin) = origins.origins_for(fir_node).first() {
        diagnostic = diagnostic.with_debug_fact("signal_id", u64::from(origin.signal.as_u32()));
    }
    add_box_origin_labels(diagnostic, &origins.box_origins_for(fir_node), output)
}

fn add_box_origin_labels(
    mut diagnostic: Diagnostic,
    box_origins: &[BoxId],
    output: &SignalCompileOutput,
) -> Diagnostic {
    for &box_node in box_origins {
        let owner = reachable_owner_definition_name_for_node(
            &output.parse.state.arena,
            output.definitions_root,
            box_node,
            &output.entrypoint_name,
        );
        if let Some(span) = owner.as_deref().and_then(|owner| {
            source_span_for_node_in_definition(
                &output.parse.state.ctx,
                &output.parse.state.arena,
                output.definitions_root,
                box_node,
                owner,
            )
        }) {
            diagnostic = diagnostic.with_label(
                Label::new(LabelStyle::Primary, span, "source of generated FIR")
                    .with_role(LabelRole::DerivedFrom),
            );
            return diagnostic;
        }
    }
    if let Some(&box_node) = box_origins.first() {
        return maybe_add_source_label(
            diagnostic,
            &output.parse.state.ctx,
            &output.parse.state.arena,
            output.definitions_root,
            box_node,
            reachable_owner_definition_name_for_node(
                &output.parse.state.arena,
                output.definitions_root,
                box_node,
                &output.entrypoint_name,
            )
            .as_deref(),
            &output.entrypoint_name,
        );
    }
    diagnostic
}

/// Runs canonical `sigtype` validation on propagated signals before later stages.
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_signal_types(
    source: &str,
    arena: &tlib::TreeArena,
    signals: &[SigId],
    ui: &UiProgram,
    signal_origins: &propagate::SignalOrigins,
    ctx: &parser::ParserCtx,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Result<DiagnosticBundle, CompilerError> {
    let mut annotator = TypeAnnotator::new(arena, ui);
    annotator.annotate(signals).map_err(|error| {
        type_error_to_compiler(
            source,
            error,
            arena,
            signal_origins,
            ctx,
            defs_root,
            entrypoint_name,
        )
    })?;

    // Inference always records its non-fatal observations; whether they reach
    // the user is the CLI's decision, not this function's.
    let mut warnings = DiagnosticBundle::new();
    for warning in annotator.warnings() {
        warnings.push(type_warning_to_diagnostic(
            warning,
            arena,
            signal_origins,
            ctx,
            defs_root,
            entrypoint_name,
        ));
    }
    Ok(warnings)
}

/// Renders one non-fatal inference observation as a warning diagnostic.
///
/// Shares the source-labeling and typed-fact vocabulary of
/// [`type_error_to_compiler`] so a warning and an error about the same rule
/// read the same way and expose the same machine fields.
fn type_warning_to_diagnostic(
    warning: &sigtype::InferenceWarning,
    arena: &tlib::TreeArena,
    signal_origins: &propagate::SignalOrigins,
    ctx: &parser::ParserCtx,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Diagnostic {
    let sigtype::InferenceWarning::PotentialMathDomain {
        signal,
        operand,
        operation,
        actual,
        required,
    } = warning;

    let diagnostic = Diagnostic::new(
        Severity::Warning,
        Stage::TypeInference,
        COMP_TYPE_FAILED,
        warning.message(),
    )
    .with_category(DiagnosticCategory::UserCode)
    .with_detail_code(warning.rule().as_str())
    .with_note("cause: the operand interval extends outside the operation's domain")
    .with_note(format!(
        "rule: {operation} requires its operand to stay within {required}"
    ))
    .with_note(format!("computed: inferred operand interval = {actual}"))
    .with_fact("inference_rule", warning.rule().as_str())
    .with_fact("operation", operation.clone())
    .with_fact("expected", required.clone())
    .with_fact("actual_interval", interval_fact(*actual))
    .with_fact("potential_runtime_failure", true)
    .with_debug_fact("signal_id", u64::from(signal.as_u32()))
    .with_debug_fact("operand_signal_id", u64::from(operand.as_u32()))
    .with_debug_fact("signal_expr", signals::dump_sig_readable(arena, *signal))
    .with_help(format!(
        "constrain the operand to {required}, for example with `max`/`min`, so the domain holds for every sample"
    ));

    add_signal_source_labels(
        diagnostic,
        *signal,
        signal_origins,
        ctx,
        arena,
        defs_root,
        entrypoint_name,
    )
}

/// Wraps a signal type validation error into the compiler facade error surface.
#[allow(clippy::too_many_arguments)]
pub(crate) fn type_error_to_compiler(
    source: &str,
    error: InferenceError,
    arena: &tlib::TreeArena,
    signal_origins: &propagate::SignalOrigins,
    ctx: &parser::ParserCtx,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> CompilerError {
    let mut diagnostic = Diagnostic::new(
        Severity::Error,
        Stage::TypeInference,
        COMP_TYPE_FAILED,
        error.message(),
    )
    .with_category(DiagnosticCategory::UserCode)
    .with_detail_code(error.rule().as_str())
    .with_note("cause: an inferred signal type or interval violates a typing rule")
    .with_note(format!("rule: {}", error.rule().statement()))
    .with_fact("inference_rule", error.rule().as_str());

    // A structural failure is a compiler invariant, not a DSP mistake, and its
    // help must not suggest the programmer did something wrong.
    if matches!(
        error.rule(),
        sigtype::InferenceRule::SignalStructure | sigtype::InferenceRule::RecursiveGroup
    ) {
        diagnostic = diagnostic.with_category(DiagnosticCategory::CompilerBug);
    }

    if let Some(actual) = error.actual_type() {
        diagnostic = diagnostic.with_fact("actual_type", actual.to_string());
    }
    if let Some(actual) = error.actual_interval() {
        diagnostic = diagnostic.with_fact("actual_interval", interval_fact(actual));
    }
    if let Some((min, max)) = error.required_integer_interval() {
        diagnostic = diagnostic.with_fact(
            "required_interval",
            DiagnosticValue::IntegerRange {
                min: i64::from(min),
                max: i64::from(max),
            },
        );
    }
    if let Some(expected) = error.expected() {
        diagnostic = diagnostic.with_fact("expected", expected);
    }
    let intervals = error.actual_intervals();
    if !intervals.is_empty() {
        let rendered = intervals
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let label = if intervals.len() > 1 {
            "inferred operand intervals"
        } else {
            "inferred interval"
        };
        diagnostic = diagnostic.with_note(match error.expected() {
            Some(expected) => format!("computed: {label} = {rendered}, expected {expected}"),
            None => format!("computed: {label} = {rendered}"),
        });
    } else if let Some(actual) = error.actual_type() {
        diagnostic = diagnostic.with_note(match error.expected() {
            Some(expected) => format!("computed: inferred type = {actual}, expected {expected}"),
            None => format!("computed: inferred type = {actual}"),
        });
    }
    diagnostic = diagnostic.with_help(error.rule().help());
    let operands = error.operands();
    if !operands.is_empty() {
        diagnostic = diagnostic.with_debug_fact(
            "operand_signal_ids",
            operands
                .iter()
                .map(|sig| sig.as_u32().to_string())
                .collect::<Vec<_>>(),
        );
    }
    if let Some(signal) = error.signal() {
        diagnostic = diagnostic
            .with_debug_fact("signal_id", u64::from(signal.as_u32()))
            .with_debug_fact("signal_expr", signals::dump_sig_readable(arena, signal));
        diagnostic = add_signal_source_labels(
            diagnostic,
            signal,
            signal_origins,
            ctx,
            arena,
            defs_root,
            entrypoint_name,
        );
    }
    CompilerError::Type {
        source: source.into(),
        error: Box::new(error),
        diagnostics: bundle_from_diagnostic(diagnostic),
    }
}

fn interval_fact(interval: interval::Interval) -> DiagnosticValue {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        FactKey::new("min"),
        DiagnosticValue::Real(interval.lo().to_string().into()),
    );
    fields.insert(
        FactKey::new("max"),
        DiagnosticValue::Real(interval.hi().to_string().into()),
    );
    fields.insert(
        FactKey::new("lsb"),
        DiagnosticValue::Integer(i64::from(interval.lsb())),
    );
    DiagnosticValue::Object(fields)
}

fn add_signal_source_labels(
    mut diagnostic: Diagnostic,
    signal: SigId,
    signal_origins: &propagate::SignalOrigins,
    ctx: &parser::ParserCtx,
    arena: &tlib::TreeArena,
    defs_root: BoxId,
    entrypoint_name: &str,
) -> Diagnostic {
    for &box_node in signal_origins.origins_for(signal) {
        let owner =
            reachable_owner_definition_name_for_node(arena, defs_root, box_node, entrypoint_name);
        let exact = owner.as_deref().and_then(|owner| {
            source_span_for_node_in_definition(ctx, arena, defs_root, box_node, owner)
        });
        if let Some(span) = exact {
            diagnostic = diagnostic.with_label(
                Label::new(LabelStyle::Primary, span, "source expression")
                    .with_role(LabelRole::DerivedFrom),
            );
            if let Some(owner) = owner
                && let Some(definition) =
                    source_span_for_definition_name(ctx, arena, defs_root, &owner)
            {
                diagnostic = diagnostic.with_label(
                    Label::new(LabelStyle::Secondary, definition, "enclosing definition")
                        .with_role(LabelRole::DefinitionSite),
                );
            }
            return diagnostic;
        }
    }

    if let Some(&box_node) = signal_origins.origins_for(signal).first() {
        return maybe_add_source_label(
            diagnostic,
            ctx,
            arena,
            defs_root,
            box_node,
            reachable_owner_definition_name_for_node(arena, defs_root, box_node, entrypoint_name)
                .as_deref(),
            entrypoint_name,
        );
    }
    diagnostic
}

// ─── DiagCtx: shared pipeline diagnostic enrichment ──────────────────────────

/// Builds a `CompilerError::Propagate` with standard node-level enrichment.
///
/// Used by the three propagate-stage steps in `pipeline_to_signals`
/// (flat-box boundary, arity inference, signal propagation) which share the
/// same enrichment policy.  Set `add_paired` for composition errors
/// (seq/split/merge/rec) that benefit from paired A/B arity context.
pub(crate) fn make_propagate_compiler_error(
    source: &str,
    error: propagate::PropagateError,
    arena: &tlib::TreeArena,
    ctx: &parser::ParserCtx,
    root: BoxId,
    entrypoint_name: &str,
    add_paired: bool,
) -> CompilerError {
    let node = propagate_error_node(&error);
    let owner = node
        .and_then(|n| reachable_owner_definition_name_for_node(arena, root, n, entrypoint_name));
    let mut diagnostic = error.to_diagnostic();
    if let Some(n) = node {
        diagnostic = enrich_diagnostic_with_node(
            diagnostic,
            arena,
            root,
            n,
            owner.as_deref(),
            entrypoint_name,
        );
        if add_paired {
            diagnostic = add_paired_propagate_context(diagnostic, &error, arena);
        }
        diagnostic = maybe_add_source_label(
            diagnostic,
            ctx,
            arena,
            root,
            n,
            owner.as_deref(),
            entrypoint_name,
        );
    }
    CompilerError::Propagate {
        source: source.into(),
        error,
        diagnostics: bundle_from_diagnostic(diagnostic),
    }
}

/// Enriches a diagnostic with the standard node-level notes shared across
/// eval, arity, and propagate error handlers.
///
/// Takes the arena and root by reference at call-site (not stored) so that
/// mutable borrows of the arena remain possible between phase calls.
pub(crate) fn enrich_diagnostic_with_node(
    mut diagnostic: Diagnostic,
    arena: &tlib::TreeArena,
    root: BoxId,
    node: BoxId,
    owner: Option<&str>,
    entrypoint_name: &str,
) -> Diagnostic {
    diagnostic = diagnostic
        .with_note(format!("node_id={}", node.as_u32()))
        .with_note(format!("box_expr={}", compact_box_preview(arena, node)))
        .with_note(format!("expr={}", compact_human_box_preview(arena, node)))
        .with_debug_fact("node_id", u64::from(node.as_u32()))
        .with_debug_fact("box_expr", compact_box_preview(arena, node));
    if let Some(owner) = owner {
        diagnostic = diagnostic
            .with_note(format!("error originates from definition '{owner}'"))
            .with_fact("owner_definition", owner);
    }
    if let Some(trace) = alias_binding_trace_for_node(arena, root, node, entrypoint_name) {
        let path = trace.split(" -> ").map(str::to_owned).collect::<Vec<_>>();
        diagnostic = diagnostic
            .with_note(format!("binding_trace={trace}"))
            .with_fact("binding_trace_path", path);
    }
    diagnostic
}
