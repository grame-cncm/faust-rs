//! The complete text of the last error, beyond the 4096-byte `error_msg` buffer.
//!
//! The Faust C API reports a failure through a caller-allocated buffer whose
//! size it never receives: the contract is the documented constant, 4096
//! bytes, and [`crate::write_error_4096`] truncates to it. A compiler
//! diagnostic rendered for a terminal (location, source snippet, notes, fixes)
//! does not always fit, and an existing host cannot be handed more than it
//! allocated. So the buffer keeps receiving the summary it always received,
//! and each backend adds one entry point, `getCComplete<Backend>DSPFactoryError`,
//! that returns the whole text from storage the library owns.
//!
//! # Contract of the returned pointer
//!
//! The storage is per thread, as `dlerror` does it: the pointer is the
//! complete text of the last error *this thread's* calls reported through an
//! `error_msg` buffer, null while there has been none, and valid until the
//! next such report on the same thread. A call that succeeds leaves it alone,
//! so it is to be read after a call that failed, not instead of checking one.
//!
//! # How a backend uses it
//!
//! One `thread_local!` [`CompleteError`] per backend. Where a typed compiler
//! error is flattened to its one-line summary, [`CompleteError::attach`]
//! records the rendered diagnostics that summary stands for; the backend's
//! `write_error` funnel calls [`CompleteError::report`] with the message it
//! writes to the buffer, which publishes that message followed by the attached
//! diagnostics when the message carries the summary, and the message alone
//! otherwise (argument errors, I/O errors: the buffer already has everything).
//!
//! # The typed channel
//!
//! The rendered text is for a person. The same failure has a typed form, the
//! compiler's diagnostics-v2 JSON report (code, byte ranges, facts,
//! machine-applicable fixes), which a host applies a fix from without reading
//! prose. [`CompleteError::attach_with_diagnostics`] records it with the
//! text, and it is published under the same rule and with the same lifetime
//! (`getC<Backend>...ErrorDiagnostics`), with one difference that follows from
//! what it is: **an error that carries no typed diagnostics publishes none**,
//! so the pointer is null after an argument error even if an earlier failure
//! had a report. A document never outlives the failure it describes.

use std::cell::RefCell;
use std::ffi::{CString, c_char};

/// Per-thread record of the last reported error. See the module documentation.
#[derive(Debug, Default)]
pub struct CompleteError {
    /// What waits for the report of its summary.
    attached: RefCell<Option<Attached>>,
    /// What the text getter returns.
    published: RefCell<Option<CString>>,
    /// What the diagnostics getter returns.
    published_diagnostics: RefCell<Option<CString>>,
}

/// The two forms of one failure's diagnostics, with the summary they wait for.
#[derive(Debug)]
struct Attached {
    summary: String,
    /// Rendered for a terminal; may be empty.
    details: String,
    /// The diagnostics-v2 JSON report, when the failure is typed.
    diagnostics: Option<String>,
}

impl CompleteError {
    /// An empty record, usable as a `thread_local!` const initializer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            attached: RefCell::new(None),
            published: RefCell::new(None),
            published_diagnostics: RefCell::new(None),
        }
    }

    /// Records that `summary`, about to travel as a plain string, stands for
    /// `details`, the rendered diagnostics of the same failure.
    ///
    /// Nothing is published yet: an error attached and then recovered from
    /// never reaches the host, exactly as it never reaches the buffer.
    pub fn attach(&self, summary: &str, details: &str) {
        *self.attached.borrow_mut() = if details.is_empty() {
            None
        } else {
            Some(Attached {
                summary: summary.to_owned(),
                details: details.to_owned(),
                diagnostics: None,
            })
        };
    }

    /// [`Self::attach`] for a typed failure: `diagnostics` is its
    /// diagnostics-v2 JSON report, published with the text by the report of
    /// `summary`.
    pub fn attach_with_diagnostics(&self, summary: &str, details: &str, diagnostics: &str) {
        *self.attached.borrow_mut() = Some(Attached {
            summary: summary.to_owned(),
            details: details.to_owned(),
            diagnostics: Some(diagnostics.to_owned()),
        });
    }

    /// Publishes the complete text of the error whose message is `message`.
    ///
    /// Attached diagnostics are appended when `message` contains their
    /// summary (it may have been wrapped in a prefix on the way), and dropped
    /// either way: they describe one failure and must not leak into the next.
    /// The text never ends with a newline, with or without diagnostics, so a
    /// host prints it the way it prints `error_msg`.
    ///
    /// The typed report is published when the attached failure had one and
    /// `message` carries its summary, and **cleared otherwise**: this error
    /// has none, and the previous one's is not this one's.
    pub fn report(&self, message: &str) {
        let attached = self
            .attached
            .borrow_mut()
            .take()
            .filter(|attached| message.contains(attached.summary.as_str()));
        let (complete, diagnostics) = match attached {
            Some(attached) => {
                let details = attached.details.trim_end();
                let complete = if details.is_empty() {
                    message.to_owned()
                } else {
                    format!("{message}\n{details}")
                };
                (complete, attached.diagnostics)
            }
            None => (message.to_owned(), None),
        };
        let sanitized = complete.replace('\0', "\\0");
        *self.published.borrow_mut() = CString::new(sanitized).ok();
        // JSON escapes a NUL as \u0000: a report holds no interior NUL
        *self.published_diagnostics.borrow_mut() =
            diagnostics.and_then(|report| CString::new(report).ok());
    }

    /// The published text, or null. Valid until the next [`Self::report`] on
    /// this thread.
    #[must_use]
    pub fn as_ptr(&self) -> *const c_char {
        self.published
            .borrow()
            .as_ref()
            .map_or(std::ptr::null(), |text| text.as_ptr())
    }

    /// The published diagnostics-v2 JSON report, or null when the last
    /// reported error carried none. Valid until the next [`Self::report`] on
    /// this thread.
    #[must_use]
    pub fn diagnostics_ptr(&self) -> *const c_char {
        self.published_diagnostics
            .borrow()
            .as_ref()
            .map_or(std::ptr::null(), |report| report.as_ptr())
    }

    /// [`Self::diagnostics_ptr`] for a Rust caller.
    #[must_use]
    pub fn diagnostics(&self) -> Option<String> {
        self.published_diagnostics
            .borrow()
            .as_ref()
            .map(|report| report.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::CompleteError;
    use std::ffi::CStr;

    fn text(record: &CompleteError) -> Option<String> {
        let ptr = record.as_ptr();
        (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned())
    }

    #[test]
    fn nothing_is_published_before_a_report() {
        let record = CompleteError::new();
        record.attach("parse failed", "2:21: error");
        assert_eq!(text(&record), None);
    }

    #[test]
    fn the_report_of_a_summary_publishes_its_details() {
        let record = CompleteError::new();
        record.attach("parse failed", "2:21: error");
        record.report("parse failed");
        assert_eq!(text(&record).as_deref(), Some("parse failed\n2:21: error"));
    }

    #[test]
    fn the_text_never_ends_with_a_newline() {
        let record = CompleteError::new();
        record.attach("parse failed", "2:21: error\n  = fix: insert `)`\n\n");
        record.report("parse failed");
        assert_eq!(
            text(&record).as_deref(),
            Some("parse failed\n2:21: error\n  = fix: insert `)`")
        );
    }

    #[test]
    fn a_wrapped_summary_still_gets_its_details() {
        let record = CompleteError::new();
        record.attach("parse failed", "2:21: error");
        record.report("effect: parse failed");
        assert_eq!(
            text(&record).as_deref(),
            Some("effect: parse failed\n2:21: error")
        );
    }

    #[test]
    fn details_never_leak_into_another_error() {
        let record = CompleteError::new();
        record.attach("parse failed", "2:21: error");
        record.report("null signals pointer");
        assert_eq!(text(&record).as_deref(), Some("null signals pointer"));
        // and they are gone, even for a later report of the same summary
        record.report("parse failed");
        assert_eq!(text(&record).as_deref(), Some("parse failed"));
    }

    fn diagnostics(record: &CompleteError) -> Option<String> {
        let ptr = record.diagnostics_ptr();
        (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned())
    }

    #[test]
    fn a_typed_failure_publishes_its_report_with_its_text() {
        let record = CompleteError::new();
        record.attach_with_diagnostics("parse failed", "2:21: error", "{\"schema_version\":2}");
        assert_eq!(diagnostics(&record), None, "nothing before the report");
        record.report("effect: parse failed");
        assert_eq!(
            text(&record).as_deref(),
            Some("effect: parse failed\n2:21: error")
        );
        assert_eq!(
            diagnostics(&record).as_deref(),
            Some("{\"schema_version\":2}")
        );
        assert_eq!(record.diagnostics(), diagnostics(&record));
    }

    #[test]
    fn a_report_does_not_outlive_the_failure_it_describes() {
        let record = CompleteError::new();
        record.attach_with_diagnostics("parse failed", "2:21: error", "{}");
        record.report("parse failed");
        assert!(diagnostics(&record).is_some());
        // an error without typed diagnostics: its text, and no report, not
        // the previous one's
        record.report("null signals pointer");
        assert_eq!(text(&record).as_deref(), Some("null signals pointer"));
        assert_eq!(diagnostics(&record), None);
        // nor does a report attached to another failure leak into this one
        record.attach_with_diagnostics("parse failed", "2:21: error", "{}");
        record.report("cannot read file");
        assert_eq!(diagnostics(&record), None);
        // a failure with rendered text only has no report either
        record.attach("evaluation failed", "3:1: error");
        record.report("evaluation failed");
        assert_eq!(diagnostics(&record), None);
    }

    #[test]
    fn a_typed_failure_without_rendered_text_still_has_its_report() {
        let record = CompleteError::new();
        record.attach_with_diagnostics("transform failed", "", "{\"status\":\"failed\"}");
        record.report("transform failed");
        assert_eq!(text(&record).as_deref(), Some("transform failed"));
        assert_eq!(
            diagnostics(&record).as_deref(),
            Some("{\"status\":\"failed\"}")
        );
    }

    #[test]
    fn a_text_longer_than_the_error_buffer_is_kept_whole() {
        let record = CompleteError::new();
        let details = "x".repeat(20_000);
        record.attach("evaluation failed", &details);
        record.report("evaluation failed");
        assert_eq!(
            text(&record).unwrap().len(),
            "evaluation failed\n".len() + 20_000
        );
    }
}
