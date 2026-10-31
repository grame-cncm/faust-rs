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
    for (index, (number, _)) in sequences(&text).iter().enumerate() {
        assert_eq!(*number, index + 1);
    }
    // a deletion is worded as lrpar words it, and read before an insertion
    let text = messages("process = 1 2;\n");
    assert!(text.contains("\n    1: Delete 2\n    2: Insert "), "{text}");
}

/// A token that error recovery inserted (lrpar hands it to the action as
/// `Err`) has no text. The literal actions used to read that empty text and
/// report `invalid FLOAT literal` for an inserted `FLOAT`, and nothing for an
/// inserted `INT` (every byte of "" is a digit): the number of errors then
/// followed which repair the recovery had picked, and that pick changed from
/// run to run.
#[test]
fn a_literal_the_recovery_inserted_is_its_default_without_a_diagnostic() {
    use lrpar::Lexeme as _;
    use parser::ParseState;
    let lexerdef = parser::lexerdef();
    let lexer = lexerdef.lexer("1.5");
    let inserted = || Err(lrlex::DefaultLexeme::new(0, 0, 0));
    let mut state = ParseState::new("inserted.dsp");
    let _ = state.int_from_token(&lexer, inserted());
    let _ = state.float_from_token(&lexer, inserted());
    let _ = state.signed_int_from_token(&lexer, inserted(), -1);
    let _ = state.signed_float_from_token(&lexer, inserted(), -1.0);
    assert!(state.ctx.diagnostics_is_empty());
    // a token the lexer read is still checked
    let read = Ok(lrlex::DefaultLexeme::new(0, 0, 3));
    let _ = state.int_from_token(&lexer, read);
    assert!(!state.ctx.diagnostics_is_empty(), "`1.5` is not an INT");
}

/// The diagnostics say it in the tokens of the language, and the first repair
/// of each sequence is what is expected next.
#[test]
fn a_diagnostic_names_the_tokens_of_the_language() {
    let first = |source: &str| {
        let output = parse_program(source, "readable.dsp");
        output.diagnostics.as_slice()[0].message.to_string()
    };
    assert_eq!(
        first("a = 1\nprocess = a;\n"),
        "syntax error: unexpected `process`, expected `;` or `[`"
    );
    assert_eq!(
        first("g = hslider(\"g\", 0.5, 0, 1);\n"),
        "syntax error: unexpected `)`, expected `,`"
    );
    assert_eq!(first("process = 1 +;\n"), "syntax error: unexpected `;`");
}

/// The `;` is proposed at the end of the statement, as the repair the parser
/// cannot single out, and what recovery made of the rest is not reported.
#[test]
fn a_missing_semicolon_has_a_fix_and_no_cascade() {
    let output = parse_program("a = 1\nb = 2;\nprocess = a + b;\n", "semi.dsp");
    let diagnostics = output.diagnostics.as_slice();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].labels[0].span.line, 2);
    let fix = &diagnostics[0].fixes[0];
    assert_eq!(fix.edits[0].range.start, 5);
    assert_eq!(fix.edits[0].replacement.as_ref(), ";");
}

/// Two statements without `;` are two errors: the check against a repaired text
/// keeps what is really there.
#[test]
fn two_missing_semicolons_stay_two_errors() {
    let source = "a = 1\nb = 2;\nc = 3\nd = 4;\nprocess = a + b + c + d;\n";
    let output = parse_program(source, "semi.dsp");
    let lines: Vec<u32> = output
        .diagnostics
        .as_slice()
        .iter()
        .map(|diagnostic| diagnostic.labels[0].span.line)
        .collect();
    assert_eq!(lines, [2, 4]);
}

fn error_lines(source: &str) -> Vec<u32> {
    let output = parse_program(source, "semi.dsp");
    output
        .diagnostics
        .as_slice()
        .iter()
        .map(|diagnostic| diagnostic.labels[0].span.line)
        .collect()
}

/// The `;` goes after the last token, not after the comment that ends the line,
/// where it would change nothing.
#[test]
fn a_missing_semicolon_is_inserted_before_a_comment() {
    for source in [
        "a = 1 // one\nb = 2;\nprocess = a + b;\n",
        "a = 1 /* one\n   two */\nb = 2;\nprocess = a + b;\n",
    ] {
        let output = parse_program(source, "semi.dsp");
        let diagnostics = output.diagnostics.as_slice();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].fixes[0].edits[0].range.start, 5);
    }
    // what looks like a comment in a string is the string
    let source = "a = hslider(\"//\", 0, 0, 1, 0.1)\nb = 2;\n";
    let output = parse_program(source, "semi.dsp");
    let fix = &output.diagnostics.as_slice()[0].fixes[0];
    assert_eq!(
        fix.edits[0].range.start as usize,
        source.find('\n').unwrap()
    );
}

/// Dropping what a missing `;` explains keeps the errors further down.
#[test]
fn the_errors_after_a_missing_semicolon_stay() {
    let source = "a = 1 // one\nb = 2;\nc = (3;\nprocess = a + b + c;\n";
    for _ in 0..16 {
        assert_eq!(error_lines(source), [2, 3]);
    }
}

/// The end of the input errs on the line of the last missing `;`, and only the
/// position tells them apart.
#[test]
fn a_cascade_on_the_line_of_a_real_error_goes() {
    for _ in 0..16 {
        assert_eq!(error_lines("a = 1\nb = 2\nprocess = a + b;\n"), [2, 3]);
    }
}
