//! Owned structured diagnostic state retained behind WASM result handles.
//!
//! This module deliberately contains no exported ABI symbols. It separates
//! compiler diagnostics from raw pointer/handle mechanics so a
//! [`CompilerError`] is converted into an owned [`DiagnosticBundle`] before
//! the foreign boundary can reduce it to compatibility text.
//!
//! # Source provenance
//!
//! This is an adapted Rust ownership layer for the error string returned by
//! the C++ `libFaustWasm` factory API. Unlike the historical text-only shape,
//! the record retains the Rust compiler's typed diagnostics-v2 data.

use compiler::diagnostics_json::{DiagnosticsRequestMetadata, SourceTextPolicy};
use compiler::{CompilerError, DiagnosticBundle};

/// One owned failure record associated with a specific compile-result handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FfiDiagnosticRecord {
    message: String,
    diagnostics: Option<DiagnosticBundle>,
    request: DiagnosticsRequestMetadata,
    source_text: SourceTextPolicy,
}

impl FfiDiagnosticRecord {
    /// Captures a typed compiler failure without parsing its display message.
    pub(crate) fn from_compiler_error(
        error: &CompilerError,
        request: DiagnosticsRequestMetadata,
    ) -> Self {
        Self {
            message: error.to_string(),
            diagnostics: Some(error.diagnostic_bundle().clone()),
            request,
            source_text: SourceTextPolicy::None,
        }
    }

    /// Captures an FFI transport or argument failure with no fabricated
    /// compiler diagnostic.
    pub(crate) fn transport(
        message: impl Into<String>,
        request: DiagnosticsRequestMetadata,
    ) -> Self {
        Self {
            message: message.into(),
            diagnostics: None,
            request,
            source_text: SourceTextPolicy::None,
        }
    }

    /// Returns the unchanged human-readable compatibility message.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// Returns the typed compiler bundle, if this is a compiler failure.
    #[cfg(test)]
    pub(crate) fn diagnostics(&self) -> Option<&DiagnosticBundle> {
        self.diagnostics.as_ref()
    }
}
