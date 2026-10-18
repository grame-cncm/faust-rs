//! Parity test: `dx7_tests.dsp` / `operator_test` — formerly a known gap, now closed.
//!
//! Previously the Rust compiler failed on `operator_test` from `dx7_tests.dsp`
//! with "undefined symbol `ba`".  Both `stdfaust.lib` and `demos.lib` define
//! `ba = library("basics.lib")` (and 18 other library aliases).  The parser
//! grouped these as pattern-match variants with arity 0 and errored.
//!
//! The gap was closed by the zero-arity last-import-wins fix (commit c5ffe67):
//! duplicate zero-arity definitions are now silently resolved to the latest
//! import, matching C++ behaviour.
//!
//! This test guards against regression of that fix.  It requires a large stack
//! because `dx7_tests.dsp` drives deep evaluation.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use compiler::Compiler;

/// The faustlibraries checkout used when `FAUST_RS_FAUSTLIBRARIES_ROOT` is
/// unset: `faustlibraries` next to the directory that holds this repository.
fn default_faustlibraries_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .map_or_else(std::path::PathBuf::new, |dir| dir.join("faustlibraries"))
}

fn cpp_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FAUST_CPP_BIN") {
        return Some(PathBuf::from(path));
    }
    let default = PathBuf::from("/usr/local/bin/faust");
    default.exists().then_some(default)
}

fn faustlibraries_root() -> Option<PathBuf> {
    std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let default = default_faustlibraries_root();
            default.exists().then_some(default)
        })
}

fn cpp_accepts_file(cpp_bin: &Path, input: &Path, import_root: &Path) -> Result<(), String> {
    let mut out_path = std::env::temp_dir();
    out_path.push(format!(
        "faust_rs_import_gap_{}_{}.cpp",
        std::process::id(),
        input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("fixture")
    ));
    let output = Command::new(cpp_bin)
        .arg("-pn")
        .arg("operator_test")
        .arg("-I")
        .arg(import_root)
        .arg(input)
        .arg("-lang")
        .arg("cpp")
        .arg("-o")
        .arg(&out_path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", cpp_bin.display()))?;
    let _ = std::fs::remove_file(&out_path);
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

fn run_with_large_stack<T>(f: impl FnOnce() -> T + Send + 'static) -> T
where
    T: Send + 'static,
{
    thread::Builder::new()
        .name("dx7-import-parity".to_owned())
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .expect("spawn dx7 worker thread")
        .join()
        .expect("dx7 worker thread should not panic")
}

/// Rust now accepts `operator_test` from `dx7_tests.dsp`, matching C++.
///
/// Guards against regression of the zero-arity last-import-wins fix.
#[test]
fn operator_test_import_loading_parity_is_closed() {
    let Some(root) = faustlibraries_root() else {
        eprintln!("Skipping import/loading parity: faustlibraries root unavailable");
        return;
    };
    let Some(cpp) = cpp_bin() else {
        eprintln!("Skipping import/loading parity: Faust C++ binary unavailable");
        return;
    };

    let dsp = root.join("tests").join("dx7_tests.dsp");
    cpp_accepts_file(&cpp, &dsp, &root)
        .unwrap_or_else(|e| panic!("Faust C++ should accept operator_test: {e}"));

    run_with_large_stack(move || {
        Compiler::new()
            .with_process_name("operator_test")
            .compile_file_default_to_signals(&dsp)
            .unwrap_or_else(|e| panic!("Rust should now match C++ and accept operator_test: {e}"));
    });
}
