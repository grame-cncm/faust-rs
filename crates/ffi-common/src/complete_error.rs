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

use std::cell::RefCell;
use std::ffi::{CString, c_char};

/// Per-thread record of the last reported error. See the module documentation.
#[derive(Debug, Default)]
pub struct CompleteError {
    /// Rendered diagnostics waiting for the report of their summary.
    attached: RefCell<Option<(String, String)>>,
    /// What the getter returns.
    published: RefCell<Option<CString>>,
}

impl CompleteError {
    /// An empty record, usable as a `thread_local!` const initializer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            attached: RefCell::new(None),
            published: RefCell::new(None),
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
            Some((summary.to_owned(), details.to_owned()))
        };
    }

    /// Publishes the complete text of the error whose message is `message`.
    ///
    /// Attached diagnostics are appended when `message` contains their
    /// summary (it may have been wrapped in a prefix on the way), and dropped
    /// either way: they describe one failure and must not leak into the next.
    /// The text never ends with a newline, with or without diagnostics, so a
    /// host prints it the way it prints `error_msg`.
    pub fn report(&self, message: &str) {
        let complete = match self.attached.borrow_mut().take() {
            Some((summary, details)) if message.contains(summary.as_str()) => {
                format!("{message}\n{}", details.trim_end())
            }
            _ => message.to_owned(),
        };
        let sanitized = complete.replace('\0', "\\0");
        *self.published.borrow_mut() = CString::new(sanitized).ok();
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
