//! Diagnostics for three mistakes met porting coopiano
//! (faust-coopiano `faust-rs-issues/RAPPORT.md`, points 7 to 9):
//!
//! - a name Faust C++ accepts but faust-rs reserves (`rad`, `ondemand`…);
//! - a unary minus before a primitive (`-max(1, 2)`) or before an access
//!   (`-L.g(2, k)`, which reads `(0 - L).g`);
//! - an arity error on a composition built by evaluation, which was labelled
//!   at the first located node inside the function it called.

use compiler::{Compiler, Diagnostic, LabelRole, LabelStyle};

fn first_error(name: &str, source: &str) -> Diagnostic {
    let error = Compiler::new()
        .compile_source_to_signals(name, source)
        .expect_err("the program must be rejected");
    error.diagnostic_bundle().as_slice()[0].clone()
}

fn mentions(texts: &[Box<str>], needle: &str) -> bool {
    texts.iter().any(|text| text.contains(needle))
}

fn primary(diagnostic: &Diagnostic) -> (u32, u32, LabelRole) {
    let label = diagnostic
        .labels
        .iter()
        .find(|label| label.style == LabelStyle::Primary)
        .unwrap_or_else(|| panic!("no primary label: {diagnostic:?}"));
    (label.span.line, label.span.col, label.role)
}

#[test]
fn a_reserved_word_used_as_a_name_says_so() {
    for (source, word) in [
        // The parser takes `rad` for its primitive and fails on `with`.
        (
            "f(x) = x * rad with { rad = 2.0; };\nprocess = f(3);\n",
            "rad",
        ),
        // The definition's name itself.
        ("fad = 2.0;\nprocess = fad;\n", "fad"),
        (
            "f(x) = x * cinputs with { cinputs = 2.0; };\nprocess = f(3);\n",
            "cinputs",
        ),
    ] {
        let diagnostic = first_error("reserved.dsp", source);
        assert!(
            mentions(
                &diagnostic.notes,
                &format!("`{word}` is a reserved word of faust-rs")
            ) && mentions(&diagnostic.notes, "Faust C++ accepts it as an identifier"),
            "{source}: {:?}",
            diagnostic.notes
        );
        assert!(
            mentions(&diagnostic.help, &format!("`{word}_`")),
            "{:?}",
            diagnostic.help
        );
    }
    // Reserved by the C++ reference branch too, accepted by the releases.
    let diagnostic = first_error("reserved.dsp", "ondemand = 1;\nprocess = ondemand;\n");
    assert!(
        mentions(
            &diagnostic.notes,
            "the C++ reference branch of faust-rs reserves it too"
        ),
        "{:?}",
        diagnostic.notes
    );
}

#[test]
fn an_unrelated_parse_error_gets_no_reserved_word_note() {
    let diagnostic = first_error("other.dsp", "process = 1 +;\n");
    assert!(
        !mentions(&diagnostic.notes, "reserved word"),
        "{:?}",
        diagnostic.notes
    );
    assert!(
        !mentions(&diagnostic.notes, "unary `-`"),
        "{:?}",
        diagnostic.notes
    );
}

#[test]
fn a_unary_minus_before_a_primitive_explains_the_rule() {
    for (source, primitive) in [
        ("process = -max(1, 2);\n", "max"),
        ("process = exp(-sin(0.5));\n", "sin"),
    ] {
        let diagnostic = first_error("minus.dsp", source);
        assert!(
            mentions(
                &diagnostic.notes,
                "the unary `-` applies only to a name or a number"
            ) && mentions(&diagnostic.notes, &format!("`{primitive}`")),
            "{source}: {:?}",
            diagnostic.notes
        );
        assert!(
            mentions(&diagnostic.help, &format!("`0 - {primitive}(...)`"))
                && mentions(&diagnostic.help, "partial application"),
            "{:?}",
            diagnostic.help
        );
    }
    // A unary minus on a name, and a binary minus before a primitive, parse.
    for source in ["x = 2;\nprocess = -x;\n", "process = 3 -max(1, 2);\n"] {
        Compiler::new()
            .compile_source_to_signals("minus.dsp", source)
            .unwrap_or_else(|error| panic!("{source}: {error}"));
    }
}

#[test]
fn an_access_into_a_unary_minus_is_explained_and_located() {
    let source = "L = environment { g(m, k) = m * k; };\nprocess = exp(-L.g(2, 3));\n";
    let diagnostic = first_error("access.dsp", source);
    assert!(
        mentions(&diagnostic.notes, "the unary `-` applies to `L` alone"),
        "{:?}",
        diagnostic.notes
    );
    assert!(
        mentions(&diagnostic.help, "`0 - L.f(...)`"),
        "{:?}",
        diagnostic.help
    );
    // The `-` itself, line 2 column 15.
    let (line, col, _) = primary(&diagnostic);
    assert_eq!((line, col), (2, 15), "{diagnostic:?}");
}

#[test]
fn an_arity_error_points_at_the_composition_written_in_the_program() {
    // The faulty `:` is on line 4; the first located node below the
    // composition built by evaluation is `(1, 2)` on line 3, and for the
    // lambda case `(_, _)` in `m`'s body, also on line 3.
    for (source, col) in [
        (
            "unrelated = (1, 2, 3) : !, _, _;\npair = (1, 2);\nnoise = 0;\nbad = pair : +, pair : +;\nprocess = unrelated, bad;\n",
            22,
        ),
        (
            "unrelated = (1, 2, 3) : !, _, _;\nm(x) = (\\(a, b).(a + x, b)) ~ (_, _);\nnoise = 0;\nbad = m(1) : \\(a, b).(a), m(2) : \\(a, b).(b);\nprocess = unrelated, bad;\n",
            32,
        ),
    ] {
        let diagnostic = first_error("arity.dsp", source);
        let (line, label_col, role) = primary(&diagnostic);
        assert_eq!((line, label_col), (4, col), "{source}\n{diagnostic:?}");
        assert_eq!(role, LabelRole::Operator);
    }
}

#[test]
fn a_composition_with_two_origins_gets_no_single_location() {
    // One faulty box built from two places, which no single line describes:
    // the same text written twice (one source node, two positions), or two
    // different compositions that evaluate to the same box (`p` is `pair`).
    // No composition is blamed; the label falls back to the program's call.
    for source in [
        "pair = (1, 2);\nbad1 = pair : +, pair : +;\nbad2 = pair : +, pair : +;\nprocess = bad1, bad2;\n",
        "pair = (1, 2);\np = pair;\nbad1 = pair : +, pair : +;\nbad2 = p : +, p : +;\nprocess = bad1, bad2;\n",
    ] {
        let diagnostic = first_error("twice.dsp", source);
        let (line, _, role) = primary(&diagnostic);
        assert_ne!(role, LabelRole::Operator, "{source}\n{diagnostic:?}");
        assert_ne!(line, 1, "must not blame `(1, 2)`: {diagnostic:?}");
    }
}
