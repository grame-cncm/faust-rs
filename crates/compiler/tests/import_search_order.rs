//! The order in which an import is looked up is the C++ compiler's
//! (`fopenSearch` in `compiler/parser/enrobage.cpp`, `gImportDirList` in
//! `compiler/global.cpp`):
//!
//! 1. the name relative to the process's working directory, before any
//!    search path, `-I` included;
//! 2. the `-I` directories, the last one given first (each is inserted at
//!    the front of `gImportDirList`);
//! 3. `FAUST_LIB_PATH` and the installed libraries;
//! 4. the main file's directory, last.
//!
//! The case that revealed it: `faust-rs -pn pink_trombone_demo_test
//! tests/demos_tests.dsp`, run from a faustlibraries checkout, loaded the
//! installed `demos.lib` (without `pink_trombone_demo`) where the C++
//! compiler loads the checkout's. Each rule is checked with `import("x.lib")`
//! (resolved by the parser) and `library("x.lib")` (resolved at evaluation),
//! which share one candidate list (`parser::import_candidates`). Every
//! expected value was checked against the C++ compiler 2.89.2.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const PROGRAMS: [&str; 2] = ["with_library.dsp", "with_import.dsp"];

/// A scratch tree whose `x.lib` files each define a different `f`: 0.125 in
/// the root, 0.375 in `a/`, 0.625 in `b/`, 0.875 next to the programs of
/// `beside/`; the programs of `programs/` have no `x.lib` beside them.
fn scratch_tree() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "faust_rs_import_order_{}_{nanos}",
        std::process::id()
    ));
    for (dir, value) in [
        (root.clone(), "0.125"),
        (root.join("a"), "0.375"),
        (root.join("b"), "0.625"),
        (root.join("beside"), "0.875"),
    ] {
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("x.lib"), format!("f = {value};\n")).expect("x.lib");
    }
    std::fs::create_dir_all(root.join("programs")).expect("programs");
    for dir in ["programs", "beside"] {
        std::fs::write(
            root.join(dir).join("with_library.dsp"),
            "process = library(\"x.lib\").f;\n",
        )
        .expect("with_library.dsp");
        std::fs::write(
            root.join(dir).join("with_import.dsp"),
            "import(\"x.lib\");\nprocess = f;\n",
        )
        .expect("with_import.dsp");
    }
    root
}

/// The value of `f` in the C++ code `faust-rs` prints for `program`, run from
/// `cwd` with `includes` as `-I` and `FAUST_LIB_PATH` set to `lib_path`.
fn imported_value(
    cwd: &Path,
    program: &Path,
    includes: &[&Path],
    lib_path: Option<&Path>,
) -> &'static str {
    let mut command = Command::new(env!("CARGO_BIN_EXE_faust-rs"));
    command.current_dir(cwd);
    match lib_path {
        Some(dir) => command.env("FAUST_LIB_PATH", dir),
        None => command.env_remove("FAUST_LIB_PATH"),
    };
    for dir in includes {
        command.arg("-I").arg(dir);
    }
    let output = command.arg(program).output().expect("run faust-rs");
    assert!(
        output.status.success(),
        "{} failed: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let code = String::from_utf8(output.stdout).expect("utf-8");
    let found: Vec<&str> = ["0.125", "0.375", "0.625", "0.875"]
        .into_iter()
        .filter(|value| code.contains(value))
        .collect();
    assert_eq!(found.len(), 1, "{}: {found:?}", program.display());
    found[0]
}

#[test]
fn imports_are_searched_in_the_cpp_order() {
    let root = scratch_tree();
    let (a, b) = (root.join("a"), root.join("b"));
    let programs = root.join("programs");
    for program in PROGRAMS {
        let from_programs = programs.join(program);
        // 1. the working directory first, whatever the -I
        assert_eq!(
            imported_value(&root, &from_programs, &[], None),
            "0.125",
            "{program}"
        );
        assert_eq!(
            imported_value(&root, &from_programs, &[&a], None),
            "0.125",
            "{program}"
        );
        // 2. the last -I first
        assert_eq!(
            imported_value(&programs, &from_programs, &[&a], None),
            "0.375",
            "{program}"
        );
        assert_eq!(
            imported_value(&programs, &from_programs, &[&a, &b], None),
            "0.625",
            "{program}"
        );
        // 3. then 4. FAUST_LIB_PATH before the main file's directory
        let from_beside = root.join("beside").join(program);
        assert_eq!(
            imported_value(&programs, &from_beside, &[], Some(&a)),
            "0.375",
            "{program}"
        );
        // the main file's directory is still searched
        assert_eq!(
            imported_value(&programs, &from_beside, &[], None),
            "0.875",
            "{program}"
        );
    }
    std::fs::remove_dir_all(&root).ok();
}
