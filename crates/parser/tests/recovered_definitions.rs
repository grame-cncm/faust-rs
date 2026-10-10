//! A statement the parser repaired is not a definition of the program (W6 of
//! `porting/foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md`).
//!
//! `ffunction(...)` alone on a line is a syntax error in both compilers (C++:
//! "syntax error, unexpected FFUNCTION"). lrpar repairs it by inserting a
//! definition name, which the grammar read as an identifier with empty text:
//! two such statements became "multiple definitions of symbol ''", a
//! definition the program never wrote. A repaired name is now dropped, and the
//! error says what to write instead.
//!
//! The assertions do not depend on which repairs lrpar lists, nor on their
//! order, nor on whether its search runs out of time.

use parser::parse_program;

/// Every diagnostic of `source`: its message, notes and help, one string each.
fn diagnostics(source: &str) -> Vec<String> {
    let output = parse_program(source, "recovered.dsp");
    assert!(!output.errors.is_empty(), "no error for {source:?}");
    output
        .diagnostics
        .as_slice()
        .iter()
        .map(|d| {
            let mut text = d.message.to_string();
            for note in &d.notes {
                text.push_str("\nnote: ");
                text.push_str(note);
            }
            for help in &d.help {
                text.push_str("\nhelp: ");
                text.push_str(help);
            }
            text
        })
        .collect()
}

const BARE: &str = "ffunction(float foo(int), \"f.h\", \"\");\n";
const BARE2: &str = "ffunction(float bar(int), \"f.h\", \"\");\n";

#[test]
fn bare_foreign_statements_are_one_error_each_with_what_to_write() {
    for source in [
        format!("{BARE}process = 1;\n"),
        format!("{BARE}{BARE2}process = 1;\n"),
        format!("g = 2;\n{BARE}h = 3;\n{BARE2}process = g + h;\n"),
    ] {
        let all = diagnostics(&source);
        assert!(
            all.iter().all(|d| !d.contains("multiple definitions")),
            "{source}\n{all:#?}"
        );
        let bare = source.matches("ffunction(").count();
        let explained: Vec<&String> = all
            .iter()
            .filter(|d| d.contains("help: name it and use the name (`f = ffunction(...);`)"))
            .collect();
        assert_eq!(explained.len(), bare, "{source}\n{all:#?}");
        assert!(explained[0].contains("is an expression, not a statement"));
    }
    for keyword in ["fconstant", "fvariable"] {
        let all = diagnostics(&format!(
            "{keyword}(int fSamplingFreq, <math.h>);\nprocess = 1;\n"
        ));
        assert!(
            all.iter()
                .any(|d| d.contains(&format!("(`f = {keyword}(...);`)"))),
            "{all:#?}"
        );
    }
}

#[test]
fn real_duplicates_and_pattern_clauses_keep_their_contracts() {
    // A real duplicate is still one, next to a repaired statement.
    let all = diagnostics(&format!("a = 1;\n{BARE}a = 2;\nprocess = a;\n"));
    assert!(
        all.iter()
            .any(|d| d.contains("multiple definitions of symbol 'a'")),
        "{all:#?}"
    );
    assert!(all.iter().all(|d| !d.contains("symbol ''")), "{all:#?}");
    // Pattern clauses group as before.
    let output = parse_program(
        &format!("f(0) = 1;\nf(n) = 2;\n{BARE}process = f(3);\n"),
        "recovered.dsp",
    );
    assert!(
        output
            .errors
            .iter()
            .all(|e| !e.contains("multiple definitions")),
        "{:#?}",
        output.errors
    );
    // A program without the statement parses.
    assert!(
        parse_program("f(0) = 1;\nf(n) = 2;\nprocess = f(3);\n", "ok.dsp")
            .errors
            .is_empty()
    );
}

/// The `ffunction` expression itself is unchanged: named, or in `process`.
#[test]
fn a_named_or_used_foreign_function_parses() {
    for source in [
        "f = ffunction(float foo(int), \"f.h\", \"\");\nprocess = f(1);\n",
        "process = ffunction(float foo(int), \"f.h\", \"\");\n",
    ] {
        let output = parse_program(source, "ok.dsp");
        assert!(output.errors.is_empty(), "{source}: {:?}", output.errors);
    }
}
