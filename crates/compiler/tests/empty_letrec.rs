//! An empty `letrec` defines nothing (faust issue #416).
//!
//! `e letrec { }` is `e`, as `e with { }` is: the `where` definitions are only
//! visible inside the recursive bodies, so they add nothing either. The
//! expansion used to build an empty parallel list and panicked ("expression
//! list tail"); the C++ compiler crashed on the same programs. A symbol that
//! the letrec does not define stays undefined.

use compiler::Compiler;
use signals::{SigMatch, match_sig};

fn arity(source: &str) -> (u32, u32) {
    let out = Compiler::new()
        .compile_source_to_signals("t.dsp", source)
        .unwrap_or_else(|e| panic!("{source:?} should compile: {e:?}"));
    (out.process_arity.inputs as u32, out.process_arity.outputs as u32)
}

#[test]
fn an_empty_letrec_is_its_body() {
    assert_eq!(arity("process = 1 letrec { };"), (0, 1));
    assert_eq!(arity("process = _ letrec { } : +(1);"), (1, 1));
    assert_eq!(arity("process = 1 letrec { where f = 2; };"), (0, 1));
    assert_eq!(arity("process = v letrec { } with { v = 1; };"), (0, 1));
    assert_eq!(arity("process = (x letrec { }) letrec { 'x = x * 0.5 + 1; };"), (0, 1));

    let out = Compiler::new()
        .compile_source_to_signals("t.dsp", "process = 3 letrec { };")
        .expect("the program compiles");
    assert_eq!(match_sig(&out.parse.state.arena, out.signals[0]), SigMatch::Int(3));
}

#[test]
fn a_symbol_an_empty_letrec_does_not_define_stays_undefined() {
    let err = Compiler::new()
        .compile_source_to_signals("t.dsp", "process = v letrec { };")
        .expect_err("v is undefined");
    let text = format!("{err:?}");
    assert!(text.contains("undefined symbol"), "{text}");
}
