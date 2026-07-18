//! Integration tests for `diagnostic_errors`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use std::fs;
use std::path::PathBuf;

use compiler::Compiler;
use signals::{SigMatch, match_sig};

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn read_corpus(file: &str) -> String {
    let path = corpus_path(file);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

#[test]
fn parse_error_fixture_exposes_frs_parse_code() {
    let compiler = Compiler::new();
    let source = read_corpus("err_01_parse_missing_rhs.dsp");
    let err = compiler
        .compile_source("err_01_parse_missing_rhs.dsp", &source)
        .expect_err("parse error fixture should fail parse stage");

    let diagnostics = err
        .diagnostics()
        .expect("parse error should expose diagnostics");
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|d| d.code.0.starts_with("FRS-PARSE-"))
    );
}

#[test]
fn eval_error_fixture_exposes_frs_eval_code() {
    let compiler = Compiler::new();
    let source = read_corpus("err_02_eval_missing_process.dsp");
    let err = compiler
        .compile_source_to_signals("err_02_eval_missing_process.dsp", &source)
        .expect_err("eval error fixture should fail eval stage");

    let diagnostics = err
        .diagnostics()
        .expect("eval error should expose diagnostics");
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|d| d.code.0.starts_with("FRS-EVAL-"))
    );
    let first = diagnostics
        .as_slice()
        .first()
        .expect("eval diagnostics should not be empty");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.contains("available top-level definitions")),
        "missing-process diagnostics should include top-level definition context"
    );
}

#[test]
fn eval_error_fixtures_expose_source_labels_and_readable_context() {
    let compiler = Compiler::new();
    let fixtures = [
        (
            "err_09_eval_undefined_symbol.dsp",
            1u32,
            "error originates from definition 'foo'",
        ),
        (
            "err_10_eval_too_many_arguments.dsp",
            2u32,
            "error originates from definition 'process'",
        ),
        (
            "err_12_eval_case_no_match.dsp",
            1u32,
            "error originates from definition 'foo'",
        ),
        (
            "err_13_eval_undefined_symbol_alias_chain_nested.dsp",
            1u32,
            "error originates from definition 'foo'",
        ),
    ];

    for (file, expected_line, owner_note) in fixtures {
        let source = read_corpus(file);
        let err = match compiler.compile_source_to_signals(file, &source) {
            Ok(_) => panic!("{file} should fail in eval stage"),
            Err(err) => err,
        };
        let diagnostics = err
            .diagnostics()
            .unwrap_or_else(|| panic!("{file} should expose diagnostics"));
        assert!(
            diagnostics
                .as_slice()
                .iter()
                .any(|d| d.code.0.starts_with("FRS-EVAL-")),
            "{file} should expose FRS-EVAL-* code"
        );
        let first = diagnostics
            .as_slice()
            .first()
            .unwrap_or_else(|| panic!("{file} should produce one diagnostic"));
        let primary = first
            .labels
            .first()
            .unwrap_or_else(|| panic!("{file} should expose one source label"));
        assert_eq!(
            primary.span.line, expected_line,
            "{file} should point to expected source line"
        );
        assert!(
            first.notes.iter().any(|n| n.starts_with("expr=")),
            "{file} should include readable expression context"
        );
        assert!(
            first.notes.iter().any(|n| n.as_ref() == owner_note),
            "{file} should expose owner definition note"
        );
    }
}

#[test]
fn diverging_recursive_case_reports_eval_error_instead_of_aborting() {
    // This test deliberately drives unbounded recursion and relies on the
    // *default* eval budget (1024) tripping before the 64 MiB thread stack
    // overflows. A raised ambient `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH` explicitly
    // opts into real OS-stack overflow (documented in
    // `crates/eval/src/loop_detector.rs`): the budget then no longer protects
    // this stack, so the process would SIGABRT here. Skip gracefully in that
    // case rather than aborting the whole test binary — the raised budget is a
    // user choice for compiling deep programs (e.g. large FFTs), not a
    // regression. Run the suite with the variable unset for full coverage.
    const TEST_STACK_SAFE_MAX_DEPTH: usize = 1_024;
    if let Some(raised) = std::env::var("FAUST_RS_DEFAULT_EVAL_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&d| d > TEST_STACK_SAFE_MAX_DEPTH)
    {
        eprintln!(
            "skipping diverging_recursive_case: FAUST_RS_DEFAULT_EVAL_MAX_DEPTH={raised} \
             exceeds the {TEST_STACK_SAFE_MAX_DEPTH}-frame budget this test's 64 MiB stack \
             is sized for (unset it to run this case)"
        );
        return;
    }

    let source = r#"
fact(1) = 1;
fact(n) = n * fact(n-1);

process = par(i, 3, fact(i));
"#;

    std::thread::Builder::new()
        .name("recursive-case-stack-overflow".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let compiler = Compiler::new();
            let err = compiler
                .compile_source_to_signals("fact_stack_overflow.dsp", source)
                .expect_err("missing factorial base case for fact(0) should fail in eval stage");

            let diagnostics = err
                .diagnostics()
                .expect("recursive eval failure should expose diagnostics");
            let first = diagnostics
                .as_slice()
                .first()
                .expect("recursive eval failure should produce one diagnostic");
            assert!(
                first.code.0.starts_with("FRS-EVAL-"),
                "recursive eval failure should stay in eval stage"
            );
            assert!(
                first.message.contains("stack overflow in eval"),
                "diagnostic should mirror C++ stack-overflow wording, got: {}",
                first.message
            );
            assert!(
                first.notes.iter().any(|n| n.contains("missing base case"))
                    || first
                        .help
                        .iter()
                        .any(|h| h.contains("non-decreasing recursive call"))
                    || first.help.iter().any(|h| h.contains("missing base case")),
                "diagnostic should explain likely recursive-definition cause"
            );
        })
        .expect("spawn worker")
        .join()
        .expect("worker thread should finish");
}

#[test]
fn eval_undefined_symbol_exposes_binding_trace() {
    let compiler = Compiler::new();
    let source = read_corpus("err_09_eval_undefined_symbol.dsp");
    let err = compiler
        .compile_source_to_signals("err_09_eval_undefined_symbol.dsp", &source)
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("eval error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("eval diagnostics should not be empty");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref() == "binding_trace=process -> foo"),
        "undefined symbol diagnostics should include alias-resolution trace"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref().starts_with("scope.local=")),
        "undefined symbol diagnostics should include local scope context"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref().starts_with("scope.visible=")),
        "undefined symbol diagnostics should include visible scope context"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref().starts_with("scope.top_level=")),
        "undefined symbol diagnostics should include top-level scope context"
    );
}

#[test]
fn eval_undefined_symbol_exposes_multi_label_call_and_definition_sites() {
    let compiler = Compiler::new();
    let source = read_corpus("err_13_eval_undefined_symbol_alias_chain_nested.dsp");
    let err = compiler
        .compile_source_to_signals(
            "err_13_eval_undefined_symbol_alias_chain_nested.dsp",
            &source,
        )
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("eval error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("eval diagnostics should not be empty");
    assert!(
        !first.labels.is_empty(),
        "eval undefined-symbol diagnostics should expose at least one source label"
    );
    assert_eq!(first.labels[0].message.as_ref(), "definition site");
    assert_eq!(first.labels[0].span.line, 1);
    if first.labels.len() >= 2 {
        assert_eq!(first.labels[1].message.as_ref(), "call site");
        assert_eq!(first.labels[1].span.line, 4);
    } else {
        assert!(
            first
                .notes
                .iter()
                .any(|n| n.as_ref().starts_with("error originates from definition ")),
            "single-label fallback should still expose owning definition context"
        );
    }
}

#[test]
fn eval_undefined_symbol_alias_chain_exposes_rule_computed_and_template_help() {
    let compiler = Compiler::new();
    let source = read_corpus("err_13_eval_undefined_symbol_alias_chain_nested.dsp");
    let err = compiler
        .compile_source_to_signals(
            "err_13_eval_undefined_symbol_alias_chain_nested.dsp",
            &source,
        )
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("eval error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("eval diagnostics should not be empty");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref().starts_with("rule: referenced identifier")),
        "undefined-symbol diagnostics should expose rule note first-class"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref().starts_with("computed: `z` is not present")),
        "undefined-symbol diagnostics should expose computed note"
    );
    assert!(
        first
            .help
            .iter()
            .any(|h| h.as_ref().starts_with("template: z = ...;")),
        "undefined-symbol diagnostics should expose deterministic correction template"
    );
}

#[test]
fn eval_compound_fixture_now_lowers_through_case_semantics() {
    let compiler = Compiler::new();
    let source = read_corpus("err_15_eval_compound_with_letrec_case_arity.dsp");
    let out = compiler
        .compile_source_to_signals("err_15_eval_compound_with_letrec_case_arity.dsp", &source)
        .expect("fixture should now compile to signals");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Int(1)
    );
}

#[test]
fn case_arity_fixture_now_lowers_through_under_application_semantics() {
    let compiler = Compiler::new();
    let source = read_corpus("err_11_eval_case_arity_mismatch.dsp");
    let out = compiler
        .compile_source_to_signals("err_11_eval_case_arity_mismatch.dsp", &source)
        .expect("fixture should now compile to signals");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Int(1)
    );
}

#[test]
fn propagate_error_fixture_exposes_frs_prop_code() {
    let compiler = Compiler::new();
    let source = read_corpus("err_03_propagate_split_mismatch.dsp");
    let err = compiler
        .compile_source_to_signals("err_03_propagate_split_mismatch.dsp", &source)
        .expect_err("propagate error fixture should fail propagate stage");

    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|d| d.code.0.starts_with("FRS-PROP-"))
    );
}

#[test]
fn reverse_ad_delay1_fixture_falls_back_to_block_reverse_ad() {
    // Phase B1: `rad(x', x)` no longer errors — it falls back to the
    // SigBlockReverseAD carrier.  The two output signals must each be
    // `Proj(slot, BlockReverseAD { .. })`.
    let compiler = Compiler::new();
    let source = read_corpus("rad_delay1_block_fallback.dsp");
    let out = compiler
        .compile_source_to_signals("rad_delay1_block_fallback.dsp", &source)
        .expect("rad over a delay1 must succeed with BlockReverseAD fallback (Phase B1)");

    assert_eq!(
        out.signals.len(),
        2,
        "rad bundle = [primal_proj, adjoint_proj]"
    );
    let SigMatch::Proj(s0, carrier) = match_sig(&out.parse.state.arena, out.signals[0]) else {
        panic!("first output must be Proj");
    };
    assert_eq!(s0, 0);
    assert!(
        matches!(
            match_sig(&out.parse.state.arena, carrier),
            SigMatch::BlockReverseAD { .. }
        ),
        "carrier must be SigBlockReverseAD"
    );
    let SigMatch::Proj(s1, carrier2) = match_sig(&out.parse.state.arena, out.signals[1]) else {
        panic!("second output must be Proj");
    };
    assert_eq!(s1, 1);
    assert_eq!(
        carrier, carrier2,
        "both outputs project from the same carrier"
    );
}

#[test]
fn soundfile_part_interval_error_exposes_compiler_type_diagnostic() {
    let compiler = Compiler::new();
    let path = corpus_path("rep_74_soundfile_basic.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("soundfile part interval fixture should fail type validation");

    let diagnostics = err
        .diagnostics()
        .expect("type validation error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("type validation bundle should not be empty");

    assert_eq!(first.code.0, "FRS-COMP-0004");
    assert!(
        first.message.contains("out of range soundfile part number"),
        "unexpected message: {}",
        first.message
    );
    assert!(
        first.message.contains("interval(0,255)"),
        "unexpected message: {}",
        first.message
    );
}

#[test]
fn propagate_error_operator_span_points_to_composition_token() {
    let compiler = Compiler::new();
    let source = read_corpus("err_03_propagate_split_mismatch.dsp");
    let err = compiler
        .compile_source_to_signals("err_03_propagate_split_mismatch.dsp", &source)
        .expect_err("propagate error fixture should fail propagate stage");

    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("propagate error bundle should not be empty");
    let primary = first
        .labels
        .first()
        .expect("propagate error should include one source label");

    assert_eq!(primary.span.line, 1);
    assert!(
        primary.span.col > 1,
        "operator-level span should not point to definition column 1"
    );
    let readable_expr = first
        .notes
        .iter()
        .find(|note| note.as_ref().starts_with("expr="))
        .expect("diagnostic should expose readable expression note");
    assert!(
        readable_expr.contains("<:"),
        "readable expression note should preserve split operator context"
    );
}

#[test]
fn propagate_error_complex_fixtures_expose_codes_and_source_labels() {
    let compiler = Compiler::new();
    let fixtures = [
        ("err_04_propagate_seq_mismatch_alias.dsp", 1u32),
        ("err_05_propagate_merge_mismatch_alias.dsp", 1u32),
        ("err_06_propagate_split_mismatch_chain.dsp", 1u32),
        ("err_07_propagate_rec_mismatch_alias.dsp", 1u32),
        ("err_08_propagate_seq_ui_mismatch.dsp", 1u32),
        ("err_14_propagate_split_mismatch_nested_alias.dsp", 1u32),
        ("err_16_propagate_compound_with_letrec_split.dsp", 1u32),
    ];

    for (file, expected_line) in fixtures {
        let source = read_corpus(file);
        let err = match compiler.compile_source_to_signals(file, &source) {
            Ok(_) => panic!("{file} should fail in propagate stage"),
            Err(err) => err,
        };

        let diagnostics = err
            .diagnostics()
            .unwrap_or_else(|| panic!("{file} should expose diagnostics"));
        assert!(
            diagnostics
                .as_slice()
                .iter()
                .any(|d| d.code.0.starts_with("FRS-PROP-")),
            "{file} should expose FRS-PROP-* code"
        );
        let first = diagnostics
            .as_slice()
            .first()
            .unwrap_or_else(|| panic!("{file} should produce one diagnostic"));
        let primary = first
            .labels
            .first()
            .unwrap_or_else(|| panic!("{file} should include one source label"));
        assert_eq!(
            primary.span.line, expected_line,
            "{file} should point to the expected source line"
        );
    }
}

#[test]
fn propagate_split_nested_alias_exposes_trace_and_template_help() {
    let compiler = Compiler::new();
    let source = read_corpus("err_14_propagate_split_mismatch_nested_alias.dsp");
    let err = compiler
        .compile_source_to_signals("err_14_propagate_split_mismatch_nested_alias.dsp", &source)
        .expect_err("fixture should fail in propagate stage");
    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("propagate error bundle should not be empty");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.as_ref() == "binding_trace=process -> baz -> bar -> foo"),
        "nested alias fixture should expose full binding trace"
    );
    assert!(
        first.help.iter().any(|h| {
            h.as_ref()
                .starts_with("template: process = A <: B; // inputs(B) % outputs(A) == 0")
        }),
        "split mismatch should expose deterministic template help"
    );
}

#[test]
fn propagate_compound_fixture_exposes_cause_and_template_notes() {
    let compiler = Compiler::new();
    let source = read_corpus("err_16_propagate_compound_with_letrec_split.dsp");
    let err = compiler
        .compile_source_to_signals("err_16_propagate_compound_with_letrec_split.dsp", &source)
        .expect_err("fixture should fail in propagate stage");
    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("propagate error bundle should not be empty");
    assert!(
        first.notes.iter().any(|n| n
            .as_ref()
            .starts_with("cause: split composition divisibility")),
        "compound propagate fixture should expose explicit cause note"
    );
    assert!(
        first.help.iter().any(|h| {
            h.as_ref()
                .starts_with("template: process = A <: B; // inputs(B) % outputs(A) == 0")
        }),
        "compound propagate fixture should expose deterministic template help"
    );
}

#[test]
fn propagate_error_alias_chain_exposes_binding_trace_note() {
    let compiler = Compiler::new();
    let source = read_corpus("err_06_propagate_split_mismatch_chain.dsp");
    let err = compiler
        .compile_source_to_signals("err_06_propagate_split_mismatch_chain.dsp", &source)
        .expect_err("fixture should fail in propagate stage");

    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("propagate error bundle should not be empty");
    assert!(
        first
            .notes
            .iter()
            .any(|note| note.as_ref() == "binding_trace=process -> baz -> bar -> foo"),
        "alias chain note should expose the ownership trace"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|note| note.as_ref() == "error originates from definition 'foo'"),
        "alias chain note should expose the owner definition"
    );
}

#[test]
fn propagate_error_includes_paired_side_context_notes() {
    let compiler = Compiler::new();
    let source = read_corpus("err_05_propagate_merge_mismatch_alias.dsp");
    let err = compiler
        .compile_source_to_signals("err_05_propagate_merge_mismatch_alias.dsp", &source)
        .expect_err("fixture should fail in propagate stage");

    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("propagate error bundle should not be empty");
    assert!(
        first
            .notes
            .iter()
            .any(|note| note.as_ref().starts_with("A (merge left) = ")),
        "diagnostic should expose left-side expression context"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|note| note.as_ref().starts_with("B (merge right) = ")),
        "diagnostic should expose right-side expression context"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|note| note.as_ref().starts_with("A arity: ")),
        "diagnostic should expose left-side arity context"
    );
    assert!(
        first
            .notes
            .iter()
            .any(|note| note.as_ref().starts_with("B arity: ")),
        "diagnostic should expose right-side arity context"
    );
}

#[test]
fn propagate_error_ui_expr_note_is_pretty_printed() {
    let compiler = Compiler::new();
    let source = read_corpus("err_08_propagate_seq_ui_mismatch.dsp");
    let err = compiler
        .compile_source_to_signals("err_08_propagate_seq_ui_mismatch.dsp", &source)
        .expect_err("fixture should fail in propagate stage");

    let diagnostics = err
        .diagnostics()
        .expect("propagate error should expose diagnostics");
    let first = diagnostics
        .as_slice()
        .first()
        .expect("propagate error bundle should not be empty");
    let expr_note = first
        .notes
        .iter()
        .find(|note| note.starts_with("expr="))
        .expect("diagnostic should expose readable expression note");
    assert!(expr_note.contains("hslider("));
    assert!(expr_note.contains(" : +"));
    assert!(!expr_note.contains("float_bits("));
    assert!(!expr_note.contains("cons("));
}
