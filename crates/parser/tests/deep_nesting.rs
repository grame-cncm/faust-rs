//! A deeply nested program parses on a small thread (grame-cncm/faust-rs#16).
//!
//! The import expansion that follows the parse rewrites the box tree
//! recursively, one native frame per level. It overflowed the CLI's 512 MiB
//! stack between 400 000 and 600 000 levels of `1+1+...+1`, before the
//! evaluator could report its budget, and an 8 MiB host thread far earlier.
//! It now continues on stack segments grown on demand.

use parser::{ParseOptions, parse_program_with_imports};

#[test]
fn a_deep_chain_parses_on_a_one_mib_thread() {
    let source = format!("process = {};", vec!["1"; 20_000].join("+"));
    let output = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            parse_program_with_imports(&source, "deep.dsp", &ParseOptions::default())
                .map(|output| output.diagnostics.error_count())
        })
        .expect("spawn")
        .join()
        .expect("the parse should finish, not abort");
    assert_eq!(output.ok(), Some(0));
}
