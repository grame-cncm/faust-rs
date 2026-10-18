//! Production parser API coverage on corpus and malformed fixtures.
//!
//! Scope:
//! - Verifies `crates/parser` public entry points on representative corpus inputs.
//! - Keeps end-to-end checks on the production API boundary (not parser internals).

use std::fs;
use std::path::Path;

use parser::parse_program;

#[test]
fn production_api_accepts_rep_corpus() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut files = fs::read_dir(&corpus)
        .expect("corpus directory should be readable")
        .map(|entry| entry.expect("corpus entry should be readable").path())
        .filter(|path| {
            let is_dsp = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dsp"));
            let is_rep = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rep_"));
            is_dsp && is_rep
        })
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "rep_*.dsp corpus should not be empty");

    for path in files {
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("corpus filename should be valid utf-8");
        let source = fs::read_to_string(&path).expect("corpus file should be readable");
        let out = parse_program(&source, file);
        assert!(
            out.errors.is_empty(),
            "{file} should parse without parser errors, got {:?}",
            out.errors
        );
        assert!(out.root.is_some(), "{file} should produce a parser root");
    }
}

#[test]
fn production_api_reports_malformed_inputs() {
    let malformed = [
        ("missing_rhs", "process = ;\n"),
        (
            "missing_rpar",
            "process = hslider(\"g\", 0.5, 0.0, 1.0, 0.01;\n",
        ),
        (
            "legacy_minput_modulation",
            "process = minput(\"gain\" : _).(_);\n",
        ),
        ("missing_enddef", "process = _\n"),
    ];

    for (name, source) in malformed {
        let out = parse_program(source, name);
        let rust_class_ok =
            out.root.is_some() && out.state.ctx.parse_error_count() == 0 && out.errors.is_empty();
        assert!(!rust_class_ok, "{name} should not parse as valid");
    }
}
