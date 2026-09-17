//! Integration tests for `diagnostic_errors`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use std::fs;
use std::path::PathBuf;

use compiler::{Compiler, DiagnosticValue, LabelRole, Stage};
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
fn identical_unresolved_nodes_blame_the_reachable_occurrence() {
    let source = "unused = missing;\nactive = missing;\nprocess = active;\n";
    let err = Compiler::new()
        .compile_source_to_signals("reachable_origin.dsp", source)
        .expect_err("the reachable undefined symbol should fail");
    let diagnostic = &err.diagnostic_bundle().as_slice()[0];
    assert_eq!(diagnostic.labels[0].role, compiler::LabelRole::UseSite);
    assert_eq!(diagnostic.labels[0].span.line, 2);
    assert_eq!(diagnostic.labels[0].span.col, 10);
    assert!(
        diagnostic
            .facts
            .iter()
            .any(|(key, value)| key.as_str() == "owner_definition"
                && value == &compiler::DiagnosticValue::from("active"))
    );
}

#[test]
fn parse_error_fixture_exposes_frs_parse_code() {
    let compiler = Compiler::new();
    let source = read_corpus("err_01_parse_missing_rhs.dsp");
    let err = compiler
        .compile_source("err_01_parse_missing_rhs.dsp", &source)
        .expect_err("parse error fixture should fail parse stage");

    let diagnostics = err.diagnostic_bundle();
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

    let diagnostics = err.diagnostic_bundle();
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
        let diagnostics = err.diagnostic_bundle();
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

/// A deeply nested but acyclic expression (issue #16): it pushes no
/// `call_stack` frame, so the eval depth budget never saw it, and it overflowed
/// the native stack of whatever thread ran the compiler, killing an embedding
/// host with `SIGABRT`. The evaluator now recurses on stack segments it grows
/// on demand: a chain of five thousand additions compiles from start to end
/// on a 64 MiB thread, where the previous evaluator aborted (the evaluator
/// alone handles it on 1 MiB, see the `eval` crate's tests; the rest of the
/// pipeline still recurses on the native stack, a few KiB per level in debug
/// builds, which is what sizes this thread).
#[test]
fn deep_acyclic_expression_compiles_on_a_host_thread() {
    let source = format!("process = {};", vec!["1"; 5_000].join("+"));
    std::thread::Builder::new()
        .name("deep-expression-host-stack".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let compiler = Compiler::new();
            compiler
                .compile_source_to_signals("deep_expression.dsp", &source)
                .expect("a 5 000-deep acyclic expression compiles on a grown stack");
        })
        .expect("spawn worker")
        .join()
        .expect("worker thread should finish, not abort");
}

#[test]
fn diverging_recursive_case_reports_eval_error_instead_of_aborting() {
    // This test deliberately drives unbounded recursion and relies on the
    // profile's *default* eval budget tripping before the 512 MiB thread stack
    // overflows: the diverging-`case` path costs ~64 KiB of real stack per
    // logical frame in debug and ~4 KiB in release (measured in
    // `crates/eval/src/loop_detector.rs`), so the profile defaults (1 024 /
    // 32 768) touch at most 64 MiB / 128 MiB of this stack. A raised ambient
    // `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH` explicitly opts into real OS-stack
    // overflow: past the safe depth below, the budget no longer protects this
    // stack, so the process would SIGABRT here. Skip gracefully in that case
    // rather than aborting the whole test binary — the raised budget is a
    // user choice for compiling deep programs (e.g. large FFTs), not a
    // regression. Run the suite with the variable unset for full coverage.
    const TEST_STACK_SAFE_MAX_DEPTH: usize = if cfg!(debug_assertions) {
        // 512 MiB / ~64 KiB per frame, halved for margin.
        4_096
    } else {
        // 512 MiB / ~4 KiB per frame, halved for margin.
        65_536
    };
    if let Some(raised) = std::env::var("FAUST_RS_DEFAULT_EVAL_MAX_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&d| d > TEST_STACK_SAFE_MAX_DEPTH)
    {
        eprintln!(
            "skipping diverging_recursive_case: FAUST_RS_DEFAULT_EVAL_MAX_DEPTH={raised} \
             exceeds the {TEST_STACK_SAFE_MAX_DEPTH}-frame budget this test's 512 MiB stack \
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
        .stack_size(512 * 1024 * 1024)
        .spawn(move || {
            let compiler = Compiler::new();
            let err = compiler
                .compile_source_to_signals("fact_stack_overflow.dsp", source)
                .expect_err("missing factorial base case for fact(0) should fail in eval stage");

            let diagnostics = err.diagnostic_bundle();
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
    let diagnostics = err.diagnostic_bundle();
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
    let diagnostics = err.diagnostic_bundle();
    let first = diagnostics
        .as_slice()
        .first()
        .expect("eval diagnostics should not be empty");
    assert!(
        !first.labels.is_empty(),
        "eval undefined-symbol diagnostics should expose at least one source label"
    );
    assert_eq!(first.labels[0].message.as_ref(), "failing use");
    assert_eq!(first.labels[0].span.line, 1);
    assert_eq!(first.labels[0].span.col, 14);
    assert_eq!(first.labels[1].message.as_ref(), "enclosing definition");
    assert_eq!(first.labels[1].span.line, 1);
    assert_eq!(first.labels[1].span.col, 1);
    assert_eq!(first.labels[2].message.as_ref(), "call site");
    assert_eq!(first.labels[2].span.line, 4);
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
    let diagnostics = err.diagnostic_bundle();
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

    let diagnostics = err.diagnostic_bundle();
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

    let diagnostics = err.diagnostic_bundle();
    let first = diagnostics
        .as_slice()
        .first()
        .expect("type validation bundle should not be empty");

    assert_eq!(first.code.0, "FRS-COMP-0004");
    assert_eq!(first.stage, Stage::TypeInference);
    assert_eq!(
        first.detail_code.as_ref().map(|code| code.as_str()),
        Some("soundfile-part-interval")
    );
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
    assert!(
        !first.message.contains("SIG"),
        "standard message must not expose raw Signal IR: {}",
        first.message
    );
    assert!(matches!(
        first
            .facts
            .iter()
            .find(|(key, _)| key.as_str() == "required_interval")
            .map(|(_, value)| value),
        Some(DiagnosticValue::IntegerRange { min: 0, max: 255 })
    ));
    assert!(
        first
            .facts
            .keys()
            .any(|key| key.as_str() == "actual_interval")
    );
    assert!(
        first.labels.iter().any(|label| matches!(
            label.role,
            LabelRole::DerivedFrom | LabelRole::DefinitionSite
        )),
        "type diagnostic should point back to Faust source"
    );
}

#[test]
fn invalid_delay_interval_points_to_faust_and_exposes_inferred_type() {
    let err = Compiler::new()
        .compile_source_to_signals("delay_interval.dsp", "process = _ : @(-1);")
        .expect_err("negative delay upper bound must fail type validation");
    let first = &err.diagnostic_bundle().as_slice()[0];

    assert_eq!(first.stage, Stage::TypeInference);
    assert_eq!(
        first.detail_code.as_ref().map(|code| code.as_str()),
        Some("delay-interval")
    );
    assert!(first.facts.keys().any(|key| key.as_str() == "actual_type"));
    assert!(
        first
            .labels
            .iter()
            .any(|label| label.span.file.ends_with("delay_interval.dsp")),
        "delay diagnostic should retain its Faust source"
    );
    assert!(!first.message.contains("SIG"));
}

#[test]
fn compile_time_math_domain_error_is_typed_and_source_located() {
    let err = Compiler::new()
        .compile_source_to_signals("modulo_zero.dsp", "process = _ % 0;")
        .expect_err("compile-time modulo by zero must fail type validation");
    let first = &err.diagnostic_bundle().as_slice()[0];

    assert_eq!(first.stage, Stage::TypeInference);
    assert_eq!(
        first.detail_code.as_ref().map(|code| code.as_str()),
        Some("math-domain")
    );
    assert!(first.facts.keys().any(|key| key.as_str() == "expected"));
    assert!(!first.labels.is_empty());
    assert!(!first.message.contains("SIG"));
}

#[test]
fn invalid_table_generator_is_typed_and_source_located() {
    let err = Compiler::new()
        .compile_source_to_signals("table_generator.dsp", "process = rdtable(9, +, 4);")
        .expect_err("sample-time table generator must fail static table validation");
    let first = &err.diagnostic_bundle().as_slice()[0];

    assert_eq!(first.stage, Stage::TypeInference);
    assert_eq!(
        first.detail_code.as_ref().map(|code| code.as_str()),
        Some("table-construction")
    );
    assert!(first.facts.keys().any(|key| key.as_str() == "actual_type"));
    assert!(first.facts.keys().any(|key| key.as_str() == "expected"));
    assert!(!first.labels.is_empty());
    assert!(!first.message.contains("SIG"));
}

#[test]
fn canonical_fir_retains_signal_and_box_derivations() {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(
            "fir_origins.dsp",
            "gain = 0.5; process = _ * gain;",
            compiler::SignalFirLane::TransformFastLane,
        )
        .expect("valid source should lower to FIR");

    let origins = fir.origins.origins_for(fir.module);
    assert!(
        !origins.is_empty(),
        "canonical module must inherit at least one Signal producer"
    );
    assert!(
        origins.iter().any(|origin| !origin.boxes.is_empty()),
        "FIR producers must retain their Box derivations"
    );
}

#[test]
fn propagate_error_operator_span_points_to_composition_token() {
    let compiler = Compiler::new();
    let source = read_corpus("err_03_propagate_split_mismatch.dsp");
    let err = compiler
        .compile_source_to_signals("err_03_propagate_split_mismatch.dsp", &source)
        .expect_err("propagate error fixture should fail propagate stage");

    let diagnostics = err.diagnostic_bundle();
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

        let diagnostics = err.diagnostic_bundle();
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
    let diagnostics = err.diagnostic_bundle();
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
    let diagnostics = err.diagnostic_bundle();
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

    let diagnostics = err.diagnostic_bundle();
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

    let diagnostics = err.diagnostic_bundle();
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

    let diagnostics = err.diagnostic_bundle();
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

// ─── Constant division by zero ───────────────────────────────────────────────
//
// `process = 2.0 / 0;` used to panic the compiler, and through the FFI to abort
// the host: the evaluator folds numeric sequences with the normalizer, which
// reports `x / 0` by unwinding (as C++ `mterm::operator/=` throws), and nothing
// caught that unwind on the evaluator's side. The reference behaviour, checked
// against Faust 2.89, is an error in integers and in reals alike
// (`ERROR : division by 0 in 2 / 0`): no infinity is folded.

/// The first diagnostic of a program that must not compile.
fn first_diagnostic(name: &str, source: &str) -> compiler::Diagnostic {
    let err = Compiler::new()
        .compile_source_to_signals(name, source)
        .expect_err("the program divides by a constant zero");
    err.diagnostic_bundle().as_slice()[0].clone()
}

#[test]
fn a_constant_division_by_zero_is_a_located_eval_error() {
    for (source, detail) in [
        // the four programs of the report
        ("process = 2.0 / 0;", "2.0 / 0"),
        ("process = 2.0 / 0.0;", "2.0 / 0"),
        ("process = 2 / 0;", "2 / 0"),
        ("process = _ : *(2.0 / 0);", "2.0 / 0"),
        // a divisor that folds to zero, and zero over zero
        ("process = 1.0 / (2 - 2);", "1.0 / 0"),
        ("process = 0 / 0;", "0 / 0"),
        ("process = 0.0 / 0.0;", "0.0 / 0"),
    ] {
        let first = first_diagnostic("division.dsp", source);
        assert_eq!(first.code.0, "FRS-EVAL-0007", "{source}");
        assert_eq!(first.stage, Stage::Eval, "{source}");
        assert_eq!(
            first.message.as_ref(),
            format!("division by 0 in {detail}"),
            "{source}"
        );
        assert!(!first.labels.is_empty(), "{source} is not located");
        assert!(
            first.notes.iter().any(|note| note.contains(detail)),
            "{source}: {:?}",
            first.notes
        );
    }
}

#[test]
fn a_division_by_a_zero_argument_or_index_is_reported_where_it_is_folded() {
    // the case that revealed the panic: a coefficient function given a 0
    let source = read_corpus("err_19_eval_division_by_zero_argument.dsp");
    let first = first_diagnostic("err_19_eval_division_by_zero_argument.dsp", &source);
    assert_eq!(first.code.0, "FRS-EVAL-0007");
    assert_eq!(first.message.as_ref(), "division by 0 in 2.0 / 0");
    // located in the definition that divides, line 5 of the fixture
    assert!(
        first.labels.iter().any(|label| label.span.line == 5),
        "{:?}",
        first.labels
    );

    for source in [
        // an iteration index starts at 0
        "process = par(i, 2, 1.0 / i);",
        // contexts where the evaluator needs the constant itself
        "process = par(i, 4 / 0, _);",
        "process = route(2 / 0, 2, 1, 1);",
        "N = 1 / 0; process = hslider(\"g%N\", 0, 0, 1, 0.1);",
        // pattern matching: the reference fails even though `f(n)` ignores `n`
        "f(0) = 1; f(n) = 2; process = f(1 / 0);",
    ] {
        let first = first_diagnostic("division.dsp", source);
        assert!(
            first.message.starts_with("division by 0 in ")
                || first.message.contains("divides by a constant zero"),
            "{source}: {}",
            first.message
        );
    }
}

#[test]
fn a_remainder_by_a_constant_zero_is_a_typed_error_even_for_zero_itself() {
    // `x % x` is 0 for any x but the constant 0: the simplifier used to cancel
    // `0 % 0` before the typing stage could see it
    for source in [
        "process = 2 % 0;",
        "process = 0 % 0;",
        "process = _ % (1 - 1);",
    ] {
        let first = first_diagnostic("remainder.dsp", source);
        assert_eq!(first.stage, Stage::TypeInference, "{source}");
        assert!(
            first.message.contains("% by 0"),
            "{source}: {}",
            first.message
        );
    }
}

#[test]
fn the_folds_next_to_the_division_keep_their_values() {
    // what the fix must not disturb, and the integer operations that overflow
    // in Rust where the reference wraps: `i32::MIN % -1` panicked
    for (source, expected) in [
        ("process = 6 / 3;", SigMatch::Int(2)),
        ("process = 7 / 2;", SigMatch::Real(3.5)),
        ("process = 0 / 2;", SigMatch::Int(0)),
        ("process = 4 % 3;", SigMatch::Int(1)),
        ("process = 7 % -2;", SigMatch::Int(1)),
        ("process = (-2147483647 - 1) % -1;", SigMatch::Int(0)),
        (
            "process = (-2147483647 - 1) / -1;",
            SigMatch::Real(2_147_483_648.0),
        ),
        ("process = 2147483647 + 1;", SigMatch::Int(i32::MIN)),
        ("process = 1 << 40;", SigMatch::Int(256)),
        ("process = 1 << -1;", SigMatch::Int(i32::MIN)),
        ("process = 1 >> 40;", SigMatch::Int(0)),
    ] {
        let out = Compiler::new()
            .compile_source_to_signals("fold.dsp", source)
            .unwrap_or_else(|e| panic!("{source} must compile: {e}"));
        assert_eq!(out.signals.len(), 1, "{source}");
        assert_eq!(
            match_sig(&out.parse.state.arena, out.signals[0]),
            expected,
            "{source}"
        );
    }
    // a zero numerator over a signal is no division by a constant zero
    Compiler::new()
        .compile_source_to_signals("fold.dsp", "process = 0 / _;")
        .expect("0 / x compiles");
}

#[test]
fn a_zero_divisor_that_is_no_literal_is_reported_by_whoever_needs_the_constant() {
    // `z` is the constant 0 without being a numerical tuple, so the division is
    // not folded when its sequence is evaluated. No path may unwind on it.
    let z = "z = 0 <: _, !;\n";
    for (program, code) in [
        // the evaluator needs the constant: its error, with the division named
        ("process = par(i, 1 / z, _);", "FRS-EVAL-0007"),
        ("process = route(1 / z, 1, 1, 1);", "FRS-EVAL-0007"),
        (
            "N = 1 / z; process = hslider(\"g%N\", 0, 0, 1, 0.1);",
            "FRS-EVAL-0007",
        ),
        // the value reaches the signals: the typing stage reports it
        ("process = 1 / z;", "FRS-COMP-0004"),
        ("process = _ : @(1 / z);", "FRS-COMP-0004"),
    ] {
        let first = first_diagnostic("late_zero.dsp", &format!("{z}{program}"));
        assert_eq!(first.code.0, code, "{program}: {}", first.message);
        assert!(
            first.message.contains("division by 0"),
            "{program}: {}",
            first.message
        );
    }
}

#[test]
fn a_known_divergence_an_unused_late_zero_division_in_a_pattern_argument() {
    // The reference fails here (`ERROR : division by 0 in 1 / 0`): it simplifies
    // the argument of `f` eagerly and its exception is fatal. faust-rs folds a
    // pattern argument as an optimization, gives up on this one, matches the
    // general rule, and the quotient is never used. With a literal divisor
    // (`f(1 / 0)`) both fail. Recorded so that a change is noticed.
    let source = "z = 0 <: _, !;\nf(0) = 1; f(n) = 2;\nprocess = f(1 / z);\n";
    let out = Compiler::new()
        .compile_source_to_signals("late_zero_pattern.dsp", source)
        .expect("compiles: the division is never evaluated to a value that is used");
    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Int(2)
    );
    let literal = first_diagnostic("pattern.dsp", "f(0) = 1; f(n) = 2; process = f(1 / 0);");
    assert_eq!(literal.code.0, "FRS-EVAL-0007");
}

#[test]
fn abs_of_an_integer_constant_is_an_integer_and_an_infinity_keeps_its_sign() {
    // These folds happen in the normalization of the signals, after
    // `compile_source_to_signals`: they are read in the generated code.
    let output = |source: &str| -> String {
        let code = Compiler::new()
            .compile_source_to_cpp(
                "fold.dsp",
                source,
                &codegen::backends::cpp::CppOptions::default(),
            )
            .unwrap_or_else(|e| panic!("{source} must compile: {e}"));
        code.lines()
            .find(|line| line.contains("output0[i0] ="))
            .unwrap_or_else(|| panic!("no output line for {source}"))
            .trim()
            .to_owned()
    };
    // `abs` keeps an integer an integer (it was the real 3.0), and wraps on
    // `i32::MIN` as the `std::abs(int)` emitted for a non-constant argument does
    assert_eq!(
        output("process = abs(-3);"),
        "output0[i0] = ((FAUSTFLOAT)(3));"
    );
    assert_eq!(
        output("process = abs(-2147483647);"),
        "output0[i0] = ((FAUSTFLOAT)(2147483647));"
    );
    assert_eq!(
        output("process = abs(-2147483647 - 1);"),
        "output0[i0] = ((FAUSTFLOAT)(-2147483648));"
    );
    assert_eq!(
        output("process = abs(-2.5);"),
        "output0[i0] = ((FAUSTFLOAT)(2.5f));"
    );
    // a negative infinity keeps its sign: Faust 2.89 prints `INFINITY` here
    assert_eq!(
        output("process = 0 - exp(1000);"),
        "output0[i0] = ((FAUSTFLOAT)(-INFINITY));"
    );
}
