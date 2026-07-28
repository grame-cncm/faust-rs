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
use codegen::backend_error::BackendCodegenError;

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
    backend: &'static str,
    error: LowerError<E>,
    wrap: impl FnOnce(Box<str>, E, DiagnosticBundle) -> CompilerError,
) -> CompilerError {
    match error {
        LowerError::ExecutionOptions(error) => execution_error_to_compiler(source, backend, error),
        LowerError::Transform(error) => transform_error_to_compiler(source, error),
        LowerError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerError::Codegen(error) => {
            let diagnostics = CompilerError::codegen_diagnostics(
                source,
                backend,
                error.code_str(),
                error.message_str(),
                DiagnosticCategory::UnsupportedFeature,
            );
            wrap(source.into(), error, diagnostics)
        }
    }
}

/// Maps a `LowerToCppError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_cpp_error_to_compiler(source: &str, error: LowerToCppError) -> CompilerError {
    lower_error_to_compiler(source, "cpp", error, |source, error, diagnostics| {
        CompilerError::CodegenCpp {
            source,
            error,
            diagnostics,
        }
    })
}

/// Maps a `LowerToCError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_c_error_to_compiler(source: &str, error: LowerToCError) -> CompilerError {
    lower_error_to_compiler(source, "c", error, |source, error, diagnostics| {
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
    error: LowerToJuliaError,
) -> CompilerError {
    lower_error_to_compiler(source, "julia", error, |source, error, diagnostics| {
        CompilerError::CodegenJulia {
            source,
            error,
            diagnostics,
        }
    })
}

/// Maps a `LowerToAscError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_asc_error_to_compiler(source: &str, error: LowerToAscError) -> CompilerError {
    lower_error_to_compiler(source, "asc", error, |source, error, diagnostics| {
        CompilerError::CodegenAsc {
            source,
            error,
            diagnostics,
        }
    })
}

/// Maps a `LowerToCodeboxError` into a [`CompilerError`], attaching the source
/// name.
pub(crate) fn lower_codebox_error_to_compiler(
    source: &str,
    error: LowerToCodeboxError,
) -> CompilerError {
    lower_error_to_compiler(source, "codebox", error, |source, error, diagnostics| {
        CompilerError::CodegenCodebox {
            source,
            error,
            diagnostics,
        }
    })
}

/// Maps a `LowerToRustError` into a [`CompilerError`], attaching the source name.
pub(crate) fn lower_rust_error_to_compiler(source: &str, error: LowerToRustError) -> CompilerError {
    lower_error_to_compiler(source, "rust", error, |source, error, diagnostics| {
        CompilerError::CodegenRust {
            source,
            error,
            diagnostics,
        }
    })
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
    error: LowerToCraneliftError,
) -> CompilerError {
    match error {
        LowerToCraneliftError::ExecutionOptions(error) => {
            execution_error_to_compiler(source, "cranelift", error)
        }
        LowerToCraneliftError::Transform(error) => transform_error_to_compiler(source, error),
        LowerToCraneliftError::Verify(report) => fir_verify_error_to_compiler(source, report),
        LowerToCraneliftError::Codegen(error) => CompilerError::CodegenCranelift {
            source: source.into(),
            diagnostics: CompilerError::codegen_diagnostics(
                source,
                "cranelift",
                error.code.as_str(),
                &error.message,
                DiagnosticCategory::UnsupportedFeature,
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
                DiagnosticCategory::UnsupportedFeature,
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
        .map_err(|error| type_error_to_compiler(source, error))
}

/// Wraps a signal type validation error into the compiler facade error surface.
pub(crate) fn type_error_to_compiler(source: &str, error: InferenceError) -> CompilerError {
    let diagnostic = Diagnostic::new(
        Severity::Error,
        Stage::Compiler,
        COMP_TYPE_FAILED,
        error.0.clone(),
    )
    .with_category(DiagnosticCategory::UserCode)
    .with_detail_code("signal-type-inference")
    .with_note("stage=sigtype");
    CompilerError::Type {
        source: source.into(),
        error,
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
