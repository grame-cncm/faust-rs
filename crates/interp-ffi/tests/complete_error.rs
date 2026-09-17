//! `getCCompleteInterpreterDSPFactoryError`: the whole text of a failure, beyond
//! the 4096 bytes of `error_msg`.
//!
//! The C API's error buffer is caller-allocated and its size is a documented
//! constant, so it cannot grow without overflowing existing hosts. It keeps
//! receiving the summary; the complete text (summary, then the compiler's
//! rendered diagnostics) is read from library-owned, per-thread storage. Every
//! test runs on its own thread, which is what keeps them independent.

use std::ffi::{CStr, CString, c_char};

use faust_interp::factory::{
    createCInterpreterDSPFactoryFromString, deleteCInterpreterDSPFactory,
    expandCInterpreterDSPFromString, getCCompleteInterpreterDSPFactoryError,
};

/// Compiles `source`; returns whether it succeeded and what `error_msg` got.
fn compile(source: Option<&str>) -> (bool, String) {
    let name = CString::new("probe").unwrap();
    let content = source.map(|text| CString::new(text).unwrap());
    let mut buffer = [0 as c_char; 4096];
    let factory = unsafe {
        createCInterpreterDSPFactoryFromString(
            name.as_ptr(),
            content
                .as_ref()
                .map_or(std::ptr::null(), |text| text.as_ptr()),
            0,
            std::ptr::null(),
            buffer.as_mut_ptr(),
        )
    };
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if factory.is_null() {
        (false, message)
    } else {
        unsafe { deleteCInterpreterDSPFactory(factory) };
        (true, message)
    }
}

fn complete() -> Option<String> {
    let text = getCCompleteInterpreterDSPFactoryError();
    (!text.is_null()).then(|| {
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    })
}

const UNCLOSED: &str = "process = _ : *(0.5 ;\n";

#[test]
fn null_before_any_error_and_untouched_by_a_success() {
    assert_eq!(complete(), None);
    assert!(compile(Some("process = _;")).0);
    assert_eq!(complete(), None);
}

#[test]
fn a_syntax_error_is_located_and_shown() {
    let (ok, summary) = compile(Some(UNCLOSED));
    assert!(!ok);
    // The buffer still gets what it always got: a count, no location.
    assert!(summary.starts_with("parse failed for"), "{summary}");
    assert!(!summary.contains("FRS-PARSE"), "{summary}");

    let complete = complete().expect("a failure publishes its complete text");
    assert!(complete.starts_with(&summary), "{complete}");
    assert!(
        complete.contains(":1:21: error [FRS-PARSE-0001]"),
        "{complete}"
    );
    assert!(
        complete.contains("  1 | process = _ : *(0.5 ;"),
        "{complete}"
    );
    assert!(complete.contains("insert `)`"), "{complete}");
}

#[test]
fn a_text_longer_than_the_buffer_is_whole() {
    // An undefined symbol lists the visible scope; two hundred long names
    // make that list longer than the buffer.
    let mut source = String::new();
    for index in 0..200 {
        source.push_str(&format!(
            "a_rather_long_definition_name_{index:03} = {index};\n"
        ));
    }
    source.push_str("process = _ : missing_symbol;\n");

    let (ok, summary) = compile(Some(&source));
    assert!(!ok);
    assert!(summary.len() < 4096);
    let complete = complete().unwrap();
    assert!(complete.len() > 4096, "only {} bytes", complete.len());
    assert!(complete.contains("undefined symbol `missing_symbol`"));
    assert!(complete.contains("a_rather_long_definition_name_199"));
    assert!(
        complete
            .trim_end()
            .ends_with("define target before first use"),
        "{complete}"
    );
}

#[test]
fn an_error_without_diagnostics_is_its_message_and_inherits_nothing() {
    assert!(!compile(Some(UNCLOSED)).0);
    assert!(complete().unwrap().contains("FRS-PARSE"));

    let (ok, message) = compile(None);
    assert!(!ok);
    assert!(!message.is_empty());
    assert_eq!(complete().as_deref(), Some(message.as_str()));
}

#[test]
fn a_success_does_not_reset_it_and_another_thread_does_not_see_it() {
    assert!(!compile(Some(UNCLOSED)).0);
    let before = complete().unwrap();
    assert!(compile(Some("process = _;")).0);
    assert_eq!(complete().as_deref(), Some(before.as_str()));

    let elsewhere = std::thread::spawn(complete).join().unwrap();
    assert_eq!(elsewhere, None);
}

#[test]
fn expansion_failures_carry_their_diagnostics_too() {
    let name = CString::new("probe").unwrap();
    let content = CString::new(UNCLOSED).unwrap();
    let mut sha = [0 as c_char; 64];
    let mut buffer = [0 as c_char; 4096];
    let expanded = unsafe {
        expandCInterpreterDSPFromString(
            name.as_ptr(),
            content.as_ptr(),
            0,
            std::ptr::null(),
            sha.as_mut_ptr(),
            buffer.as_mut_ptr(),
        )
    };
    assert!(expanded.is_null());
    let summary = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
    let complete = complete().unwrap();
    assert!(complete.starts_with(summary.as_ref()), "{complete}");
    assert!(complete.contains("[FRS-PARSE-0001]"), "{complete}");
}
