//! `--svg DIR`: the block diagrams of the program a probe would render.
//!
//! The drawing is faust-rs's own (`draw`, the port of C++ `compiler/draw/`),
//! on the box the faust-rs front end evaluated: the program measured by the
//! other modes is the one drawn, `-I`, `-pn` and `--eval` included. Three
//! things differ from `faust-rs -svg`:
//!
//! - the files go into a directory the caller names, never next to the source;
//! - they are named by definition ([`FileNaming::DefinitionName`]:
//!   `feedbackNetwork.svg`, then `feedbackNetwork-2.svg` for a second distinct
//!   diagram of that name), so a run gives the same files as the last one and a
//!   document can link them;
//! - under `--eval` the root is the expressions, titled with their text and
//!   written to `process.svg`, and an expression folded out of the root is
//!   labelled with its text rather than the wrapper's member name.

use std::collections::HashMap;

pub use draw::{Diagram, DrawConfig, FileNaming};

use crate::factory::compile_signals_with_argv;
use crate::probe::engine::compile_failure;
use crate::probe::eval::EvalProgram;

/// How to draw.
#[derive(Clone, Debug)]
pub struct DiagramOptions {
    /// Folding, label length and the look of the SVG.
    pub draw: DrawConfig,
    /// Replace every constant sub-diagram by its value before drawing (C++
    /// `-sd`), the drawing only: the program is not changed.
    pub simplify: bool,
}

impl Default for DiagramOptions {
    fn default() -> Self {
        Self {
            draw: DrawConfig {
                file_naming: FileNaming::DefinitionName,
                ..DrawConfig::default()
            },
            simplify: false,
        }
    }
}

/// Draws the program at `path` (its definition `root`, `process` unless
/// `-pn` chose another), or under `eval` the expressions evaluated in its
/// scope: root first, then the folded sub-diagrams in drawing order.
///
/// `compile_args` are the compile options as the C API `argv` spells them;
/// `-I` comes from `import_dirs`.
///
/// # Errors
/// The compiler's diagnostic text, recorded as the last compile failure of
/// this thread as a factory's would be; under `eval`, located as
/// [`EvalProgram::explain`] does.
pub fn draw_program(
    path: &str,
    eval: Option<&EvalProgram>,
    import_dirs: &[String],
    compile_args: &[String],
    root: &str,
    options: &DiagramOptions,
) -> Result<Vec<Diagram>, String> {
    let mut argv = Vec::with_capacity(2 * import_dirs.len() + compile_args.len());
    for dir in import_dirs {
        argv.push("-I".to_owned());
        argv.push(dir.clone());
    }
    argv.extend(compile_args.iter().cloned());

    let source = eval.map(EvalProgram::source);
    let mut output = compile_signals_with_argv(path, source.as_deref(), &argv)
        .map_err(compile_failure)
        .map_err(|error| match eval {
            Some(eval) => eval.explain(path, &error),
            None => error,
        })?;

    let mut def_names: HashMap<_, _> = match eval {
        None => output.def_names,
        Some(eval) => output
            .def_names
            .into_iter()
            .map(|(id, name)| match eval.member_expr(&name) {
                Some(expr) => (id, expr.to_owned()),
                None => (id, name),
            })
            .collect(),
    };
    let arena = &mut output.parse.state.arena;
    let mut process = output.process_box;
    if options.simplify {
        process = eval::simplify_diagram(arena, process, &mut def_names);
    }
    let (name, title) = match eval {
        None => (root.to_owned(), root.to_owned()),
        Some(eval) => ("process".to_owned(), eval.exprs().join(", ")),
    };
    draw::draw_diagrams(arena, process, &name, &title, &options.draw, &def_names)
        .map_err(|error| format!("cannot draw the diagram: {error}"))
}
