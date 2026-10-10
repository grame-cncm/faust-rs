//! Deeply nested programs compiled on the thread of an embedding host
//! (grame-cncm/faust-rs#16).
//!
//! The passes of the pipeline recurse on the structure of the program, one
//! native frame or more per level of nesting. A host thread of 8 MiB used to
//! abort the process (`SIGABRT`, no diagnostic) at a few thousand levels, in
//! whichever pass came first: propagation, typing, normalization, FIR
//! lowering and verification, each backend's code generation. Each stage now
//! runs on the compile stack of `tlib::stack` when its caller's stack is too
//! small, and the evaluator and the parser grow their own.

use codegen::backends::asc::AscOptions;
use codegen::backends::c::COptions;
use codegen::backends::cmajor::CmajorOptions;
use codegen::backends::codebox::CodeboxOptions;
use codegen::backends::cpp::CppOptions;
use codegen::backends::cranelift::CraneliftOptions;
use codegen::backends::interp::InterpOptions;
use codegen::backends::julia::JuliaOptions;
use codegen::backends::rust::RustOptions;
use codegen::backends::wasm::WasmOptions;
use compiler::Compiler;

/// The stack of a typical host thread (the main thread on Linux and macOS).
const HOST_STACK: usize = 8 * 1024 * 1024;

fn on_host_thread(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(HOST_STACK)
        .spawn(f)
        .expect("spawn host thread")
        .join()
        .expect("the compilation should finish, not abort");
}

/// A chain of nonlinear functions, which no pass simplifies away: the
/// program stays as deep from evaluation to code generation. A thousand
/// levels stay under the debug build's structural budget (1 024); without
/// the compile stack, this test aborts on its 8 MiB thread.
#[test]
fn a_deep_expression_compiles_on_a_host_thread_with_every_backend() {
    let source = format!("process = _{};", " : sin".repeat(1_000));
    on_host_thread("deep-expression-backends", move || {
        let c = Compiler::new();
        let name = "deep_sin.dsp";
        let expect = |backend: &str, result: Result<(), compiler::CompilerError>| {
            if let Err(e) = result {
                panic!("{backend}: {e}");
            }
        };
        expect(
            "cpp",
            c.compile_source_to_cpp(name, &source, &CppOptions::default())
                .map(drop),
        );
        expect(
            "c",
            c.compile_source_to_c(name, &source, &COptions::default())
                .map(drop),
        );
        expect(
            "rust",
            c.compile_source_to_rust(name, &source, &RustOptions::default())
                .map(drop),
        );
        expect(
            "julia",
            c.compile_source_to_julia(name, &source, &JuliaOptions::default())
                .map(drop),
        );
        expect(
            "asc",
            c.compile_source_to_asc(name, &source, &AscOptions::default())
                .map(drop),
        );
        expect(
            "codebox",
            c.compile_source_to_codebox(name, &source, &CodeboxOptions::default())
                .map(drop),
        );
        expect(
            "cmajor",
            c.compile_source_to_cmajor(name, &source, &CmajorOptions::default())
                .map(drop),
        );
        expect(
            "interp",
            c.compile_source_to_interp(name, &source, &InterpOptions::default())
                .map(drop),
        );
        expect(
            "cranelift",
            c.compile_source_to_cranelift_report(name, &source, &CraneliftOptions::default())
                .map(drop),
        );
        expect(
            "wasm",
            c.compile_source_to_wasm(name, &source, &WasmOptions::default())
                .map(drop),
        );
    });
}

/// A chain of additions, which the evaluator folds into one number: it pushes
/// no `call_stack` frame, so the evaluator's depth budget never saw it, and
/// it overflowed the native stack of whatever thread ran the compiler. The
/// evaluator recurses on stack segments it grows on demand (it handles this
/// chain alone on 1 MiB, see the `eval` crate's tests).
#[test]
fn deep_acyclic_expression_compiles_on_a_host_thread() {
    let source = format!("process = {};", vec!["1"; 5_000].join("+"));
    on_host_thread("deep-expression-host-stack", move || {
        Compiler::new()
            .compile_source_to_signals("deep_expression.dsp", &source)
            .expect("a 5 000-deep acyclic expression compiles on a grown stack");
    });
}
