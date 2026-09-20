//! `-I DIR` overrides a standard library.
//!
//! The search order is the C++ compiler's: the `-I` dirs, then the source's
//! directory, then the installed libraries. The FFI file constructor used to
//! append the `-I` dirs after the defaults, so `faustprobe -I checkout` on a
//! program importing `analyzers.lib` silently measured the installed copy: a
//! deliberately wrong permutation in the checkout went unseen. The witness is
//! a library carrying a standard name that the installed one does not define.

mod common;
use common::{Fixtures, probe};

/// An `analyzers.lib` that defines a symbol the real one does not.
const MUTATED: &str = "declare name \"analyzers.lib\";\nprobe_marker = 42.0;\n";

/// A program that compiles only against the mutated library.
const PROGRAM: &str = "an = library(\"analyzers.lib\");\nprocess = an.probe_marker;\n";

#[test]
fn an_import_dir_overrides_an_installed_standard_library() {
    let fixtures = Fixtures::new("import_dir_override");
    let lib_dir = fixtures.path("mutated");
    fixtures.write("mutated/analyzers.lib", MUTATED);
    let program = fixtures.write("program/marker.dsp", PROGRAM);

    let (ok, stdout, stderr) = probe(&[
        "--double", "-I", &lib_dir, "-n", "1", "--in", "zero", &program,
    ]);
    assert!(
        ok,
        "the mutated library was not found through -I:\n{stderr}"
    );
    assert!(
        stdout.lines().any(|line| line == "0,42.0"),
        "expected the mutated library's value on frame 0:\n{stdout}"
    );

    // The control: without `-I` the installed `analyzers.lib` wins and the
    // symbol is undefined, so the override above is not an accident of the
    // search order falling through.
    let (ok, _, stderr) = probe(&["--double", "-n", "1", "--in", "zero", &program]);
    assert!(!ok, "the program compiled without the mutated library");
    assert!(stderr.contains("probe_marker"), "{stderr}");
}

#[test]
fn an_import_dir_comes_before_the_source_directory() {
    // The same library name next to the program and in a `-I` dir: `-I` wins,
    // as with `faust -I`.
    let fixtures = Fixtures::new("import_dir_before_source_dir");
    let lib_dir = fixtures.path("mutated");
    fixtures.write("mutated/analyzers.lib", MUTATED);
    fixtures.write(
        "program/analyzers.lib",
        "declare name \"analyzers.lib\";\nprobe_marker = 7.0;\n",
    );
    let program = fixtures.write("program/marker.dsp", PROGRAM);

    let (ok, stdout, stderr) = probe(&[
        "--double", "-I", &lib_dir, "-n", "1", "--in", "zero", &program,
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.lines().any(|line| line == "0,42.0"), "{stdout}");

    let (ok, stdout, stderr) = probe(&["--double", "-n", "1", "--in", "zero", &program]);
    assert!(ok, "{stderr}");
    assert!(stdout.lines().any(|line| line == "0,7.0"), "{stdout}");
}
