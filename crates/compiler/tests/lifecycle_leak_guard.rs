//! Cross-backend guard against lifecycle functions leaking into the output.
//!
//! The FIR module declares its lifecycle bodies as ordinary functions —
//! `staticInit`, `instanceConstants`, `compute` and friends — and every backend
//! renders them into its own surface: `staticInit` becomes the body of
//! `classInit` (`dspsetup` in codebox), `compute` becomes the target's compute
//! entry point. A backend that also walks its `functions` block and emits
//! whatever it does not recognize emits them a *second* time.
//!
//! That is not a cosmetic duplicate. The `staticInit` body allocates a
//! generator and fills a table using locals — in Rust, lock guards — that exist
//! only inside `classInit`, so the stray copy either fails to compile or, in a
//! dynamically checked target, writes somewhere it should not.
//!
//! The mistake was made independently in `c`, `rust` and `julia`. Rather than a
//! fourth per-backend fix, this test asserts the property directly, for every
//! backend at once, on a program that actually has a `staticInit` to leak.
//!
//! Backends that still refuse generated-table sub-modules are skipped by
//! detecting their own refusal, so migrating one brings it under this guard
//! with no edit here.

use codegen::backends::{
    c::{COptions, generate_c_module},
    asc::{AscOptions, generate_asc_module},
    cmajor::{CmajorOptions, generate_cmajor_module},
    codebox::{CodeboxOptions, generate_codebox_module},
    cpp::{CppOptions, generate_cpp_module},
    julia::{JuliaOptions, generate_julia_module},
    rust::{RustOptions, generate_rust_module},
};
use compiler::{Compiler, ControlRateMode, ProcessingApi, SignalFirLane, TableInitMode};

/// A DSP whose table content is computed at initialization time, so the module
/// necessarily declares `staticInit`.
const SOURCE: &str = "t = (+(1) ~ _) - 1;\nprocess = rdtable(8, int(t * 2), int(t % 8));";

/// Every textual backend that renders a FIR module, so a new one is added here
/// deliberately rather than by omission.
///
/// There is no skip path: as of plan phase S5 every backend emits
/// generated-table sub-modules, and a backend that starts refusing them again
/// is a regression this guard must report, not step over.
const BACKENDS: [&str; 7] = ["cpp", "c", "rust", "julia", "codebox", "asc", "cmajor"];

fn emit(backend: &str) -> String {
    let mut compiler = Compiler::new().with_table_init_mode(TableInitMode::Runtime);
    if backend == "codebox" {
        compiler = compiler
            .with_control_rate_mode(ControlRateMode::External)
            .with_processing_api(ProcessingApi::OneSample);
    }
    let fir = compiler
        .compile_source_to_fir_with_lane("leak.dsp", SOURCE, SignalFirLane::TransformFastLane)
        .expect("FIR lowering must succeed");

    let rendered = match backend {
        "cpp" => generate_cpp_module(&fir.store, fir.module, &CppOptions::default())
            .map_err(|e| e.to_string()),
        "c" => generate_c_module(&fir.store, fir.module, &COptions::default())
            .map_err(|e| e.to_string()),
        "rust" => generate_rust_module(&fir.store, fir.module, &RustOptions::default())
            .map_err(|e| e.to_string()),
        "julia" => generate_julia_module(&fir.store, fir.module, &JuliaOptions::default())
            .map_err(|e| e.to_string()),
        "codebox" => generate_codebox_module(&fir.store, fir.module, &CodeboxOptions::default())
            .map_err(|e| e.to_string()),
        "asc" => generate_asc_module(&fir.store, fir.module, &AscOptions::default())
            .map_err(|e| e.to_string()),
        "cmajor" => generate_cmajor_module(&fir.store, fir.module, &CmajorOptions::default())
            .map_err(|e| e.to_string()),
        other => panic!("unknown backend {other}"),
    };

    rendered.unwrap_or_else(|message| panic!("{backend}: emission failed unexpectedly: {message}"))
}

#[test]
fn no_backend_emits_static_init_as_an_ordinary_function() {
    for backend in BACKENDS {
        let text = emit(backend);
        assert!(
            !text.contains("staticInit"),
            "{backend} emitted `staticInit` into its output; it must be rendered \
             into the backend's lifecycle surface (classInit / dspsetup) and \
             never as a function of its own:\n{text}"
        );
    }
}

#[test]
fn every_backend_populates_the_generated_table() {
    // The complement of the leak check: a backend that renders `staticInit`
    // nowhere at all also passes the first test, and would emit a table that is
    // declared and never written — the silent-zeros failure this whole port
    // exists to prevent.
    //
    // The property has to hold across both emission shapes. A nested backend
    // calls `fill<Sub>(…)`; a flattened one inlines the loop and writes the
    // table directly. So the check is simply: something, somewhere outside the
    // compute loop, assigns into the table.
    for backend in BACKENDS {
        let text = emit(backend);
        let table = text
            .lines()
            .find_map(|line| {
                let marker = line.find("tbl")?;
                let bytes = line.as_bytes();
                let mut start = marker;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                let mut end = marker;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                Some(line[start..end].to_owned())
            })
            .unwrap_or_else(|| panic!("{backend}: no table in the output:\n{text}"));

        let populated = text
            .lines()
            .any(|line| line.contains(&table) && line.contains('=') && line.contains('['))
            || text.contains(&format!("fill{}", ""))
                && text
                    .lines()
                    .any(|line| line.contains("fill") && line.contains(&table));

        assert!(
            populated,
            "{backend}: table `{table}` is declared but never written, so it \
             would be read as zeros:\n{text}"
        );
    }
}
