//! `--eval EXPR`: an expression evaluated in a file's scope, without a file
//! written for it.
//!
//! A question about a sub-expression ("what pole does `absorb_pole_exact`
//! give on the longest line?") used to cost a `.dsp` with an `import` and a
//! `process`, and a `.lib`, which has no `process`, could not be given to the
//! probe at all. The polyphonic engine already knew the mechanism: wrap the
//! source in `environment{ ... }` and reach inside it (it extracts `effect`
//! that way). Here the expressions are added *inside* the environment, so each
//! is evaluated in the file's own scope, with its imports and its private
//! definitions, a library's functions unprefixed as inside the library:
//!
//! ```text
//! __faustprobe_env = environment{ <line 1 of the file>
//! <the rest of the file>
//! __faustprobe_eval0 =
//! EXPR0
//! ;
//! };
//! process = __faustprobe_env.__faustprobe_eval0;
//! ```
//!
//! Two properties of this layout matter:
//!
//! - **the file keeps its line numbers**: the opener shares the file's first
//!   line instead of preceding it, so a diagnostic about line 12 of the file
//!   says line 12 (only the columns of line 1 are shifted, by the opener);
//! - **an expression is alone on its line**, starting in column 1, so a
//!   diagnostic about it has the expression as its source line and exact
//!   columns; [`EvalProgram::explain`] renames its location `<eval k>`.
//!
//! The file's own `process`, if it has one, is simply not the one evaluated,
//! so a `.lib` and a `.dsp` are treated alike.

/// What opens the environment, on the file's first line.
const OPENER: &str = "__faustprobe_env = environment{ ";

/// A file wrapped for the evaluation of some expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalProgram {
    /// The expressions, as typed but for a trailing `;` and line breaks.
    exprs: Vec<String>,
    /// The file's text, without its trailing blank lines.
    file: String,
}

impl EvalProgram {
    /// Wraps `file` for the evaluation of `exprs`.
    ///
    /// # Errors
    /// An empty expression, or no expression at all.
    pub fn new(file: &str, exprs: &[String]) -> Result<Self, String> {
        if exprs.is_empty() {
            return Err("--eval needs an expression".to_owned());
        }
        let exprs = exprs
            .iter()
            .map(|expr| {
                // one line per expression: its diagnostics are located on it
                let flat: String = expr
                    .chars()
                    .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
                    .collect();
                let flat = flat.trim().trim_end_matches(';').trim_end().to_owned();
                if flat.is_empty() {
                    Err("--eval was given an empty expression".to_owned())
                } else {
                    Ok(flat)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            exprs,
            file: file.trim_end().to_owned(),
        })
    }

    /// The expressions, in order.
    #[must_use]
    pub fn exprs(&self) -> &[String] {
        &self.exprs
    }

    /// Lines of the file as wrapped (at least one: the opener's).
    fn file_lines(&self) -> usize {
        self.file.lines().count().max(1)
    }

    /// The 1-based line of the wrapped source that holds expression `k`.
    #[must_use]
    pub fn expr_line(&self, k: usize) -> usize {
        // the file, then three lines per expression: name, expression, `;`
        self.file_lines() + 3 * k + 2
    }

    /// The environment, up to and including its closing brace.
    fn environment(&self) -> String {
        let mut source = format!("{OPENER}{}\n", self.file);
        for (k, expr) in self.exprs.iter().enumerate() {
            source.push_str(&format!("__faustprobe_eval{k} =\n{expr}\n;\n"));
        }
        source.push_str("};\n");
        source
    }

    /// The program that computes the expressions, their outputs side by side
    /// in the order given.
    #[must_use]
    pub fn source(&self) -> String {
        let members: Vec<String> = (0..self.exprs.len())
            .map(|k| format!("__faustprobe_env.__faustprobe_eval{k}"))
            .collect();
        format!("{}process = {};\n", self.environment(), members.join(", "))
    }

    /// The program whose outputs are the number of outputs of each
    /// expression, which is what attributes the columns of [`Self::source`]
    /// when there are several expressions.
    #[must_use]
    pub fn arity_source(&self) -> String {
        let members: Vec<String> = (0..self.exprs.len())
            .map(|k| format!("outputs(__faustprobe_env.__faustprobe_eval{k})"))
            .collect();
        format!("{}process = {};\n", self.environment(), members.join(", "))
    }

    /// One label per output of [`Self::source`]: the expression, with `[j]`
    /// when it has several outputs.
    ///
    /// # Errors
    /// When the arities do not add up to `outputs`, which would attribute a
    /// column to the wrong expression.
    pub fn labels(&self, arities: &[usize], outputs: usize) -> Result<Vec<String>, String> {
        if arities.len() != self.exprs.len() || arities.iter().sum::<usize>() != outputs {
            return Err(format!(
                "--eval: the expressions have {arities:?} outputs and the program {outputs}"
            ));
        }
        Ok(self
            .exprs
            .iter()
            .zip(arities)
            .flat_map(|(expr, &arity)| {
                (0..arity).map(move |j| {
                    if arity == 1 {
                        expr.clone()
                    } else {
                        format!("{expr}[{j}]")
                    }
                })
            })
            .collect())
    }

    /// Rewrites a compile error of the wrapped source for the person who
    /// typed the expressions:
    ///
    /// - a location on the lines of expression `k` becomes `<eval k>:1:COL`,
    ///   with a note saying which expression that is (locations in the file
    ///   are already right and are left alone);
    /// - an undefined symbol that the diagnostic could not locate is
    ///   attributed to the expressions that use it, with the usual reason: an
    ///   expression sees the file's top-level definitions, not those local to
    ///   a `with` block;
    /// - the wrapper's own lines, when they show, are said to be the wrapper.
    #[must_use]
    pub fn explain(&self, name: &str, error: &str) -> String {
        let mut text = error.to_owned();
        let mut notes = Vec::new();
        let mut located = vec![false; self.exprs.len()];
        for (k, expr) in self.exprs.iter().enumerate() {
            let line = self.expr_line(k);
            // the expression's own line; then its terminator, where a parser
            // reports what the expression left open: the end of the expression
            let on_expr = format!("{name}:{line}:");
            let on_terminator = format!("{name}:{}:1:", line + 1);
            located[k] = text.contains(&on_expr) || text.contains(&on_terminator);
            text = text
                .replace(
                    &on_terminator,
                    &format!("<eval {k}>:1:{}:", expr.chars().count() + 1),
                )
                .replace(&on_expr, &format!("<eval {k}>:1:"));
            if located[k] {
                notes.push(format!(
                    "  = note: <eval {k}> is `--eval '{expr}'`, line {line} of the source as wrapped"
                ));
            }
        }
        if let Some(symbol) = undefined_symbol(error) {
            for (k, expr) in self.exprs.iter().enumerate() {
                if !located[k] && identifiers(expr).any(|word| word == symbol) {
                    notes.push(format!(
                        "  = note: `{symbol}` is used by <eval {k}>, `--eval '{expr}'`: an expression sees the \
                         file's top-level definitions, not those local to a `with` block"
                    ));
                }
            }
        }
        if text.contains("__faustprobe_env") {
            notes.push(format!(
                "  = note: `{}...` and `process = ...` are the wrapper `--eval` puts around the file",
                OPENER.trim_end()
            ));
        }
        if notes.is_empty() {
            text
        } else {
            format!("{}\n{}", text.trim_end(), notes.join("\n"))
        }
    }
}

/// The symbol of an "undefined symbol `NAME`" diagnostic.
fn undefined_symbol(error: &str) -> Option<&str> {
    let rest = error.split("undefined symbol `").nth(1)?;
    rest.split('`').next()
}

/// The identifiers of an expression: maximal runs of letters, digits and `_`.
fn identifiers(expr: &str) -> impl Iterator<Item = &str> {
    expr.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// A CSV header field: quoted, as RFC 4180 has it, when it holds a comma or a
/// quote, which an expression with arguments does.
#[must_use]
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{EvalProgram, csv_field};

    fn program(file: &str, exprs: &[&str]) -> EvalProgram {
        let exprs: Vec<String> = exprs.iter().map(|e| (*e).to_owned()).collect();
        EvalProgram::new(file, &exprs).unwrap()
    }

    #[test]
    fn the_file_keeps_its_line_numbers_and_each_expression_has_its_own_line() {
        let file = "// a library\ngain = 0.5;\nhalf(x) = x * gain;\n\n\n";
        let wrapped = program(file, &["half(3)", "gain"]);
        let source = wrapped.source();
        let lines: Vec<&str> = source.lines().collect();
        // line 1 carries the opener and the file's first line; 2 and 3 are the file's
        assert!(
            lines[0].ends_with("environment{ // a library"),
            "{}",
            lines[0]
        );
        assert_eq!(lines[1], "gain = 0.5;");
        assert_eq!(lines[2], "half(x) = x * gain;");
        // an expression is alone on its line, where `expr_line` says (1-based)
        assert_eq!(lines[wrapped.expr_line(0) - 1], "half(3)");
        assert_eq!(lines[wrapped.expr_line(1) - 1], "gain");
        assert_eq!(wrapped.expr_line(0), 5);
        assert_eq!(wrapped.expr_line(1), 8);
        assert!(source.ends_with(
            "process = __faustprobe_env.__faustprobe_eval0, __faustprobe_env.__faustprobe_eval1;\n"
        ));
    }

    #[test]
    fn an_expression_is_flattened_and_loses_its_terminator() {
        let wrapped = program("x = 1;", &["  x +\n 2 ; "]);
        assert_eq!(wrapped.exprs(), ["x +  2"]);
        assert!(EvalProgram::new("x = 1;", &[" ; ".to_owned()]).is_err());
        assert!(EvalProgram::new("x = 1;", &[]).is_err());
    }

    #[test]
    fn labels_follow_the_arities() {
        let wrapped = program("", &["a", "m(2)", "b"]);
        assert_eq!(
            wrapped.labels(&[1, 4, 1], 6).unwrap(),
            ["a", "m(2)[0]", "m(2)[1]", "m(2)[2]", "m(2)[3]", "b"]
        );
        // an expression with no output owns no column
        assert_eq!(wrapped.labels(&[1, 0, 1], 2).unwrap(), ["a", "b"]);
        assert!(wrapped.labels(&[1, 4, 1], 5).is_err());
        assert!(wrapped.labels(&[1, 1], 2).is_err());
    }

    #[test]
    fn an_error_in_an_expression_is_located_in_it() {
        let wrapped = program("// lib\ngain = 0.5;", &["gain", "quarter(3)"]);
        let line = wrapped.expr_line(1);
        let error =
            format!("evaluation failed\nf.lib:{line}:1: error undefined symbol `quarter`\n");
        let explained = wrapped.explain("f.lib", &error);
        assert!(
            explained.contains("<eval 1>:1:1: error undefined symbol"),
            "{explained}"
        );
        assert!(
            explained.contains("<eval 1> is `--eval 'quarter(3)'`"),
            "{explained}"
        );
        assert!(!explained.contains("<eval 0>"), "{explained}");

        // what an expression leaves open is reported on its terminator: its end
        let error = format!(
            "parse failed\nf.lib:{}:1: error unexpected token\n",
            wrapped.expr_line(0) + 1
        );
        let explained = wrapped.explain("f.lib", &error);
        assert!(
            explained.contains("<eval 0>:1:5: error unexpected token"),
            "{explained}"
        );

        // an error in the file is already at its line
        let error = "evaluation failed\nf.lib:2:8: error undefined symbol\n";
        assert_eq!(wrapped.explain("f.lib", error), error);
    }

    #[test]
    fn an_unlocated_undefined_symbol_is_attributed_to_the_expressions_that_use_it() {
        // `tone` is local to a `with` block of the file: not in an expression's scope
        let wrapped = program(
            "process = tone with { tone = 1; };",
            &["gain * 2", "tone + tone_up"],
        );
        let error = "evaluation failed for f.dsp: undefined symbol `tone`\n\
                     f.dsp:1:1: error undefined symbol `tone`\n  1 | __faustprobe_env = environment{ process\n";
        let explained = wrapped.explain("f.dsp", error);
        assert!(
            explained.contains("`tone` is used by <eval 1>, `--eval 'tone + tone_up'`"),
            "{explained}"
        );
        assert!(
            explained.contains("not those local to a `with` block"),
            "{explained}"
        );
        // `tone_up` is another identifier, and <eval 0> does not use `tone`
        assert!(!explained.contains("<eval 0>"), "{explained}");
        assert!(
            explained.contains("are the wrapper `--eval` puts around the file"),
            "{explained}"
        );
    }

    #[test]
    fn a_header_field_with_a_comma_is_quoted() {
        assert_eq!(csv_field("gain"), "gain");
        assert_eq!(csv_field("pole(1709, 2.0)"), "\"pole(1709, 2.0)\"");
        assert_eq!(csv_field("label(\"a\")"), "\"label(\"\"a\"\")\"");
    }
}
