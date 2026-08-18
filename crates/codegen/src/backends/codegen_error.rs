//! The carrier every textual backend's `CodegenError` is made of.
//!
//! Each backend owns a *different* set of error codes — `asc`, `julia` and
//! `rust` have three, `c` and `cpp` add `MemoryLayout`, `cmajor` and `codebox`
//! have their own entirely — but the thing carrying a code and a message was
//! written out once per backend. Four of those seven carriers were
//! byte-identical, and the other three differed only in whether they exposed
//! `code`/`message` as fields or as accessors.
//!
//! That accident is visible from outside: [`crate::backend_error`] needs two
//! macros, `impl_backend_error_via_methods!` and `impl_backend_error_via_fields!`,
//! purely to bridge the two shapes.
//!
//! [`BackendError`] is the one carrier, generic over the backend's own code
//! enum. A backend declares its codes, implements [`CodegenErrorCode`] on them,
//! and aliases its `CodegenError` to this type.

use std::fmt;

use fir::FirId;

/// A backend's own error-code enum, rendered as a stable string.
///
/// The string is the machine-readable contract (`FRS-CGEN-CPP-0001` and
/// friends); treat a change to it as a contract change.
pub trait CodegenErrorCode: Copy + fmt::Debug {
    /// Stable machine-readable spelling of this code.
    fn as_str(&self) -> &'static str;
}

/// One backend emission failure: a code, a message, and optional FIR provenance.
///
/// `fir_node` records which FIR node was rejected when the backend knows it.
/// Only the `cpp` backend populated it before this type existed; every backend
/// can now attach it, and consumers that do not care simply ignore `None`.
#[derive(Clone, Debug)]
pub struct BackendError<C: CodegenErrorCode> {
    code: C,
    message: String,
    fir_node: Option<FirId>,
}

impl<C: CodegenErrorCode> BackendError<C> {
    /// Builds an error with no FIR provenance attached.
    pub fn new(code: C, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fir_node: None,
        }
    }

    /// Attaches the FIR node this failure is about.
    #[must_use]
    pub fn at_node(mut self, node: FirId) -> Self {
        self.fir_node = Some(node);
        self
    }

    /// Stable machine-readable code.
    #[must_use]
    pub fn code(&self) -> C {
        self.code
    }

    /// Human-readable message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The rejected FIR node, when the backend recorded one.
    #[must_use]
    pub const fn fir_node(&self) -> Option<FirId> {
        self.fir_node
    }
}

impl<C: CodegenErrorCode> fmt::Display for BackendError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl<C: CodegenErrorCode> std::error::Error for BackendError<C> {}
