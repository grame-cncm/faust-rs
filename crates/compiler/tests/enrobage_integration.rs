//! Integration tests for architecture wrapping assembly (`enrobage` Step E).
//!
//! Scope:
//! - Verifies wrapper orchestration around generated C++ text.
//! - Confirms marker slicing and class-name replacement in wrapped output.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use compiler::enrobage::{EnrobageOptions, wrap_cpp_with_architecture};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("enrobage")
}

fn fixture_arch(file: &str) -> PathBuf {
    fixture_root().join("arch").join(file)
}

fn fixture_corpus(file: &str) -> PathBuf {
    fixture_root().join("corpus").join(file)
}

fn dsp_corpus(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

#[test]
fn wrap_cpp_with_architecture_matches_expected_fixture() {
    let generated_cpp = "// GENERATED CLASS\nclass customdsp : public faust_dsp {};\n";
    let mut options = EnrobageOptions::new(fixture_arch("wrapper.cpp"));
    options.architecture_dirs = vec![fixture_root().join("arch")];
    options.class_name = "customdsp".to_owned();
    options.super_class_name = "faust_dsp".to_owned();
    options.inline_arch_files = true;

    let wrapped = wrap_cpp_with_architecture(generated_cpp, &options)
        .expect("wrap_cpp_with_architecture should succeed");
    let expected = read(&fixture_corpus("wrapper_wrapped.expected.cpp"));
    assert_eq!(wrapped.code, expected);
    assert_eq!(
        wrapped.recoverable_error, None,
        "fixture wrapper should not produce include-injection errors"
    );
}

#[test]
fn cli_lang_julia_accepts_architecture_wrapper() {
    let output = Command::new(env!("CARGO_BIN_EXE_faust-rs"))
        .arg("-lang")
        .arg("julia")
        .arg("-a")
        .arg(fixture_arch("wrapper.jl"))
        .arg(dsp_corpus("rep_01_passthrough.dsp"))
        .output()
        .expect("run faust-rs -lang julia with architecture wrapper");

    assert!(
        output.status.success(),
        "faust-rs failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8 Julia text");
    assert!(stdout.contains("# Fixture Julia architecture wrapper."));
    assert!(stdout.contains("mutable struct mydsp{T} <: dsp"));
    assert!(stdout.contains("architecture_footer = true"));
}

#[test]
fn cli_lang_cmajor_accepts_architecture_wrapper() {
    let output = Command::new(env!("CARGO_BIN_EXE_faust-rs"))
        .arg("-lang")
        .arg("cmajor")
        .arg("-a")
        .arg(fixture_arch("wrapper.cmajor"))
        .arg(dsp_corpus("rep_01_passthrough.dsp"))
        .output()
        .expect("run faust-rs -lang cmajor with architecture wrapper");

    assert!(
        output.status.success(),
        "faust-rs failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8 Cmajor text");
    assert!(stdout.starts_with("// Fixture Cmajor architecture wrapper."));
    assert!(stdout.contains("processor mydsp"));
    assert!(stdout.ends_with("// End Cmajor architecture wrapper.\n"));
}

#[test]
fn wrap_cpp_with_architecture_accepts_julia_templates() {
    let generated_julia = "# GENERATED JULIA DSP\nmutable struct customdsp{T} <: faust_dsp\nend\n";
    let mut options = EnrobageOptions::new(fixture_arch("wrapper.jl"));
    options.class_name = "customdsp".to_owned();
    options.super_class_name = "faust_dsp".to_owned();

    let wrapped = wrap_cpp_with_architecture(generated_julia, &options)
        .expect("architecture wrapping should accept Julia templates");
    let expected = read(&fixture_corpus("wrapper_wrapped.expected.jl"));
    assert_eq!(wrapped.code, expected);
    assert_eq!(
        wrapped.recoverable_error, None,
        "non-inlined Julia wrapper should not produce include-injection errors"
    );
}
