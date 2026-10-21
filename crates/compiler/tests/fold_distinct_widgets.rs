//! Constant folding keeps distinct widgets distinct.
//!
//! Every argument of a pattern-matched call is folded to a number when it is
//! one (C++ `simplifyPattern` / `isBoxNumeric`). The fold propagates the
//! argument without a UI program, so widgets get placeholder control ids; with
//! one id shared by all of them, two widgets became one signal, and `b - a`
//! folded to `0`, `b / a` to `1`, `b < a` to `0`: the widgets disappeared from
//! the interface and the output was a constant. abclib's irregular decoder
//! (`circularScaledVBAP`, which reads its loudspeaker angles with `ba.take`)
//! lost all its angle controls that way.

use std::io::Cursor;

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::Compiler;

// ba.take, written out so that the test needs no library
const TAKE: &str = r#"take(1, (x, xs)) = x;
take(1, x) = x;
take(n, (x, xs)) = take(n - 1, xs);
"#;

/// Renders one frame of a program with no input and one output per entry of
/// `expected`, and compares it with `expected`.
fn assert_first_frame(name: &str, body: &str, expected: &[f32]) {
    let source = format!("{TAKE}{body}");
    let fbc = Compiler::new()
        .compile_source_to_interp(name, &source, &InterpOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("parse fbc");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    assert_eq!(
        instance.get_num_outputs() as usize,
        expected.len(),
        "{name}"
    );
    let mut buffers = vec![vec![0.0_f32; 1]; expected.len()];
    let inputs: [&[f32]; 0] = [];
    let mut outputs: Vec<&mut [f32]> = buffers.iter_mut().map(|b| b.as_mut_slice()).collect();
    instance
        .try_compute(1, &inputs, &mut outputs)
        .expect("compute");
    let got: Vec<f32> = buffers.iter().map(|b| b[0]).collect();
    assert_eq!(got, expected, "{name}:\n{source}");
}

#[test]
fn two_widgets_in_a_pattern_argument_stay_two_signals() {
    let widgets = r#"a = nentry("a", 1, 0, 10, 1);
b = nentry("b", 4, 0, 10, 1);
"#;
    for (name, process, expected) in [
        ("fold-sub", "process = take(1, b - a);", 3.0),
        ("fold-div", "process = take(1, b / a);", 4.0),
        ("fold-lt", "process = take(1, a < b);", 1.0),
        ("fold-pair", "process = take(1, (b - a, 0));", 3.0),
    ] {
        assert_first_frame(name, &format!("{widgets}{process}"), &[expected]);
    }
}

#[test]
fn widgets_of_one_list_read_by_take_stay_distinct() {
    // the shape of abclib's `circularScaledVBAP`: two elements of a list of
    // widgets, read with `take`, compared inside another `take` argument
    assert_first_frame(
        "fold-list",
        r#"l = par(i, 2, nentry("a%i", 30 * (i + 1), 0, 360, 1));
process = take(1, (take(2, l) - take(1, l), 0));
"#,
        &[30.0],
    );
}

#[test]
fn one_widget_in_two_groups_is_two_controls() {
    // the same box in two groups is two controls (two label paths), so the
    // difference is no constant either, and both stay in the interface
    let cpp = Compiler::new()
        .compile_source_to_cpp(
            "fold-groups",
            &format!(
                "{TAKE}w = nentry(\"a\", 1, 0, 10, 1);\nprocess = take(1, hgroup(\"x\", w) - hgroup(\"y\", w));\n"
            ),
            &CppOptions::default(),
        )
        .expect("C++ codegen");
    assert_eq!(cpp.matches("addNumEntry(\"a\"").count(), 2, "{cpp}");
}

#[test]
fn one_widget_minus_itself_still_folds() {
    // C++ folds `a - a` to 0 in a pattern argument; distinct ids must not
    // break that
    let cpp = Compiler::new()
        .compile_source_to_cpp(
            "fold-same",
            &format!("{TAKE}a = nentry(\"a\", 1, 0, 10, 1);\nprocess = take(1, a - a);\n"),
            &CppOptions::default(),
        )
        .expect("C++ codegen");
    assert!(!cpp.contains("addNumEntry"), "{cpp}");
}

#[test]
fn the_folded_widgets_are_in_the_interface() {
    let cpp = Compiler::new()
        .compile_source_to_cpp(
            "fold-ui",
            &format!(
                "{TAKE}a = nentry(\"a\", 1, 0, 10, 1);\nb = nentry(\"b\", 4, 0, 10, 1);\nprocess = take(1, b - a);\n"
            ),
            &CppOptions::default(),
        )
        .expect("C++ codegen");
    assert!(cpp.contains("addNumEntry(\"a\""), "{cpp}");
    assert!(cpp.contains("addNumEntry(\"b\""), "{cpp}");
}
