use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};

use cranelift_ffi::factory::{
    createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory, freeCMemory,
    getCCraneliftDSPFactoryJSON,
};

fn cstr_opt(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn main() {
    let corpus_dir = Path::new("tests/corpus");
    let mut files: Vec<PathBuf> = fs::read_dir(corpus_dir)
        .expect("read corpus dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("dsp"))
        .collect();
    files.sort();

    let mut ok_lowered = Vec::new();
    let mut ok_stub = Vec::new();
    let mut errors = Vec::new();

    for path in files {
        let c_path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let mut err = [0 as c_char; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                c_path.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };

        if factory.is_null() {
            let msg = cstr_opt(err.as_ptr()).unwrap_or_else(|| "<no error msg>".to_string());
            errors.push((path, msg));
            continue;
        }

        let json_ptr = unsafe { getCCraneliftDSPFactoryJSON(factory) };
        let json = cstr_opt(json_ptr).unwrap_or_default();
        if json.contains("\"compute_body_lowered\":true") {
            ok_lowered.push(path);
        } else {
            ok_stub.push(path);
        }

        unsafe {
            if !json_ptr.is_null() {
                freeCMemory(json_ptr.cast());
            }
            let _ = deleteCCraneliftDSPFactory(factory);
        }
    }

    println!("Cranelift corpus scan over tests/corpus (*.dsp)");
    println!(
        "lowered_ok={} stub_ok={} errors={}",
        ok_lowered.len(),
        ok_stub.len(),
        errors.len()
    );

    if !ok_stub.is_empty() {
        println!("\nStub fallback examples (up to 15):");
        for p in ok_stub.iter().take(15) {
            println!("  {}", p.display());
        }
    }

    if !errors.is_empty() {
        println!("\nErrors (up to 20):");
        for (p, e) in errors.iter().take(20) {
            println!("  {} => {}", p.display(), e.lines().next().unwrap_or(""));
        }
    }

    if !ok_lowered.is_empty() {
        println!("\nLowered examples (up to 15):");
        for p in ok_lowered.iter().take(15) {
            println!("  {}", p.display());
        }
    }
}
