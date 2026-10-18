//! The one thing every backend error has in common.
//!
//! Each backend defines its own error type, and they were not reachable
//! uniformly: eight of them expose `code()` / `message()` as methods, while the
//! interpreter and Cranelift errors expose `code` / `message` as public fields.
//! Every consumer that wanted "the stable code and the message" therefore had
//! to know which backend it was holding — which is why the compiler facade
//! carried one nearly identical error mapper per backend.
//!
//! [`BackendCodegenError`] is that common surface. Besides code/message, it
//! classifies unsupported features versus compiler invariants and optionally
//! exposes the rejected FIR node for diagnostic provenance. It
//! deliberately does not abstract emission, options, or failure *kinds*: those
//! genuinely differ per backend and unifying them would hide real differences.

/// A backend codegen error that can report its stable code and its message.
///
/// Implemented by every backend error type in [`crate::backends`]. The two
/// accessors plus typed classification and optional FIR identity let one
/// generic mapper serve every backend instead of one hand-written mapper each.
use fir::FirId;

use crate::backends::codegen_error::{BackendError, CodegenErrorCode};

/// Backend-independent classification of an emission failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendFailureKind {
    /// The backend deliberately does not implement a valid FIR construct.
    UnsupportedFeature,
    /// The FIR/backend boundary violated an internal compiler invariant.
    CompilerInvariant,
}

pub trait BackendCodegenError {
    /// Stable machine-readable error code, e.g. `FRS-CGEN-CPP-0001`.
    ///
    /// Stable for tooling and tests: treat a change here as a contract change.
    fn code_str(&self) -> &'static str;

    /// Human-readable message. May evolve as diagnostics improve.
    fn message_str(&self) -> &str;

    /// Typed classification used by the compiler diagnostic facade.
    fn kind(&self) -> BackendFailureKind {
        match self.code_str() {
            "FRS-CGEN-CPP-0003"
            | "FRS-CGEN-C-0003"
            | "FRS-CGEN-JULIA-0003"
            | "FRS-CGEN-ASC-0003"
            | "FRS-CGEN-RUST-0003"
            | "FRS-CGEN-CBOX-0002"
            | "FRS-CGEN-CMAJ-0002"
            | "FRS-CGEN-WASM-0003" => BackendFailureKind::UnsupportedFeature,
            _ => BackendFailureKind::CompilerInvariant,
        }
    }

    /// Offending FIR node when the backend retained one.
    fn fir_node(&self) -> Option<FirId> {
        None
    }
}

/// Implements [`BackendCodegenError`] for a backend error exposing `code` and
/// `message` as public fields.
macro_rules! impl_backend_error_via_fields {
    ($error:ty) => {
        impl $crate::backend_error::BackendCodegenError for $error {
            fn code_str(&self) -> &'static str {
                self.code.as_str()
            }
            fn message_str(&self) -> &str {
                &self.message
            }
        }
    };
}

/// Every carrier built on [`BackendError`] — the seven textual backends and
/// wasm — reports its FIR provenance. One impl instead of one per backend: the
/// per-backend bridges forwarded `fir_node` for `cpp` only, so a node attached
/// by any other backend was dropped on the way to the diagnostic.
impl<C: CodegenErrorCode> BackendCodegenError for BackendError<C> {
    fn code_str(&self) -> &'static str {
        self.code().as_str()
    }

    fn message_str(&self) -> &str {
        self.message()
    }

    fn fir_node(&self) -> Option<FirId> {
        BackendError::fir_node(self)
    }
}
impl_backend_error_via_fields!(crate::backends::interp::CodegenError);
#[cfg(not(target_arch = "wasm32"))]
impl_backend_error_via_fields!(crate::backends::cranelift::CraneliftBackendError);

#[cfg(test)]
mod tests {
    use super::{BackendCodegenError, BackendFailureKind};
    use fir::{FirBuilder, FirStore};

    /// Both accessor shapes must reach the same strings, so a generic consumer
    /// cannot tell a method-based backend from a field-based one.
    #[test]
    fn both_accessor_shapes_report_code_and_message() {
        let via_methods = crate::backends::cpp::CodegenError::new(
            crate::backends::cpp::CodegenErrorCode::UnsupportedNode,
            "boom",
        );
        assert!(via_methods.code_str().starts_with("FRS-CGEN-"));
        assert_eq!(via_methods.message_str(), "boom");

        let via_fields = crate::backends::interp::CodegenError {
            code: crate::backends::interp::CodegenErrorCode::CompilationFailed,
            message: "boom".to_owned(),
        };
        assert!(via_fields.code_str().starts_with("FRS-CGEN-"));
        assert_eq!(via_fields.message_str(), "boom");
    }

    #[test]
    fn cpp_unsupported_error_retains_typed_kind_and_fir_node() {
        let mut store = FirStore::new();
        let node = FirBuilder::new(&mut store).int32(1);
        let error = crate::backends::cpp::CodegenError::new(
            crate::backends::cpp::CodegenErrorCode::UnsupportedNode,
            "unsupported",
        )
        .at_node(node);

        assert_eq!(error.kind(), BackendFailureKind::UnsupportedFeature);
        assert_eq!(BackendCodegenError::fir_node(&error), Some(node));

        let invariant = crate::backends::cpp::CodegenError::new(
            crate::backends::cpp::CodegenErrorCode::RootNotModule,
            "not a module",
        );
        assert_eq!(invariant.kind(), BackendFailureKind::CompilerInvariant);
    }

    /// The bridge forwarded `fir_node` for `cpp` only; every carrier built on
    /// `BackendError` must report the node it was given.
    #[test]
    fn every_shared_carrier_forwards_its_fir_node() {
        let mut store = FirStore::new();
        let node = FirBuilder::new(&mut store).int32(1);
        fn forwarded(error: &dyn BackendCodegenError) -> Option<fir::FirId> {
            error.fir_node()
        }
        use crate::backends::{asc, c, cmajor, codebox, julia, rust, wasm};
        let nodes = [
            forwarded(
                &c::CodegenError::new(c::CodegenErrorCode::UnsupportedNode, "x").at_node(node),
            ),
            forwarded(
                &julia::CodegenError::new(julia::CodegenErrorCode::UnsupportedNode, "x")
                    .at_node(node),
            ),
            forwarded(
                &asc::CodegenError::new(asc::CodegenErrorCode::UnsupportedNode, "x").at_node(node),
            ),
            forwarded(
                &rust::CodegenError::new(rust::CodegenErrorCode::UnsupportedNode, "x")
                    .at_node(node),
            ),
            forwarded(
                &codebox::CodegenError::new(codebox::CodegenErrorCode::Unsupported, "x")
                    .at_node(node),
            ),
            forwarded(
                &cmajor::CodegenError::new(cmajor::CodegenErrorCode::Unsupported, "x")
                    .at_node(node),
            ),
            forwarded(
                &wasm::WasmBackendError::new(wasm::WasmBackendErrorCode::UnsupportedFirNode, "x")
                    .at_node(node),
            ),
        ];
        assert!(nodes.iter().all(|n| *n == Some(node)), "{nodes:?}");
    }
}
