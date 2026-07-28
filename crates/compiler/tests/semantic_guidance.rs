//! Integration tests for G7 semantic guidance and warning policy.
//!
//! Scope:
//! - near-name suggestions and the rename edit derived from them;
//! - `case` rule/attempt traces;
//! - duplicate declaration reporting for symbols and UI addresses;
//! - the opt-in potential out-of-domain warning;
//! - the canonical cause/rule/computed note order.
//!
//! Every assertion goes through typed diagnostic fields rather than rendered
//! prose, which is the contract these phases exist to establish.

use compiler::{
    Applicability, Compiler, CompilerError, Diagnostic, DiagnosticValue, FactKey, LabelRole,
    LabelStyle, Severity, TraceKind,
};

/// Compiles `source` and returns the diagnostics of the expected failure.
fn failing_diagnostics(source: &str) -> Vec<Diagnostic> {
    let Err(error) = Compiler::new().compile_source_to_signals("guidance.dsp", source) else {
        panic!("source is expected to fail");
    };
    error.diagnostic_bundle().as_slice().to_vec()
}

/// Returns one typed fact by key.
fn fact<'a>(diagnostic: &'a Diagnostic, key: &str) -> Option<&'a DiagnosticValue> {
    diagnostic.facts.get(&FactKey::new(key))
}

#[test]
fn an_undefined_symbol_suggests_the_closest_visible_name() {
    let diagnostics = failing_diagnostics("filter(x) = x * 0.5;\nprocess = filtre;\n");
    let diagnostic = &diagnostics[0];

    assert_eq!(
        fact(diagnostic, "suggested_symbols"),
        Some(&DiagnosticValue::StringList(vec!["filter".into()])),
    );
}

#[test]
fn an_unambiguous_suggestion_carries_an_exact_rename_edit() {
    let diagnostics = failing_diagnostics("filter(x) = x * 0.5;\nprocess = filtre;\n");
    let fix = diagnostics[0]
        .fixes
        .first()
        .expect("an unambiguous suggestion must offer a rename");

    assert_eq!(fix.applicability, Applicability::MaybeIncorrect);
    assert_eq!(fix.edits.len(), 1);
    assert_eq!(fix.edits[0].replacement.as_ref(), "filter");
    // `filtre` starts one line and ten columns in: "filter(x) = x * 0.5;\n"
    // is 21 bytes, "process = " is 10 more.
    assert_eq!(fix.edits[0].range.start, 31);
    assert_eq!(fix.edits[0].range.end, 37);
}

#[test]
fn a_misspelled_entry_point_renames_the_definition_not_the_suggestion() {
    // The two rename shapes go in opposite directions: an undefined symbol is a
    // misspelled use, a missing entry point is a misspelled definition. Getting
    // this backwards produced an edit that replaced `proces` with `proces`.
    let diagnostics = failing_diagnostics("gain = 0.5;\nproces = *(gain);\n");
    let fix = diagnostics[0]
        .fixes
        .first()
        .expect("a near-miss entry point must offer a rename");

    assert_eq!(fix.edits[0].replacement.as_ref(), "process");
    assert_eq!(fix.edits[0].range.start, 12, "the `proces` definition name");
    assert_eq!(fix.edits[0].range.end, 18);
}

#[test]
fn two_equally_close_candidates_offer_no_edit() {
    let diagnostics = failing_diagnostics("gaina = 1;\ngainb = 2;\nprocess = gainx;\n");
    assert!(
        diagnostics[0].fixes.is_empty(),
        "an ambiguous rename must not be proposed"
    );
}

#[test]
fn a_suggestion_never_names_a_symbol_outside_the_visible_scope() {
    // `hidden` is local to `wrapper`, so it is not visible at the failing site
    // even though it is the closest spelling to `hidde`.
    let diagnostics = failing_diagnostics(
        "wrapper = out with { hidden = 1; out = hidden; };\nprocess = hidde;\n",
    );
    let suggestions = fact(&diagnostics[0], "suggested_symbols");
    assert!(
        !matches!(suggestions, Some(DiagnosticValue::StringList(names)) if names.iter().any(|name| name.as_ref() == "hidden")),
        "got {suggestions:?}"
    );
}

#[test]
fn a_failed_case_match_traces_the_declared_rules() {
    let diagnostics =
        failing_diagnostics("f = case { (0, x) => x; (1, x) => x + 1; };\nprocess = f(2, 3);\n");
    let diagnostic = &diagnostics[0];

    assert_eq!(
        fact(diagnostic, "pattern_rules"),
        Some(&DiagnosticValue::StringList(vec![
            "(0, x)".into(),
            "(1, x)".into()
        ])),
        "patterns must read in written order, without evaluator wrappers"
    );

    let trace = diagnostic
        .traces
        .iter()
        .find(|trace| trace.kind == TraceKind::Evaluation)
        .expect("a failed match must carry an evaluation trace");
    assert_eq!(trace.frames[0].name.as_deref(), Some("arguments"));
    assert_eq!(trace.frames.len(), 3, "one argument frame plus two rules");
}

#[test]
fn a_redefined_symbol_labels_both_declarations() {
    let diagnostics =
        failing_diagnostics("process = out with {\n    a = 1;\n    a = 2;\n    out = a;\n};\n");
    let diagnostic = &diagnostics[0];

    let primary = diagnostic
        .labels
        .iter()
        .find(|label| label.style == LabelStyle::Primary)
        .expect("the conflicting declaration must be labeled");
    let conflicting = diagnostic
        .labels
        .iter()
        .find(|label| label.role == LabelRole::ConflictsWith)
        .expect("the first declaration must be labeled too");

    assert_eq!(primary.span.line, 3, "the later clause is the cause");
    assert_eq!(conflicting.span.line, 2);
    assert_eq!(
        fact(diagnostic, "symbol"),
        Some(&DiagnosticValue::String("a".into()))
    );
}

#[test]
fn two_controls_at_one_ui_address_are_rejected_with_both_sites() {
    let source = "process = hslider(\"gain\", 0, 0, 1, 0.01) + vslider(\"gain\", 0, 0, 2, 0.01);\n";
    let Err(error) = Compiler::new().compile_source_to_signals("guidance.dsp", source) else {
        panic!("a duplicated UI address must be rejected, as in C++");
    };
    assert!(matches!(error, CompilerError::UiLayout { .. }));

    let diagnostic = &error.diagnostic_bundle().as_slice()[0];
    assert_eq!(diagnostic.code.0, "FRS-UI-0001");
    assert_eq!(
        fact(diagnostic, "ui_path"),
        Some(&DiagnosticValue::String("/guidance/gain".into()))
    );
    assert_eq!(
        diagnostic.labels.len(),
        2,
        "both widget declarations must be labeled"
    );
    assert_eq!(diagnostic.labels[0].role, LabelRole::PrimaryCause);
    assert_eq!(diagnostic.labels[1].role, LabelRole::ConflictsWith);
}

#[test]
fn bargraphs_sharing_one_ui_address_still_compile() {
    // C++ only warns for this shape; rejecting it would break acceptance parity.
    let source = "process = _ <: vbargraph(\"level\", 0, 1), hbargraph(\"level\", 0, 1) :> _;\n";
    assert!(
        Compiler::new()
            .compile_source_to_signals("guidance.dsp", source)
            .is_ok()
    );
}

#[test]
fn a_potential_out_of_domain_operand_warns_only_when_requested() {
    let source = "process = sqrt;\n";

    let quiet = Compiler::new()
        .compile_source_to_signals("guidance.dsp", source)
        .expect("a potential domain problem must not block compilation");
    assert!(quiet.warnings.is_empty(), "warnings are opt-in");

    let loud = Compiler::new()
        .with_semantic_warnings(true)
        .compile_source_to_signals("guidance.dsp", source)
        .expect("enabling warnings must not change the compilation result");
    let warning = loud
        .warnings
        .as_slice()
        .first()
        .expect("sqrt over a signed input may leave its domain");

    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(
        fact(warning, "operation"),
        Some(&DiagnosticValue::String("sqrt".into()))
    );
    assert_eq!(
        fact(warning, "potential_runtime_failure"),
        Some(&DiagnosticValue::Boolean(true))
    );
}

#[test]
fn a_disabled_warning_channel_builds_no_diagnostic_at_all() {
    // Rendering one warning walks the definition graph and dumps the offending
    // Signal expression to a string. Building that and discarding it is pure
    // waste, so the opt-out has to reach the producer, not just the result.
    //
    // Observing "it was not built" from outside means observing that nothing
    // that depends on the build is present: an empty bundle whose source map
    // was never attached either.
    let source = "process = sqrt;\n";

    let quiet = Compiler::new()
        .compile_source_to_signals("guidance.dsp", source)
        .expect("a potential domain problem must not block compilation");
    assert!(quiet.warnings.is_empty());
    assert!(
        quiet.warnings.source_map().is_empty(),
        "a bundle that was never populated must not carry a source map either"
    );

    let loud = Compiler::new()
        .with_semantic_warnings(true)
        .compile_source_to_signals("guidance.dsp", source)
        .expect("enabling warnings must not change the compilation result");
    assert_eq!(loud.warnings.len(), 1);
    assert!(
        !loud.warnings.source_map().is_empty(),
        "a populated bundle needs its snapshots so the renderer can quote the line"
    );
}

#[test]
fn notes_follow_the_canonical_cause_rule_computed_order() {
    let diagnostics = failing_diagnostics("filter(x) = x * 0.5;\nprocess = filtre;\n");
    let ranks = diagnostics[0]
        .notes
        .iter()
        .map(|note| {
            if note.starts_with("cause:") {
                0
            } else if note.starts_with("rule:") {
                1
            } else if note.starts_with("computed:") {
                2
            } else {
                3
            }
        })
        .collect::<Vec<u8>>();
    assert!(
        ranks.windows(2).all(|pair| pair[0] <= pair[1]),
        "note order drifted: {:?}",
        diagnostics[0].notes
    );
}
