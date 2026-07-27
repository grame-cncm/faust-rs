//! The one thing every backend error has in common.
//!
//! Each backend defines its own error type, and they were not reachable
//! uniformly: five of them expose `code()` / `message()` as methods, while the
//! interpreter and Cranelift errors expose `code` / `message` as public fields.
//! Every consumer that wanted "the stable code and the message" therefore had
//! to know which backend it was holding — which is why the compiler facade
//! carried one nearly identical error mapper per backend.
//!
//! [`BackendCodegenError`] is that common surface, and nothing more. It
//! deliberately does not abstract emission, options, or failure *kinds*: those
//! genuinely differ per backend and unifying them would hide real differences.

/// A backend codegen error that can report its stable code and its message.
///
/// Implemented by every backend error type in [`crate::backends`]. The two
/// accessors are all a diagnostic renderer needs, so one generic mapper can
/// serve every backend instead of one hand-written mapper each.
pub trait BackendCodegenError {
    /// Stable machine-readable error code, e.g. `FRS-CGEN-CPP-0001`.
    ///
    /// Stable for tooling and tests: treat a change here as a contract change.
    fn code_str(&self) -> &'static str;

    /// Human-readable message. May evolve as diagnostics improve.
    fn message_str(&self) -> &str;
}

/// Implements [`BackendCodegenError`] for a backend error exposing `code()` and
/// `message()` as methods.
macro_rules! impl_backend_error_via_methods {
    ($error:ty) => {
        impl $crate::backend_error::BackendCodegenError for $error {
            fn code_str(&self) -> &'static str {
                self.code().as_str()
            }
            fn message_str(&self) -> &str {
                self.message()
            }
        }
    };
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

impl_backend_error_via_methods!(crate::backends::cpp::CodegenError);
impl_backend_error_via_methods!(crate::backends::c::CodegenError);
impl_backend_error_via_methods!(crate::backends::julia::CodegenError);
impl_backend_error_via_methods!(crate::backends::asc::CodegenError);
impl_backend_error_via_methods!(crate::backends::rust::CodegenError);
impl_backend_error_via_fields!(crate::backends::codebox::CodegenError);
impl_backend_error_via_fields!(crate::backends::interp::CodegenError);
#[cfg(not(target_arch = "wasm32"))]
impl_backend_error_via_fields!(crate::backends::cranelift::CraneliftBackendError);

#[cfg(test)]
mod tests {
    use super::BackendCodegenError;

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
}
