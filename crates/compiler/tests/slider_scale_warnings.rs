//! Integration tests for the slider scale warning `FRS-EVAL-0014`.
//!
//! Scope:
//! - `[scale:log]` with min = 0, or over a range that is not positive;
//! - `[scale:exp]` over a range wider than `eval::EXP_SCALE_MAX_WIDTH`;
//! - an unknown scale;
//! - no warning for the scales a slider can follow, nor without the semantic
//!   warnings option, and one warning for a widget evaluated several times.
//!
//! C++ equivalent: `SignalChecker::checkScale` in
//! `compiler/transform/sigPromotion.cpp` (Faust `master-dev` `0fc389b66`),
//! whose warnings are printed under `-wall`.

use compiler::{Compiler, Diagnostic, DiagnosticValue, FactKey, Severity};

const CODE: &str = "FRS-EVAL-0014";

/// The scale warnings of `source`, compiled with the semantic warnings.
fn scale_warnings(source: &str) -> Vec<Diagnostic> {
    Compiler::new()
        .with_semantic_warnings(true)
        .compile_source_to_signals("scale.dsp", source)
        .expect("a scale warning must not block compilation")
        .warnings
        .as_slice()
        .iter()
        .filter(|diagnostic| diagnostic.code.0 == CODE)
        .cloned()
        .collect()
}

fn fact<'a>(diagnostic: &'a Diagnostic, key: &str) -> Option<&'a DiagnosticValue> {
    diagnostic.facts.get(&FactKey::new(key))
}

/// The single scale warning of `source`, checked for its severity and scale.
fn single_warning(source: &str, scale: &str) -> Diagnostic {
    let warnings = scale_warnings(source);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = warnings.into_iter().next().unwrap();
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(
        fact(&warning, "widget_scale"),
        Some(&DiagnosticValue::String(scale.into()))
    );
    warning
}

#[test]
fn a_log_scale_with_a_zero_minimum_warns() {
    let warning = single_warning(
        "process = hslider(\"gain[scale:log]\", 0.5, 0, 1, 0.01);\n",
        "log",
    );
    let message = warning.message.to_string();
    assert!(message.contains("min = 0"), "{message}");
    assert!(message.contains("log(0) is not defined"), "{message}");
    assert_eq!(
        fact(&warning, "widget_min"),
        Some(&DiagnosticValue::String("0".into()))
    );
}

#[test]
fn a_log_scale_over_a_range_that_is_not_positive_warns() {
    for source in [
        "process = hslider(\"level[scale:log]\", -20, -60, 0, 0.1);\n",
        "process = nentry(\"pan[scale:log]\", 0, -1, 1, 0.01);\n",
        "process = vslider(\"x[scale:log]\", -2, -3, -1, 0.1);\n",
    ] {
        let warning = single_warning(source, "log");
        let message = warning.message.to_string();
        assert!(message.contains("needs positive values"), "{message}");
    }
}

#[test]
fn an_exp_scale_over_a_wide_range_warns() {
    let warning = single_warning(
        "process = vslider(\"amount[scale:exp]\", 500, 0, 1000, 1);\n",
        "exp",
    );
    let message = warning.message.to_string();
    assert!(message.contains("width 1000"), "{message}");
    assert!(message.contains("less than 0.1%"), "{message}");
}

#[test]
fn an_unknown_scale_warns() {
    let warning = single_warning(
        "process = hslider(\"freq[scale:logarithmic]\", 440, 20, 20000, 1);\n",
        "logarithmic",
    );
    let message = warning.message.to_string();
    assert!(message.contains("lin, log or exp expected"), "{message}");
}

#[test]
fn the_scales_a_slider_can_follow_do_not_warn() {
    let source = "process = hslider(\"f[scale:log]\", 440, 20, 20000, 1)\n\
                  + hslider(\"g[scale:exp]\", 5, 1, 10, 0.01)\n\
                  + hslider(\"h[scale:exp]\", 0, 0, 13.8, 0.01)\n\
                  + hslider(\"l[scale:lin]\", 0, -1, 1, 0.01)\n\
                  + hslider(\"m\", 0, -60, 0, 0.1);\n";
    let warnings = scale_warnings(source);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn the_exp_threshold_is_a_width_of_13_8() {
    // 1/(1+exp(w/2)) = 0.001 for w = 2*ln(999)
    assert!((eval::EXP_SCALE_MAX_WIDTH - 2.0 * 999.0_f64.ln()).abs() < 1e-12);
    let just_above = "process = hslider(\"x[scale:exp]\", 0, 0, 13.82, 0.01);\n";
    assert_eq!(scale_warnings(just_above).len(), 1);
}

#[test]
fn a_widget_used_twice_warns_once() {
    let source = "g = hgroup(\"grp\", hslider(\"gain[scale:log]\", 0.5, 0, 1, 0.01));\n\
                  process = g + g * 2;\n";
    assert_eq!(scale_warnings(source).len(), 1);
}

#[test]
fn the_scale_warning_needs_the_semantic_warnings_option() {
    let output = Compiler::new()
        .compile_source_to_signals(
            "scale.dsp",
            "process = hslider(\"gain[scale:log]\", 0.5, 0, 1, 0.01);\n",
        )
        .expect("a scale warning must not block compilation");
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
}
