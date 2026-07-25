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
        ),
        error,
    }
}

pub(crate) fn lower_cpp_error_to_compiler(source: &str, error: LowerToCppError) -> CompilerError {
    match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, "cpp", error),
        LowerError::Transform(error) => transform_error_to_compiler(source, error),
        LowerError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerError::Codegen(error) => CompilerError::CodegenCpp {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "cpp",
                error.code().as_str(),
                error.message(),
            ),
            error,
        },
    }
}

/// Maps a `LowerToCError` into a `CompilerError`, attaching the source name.
///
/// Each backend-specific error variant is mapped to the matching `CompilerError`
/// variant so callers never need to depend on the internal lower-pipeline types.
pub(crate) fn lower_c_error_to_compiler(source: &str, error: LowerToCError) -> CompilerError {
    match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, "c", error),
        LowerError::Transform(error) => transform_error_to_compiler(source, error),
        LowerError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerError::Codegen(error) => CompilerError::CodegenC {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "c",
                error.code().as_str(),
                error.message(),
            ),
            error,
        },
    }
}

/// Maps a `LowerToJuliaError` into a `CompilerError`, attaching the source name.
pub(crate) fn lower_julia_error_to_compiler(
    source: &str,
    error: LowerToJuliaError,
) -> CompilerError {
    match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, "julia", error),
        LowerError::Transform(error) => transform_error_to_compiler(source, error),
        LowerError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerError::Codegen(error) => CompilerError::CodegenJulia {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "julia",
                error.code().as_str(),
                error.message(),
            ),
            error,
        },
    }
}

/// Maps a `LowerToAscError` into a `CompilerError`, attaching the source name.
pub(crate) fn lower_asc_error_to_compiler(source: &str, error: LowerToAscError) -> CompilerError {
    match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, "asc", error),
        LowerError::Transform(error) => transform_error_to_compiler(source, error),
        LowerError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerError::Codegen(error) => CompilerError::CodegenAsc {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "asc",
                error.code().as_str(),
                error.message(),
            ),
            error,
        },
    }
}

/// Maps a `LowerToRustError` into a `CompilerError`, attaching the source name.
pub(crate) fn lower_rust_error_to_compiler(source: &str, error: LowerToRustError) -> CompilerError {
    match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, "rust", error),
        LowerError::Transform(error) => transform_error_to_compiler(source, error),
        LowerError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerError::Codegen(error) => CompilerError::CodegenRust {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "rust",
                error.code().as_str(),
                error.message(),
            ),
            error,
        },
    }
}

/// Maps a `LowerToInterpError` into a `CompilerError`, attaching the source name.
///
/// The serialization failure arm is normalized into the interpreter backend
/// error surface so CLI and library callers do not need a fourth dedicated
/// interpreter-specific error branch.
pub(crate) fn lower_interp_error_to_compiler(
    source: &str,
    error: LowerToInterpError,
) -> CompilerError {
    match error {
        LowerToInterpError::ExecutionOptions(error) => {
            execution_error_to_compiler(source, "interp", error)
        }
        LowerToInterpError::Transform(error) => transform_error_to_compiler(source, error),
        LowerToInterpError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerToInterpError::Codegen(error) => CompilerError::CodegenInterp {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "interp",
                error.code.as_str(),
                &error.message,
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
            ),
            error: InterpCodegenError {
                code: InterpCodegenErrorCode::CompilationFailed,
                message,
            },
        },
    }
}

/// Maps a `LowerToFirError` into a `CompilerError`, attaching the source name.
pub(crate) fn lower_fir_error_to_compiler(source: &str, error: LowerToFirError) -> CompilerError {
    match error {
        LowerToFirError::ExecutionOptions(error) => {
            execution_error_to_compiler(source, "fir", error)
        }
        LowerToFirError::Transform(error) => transform_error_to_compiler(source, error),
        LowerToFirError::Verify(report) => fir_verify_error_to_compiler(source, report),
    }
}

/// Wraps a `SignalFirError` into a `CompilerError::Transform` with one diagnostic.
///
/// The diagnostic bundle is built by [`signal_fir_diagnostic`] which extracts
/// source location and note information from the transform error.
pub(crate) fn transform_error_to_compiler(source: &str, error: SignalFirError) -> CompilerError {
    let diagnostic = signal_fir_diagnostic(&error);
    CompilerError::Transform {
        source: source.into(),
        diagnostics: bundle_from_diagnostic(diagnostic),
        error,
    }
}

/// Wraps a FIR verifier report into the facade error surface.
///
/// `strict` is recorded only for the warning-only case promoted to a failure by
/// compiler policy. Reports containing real verifier errors are always fatal,
/// independent from the strictness flag.
pub(crate) fn fir_verify_error_to_compiler(source: &str, report: FirVerifyReport) -> CompilerError {
    let strict = report.warnings().next().is_some() && !report.has_errors();
    CompilerError::FirVerify {
        source: source.into(),
        strict,
        diagnostics: fir_verify_bundle_from_report(&report),
    }
}

/// Runs canonical `sigtype` validation on propagated signals before later stages.
pub(crate) fn validate_signal_types(
    source: &str,
    arena: &tlib::TreeArena,
    signals: &[SigId],
    ui: &UiProgram,
) -> Result<(), CompilerError> {
    let mut annotator = TypeAnnotator::new(arena, ui);
    annotator
        .annotate(signals)
        .map(|_| ())
        .map_err(|error| type_error_to_compiler(source, error.0))
}

/// Wraps a signal type validation error into the compiler facade error surface.
pub(crate) fn type_error_to_compiler(source: &str, error: String) -> CompilerError {
    let diagnostic = Diagnostic::new(
        Severity::Error,
        Stage::Compiler,
        COMP_TYPE_FAILED,
        error.clone(),
    )
    .with_note("stage=sigtype");
    CompilerError::Type {
        source: source.into(),
        error: error.into_boxed_str(),
        diagnostics: bundle_from_diagnostic(diagnostic),
    }
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
    let owner = node.and_then(|n| owner_definition_name_for_node(arena, root, n));
    let mut diagnostic = error.clone().into_diagnostic();
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
        .with_note(format!("expr={}", compact_human_box_preview(arena, node)));
    if let Some(owner) = owner {
        diagnostic = diagnostic.with_note(format!("error originates from definition '{owner}'"));
    }
    if let Some(trace) = alias_binding_trace_for_node(arena, root, node, entrypoint_name) {
        diagnostic = diagnostic.with_note(format!("binding_trace={trace}"));
    }
    diagnostic
}
