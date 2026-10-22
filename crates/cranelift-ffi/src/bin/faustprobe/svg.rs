//! `--svg DIR`: the block diagrams of the program, written into DIR, and the
//! index of what was written on stdout.
//!
//! Draw-only: the program is evaluated, not compiled to code, and nothing is
//! rendered. A figure is a question about structure, which a render does not
//! answer, and drawing and measuring in one command would leave the index and
//! the rows competing for stdout.

use std::path::Path;

use cranelift_ffi::probe::diagram::{Diagram, DiagramOptions, draw_program};
use cranelift_ffi::probe::eval::EvalProgram;

use crate::cli::{Args, Format};
use crate::report::print_json;
use crate::setup::compiler_args;

/// Draws, writes, and prints the index.
pub(crate) fn run_svg(args: &Args) -> Result<(), String> {
    let Some(dir) = args.svg.as_deref() else {
        return Err("--svg needs a directory".to_owned());
    };
    if args.format == Format::Ir {
        return Err("--svg prints an index, as CSV or with --format json; not as .ir".to_owned());
    }
    let eval = if args.evals.is_empty() {
        None
    } else {
        let text = std::fs::read_to_string(&args.file)
            .map_err(|e| format!("cannot read '{}': {e}", args.file))?;
        Some(EvalProgram::new(&text, &args.evals)?)
    };
    let diagrams = draw_program(
        &args.file,
        eval.as_ref(),
        &args.import_dirs,
        &compiler_args(args),
        &args.compile.process_name,
        &options(args),
    )?;

    let dir_path = Path::new(dir);
    std::fs::create_dir_all(dir_path).map_err(|e| format!("cannot create '{dir}': {e}"))?;
    for diagram in &diagrams {
        let path = dir_path.join(&diagram.file);
        std::fs::write(&path, &diagram.svg)
            .map_err(|e| format!("cannot write '{}': {e}", path.display()))?;
    }

    if args.format == Format::Json {
        return print_json(index_json(args, dir, eval.as_ref(), &diagrams));
    }
    println!("file,name");
    for diagram in &diagrams {
        println!("{},{}", diagram.file, csv_field(&diagram.name));
    }
    Ok(())
}

/// The drawing options of the command line, over the probe's defaults.
fn options(args: &Args) -> DiagramOptions {
    let mut options = DiagramOptions::default();
    let draw = &mut options.draw;
    draw.fold_threshold = args.fold.unwrap_or(draw.fold_threshold);
    draw.fold_complexity = args.fold_complexity.unwrap_or(draw.fold_complexity);
    draw.max_name_size = args.max_name_size.unwrap_or(draw.max_name_size);
    draw.scaled_svg = args.scaled_svg;
    draw.shadow_blur = args.shadow_blur;
    draw.draw_route_frame = args.draw_route_frame;
    options.simplify = args.simplify_diagrams;
    options
}

/// The index as one JSON document: where, what was drawn, and the diagrams.
fn index_json(
    args: &Args,
    dir: &str,
    eval: Option<&EvalProgram>,
    diagrams: &[Diagram],
) -> serde_json::Map<String, serde_json::Value> {
    let mut document = serde_json::Map::new();
    document.insert("schema_version".to_owned(), serde_json::json!(1));
    document.insert("dsp".to_owned(), serde_json::json!(args.file));
    if let Some(eval) = eval {
        document.insert("eval".to_owned(), serde_json::json!(eval.exprs()));
    }
    document.insert("svg_dir".to_owned(), serde_json::json!(dir));
    let entries: Vec<serde_json::Value> = diagrams
        .iter()
        .map(|d| serde_json::json!({ "file": d.file, "name": d.name }))
        .collect();
    document.insert("diagrams".to_owned(), serde_json::json!(entries));
    document
}

/// `text` as one CSV field: quoted, its quotes doubled, when it holds a
/// comma, a quote or a line break (an `--eval` expression does).
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn a_field_is_quoted_only_when_it_must_be() {
        assert_eq!(csv_field("feedbackNetwork"), "feedbackNetwork");
        assert_eq!(
            csv_field("sc.parallel(7, 4, mk)"),
            "\"sc.parallel(7, 4, mk)\""
        );
        assert_eq!(csv_field("hslider(\"g\",0)"), "\"hslider(\"\"g\"\",0)\"");
    }
}
