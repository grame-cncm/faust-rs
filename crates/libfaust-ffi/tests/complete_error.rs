//! `getCCompleteDSPError`: the whole text of a failure of the backend-agnostic
//! API (`expandCDSP*`, `generateCAuxFiles*`), beyond the 4096 bytes of
//! `error_msg`.
//!
//! Same contract as the two backend entry points (see
//! `ffi_common::complete_error`): per thread, owned by the library, null
//! before the thread's first error, valid until its next one, left alone by a
//! success. Every test runs on its own thread, which keeps them independent.

use std::ffi::{CStr, CString, c_char};

use faust_libfaust::{
    expandCDSPFromFile, expandCDSPFromString, generateCAuxFilesFromString2, getCCompleteDSPError,
};

/// An unclosed parenthesis on line 2, column 21.
const UNCLOSED: &str = "// a comment line\nprocess = _ : *(0.5 ;\n";

/// Expands `source`; returns whether it succeeded and what `error_msg` got.
fn expand(source: &str) -> (bool, String) {
    let name = CString::new("probe").unwrap();
    let content = CString::new(source).unwrap();
    let mut sha = [0 as c_char; 64];
    let mut buffer = [0 as c_char; 4096];
    let expanded = unsafe {
        expandCDSPFromString(
            name.as_ptr(),
            content.as_ptr(),
            0,
            std::ptr::null(),
            sha.as_mut_ptr(),
            buffer.as_mut_ptr(),
        )
    };
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (!expanded.is_null(), message)
}

fn complete() -> Option<String> {
    let text = getCCompleteDSPError();
    (!text.is_null()).then(|| {
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    })
}

#[test]
fn null_before_any_error_and_untouched_by_a_success() {
    assert_eq!(complete(), None);
    assert!(expand("process = _;").0);
    assert_eq!(complete(), None);
}

#[test]
fn a_syntax_error_is_located_and_shown() {
    let (ok, summary) = expand(UNCLOSED);
    assert!(!ok);
    // The buffer still gets what it always got: a count, no location.
    assert!(summary.starts_with("parse failed for"), "{summary}");
    assert!(!summary.contains("FRS-PARSE"), "{summary}");

    let complete = complete().expect("a failure publishes its complete text");
    assert!(complete.starts_with(&summary), "{complete}");
    assert!(
        complete.contains(":2:21: error [FRS-PARSE-0001]"),
        "{complete}"
    );
    assert!(
        complete.contains("  2 | process = _ : *(0.5 ;"),
        "{complete}"
    );
    assert!(complete.contains("insert `)`"), "{complete}");
}

#[test]
fn a_text_longer_than_the_buffer_is_whole() {
    let mut source = String::new();
    for index in 0..200 {
        source.push_str(&format!(
            "a_rather_long_definition_name_{index:03} = {index};\n"
        ));
    }
    source.push_str("process = _ : missing_symbol;\n");

    let (ok, summary) = expand(&source);
    assert!(!ok);
    assert!(summary.len() < 4096);
    let complete = complete().unwrap();
    assert!(complete.len() > 4096, "only {} bytes", complete.len());
    assert!(complete.contains("undefined symbol `missing_symbol`"));
    assert!(complete.contains("a_rather_long_definition_name_199"));
}

#[test]
fn an_error_without_diagnostics_is_its_message_and_inherits_nothing() {
    assert!(!expand(UNCLOSED).0);
    assert!(complete().unwrap().contains("FRS-PARSE"));

    // A file that does not exist: an I/O error, no compiler diagnostic.
    let missing = CString::new("/nonexistent/faust_rs_complete_error.dsp").unwrap();
    let mut sha = [0 as c_char; 64];
    let mut buffer = [0 as c_char; 4096];
    let expanded = unsafe {
        expandCDSPFromFile(
            missing.as_ptr(),
            0,
            std::ptr::null(),
            sha.as_mut_ptr(),
            buffer.as_mut_ptr(),
        )
    };
    assert!(expanded.is_null());
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    assert!(message.starts_with("cannot read"), "{message}");
    assert_eq!(complete().as_deref(), Some(message.as_str()));
}

#[test]
fn auxiliary_file_failures_carry_their_diagnostics_too() {
    let name = CString::new("probe").unwrap();
    let content = CString::new(UNCLOSED).unwrap();
    let args = [CString::new("-svg").unwrap()];
    let argv = [args[0].as_ptr()];
    let mut buffer = [0 as c_char; 4096];
    let text = unsafe {
        generateCAuxFilesFromString2(
            name.as_ptr(),
            content.as_ptr(),
            1,
            argv.as_ptr(),
            buffer.as_mut_ptr(),
        )
    };
    assert!(text.is_null());
    let summary = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
    let complete = complete().unwrap();
    assert!(complete.starts_with(summary.as_ref()), "{complete}");
    assert!(complete.contains("[FRS-PARSE-0001]"), "{complete}");
}

#[test]
fn a_success_does_not_reset_it_and_another_thread_does_not_see_it() {
    assert!(!expand(UNCLOSED).0);
    let before = complete().unwrap();
    assert!(expand("process = _;").0);
    assert_eq!(complete().as_deref(), Some(before.as_str()));

    let elsewhere = std::thread::spawn(complete).join().unwrap();
    assert_eq!(elsewhere, None);
}
