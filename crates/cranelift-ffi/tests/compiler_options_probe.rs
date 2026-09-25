//! The `faust-rs` options `faustprobe` forwards to the compiler, under their
//! `faust-rs` spellings (`-pn NAME`, `-vec`, `-ss N`, ...).
//!
//! `faustprobe -pn NAME` used to be refused by Clap (`-p` unknown), and the
//! FFI argv parser dropped `-pn` in silence: the probe could not measure a
//! test definition of a library's test file. Each option is shown to reach
//! the program, not only to parse.

mod common;
use common::probe_source;

/// Two definitions: `-pn other` must render the second.
const TWO: &str = "process = 1; other = 2;";

/// A feedback loop through a long and a short delay, so the delay-line
/// options (`-mcd`, `-dlt`) and the vector loop all have work to do.
const LOOP: &str = "process = + ~ (@(10) : *(0.5)) : @(3);";

/// The frame-0 value of a render of [`TWO`] under `args`.
fn frame0(name: &str, args: &[&str]) -> String {
    let mut all = vec!["--double", "-n", "1", "--in", "zero"];
    all.extend_from_slice(args);
    let (ok, stdout, stderr) = probe_source(name, TWO, &all);
    assert!(ok, "{args:?}:\n{stderr}");
    stdout
        .lines()
        .find(|line| line.starts_with("0,"))
        .unwrap_or_else(|| panic!("no frame 0 in:\n{stdout}"))
        .to_owned()
}

#[test]
fn process_name_selects_the_definition_in_both_spellings() {
    assert_eq!(frame0("pn_default", &[]), "0,1.0");
    assert_eq!(frame0("pn_short", &["-pn", "other"]), "0,2.0");
    assert_eq!(frame0("pn_long", &["--process-name", "other"]), "0,2.0");
}

#[test]
fn process_name_reaches_the_second_process_of_check_determinism() {
    // The worker process recompiles the program from its own command line:
    // without the option forwarded it would render `process`, 1 against 2.
    let (ok, stdout, stderr) = probe_source(
        "pn_determinism",
        TWO,
        &[
            "--double",
            "-n",
            "4",
            "--in",
            "zero",
            "-pn",
            "other",
            "--check",
            "determinism",
        ],
    );
    assert!(ok, "{stdout}\n{stderr}");
}

#[test]
fn code_shape_options_render_the_same_samples_as_the_defaults() {
    let render = |name: &str, extra: &[&str]| {
        let mut args = vec!["--double", "-n", "200", "--block", "37"];
        args.extend_from_slice(extra);
        let (ok, stdout, stderr) = probe_source(name, LOOP, &args);
        assert!(ok, "{extra:?}:\n{stderr}");
        stdout
    };
    let reference = render("shape_default", &[]);
    assert!(
        reference.contains("0.5"),
        "the loop is silent:\n{reference}"
    );
    for (name, extra) in [
        ("shape_vec", &["-vec", "-vs", "8"][..]),
        ("shape_vec_lv1", &["-vec", "-vs", "16", "-lv", "1"]),
        ("shape_ss", &["-ss", "1"]),
        ("shape_mcd", &["-mcd", "0"]),
        ("shape_dlt", &["-dlt", "4"]),
        (
            "shape_long",
            &["--vec", "--scheduling-strategy", "2", "--mcd", "32"],
        ),
    ] {
        assert_eq!(render(name, extra), reference, "{extra:?}");
    }
}

#[test]
fn a_mode_that_cannot_take_the_compiler_options_refuses_them() {
    let refused = |name: &str, args: &[&str], expected: &str| {
        let (ok, _, stderr) = probe_source(name, TWO, args);
        assert!(!ok, "{args:?} was accepted");
        assert!(stderr.contains(expected), "{args:?}:\n{stderr}");
    };
    // `--eval` makes its own `process` of the expressions
    refused("with_eval", &["-pn", "other", "--eval", "3"], "--eval");
    // the polyphonic wrapper compiles its voices without them
    refused("with_poly", &["-vec", "--nvoices", "1"], "--nvoices 0");
    // `-vs` without `-vec` would size a loop that does not exist
    refused("vs_alone", &["-vs", "8"], "--vec");
    // a `faust-rs` option the probe does not take is an error, not ignored
    refused("class_name", &["-cn", "Other"], "--class-name");
}
