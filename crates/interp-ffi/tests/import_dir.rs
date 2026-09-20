//! `createCInterpreterDSPFactoryFromFile` with `-I DIR`: the dir overrides a
//! standard library, as with the C++ compiler. The file constructor used to
//! append the `-I` dirs after the installed libraries, which made them dead
//! for every standard library name.

use std::ffi::{CStr, CString, c_char};

use faust_interp::factory::{createCInterpreterDSPFactoryFromFile, deleteCInterpreterDSPFactory};

/// Compiles `file` with `args`; returns whether it succeeded and `error_msg`.
fn compile_file(file: &str, args: &[&str]) -> (bool, String) {
    let filename = CString::new(file).unwrap();
    let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
    let argv: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
    let mut buffer = [0 as c_char; 4096];
    let factory = unsafe {
        createCInterpreterDSPFactoryFromFile(
            filename.as_ptr(),
            argv.len() as i32,
            argv.as_ptr(),
            buffer.as_mut_ptr(),
        )
    };
    let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if factory.is_null() {
        (false, message)
    } else {
        unsafe { deleteCInterpreterDSPFactory(factory) };
        (true, message)
    }
}

#[test]
fn an_import_dir_overrides_an_installed_standard_library() {
    let dir = std::env::temp_dir().join(format!("interp_import_dir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("mutated")).unwrap();
    std::fs::create_dir_all(dir.join("program")).unwrap();
    std::fs::write(
        dir.join("mutated/analyzers.lib"),
        "declare name \"analyzers.lib\";\nprobe_marker = 42.0;\n",
    )
    .unwrap();
    let program = dir.join("program/marker.dsp");
    std::fs::write(
        &program,
        "an = library(\"analyzers.lib\");\nprocess = an.probe_marker;\n",
    )
    .unwrap();
    let program = program.to_string_lossy().into_owned();
    let lib_dir = dir.join("mutated").to_string_lossy().into_owned();

    let (ok, message) = compile_file(&program, &["-I", &lib_dir]);
    assert!(
        ok,
        "the mutated library was not found through -I: {message}"
    );

    // The control: the installed library does not define the symbol.
    let (ok, message) = compile_file(&program, &[]);
    assert!(!ok, "the program compiled without the mutated library");
    assert!(
        message.contains("parse failed") || message.contains("probe_marker"),
        "{message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
