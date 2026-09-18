//! A syntax error's repair suggestions come in one order, every time.
//!
//! lrpar deduplicates its repair sequences through a `HashSet` and sorts them
//! by length only; the ties keep the set's iteration order, which changes with
//! every instance. Rendered as lrpar renders them, the same error listed its
//! sixty suggestions in another order at every parse, which breaks any
//! byte-for-byte comparison of diagnostics and leaves a numbered list whose
//! numbers cannot be referred to. The parser renders the list itself, in an
//! order that is a total one.

use parser::parse_program;

/// The rendered messages of the syntax errors of `source`.
fn messages(source: &str) -> String {
    let output = parse_program(source, "order.dsp");
    assert!(!output.errors.is_empty(), "no error for {source:?}");
    output.errors.join("\n")
}

/// The numbered repair sequences of one message, as (number, text).
fn sequences(message: &str) -> Vec<(usize, String)> {
    message
        .lines()
        .filter_map(|line| {
            let (number, text) = line.trim_start().split_once(": ")?;
            Some((number.parse().ok()?, text.to_owned()))
        })
        .collect()
}

#[test]
fn the_same_error_lists_its_repairs_in_the_same_order_at_every_parse() {
    // an operator without its right operand: some sixty single-token inserts
    let first = messages("good = 1;\nbad = 1 +;\n");
    assert!(first.contains("Repair sequences found:"), "{first}");
    assert!(sequences(&first).len() > 20, "{first}");
    // each parse builds a fresh recoverer, and a fresh hasher with it
    for _ in 0..16 {
        assert_eq!(messages("good = 1;\nbad = 1 +;\n"), first);
    }
    // an unterminated expression: sequences of two repairs
    let first = messages("process = twice(;\n");
    assert!(first.contains("Insert RPAR"), "{first}");
    for _ in 0..16 {
        assert_eq!(messages("process = twice(;\n"), first);
    }
}

#[test]
fn shorter_repairs_come_first_and_equal_ones_read_alphabetically() {
    // a parenthesis left open before a stray token: some twenty repairs of
    // two edits (`Insert RPAR, Delete 2`, `Insert RPAR, Insert ADD`, ...) and
    // as many of three (`Insert ABS, Shift 2, Insert RPAR`, ...), which by
    // text alone would come first
    let listed = sequences(&messages("process = (1 2;\n"));
    assert!(listed.len() > 20, "{listed:?}");
    let cost = |text: &str| text.split(", ").count();
    assert!(listed.iter().any(|(_, text)| cost(text) == 2), "{listed:?}");
    assert!(listed.iter().any(|(_, text)| cost(text) == 3), "{listed:?}");
    // numbered from one, without a gap
    for (k, (number, _)) in listed.iter().enumerate() {
        assert_eq!(*number, k + 1);
    }
    // a repair costs the number of edits it makes, and a dearer one never
    // comes before a cheaper one
    for pair in listed.windows(2) {
        let (a, b) = (&pair[0].1, &pair[1].1);
        assert!(
            cost(a) < cost(b) || (cost(a) == cost(b) && a < b),
            "`{a}` is listed before `{b}`"
        );
    }
}

#[test]
fn the_text_is_the_one_lrpar_wrote() {
    // one repair: the text the documents quote, padding included
    assert_eq!(
        messages("process = _ : *(0.5 ;\n"),
        "Parsing error at line 1 column 21. Repair sequences found:\n   1: Insert RPAR"
    );
    // sixty: the numbers are right-aligned, as lrpar pads them
    let text = messages("good = 1;\nbad = 1 +;\n");
    assert!(text.contains("\n    9: Insert "), "{text}");
    assert!(text.contains("\n   10: Insert "), "{text}");
    // a deletion is worded as lrpar words it, and read before an insertion
    let text = messages("process = 1 2;\n");
    assert!(text.contains("\n    1: Delete 2\n    2: Insert "), "{text}");
}
