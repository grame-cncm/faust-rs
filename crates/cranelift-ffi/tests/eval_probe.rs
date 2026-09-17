//! `--eval EXPR`: an expression evaluated in a file's scope (phase F3 of
//! `porting/faustprobe-feedback-quality-analysis-and-plan-2026-09-17-en.md`).
//!
//! A question about a sub-expression used to cost a `.dsp` with an `import`
//! and a `process`, and a `.lib` could not be given to the probe at all. The
//! independent route these tests compare with is that one: the hand-written
//! program that states the same expression.

mod common;
use common::{Fixtures, probe_in};

/// A library: no `process`, a private helper, a control, a filter.
const LIB: &str = r#"// a small library
scale = 0.25;
third(x) = x / 3.0;
pair(x) = x, x * scale;
g = hslider("g", 0.5, 0, 1, 0.001);
gained(x) = x * g;
onepole(p) = + ~ *(p);
"#;

fn probe(args: &[&str]) -> (bool, String, String) {
    probe_in(&std::env::temp_dir(), args)
}

/// The rows of a CSV dump, without its header.
fn rows(stdout: &str) -> Vec<&str> {
    stdout.lines().skip(1).collect()
}

#[test]
fn an_expression_gives_what_the_hand_written_program_gives() {
    let fixtures = Fixtures::new("same");
    let lib = fixtures.write("small.lib", LIB);
    // the route used until now: a program that imports the library and states
    // the expressions, in the same order
    let program = fixtures.write(
        "by_hand.dsp",
        "s = library(\"small.lib\");\nprocess = s.third(2.0), s.scale * 8, s.onepole(0.5);\n",
    );
    let common = ["--double", "--in", "impulse", "-n", "6"];
    let (ok, by_hand, stderr) = probe(&[&common[..], &[program.as_str()]].concat());
    assert!(ok, "{stderr}");
    let (ok, evaluated, stderr) = probe(
        &[
            &common[..],
            &[
                "--eval",
                "third(2.0)",
                "--eval",
                "scale * 8",
                "--eval",
                "onepole(0.5)",
                lib.as_str(),
            ],
        ]
        .concat(),
    );
    assert!(ok, "{stderr}");
    assert_eq!(rows(&evaluated), rows(&by_hand));
    // and the values are the ones the definitions give: 2/3, 2, then 0.5^n
    assert_eq!(rows(&evaluated)[0], "0,0.6666666666666666,2.0,1.0");
    assert_eq!(rows(&evaluated)[3], "3,0.6666666666666666,2.0,0.125");
}

#[test]
fn the_expressions_head_the_columns_and_are_attributed_by_their_outputs() {
    let fixtures = Fixtures::new("labels");
    let lib = fixtures.write("small.lib", LIB);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "scale",
        "--eval",
        "pair(2.0)",
        "--eval",
        "third(3.0)",
        lib.as_str(),
    ]);
    assert!(ok, "{stderr}");
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next().unwrap(),
        "frame,scale,pair(2.0)[0],pair(2.0)[1],third(3.0)"
    );
    assert_eq!(lines.next().unwrap(), "0,0.25,2.0,0.5,1.0");
    // the legend goes with the statistics, which is all `--quiet` prints
    assert!(stderr.contains("# eval out1 = pair(2.0)[0]"), "{stderr}");
    assert!(stderr.contains("# eval out3 = third(3.0)"), "{stderr}");

    // an expression with arguments holds commas: its header field is quoted
    let (_, stdout, _) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "max(scale, 1)",
        lib.as_str(),
    ]);
    assert_eq!(stdout.lines().next().unwrap(), "frame,\"max(scale, 1)\"");
}

#[test]
fn a_program_is_evaluated_like_a_library_and_its_process_is_not_the_one_probed() {
    let fixtures = Fixtures::new("program");
    let program = fixtures.write(
        "prog.dsp",
        "declare name \"prog\";\ndeclare options \"[nvoices:4]\";\nlevel = 0.125;\nprocess = _ * level : !;\n",
    );
    // `process` has one input and no output; the expression is what is probed
    let (ok, stdout, stderr) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "level * 4",
        program.as_str(),
    ]);
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "frame,level * 4\n0,0.5\n");
}

#[test]
fn controls_sweeps_and_listing_apply_to_the_expression() {
    let fixtures = Fixtures::new("controls");
    let lib = fixtures.write("small.lib", LIB);
    let (ok, stdout, stderr) = probe(&[
        "--in",
        "dc",
        "-n",
        "4",
        "--sweep",
        "g=0.25,1",
        "--reduce",
        "peak",
        "--eval",
        "gained",
        lib.as_str(),
    ]);
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "g,peak_out0\n0.25,0.25\n1,1.0\n");
    assert!(stderr.contains("# eval out0 = gained"), "{stderr}");

    // the paths are those of the file, and only the controls the expression
    // uses exist in the program it makes
    let (ok, stdout, _) = probe(&["--list-params", "--eval", "gained", lib.as_str()]);
    assert!(ok);
    assert!(stdout.contains("/small/g "), "{stdout}");
    let (_, stdout, _) = probe(&["--list-params", "--eval", "scale", lib.as_str()]);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");

    // a value outside the control's range is still refused
    let (ok, _, stderr) = probe(&["--set", "g=3", "--eval", "gained", lib.as_str()]);
    assert!(!ok);
    assert!(
        stderr.contains("outside the range [0, 1] of /small/g"),
        "{stderr}"
    );
}

#[test]
fn the_file_directory_resolves_its_relative_imports_from_anywhere() {
    let fixtures = Fixtures::new("imports");
    fixtures.write("libs/small.lib", LIB);
    let user = fixtures.write(
        "libs/user.lib",
        "s = library(\"small.lib\");\ntwice = s.scale * 2;\n",
    );
    // run from another directory, with no -I: only the file's own directory
    // can find `small.lib`
    let elsewhere = std::env::temp_dir();
    let (ok, stdout, stderr) = probe_in(
        &elsewhere,
        &["--in", "zero", "-n", "1", "--eval", "twice", &user],
    );
    assert!(ok, "{stderr}");
    assert_eq!(rows(&stdout), ["0,0.5"]);
    // and a bare file name, whose directory is the current one
    let (ok, stdout, stderr) = probe_in(
        &fixtures.dir().join("libs"),
        &["--in", "zero", "-n", "1", "--eval", "twice", "user.lib"],
    );
    assert!(ok, "{stderr}");
    assert_eq!(rows(&stdout), ["0,0.5"]);
}

#[test]
fn an_error_in_the_file_keeps_its_line_and_one_in_an_expression_is_located_in_it() {
    let fixtures = Fixtures::new("errors");
    let broken = fixtures.write(
        "broken.lib",
        "// a library\ngain = 0.5;\nhalf(x) = x * gian;\n",
    );
    let (ok, _, stderr) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "half(3)",
        broken.as_str(),
    ]);
    assert!(!ok);
    // line 3 of the file, column of `gian`: the wrapper has not moved it
    assert!(
        stderr.contains("broken.lib:3:15: error [FRS-EVAL-0002] undefined symbol `gian`"),
        "{stderr}"
    );
    assert!(stderr.contains("  3 | half(x) = x * gian;"), "{stderr}");
    assert!(!stderr.contains("<eval"), "{stderr}");

    let lib = fixtures.write("small.lib", LIB);
    let (ok, _, stderr) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "scale",
        "--eval",
        "third(1) + quarter",
        lib.as_str(),
    ]);
    assert!(!ok);
    // column 12 of the second expression, which is its source line
    assert!(
        stderr.contains("<eval 1>:1:12: error [FRS-EVAL-0002] undefined symbol `quarter`"),
        "{stderr}"
    );
    assert!(stderr.contains("| third(1) + quarter"), "{stderr}");
    assert!(
        stderr.contains("<eval 1> is `--eval 'third(1) + quarter'`"),
        "{stderr}"
    );
    assert!(!stderr.contains("<eval 0>"), "{stderr}");

    // what an expression leaves open is reported at its end
    let (ok, _, stderr) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "scale * (2",
        lib.as_str(),
    ]);
    assert!(!ok);
    assert!(
        stderr.contains("<eval 0>:1:11: error [FRS-PARSE-0001]"),
        "{stderr}"
    );
    assert!(stderr.contains("insert `)`"), "{stderr}");
}

#[test]
fn a_definition_local_to_a_with_block_is_said_to_be_out_of_scope() {
    let fixtures = Fixtures::new("with");
    let program = fixtures.write(
        "local.dsp",
        "process = _ * depth\nwith {\n    depth = 0.5;\n};\n",
    );
    let (ok, _, stderr) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--eval",
        "depth * 2",
        program.as_str(),
    ]);
    assert!(!ok);
    assert!(stderr.contains("undefined symbol `depth`"), "{stderr}");
    assert!(
        stderr.contains("`depth` is used by <eval 0>, `--eval 'depth * 2'`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("not those local to a `with` block"),
        "{stderr}"
    );
}

#[test]
fn a_training_run_takes_its_loss_from_an_expression() {
    let fixtures = Fixtures::new("train");
    let lib = fixtures.write(
        "loss.lib",
        "w = hslider(\"w\", 0, -1, 1, 0.0001);\nloss_and_gradient = (w - 0.5) * (w - 0.5), 2 * (w - 0.5);\n",
    );
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--in",
        "zero",
        "--block",
        "8",
        "--blocks",
        "2",
        "--optimizer",
        "sgd",
        "--lr",
        "0.125",
        "--train",
        "w",
        "--eval",
        "loss_and_gradient",
        lib.as_str(),
    ]);
    assert!(ok, "{stderr}");
    // w: 0 -> 0.125 -> 0.21875, as the same program written as a file gives
    assert!(
        stdout
            .lines()
            .any(|line| line == "# trained /loss/w=0.21875"),
        "{stdout}"
    );
    assert!(stdout.contains("\n2,1.40625e-1,0.21875\n"), "{stdout}");
}

#[test]
fn json_names_what_each_output_computes() {
    let fixtures = Fixtures::new("json");
    let lib = fixtures.write("small.lib", LIB);
    let (ok, stdout, stderr) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--format",
        "json",
        "--eval",
        "scale",
        "--eval",
        "pair(1.0)",
        lib.as_str(),
    ]);
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        document["eval"],
        serde_json::json!(["scale", "pair(1.0)[0]", "pair(1.0)[1]"])
    );
    assert_eq!(document["runs"][0]["channels"].as_array().unwrap().len(), 3);
    // a run without --eval has no such key
    let program = fixtures.write("p.dsp", "process = 1;\n");
    let (_, stdout, _) = probe(&[
        "--in",
        "zero",
        "-n",
        "1",
        "--format",
        "json",
        program.as_str(),
    ]);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert!(document.get("eval").is_none());
}

#[test]
fn only_what_an_expression_uses_is_evaluated() {
    // Faust evaluates lazily: `--eval 0` checks that a library parses and
    // that its imports resolve, not that its functions are sound.
    let fixtures = Fixtures::new("lazy");
    let lib = fixtures.write(
        "lazy.lib",
        "// a library\nsound(x) = x * 2;\nunsound(x) = x * oops;\n",
    );
    let common = ["--in", "zero", "-n", "1", "--quiet"];
    let (ok, _, stderr) = probe(&[&common[..], &["--eval", "0", lib.as_str()]].concat());
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = probe(&[&common[..], &["--eval", "sound(1)", lib.as_str()]].concat());
    assert!(ok, "{stderr}");
    // the function that is evaluated is checked, and cited at its own line
    let (ok, _, stderr) = probe(&[&common[..], &["--eval", "unsound(1)", lib.as_str()]].concat());
    assert!(!ok);
    assert!(
        stderr.contains("lazy.lib:3:18: error [FRS-EVAL-0002] undefined symbol `oops`"),
        "{stderr}"
    );
    // what `--eval 0` does catch: a library that does not parse
    let broken = fixtures.write("syntax.lib", "sound(x) = x * 2\nother = 1;\n");
    let (ok, _, stderr) = probe(&[&common[..], &["--eval", "0", broken.as_str()]].concat());
    assert!(!ok);
    assert!(stderr.contains("FRS-PARSE"), "{stderr}");
}

#[test]
fn eval_refuses_what_it_cannot_do() {
    let fixtures = Fixtures::new("check");
    let lib = fixtures.write("small.lib", LIB);
    // without --eval a library has no `process` to probe
    let (ok, _, _) = probe(&["--in", "zero", "-n", "1", "--quiet", lib.as_str()]);
    assert!(!ok);

    for (args, expected) in [
        (
            vec!["--eval", "scale", "--nvoices", "2"],
            "--eval operates on the scalar Probe only",
        ),
        (
            vec!["--eval", "scale", "--protocol", "impulse-test"],
            "remove --eval",
        ),
        (vec!["--eval", " ; "], "empty expression"),
    ] {
        let (ok, _, stderr) = probe(&[&args[..], &[lib.as_str()]].concat());
        assert!(!ok, "{args:?} passed");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}
