//! Token-stream differential between the two lexer implementations.
//!
//! Phase L0 of `porting/lexer-combined-dfa-port-plan-2026-08-06-en.md`. The
//! replacement lexer's whole contract is "the parser sees exactly the same
//! tokens", so that is what this compares — every lexeme's id, start and
//! length, and the failing offset when lexing fails.
//!
//! # Why failures are compared too
//!
//! A lexer that gets every well-formed file right and reports a malformed one
//! a byte late is still wrong: the offset reaches diagnostics this project
//! gates on. `LexOutcome::Failed` carries both the position and the lexemes
//! produced before it, and both are compared.
//!
//! # Start conditions
//!
//! `faustlexer.l` declares three exclusive conditions on top of `INITIAL`, and
//! a corpus with no `<mdoc>` in it would exercise none of them while still
//! going green. The command therefore refuses to report success unless the
//! input set demonstrably reaches each one — see [`StartConditionCoverage`].
//! This is the plan's V3 obligation, and it is checked rather than assumed for
//! the reason recorded in the 2026-08-06 journal: a gate that cannot reach its
//! subject looks exactly like one that passes.

use super::*;
use parser::{LexOutcome, LexerImpl};

/// Directories whose files are lexed, relative to the workspace root. The
/// installed library directory is added separately because it is absolute.
const CORPUS_DIRS: [&str; 3] = [
    "tests/impulse-tests/dsp",
    "tests/corpus",
    // Inputs that exist to make lexing *fail*, so the error-offset comparison
    // is exercised by the corpus and not only by unit tests.
    "tests/lexer-fixtures",
];
const FAUST_LIB_DIR: &str = "/usr/local/share/faust";

/// Evidence that the input set reaches every start condition.
///
/// Each field names a construct that can only be lexed by entering the
/// corresponding exclusive condition. They are searched for in the source
/// text, not inferred from the token stream: a bug that failed to enter a
/// condition would also suppress its tokens, so asking the token stream
/// whether the condition was exercised would be circular.
#[derive(Debug, Default)]
struct StartConditionCoverage {
    comment: usize,
    doc: usize,
    lst: usize,
}

impl StartConditionCoverage {
    fn observe(&mut self, source: &str) {
        if source.contains("/*") {
            self.comment += 1;
        }
        if source.contains("<mdoc>") {
            self.doc += 1;
        }
        if source.contains("<listing") {
            self.lst += 1;
        }
    }

    fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.comment == 0 {
            out.push("comment (no file contains `/*`)");
        }
        if self.doc == 0 {
            out.push("doc (no file contains `<mdoc>`)");
        }
        if self.lst == 0 {
            out.push("lst (no file contains `<listing`)");
        }
        out
    }
}

fn describe(outcome: &LexOutcome) -> String {
    match outcome {
        LexOutcome::Complete(l) => format!("{} lexemes, complete", l.len()),
        LexOutcome::Failed { lexemes, error_at } => {
            format!("{} lexemes, then error at byte {error_at}", lexemes.len())
        }
    }
}

/// Returns the first difference between two outcomes, if any.
fn first_difference(a: &LexOutcome, b: &LexOutcome) -> Option<String> {
    let (la, lb) = match (a, b) {
        (LexOutcome::Complete(la), LexOutcome::Complete(lb)) => (la, lb),
        (
            LexOutcome::Failed {
                lexemes: la,
                error_at: ea,
            },
            LexOutcome::Failed {
                lexemes: lb,
                error_at: eb,
            },
        ) => {
            if ea != eb {
                return Some(format!("lex error at byte {ea} vs {eb}"));
            }
            (la, lb)
        }
        _ => {
            return Some(format!("{} vs {}", describe(a), describe(b)));
        }
    };
    for (i, (x, y)) in la.iter().zip(lb.iter()).enumerate() {
        if x != y {
            return Some(format!(
                "lexeme {i}: id {} at {}+{} vs id {} at {}+{}",
                x.tok_id, x.start, x.len, y.tok_id, y.start, y.len
            ));
        }
    }
    if la.len() != lb.len() {
        return Some(format!(
            "stream length {} vs {} (identical up to the shorter)",
            la.len(),
            lb.len()
        ));
    }
    None
}

fn collect_inputs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = CORPUS_DIRS.iter().map(|d| root.join(d)).collect();
    dirs.push(PathBuf::from(FAUST_LIB_DIR));
    for dir in dirs {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_source = path.extension().is_some_and(|e| e == "dsp" || e == "lib");
            if is_source {
                out.push(path);
            }
        }
    }
    out.sort();
    if out.is_empty() {
        return Err("lexer-differential: no input files found".to_owned());
    }
    Ok(out)
}

pub(crate) fn lexer_differential(
    args: LexerDifferentialArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let inputs = collect_inputs(&root)?;
    let mut coverage = StartConditionCoverage::default();
    let mut findings: Vec<String> = Vec::new();
    let mut compared = 0usize;
    let mut lexemes_compared = 0usize;
    let mut failures = 0usize;

    for path in &inputs {
        let Ok(source) = fs::read_to_string(path) else {
            continue;
        };
        coverage.observe(&source);
        let a = parser::lex_stream(&source, LexerImpl::PerRule);
        let b = parser::lex_stream(&source, LexerImpl::CombinedDfa);
        if matches!(a, LexOutcome::Failed { .. }) {
            failures += 1;
        }
        lexemes_compared += match &a {
            LexOutcome::Complete(l) | LexOutcome::Failed { lexemes: l, .. } => l.len(),
        };
        compared += 1;
        if let Some(diff) = first_difference(&a, &b) {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            findings.push(format!("{}: {diff}", rel.display()));
            if args.verbose {
                println!("  per-rule    : {}", describe(&a));
                println!("  combined-dfa: {}", describe(&b));
            }
        }
    }

    println!(
        "lexer-differential: {compared} file(s), {lexemes_compared} lexeme(s), \
         {failures} file(s) that do not lex"
    );
    if failures == 0 {
        return Err(
            "lexer-differential: no input fails to lex, so the error-offset \
                    comparison ran on nothing; tests/lexer-fixtures must contain at \
                    least one file that stops the lexer"
                .to_owned()
                .into(),
        );
    }

    let missing = coverage.missing();
    if !missing.is_empty() {
        return Err(format!(
            "lexer-differential: the input set never reaches these start conditions, \
             so a green run would say nothing about them: {}",
            missing.join(", ")
        )
        .into());
    }
    println!(
        "  start conditions reached: comment {} file(s), doc {} file(s), lst {} file(s)",
        coverage.comment, coverage.doc, coverage.lst
    );

    if findings.is_empty() {
        println!("lexer-differential: OK (token streams identical)");
        Ok(())
    } else {
        for f in &findings {
            println!("lexer-differential: {f}");
        }
        Err(format!("lexer-differential: {} differing file(s)", findings.len()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parser::RawLexeme;

    fn lx(tok_id: u32, start: usize, len: usize) -> RawLexeme {
        RawLexeme { tok_id, start, len }
    }

    #[test]
    fn identical_streams_have_no_difference() {
        let a = LexOutcome::Complete(vec![lx(1, 0, 2), lx(2, 2, 3)]);
        let b = LexOutcome::Complete(vec![lx(1, 0, 2), lx(2, 2, 3)]);
        assert!(first_difference(&a, &b).is_none());
    }

    /// The tie-break failure the plan names as its top risk: same lengths,
    /// same positions, different rule chosen. Comparing only spans would miss
    /// it entirely.
    #[test]
    fn a_differing_token_id_at_the_same_span_is_reported() {
        let a = LexOutcome::Complete(vec![lx(1, 0, 2)]);
        let b = LexOutcome::Complete(vec![lx(7, 0, 2)]);
        let diff = first_difference(&a, &b).expect("must report");
        assert!(diff.contains("id 1"), "{diff}");
        assert!(diff.contains("id 7"), "{diff}");
    }

    #[test]
    fn a_differing_error_offset_is_reported() {
        let a = LexOutcome::Failed {
            lexemes: vec![lx(1, 0, 2)],
            error_at: 5,
        };
        let b = LexOutcome::Failed {
            lexemes: vec![lx(1, 0, 2)],
            error_at: 6,
        };
        assert!(first_difference(&a, &b).is_some_and(|d| d.contains("5 vs 6")));
    }

    /// Succeeding where the other fails is the most consequential difference
    /// and must not be mistaken for a longer stream.
    #[test]
    fn success_versus_failure_is_reported() {
        let a = LexOutcome::Complete(vec![lx(1, 0, 2)]);
        let b = LexOutcome::Failed {
            lexemes: vec![lx(1, 0, 2)],
            error_at: 2,
        };
        assert!(first_difference(&a, &b).is_some());
    }

    #[test]
    fn a_truncated_stream_is_reported() {
        let a = LexOutcome::Complete(vec![lx(1, 0, 2), lx(2, 2, 1)]);
        let b = LexOutcome::Complete(vec![lx(1, 0, 2)]);
        assert!(first_difference(&a, &b).is_some_and(|d| d.contains("length")));
    }

    #[test]
    fn missing_start_conditions_are_named() {
        let mut c = StartConditionCoverage::default();
        c.observe("process = _; /* comment */");
        let missing = c.missing();
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(missing.iter().any(|m| m.starts_with("doc")));
        assert!(missing.iter().any(|m| m.starts_with("lst")));
    }
}
