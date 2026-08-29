//! Fuzz the top-level `.dsp` parser entry point: any input, valid or not,
//! must produce a `ParseOutput` (possibly with errors) without panicking.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        let _ = parser::parse_program(source, "fuzz.dsp");
    }
});
