//! Process-level orchestration for the `faust-rs` CLI.
//!
//! This module contains the code that turns parsed [`CliArgs`] into compiler
//! operations: parse/dump modes, backend dispatch, FIR fixture handling,
//! architecture wrapping, companion JSON generation, binary/text output, and
//! process exit behavior.  It deliberately remains a binary-facing layer; the
//! reusable compilation API stays in the `compiler` library crate.

use clap::Parser;
use std::path::{Path, PathBuf};

use boxes::dump_box;
use codegen::backends::c::COptions;
use codegen::backends::c::generate_c_module;
use codegen::backends::cpp::CppOptions;
use codegen::backends::cpp::generate_cpp_module;
use codegen::backends::cranelift::{
    CraneliftOptions, StructFieldKind, diagnose_cranelift_compute_subset_gap,
    generate_cranelift_module,
};
use codegen::backends::interp::{
    FbcCppOptions, InterpOptions, generate_cpp_from_fbc, generate_interp_module, read_fbc,
    write_fbc,
};
use codegen::backends::asc::{AscOptions, generate_asc_module};
use codegen::backends::julia::{JuliaOptions, JuliaRealType, generate_julia_module};
use codegen::backends::wasm::{WasmOptions, generate_wasm_module};
use codegen::fixtures::backend_test_fixtures;
use compiler::{
    Compiler, FirVerifyOptions, RealType, compile_options_json_string,
    enrobage::{EnrobageOptions, wrap_cpp_with_architecture},
    golden_snapshot_from_file,
};
use fir::{checker::verify_fir_module, dump_fir};
use signals::dump_sig_readable;

use super::args::{CliArgs, CliLang, CliSignalFirLane, normalize_legacy_args};
use super::diagnostics::print_structured_diagnostics;
use super::timer::CompilationTimer;

/// Prints top-level usage and exits the process.
pub fn print_global_usage_and_exit() -> ! {
    eprintln!("Usage:");
    eprintln!(
        "  cargo run -p compiler -- -lang asc|c|cpp|fir|julia|wast <input.dsp> [-o <file>] [-I <dir> ...] [--class-name <name>] [--super-class-name <name>] [--signal-fir-lane fast] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    eprintln!("                           [--no-fir-verify] [--fir-verify-strict]");
    eprintln!("  cargo run -p compiler -- --golden <input.dsp>");
    eprintln!(
        "  cargo run -p compiler -- --parse <input.dsp> [-I <dir> ...] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-box <input.dsp> [-o <file>] [-I <dir> ...] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-sig <input.dsp> [-o <file>] [-I <dir> ...] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-fir <input.dsp> [-o <file>] [-I <dir> ...] [--signal-fir-lane fast] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    eprintln!(
        "  cargo run -p compiler -- --json <input.dsp> [-o <file>] [-I <dir> ...] [--signal-fir-lane fast]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-fir-verify <input.dsp> [-o <file>] [-I <dir> ...] [--signal-fir-lane fast] [--fir-verify-strict]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-cpp <input.dsp> [-o <file>] [-I <dir> ...] [--class-name <name>] [--super-class-name <name>] [--signal-fir-lane fast] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-cpp-from-fbc <input.fbc> [-o <file>] [--cpp-class-name <name>]"
    );
    eprintln!(
        "  cargo run -p compiler -- --dump-c <input.dsp> [-o <file>] [-I <dir> ...] [--class-name <name>] [--signal-fir-lane fast] [--error-format human|json] [--error-verbosity standard|debug]"
    );
    std::process::exit(2);
}

/// Emits the on-demand error-format help footer.
pub fn maybe_print_error_format_help(enabled: bool) {
    if enabled {
        println!("--error-format human|json");
        println!("--error-verbosity standard|debug");
        println!("  human: file:line:col severity [CODE] message");
        println!("  json: structured diagnostics payload for CI/IDE tooling");
        println!("  standard: concise human notes, hides internal ids");
        println!("  debug: keeps full internal notes in human mode");
        std::process::exit(0);
    }
}

/// Renders the `-v` / `--version` output.
pub fn render_version_text() -> String {
    format!(
        "faust-rs {}\nCopyright (C) 2002-2026, GRAME - Centre National de Creation Musicale. All rights reserved.",
        Compiler::version()
    )
}

/// Writes generated output either to stdout or to the requested file.
pub fn emit_output(content: &str, output: Option<&PathBuf>) {
    if let Some(path) = output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            eprintln!(
                "Failed to create output directory {}: {err}",
                parent.display()
            );
            std::process::exit(1);
        }
        if let Err(err) = std::fs::write(path, content) {
            eprintln!("Failed to write output file {}: {err}", path.display());
            std::process::exit(1);
        }
    } else {
        print!("{content}");
    }
}

/// Writes generated binary output either to stdout or to the requested file.
pub fn emit_binary_output(content: &[u8], output: Option<&PathBuf>) {
    if let Some(path) = output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            eprintln!(
                "Failed to create output directory {}: {err}",
                parent.display()
            );
            std::process::exit(1);
        }
        if let Err(err) = std::fs::write(path, content) {
            eprintln!("Failed to write output file {}: {err}", path.display());
            std::process::exit(1);
        }
    } else if let Err(err) = std::io::Write::write_all(&mut std::io::stdout(), content) {
        eprintln!("Failed to write binary output to stdout: {err}");
        std::process::exit(1);
    }
}

/// Writes a WASM binary and, when writing to a file path, the companion JSON
/// metadata file next to it using the same stem and a `.json` extension.
pub fn emit_wasm_output(wasm_binary: &[u8], dsp_json: &str, output: Option<&PathBuf>) {
    if let Some(path) = output {
        emit_binary_output(wasm_binary, Some(path));
        let json_path = path.with_extension("json");
        emit_output(dsp_json, Some(&json_path));
    } else {
        emit_binary_output(wasm_binary, None);
    }
}

/// Disassembles a WASM binary into its textual WAST form for `-lang wast`.
///
/// Exits the process with status 1 if the binary cannot be printed.
pub fn render_wast_output(wasm_binary: &[u8]) -> String {
    match wasmprinter::print_bytes(wasm_binary) {
        Ok(wast) => wast,
        Err(err) => {
            eprintln!("Failed to render WAST text from generated WASM: {err}");
            std::process::exit(1);
        }
    }
}

/// Writes a JSON companion file next to an existing backend output file using
/// the same stem and a `.json` extension.
pub fn emit_json_companion_output(json_text: &str, output: &Path) {
    let json_path = output.with_extension("json");
    emit_output(json_text, Some(&json_path));
}

/// Maps a [`CliLang`] back to its canonical `-lang` token for diagnostics.
pub fn cli_lang_name(lang: CliLang) -> &'static str {
    match lang {
        CliLang::C => "c",
        CliLang::Cpp => "cpp",
        CliLang::Fir => "fir",
        CliLang::Interp => "interp",
        CliLang::Cranelift => "cranelift",
        CliLang::Asc => "asc",
        CliLang::Julia => "julia",
        CliLang::Wasm => "wasm",
        CliLang::Wast => "wast",
    }
}

/// Returns the `-o` output path, required when `--json` accompanies `-lang` so
/// the companion JSON has a destination.
///
/// Exits with status 2 if no output path was given.
pub fn require_companion_output_path(cli: &CliArgs) -> &PathBuf {
    cli.output.as_ref().unwrap_or_else(|| {
        eprintln!("--json used with -lang requires -o <file> so the companion JSON has a path");
        std::process::exit(2);
    })
}

/// Wraps generated backend code in a user-supplied architecture file.
///
/// Returns `generated` unchanged when no `-a <file>` was given. Otherwise builds
/// [`EnrobageOptions`] from the CLI (architecture dirs, inline flag, class /
/// super-class names) and applies the wrapper, exiting with status 1 on a
/// wrapping failure or recoverable error.
pub fn wrap_backend_with_architecture(generated: &str, cli: &CliArgs) -> String {
    let Some(architecture_file) = cli.architecture.as_ref() else {
        return generated.to_owned();
    };

    let mut options = EnrobageOptions::new(architecture_file.clone());
    options.architecture_dirs = cli.architecture_dir.clone();
    options.inline_arch_files = cli.inline_architecture_files;
    if let Some(class_name) = selected_class_name(cli) {
        options.class_name = class_name;
    }
    if let Some(super_class_name) = selected_super_class_name(cli) {
        options.super_class_name = super_class_name;
    }
    let wrapped = match wrap_cpp_with_architecture(generated, &options) {
        Ok(wrapped) => wrapped,
        Err(err) => {
            eprintln!("Architecture wrapping failed: {err}");
            std::process::exit(1);
        }
    };
    if let Some(err) = wrapped.recoverable_error.as_deref() {
        eprintln!("{err}");
        std::process::exit(1);
    }
    wrapped.code
}

/// Renders a short Cranelift backend status report for the CLI.
pub fn render_cranelift_report(
    compiled: &codegen::backends::cranelift::JitDspModule,
    subset_gap: Option<&str>,
) -> String {
    let layout = compiled.struct_layout();
    let mut out = String::new();
    out.push_str("backend: cranelift (experimental)\n");
    out.push_str(&format!("module: {}\n", compiled.module_name()));
    out.push_str(&format!(
        "compute_symbol: {}\n",
        compiled.compute_symbol_name()
    ));
    out.push_str(&format!(
        "compute_entry_addr: 0x{:x}\n",
        compiled.compute_entry_addr()
    ));
    out.push_str(&format!(
        "compute_body_lowered: {}\n",
        compiled.compute_body_lowered()
    ));
    if let Some(reason) = subset_gap {
        out.push_str(&format!("subset_gap: {reason}\n"));
    }
    out.push_str(&format!(
        "dsp_struct_layout: size={} align={} fields={}\n",
        layout.size_bytes(),
        layout.align_bytes(),
        layout.fields().len()
    ));
    for field in layout.fields() {
        let kind = match &field.kind {
            StructFieldKind::Scalar(typ) => format!("scalar:{typ:?}"),
            StructFieldKind::Table { elem_type, len } => {
                format!("table:{elem_type:?}[{len}]")
            }
        };
        out.push_str(&format!(
            "  - {} @{} size={} align={} {}\n",
            field.name, field.offset_bytes, field.size_bytes, field.align_bytes, kind
        ));
    }
    out
}

/// Maps CLI backend selection to the signal->FIR lane used internally.
pub fn selected_codegen_lane(cli: &CliArgs) -> CliSignalFirLane {
    cli.signal_fir_lane.unwrap_or(CliSignalFirLane::Fast)
}

/// Maps CLI switches to FIR verifier behavior.
pub fn selected_fir_verify_options(cli: &CliArgs) -> FirVerifyOptions {
    FirVerifyOptions {
        enabled: !cli.no_fir_verify,
        strict: cli.fir_verify_strict,
    }
}

/// Maps CLI precision switches to the internal DSP real type.
pub fn selected_real_type(cli: &CliArgs) -> RealType {
    if cli.double {
        RealType::Float64
    } else {
        RealType::Float32
    }
}

/// Maps CLI precision switches to the Julia backend's real type, mirroring
/// [`selected_real_type`] for the Julia code generator.
pub fn selected_julia_real_type(cli: &CliArgs) -> JuliaRealType {
    if cli.double {
        JuliaRealType::Float64
    } else {
        JuliaRealType::Float32
    }
}

/// Builds one configured [`Compiler`] instance from parsed CLI arguments.
pub fn compiler_from_cli(
    cli: &CliArgs,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Compiler {
    let mut compiler = Compiler::new()
        .with_fir_verify_options(selected_fir_verify_options(cli))
        .with_process_name(cli.process_name.clone())
        .with_real_type(selected_real_type(cli))
        .with_mcd(cli.mcd)
        .with_dlt(cli.dlt);
    if let Some(flag) = cancel {
        compiler = compiler.with_cancel(flag);
    }
    if cli.compilation_time {
        compiler = compiler.with_timing_sink(|name, duration| {
            eprintln!("end {name} (duration : {:.6})", duration.as_secs_f64());
        });
    }
    compiler
}

/// Returns the configured DSP class name, or `None` when the flag was not set
/// or was set to an empty string.
pub fn selected_class_name(cli: &CliArgs) -> Option<String> {
    cli.class_name
        .as_ref()
        .filter(|name| !name.is_empty())
        .cloned()
}

/// Returns the configured DSP superclass name, or `None` when the flag was not
/// set or was set to an empty string.
pub fn selected_super_class_name(cli: &CliArgs) -> Option<String> {
    cli.super_class_name
        .as_ref()
        .filter(|name| !name.is_empty())
        .cloned()
}

/// Renders the list of built-in FIR backend fixtures for `--fir-fixture`.
pub fn render_fir_fixture_list() -> String {
    let mut out = String::from("Built-in FIR fixtures:\n");
    for (name, _) in backend_test_fixtures() {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    out
}

/// Looks up one named FIR backend fixture builder.
pub fn find_fir_fixture(name: &str) -> Option<codegen::fixtures::FirFixtureBuilder> {
    backend_test_fixtures()
        .iter()
        .find_map(|(n, build)| (*n == name).then_some(*build))
}

/// Compiles a named FIR fixture through the interpreter backend and renders summary text.
pub fn compile_fixture_to_interp_text(
    store: &fir::FirStore,
    module: fir::FirId,
    options: &InterpOptions,
) -> Result<String, String> {
    let factory =
        generate_interp_module::<f32>(store, module, options).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    write_fbc(&factory, &mut buf, false).map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

/// Compiles a named FIR fixture to strict C++-style JSON text.
pub fn compile_fixture_to_json_text(
    store: &fir::FirStore,
    module: fir::FirId,
    compile_options: String,
    double_precision: bool,
) -> Result<String, String> {
    let fir::FirMatch::Module {
        name,
        functions,
        num_inputs,
        num_outputs,
        ..
    } = fir::match_fir(store, module)
    else {
        return Err("JSON fixture generation expects a FIR Module root".to_owned());
    };
    let fir::FirMatch::Block(function_items) = fir::match_fir(store, functions) else {
        return Err("JSON fixture generation expects a FIR function block".to_owned());
    };
    let layout = codegen::backends::wasm::layout::WasmMemoryLayout::from_module(
        store,
        module,
        &WasmOptions {
            double_precision,
            ..WasmOptions::default()
        },
        0,
    )
    .map_err(|e| e.to_string())?;
    let json = codegen::json::build_json_description_from_fir(
        store,
        &function_items,
        codegen::json::JsonBuildOptions {
            name,
            filename: None,
            version: Some(Compiler::version().to_owned()),
            compile_options: Some(compile_options),
            library_list: Vec::new(),
            include_pathnames: Vec::new(),
            top_level_meta: Vec::new(),
            size: Some(layout.struct_size),
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: None,
        },
        |_var| None,
    )
    .map_err(|e| e.to_string())?;
    Ok(json.render())
}

/// Compiles `input_path` to a JSON description and writes it as a companion file
/// alongside a textual backend's output (when `--json` is combined with `-lang`).
///
/// Tags the JSON `compile_options` with `backend_lang`, picks the import-aware or
/// default pipeline depending on `-I` flags, and exits with status 1 (after
/// printing structured diagnostics) on failure.
pub fn emit_cli_json_companion_for_backend(
    compiler: &Compiler,
    cli: &CliArgs,
    input_path: &Path,
    backend_lang: CliLang,
) {
    let compile_options =
        compile_options_json_string(Some(cli_lang_name(backend_lang)), cli.double);
    let result = if cli.import_dir.is_empty() {
        compiler.compile_file_default_to_json_with_lane_and_compile_options(
            input_path,
            selected_codegen_lane(cli).into_compiler_lane(),
            compile_options,
        )
    } else {
        compiler.compile_file_to_json_with_compile_options(
            input_path,
            &cli.import_dir,
            selected_codegen_lane(cli).into_compiler_lane(),
            compile_options,
        )
    };

    match result {
        Ok(json) => emit_json_companion_output(&json, require_companion_output_path(cli)),
        Err(err) => {
            eprintln!("JSON companion pipeline failed: {err}");
            print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
            std::process::exit(1);
        }
    }
}

/// Renders a FIR verifier report in CLI-friendly text form.
pub fn render_fir_verify_report(store: &fir::FirStore, module: fir::FirId, strict: bool) -> String {
    let report = verify_fir_module(store, module);
    let errors = report.errors().count();
    let warnings = report.warnings().count();
    let fatal = errors > 0 || (strict && warnings > 0);
    let mut out = String::new();
    out.push_str(&format!(
        "FIR verify: errors={errors} warnings={warnings} strict={strict} status={}\n",
        if fatal { "FAIL" } else { "OK" }
    ));
    for d in &report.diagnostics {
        let sev = match d.severity {
            fir::checker::Severity::Error => "error",
            fir::checker::Severity::Warning => "warning",
        };
        out.push_str(&format!("- {sev} [{}] {}", d.code, d.message));
        if let Some(fun) = d.context.function_name.as_deref() {
            out.push_str(&format!(" (fn={fun})"));
        }
        out.push_str(&format!(" [node={}]\n", d.node.as_u32()));
    }
    out
}

/// Real CLI entry point, run on the deep-stack worker thread spawned by `main`.
///
/// Normalizes legacy argument spellings, parses [`CliArgs`], handles early-exit
/// flags (`--version`, error-format help), then sets up cooperative cancellation
/// plus the watchdog timeout and drives the requested compilation backend.
pub fn run_main() {
    let args = normalize_legacy_args(std::env::args());
    let cli = CliArgs::parse_from(args);
    if cli.version {
        println!("{}", render_version_text());
        return;
    }
    maybe_print_error_format_help(cli.help_error_format);

    // Cooperative cancellation flag + CLI watchdog.
    //
    // Two-pronged timeout approach:
    // 1. The cooperative cancel flag is checked by the evaluator on every
    //    recursive call and returns `EvalError::Cancelled`. This is safe for
    //    library (libfaust) usage because it never calls `process::exit`.
    // 2. The CLI watchdog calls `process::exit(1)` as a last resort if the
    //    cancel flag didn't abort in time (e.g. hang in propagation phase
    //    where cancel checks are not yet wired). This is CLI-only and
    //    acceptable for a standalone process.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let timeout_secs = cli.timeout;
        if timeout_secs > 0 {
            let cancel_clone = std::sync::Arc::clone(&cancel);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(timeout_secs));
                // First, try cooperative cancellation (for eval phase).
                cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                // Give the cooperative path a grace period to take effect.
                std::thread::sleep(std::time::Duration::from_secs(2));
                // If still alive, force exit (for non-eval phase hangs).
                eprintln!(
                    "ERROR: compilation timeout ({}s limit exceeded)",
                    timeout_secs,
                );
                std::process::exit(1);
            });
        }
    }

    if cli.list_fir_fixtures {
        if cli.fir_fixture.is_some() || cli.input.is_some() {
            eprintln!("--list-fir-fixtures does not accept --fir-fixture or input file");
            std::process::exit(2);
        }
        emit_output(&render_fir_fixture_list(), cli.output.as_ref());
        return;
    }

    let backend_mode_count = [
        cli.golden,
        cli.parse,
        cli.dump_box,
        cli.dump_sig,
        cli.dump_cpp,
        cli.dump_cpp_from_fbc,
        cli.dump_c,
        cli.dump_fir,
        cli.dump_fir_verify,
        cli.dump_interp,
        cli.dump_cranelift,
        cli.dump_json,
        cli.lang.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();

    let json_plus_lang_only = cli.dump_json && cli.lang.is_some() && backend_mode_count == 2;
    let mode_count = if json_plus_lang_only {
        1
    } else {
        backend_mode_count
    };

    if mode_count > 1 {
        print_global_usage_and_exit();
    }

    if mode_count == 0 && cli.input.is_none() && cli.fir_fixture.is_none() {
        println!("faust-rs compiler scaffold v{}", Compiler::version());
        return;
    }
    if mode_count == 0 {
        // Default compile mode: C++ backend, aligned with Faust CLI behavior.
    }

    if cli.fir_fixture.is_some() && cli.input.is_some() {
        eprintln!("--fir-fixture is incompatible with a DSP input file");
        std::process::exit(2);
    }
    if matches!(cli.class_name.as_deref(), Some("")) {
        eprintln!("--class-name cannot be empty");
        std::process::exit(2);
    }
    if matches!(cli.super_class_name.as_deref(), Some("")) {
        eprintln!("--super-class-name cannot be empty");
        std::process::exit(2);
    }

    if (cli.dump_box || cli.dump_sig || cli.parse || cli.golden) && cli.signal_fir_lane.is_some() {
        eprintln!(
            "--signal-fir-lane is only valid with --dump-cpp/--dump-c/--dump-fir/--dump-fir-verify/--dump-cranelift"
        );
        std::process::exit(2);
    }
    if cli.dump_cpp_from_fbc {
        if cli.signal_fir_lane.is_some()
            || !cli.import_dir.is_empty()
            || cli.architecture.is_some()
            || !cli.architecture_dir.is_empty()
            || cli.inline_architecture_files
            || cli.fir_fixture.is_some()
            || cli.super_class_name.is_some()
        {
            eprintln!(
                "--dump-cpp-from-fbc is incompatible with --signal-fir-lane/--import-dir/architecture/--fir-fixture/--super-class-name"
            );
            std::process::exit(2);
        }
        if let Some(input) = cli.input.as_ref()
            && input.extension().and_then(|e| e.to_str()) != Some("fbc")
        {
            eprintln!("--dump-cpp-from-fbc expects an input file with .fbc extension");
            std::process::exit(2);
        }
    } else if cli.cpp_class_name.is_some() {
        eprintln!("--cpp-class-name is only valid with --dump-cpp-from-fbc");
        std::process::exit(2);
    }
    if cli.super_class_name.is_some()
        && (cli.dump_c || matches!(cli.lang, Some(CliLang::C)))
        && cli.architecture.is_none()
    {
        eprintln!("--super-class-name is only meaningful for C++ output or architecture wrapping");
        std::process::exit(2);
    }
    if (cli.dump_fir
        || cli.dump_json
        || cli.dump_fir_verify
        || matches!(
            cli.lang,
            Some(
                CliLang::Fir
                    | CliLang::Interp
                    | CliLang::Cranelift
                    | CliLang::Wasm
                    | CliLang::Wast
                    | CliLang::Asc
            )
        ))
        && cli.architecture.is_some()
    {
        eprintln!("--architecture is currently supported only for C/C++/Julia output");
        std::process::exit(2);
    }
    if cli.no_fir_verify && cli.dump_fir_verify {
        eprintln!("--no-fir-verify is incompatible with --dump-fir-verify");
        std::process::exit(2);
    }
    if let Some(path) = cli.architecture_dir.iter().find(|path| path.is_file()) {
        eprintln!(
            "-A/--architecture-dir expects a directory, not a file: {}",
            path.display()
        );
        std::process::exit(2);
    }
    if cli.architecture.is_none()
        && (!cli.architecture_dir.is_empty() || cli.inline_architecture_files)
    {
        eprintln!("--architecture-dir/--inline-architecture-files require --architecture <file>");
        std::process::exit(2);
    }

    if cli.fir_fixture.is_some() {
        if cli.golden || cli.parse || cli.dump_box || cli.dump_sig {
            eprintln!(
                "--fir-fixture supports only FIR/backend dump modes (fir/c/cpp/interp/cranelift/wasm/wast/json)"
            );
            std::process::exit(2);
        }
        if cli.signal_fir_lane.is_some() {
            eprintln!("--signal-fir-lane is not applicable with --fir-fixture (already FIR)");
            std::process::exit(2);
        }
        if !cli.import_dir.is_empty() {
            eprintln!("--import-dir is not used with --fir-fixture");
            std::process::exit(2);
        }
    }

    if let Some(fixture_name) = cli.fir_fixture.as_deref() {
        let Some(build_fixture) = find_fir_fixture(fixture_name) else {
            eprintln!("Unknown FIR fixture: {fixture_name}");
            eprintln!("{}", render_fir_fixture_list());
            std::process::exit(2);
        };
        let (store, module) = build_fixture();

        if cli.dump_fir_verify {
            let rendered = render_fir_verify_report(&store, module, cli.fir_verify_strict);
            let report = verify_fir_module(&store, module);
            let fatal = report.has_errors()
                || (cli.fir_verify_strict && report.warnings().next().is_some());
            emit_output(&rendered, cli.output.as_ref());
            if fatal {
                std::process::exit(1);
            }
            return;
        }

        if cli.dump_fir || matches!(cli.lang, Some(CliLang::Fir)) {
            let mut rendered = dump_fir(&store, module);
            if !rendered.ends_with('\n') {
                rendered.push('\n');
            }
            emit_output(&rendered, cli.output.as_ref());
            if cli.dump_json {
                let output = require_companion_output_path(&cli);
                let compile_options = compile_options_json_string(Some("fir"), cli.double);
                match compile_fixture_to_json_text(&store, module, compile_options, cli.double) {
                    Ok(json) => emit_json_companion_output(&json, output),
                    Err(err) => {
                        eprintln!("JSON fixture generation failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
            return;
        }

        if cli.dump_interp || matches!(cli.lang, Some(CliLang::Interp)) {
            match compile_fixture_to_interp_text(&store, module, &InterpOptions::default()) {
                Ok(fbc_text) => {
                    emit_output(&fbc_text, cli.output.as_ref());
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        let compile_options =
                            compile_options_json_string(Some("interp"), cli.double);
                        match compile_fixture_to_json_text(
                            &store,
                            module,
                            compile_options,
                            cli.double,
                        ) {
                            Ok(json) => emit_json_companion_output(&json, output),
                            Err(err) => {
                                eprintln!("JSON fixture generation failed: {err}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Interp fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if cli.dump_cranelift || matches!(cli.lang, Some(CliLang::Cranelift)) {
            let subset_gap = diagnose_cranelift_compute_subset_gap(&store, module)
                .map_err(|err| err.to_string());
            let compiled =
                match generate_cranelift_module(&store, module, &CraneliftOptions::default()) {
                    Ok(compiled) => compiled,
                    Err(err) => {
                        eprintln!("Cranelift fixture codegen failed: {err}");
                        std::process::exit(1);
                    }
                };
            let rendered = render_cranelift_report(&compiled, subset_gap.ok().flatten().as_deref());
            emit_output(&rendered, cli.output.as_ref());
            if cli.dump_json {
                let output = require_companion_output_path(&cli);
                let compile_options = compile_options_json_string(Some("cranelift"), cli.double);
                match compile_fixture_to_json_text(&store, module, compile_options, cli.double) {
                    Ok(json) => emit_json_companion_output(&json, output),
                    Err(err) => {
                        eprintln!("JSON fixture generation failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
            return;
        }

        if matches!(cli.lang, Some(CliLang::Wasm)) {
            match generate_wasm_module(
                &store,
                module,
                &WasmOptions {
                    double_precision: cli.double,
                    ..WasmOptions::default()
                },
            ) {
                Ok(wasm) => {
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        emit_wasm_output(&wasm.wasm_binary, &wasm.dsp_json, Some(output));
                    } else {
                        emit_binary_output(&wasm.wasm_binary, cli.output.as_ref());
                    }
                }
                Err(err) => {
                    eprintln!("WASM fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if matches!(cli.lang, Some(CliLang::Wast)) {
            match generate_wasm_module(
                &store,
                module,
                &WasmOptions {
                    double_precision: cli.double,
                    ..WasmOptions::default()
                },
            ) {
                Ok(wasm) => {
                    let wast = render_wast_output(&wasm.wasm_binary);
                    emit_output(&wast, cli.output.as_ref());
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        let compile_options = compile_options_json_string(Some("wast"), cli.double);
                        match compile_fixture_to_json_text(
                            &store,
                            module,
                            compile_options,
                            cli.double,
                        ) {
                            Ok(json) => emit_json_companion_output(&json, output),
                            Err(err) => {
                                eprintln!("JSON fixture generation failed: {err}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("WAST fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if matches!(cli.lang, Some(CliLang::Asc)) {
            let options = AscOptions {
                class_name: selected_class_name(&cli),
                ..AscOptions::default()
            };
            match generate_asc_module(&store, module, &options) {
                Ok(asc) => {
                    emit_output(&asc, cli.output.as_ref());
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        let compile_options = compile_options_json_string(Some("asc"), cli.double);
                        match compile_fixture_to_json_text(
                            &store,
                            module,
                            compile_options,
                            cli.double,
                        ) {
                            Ok(json) => emit_json_companion_output(&json, output),
                            Err(err) => {
                                eprintln!("JSON fixture generation failed: {err}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("AssemblyScript fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if matches!(cli.lang, Some(CliLang::Julia)) {
            let options = JuliaOptions {
                class_name: selected_class_name(&cli),
                real_type: selected_julia_real_type(&cli),
            };
            match generate_julia_module(&store, module, &options) {
                Ok(julia) => {
                    let rendered = wrap_backend_with_architecture(&julia, &cli);
                    emit_output(&rendered, cli.output.as_ref());
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        let compile_options =
                            compile_options_json_string(Some("julia"), cli.double);
                        match compile_fixture_to_json_text(
                            &store,
                            module,
                            compile_options,
                            cli.double,
                        ) {
                            Ok(json) => emit_json_companion_output(&json, output),
                            Err(err) => {
                                eprintln!("JSON fixture generation failed: {err}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Julia fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if cli.dump_json {
            let compile_options =
                compile_options_json_string(cli.lang.map(cli_lang_name), cli.double);
            match compile_fixture_to_json_text(&store, module, compile_options, cli.double) {
                Ok(json) => {
                    if cli.lang.is_some() {
                        let output = require_companion_output_path(&cli);
                        emit_json_companion_output(&json, output);
                    } else {
                        emit_output(&json, cli.output.as_ref());
                    }
                }
                Err(err) => {
                    eprintln!("JSON fixture generation failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if cli.dump_cpp || matches!(cli.lang, Some(CliLang::Cpp)) || mode_count == 0 {
            let options = CppOptions {
                class_name: selected_class_name(&cli),
                super_class_name: selected_super_class_name(&cli),
                ..CppOptions::default()
            };
            match generate_cpp_module(&store, module, &options) {
                Ok(cpp) => {
                    let rendered = if let Some(architecture_file) = cli.architecture.as_ref() {
                        let mut options = EnrobageOptions::new(architecture_file.clone());
                        options.architecture_dirs = cli.architecture_dir.clone();
                        options.inline_arch_files = cli.inline_architecture_files;
                        if let Some(class_name) = selected_class_name(&cli) {
                            options.class_name = class_name;
                        }
                        if let Some(super_class_name) = selected_super_class_name(&cli) {
                            options.super_class_name = super_class_name;
                        }
                        let wrapped = match wrap_cpp_with_architecture(&cpp, &options) {
                            Ok(wrapped) => wrapped,
                            Err(err) => {
                                eprintln!("Architecture wrapping failed: {err}");
                                std::process::exit(1);
                            }
                        };
                        if let Some(err) = wrapped.recoverable_error.as_deref() {
                            eprintln!("{err}");
                            std::process::exit(1);
                        }
                        wrapped.code
                    } else {
                        cpp
                    };
                    emit_output(&rendered, cli.output.as_ref());
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        let compile_options = compile_options_json_string(Some("cpp"), cli.double);
                        match compile_fixture_to_json_text(
                            &store,
                            module,
                            compile_options,
                            cli.double,
                        ) {
                            Ok(json) => emit_json_companion_output(&json, output),
                            Err(err) => {
                                eprintln!("JSON fixture generation failed: {err}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("C++ fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if cli.dump_c || matches!(cli.lang, Some(CliLang::C)) {
            let options = COptions {
                class_name: selected_class_name(&cli),
                ..COptions::default()
            };
            match generate_c_module(&store, module, &options) {
                Ok(c_code) => {
                    let rendered = if let Some(architecture_file) = cli.architecture.as_ref() {
                        let mut options = EnrobageOptions::new(architecture_file.clone());
                        options.architecture_dirs = cli.architecture_dir.clone();
                        options.inline_arch_files = cli.inline_architecture_files;
                        if let Some(class_name) = selected_class_name(&cli) {
                            options.class_name = class_name;
                        }
                        let wrapped = match wrap_cpp_with_architecture(&c_code, &options) {
                            Ok(wrapped) => wrapped,
                            Err(err) => {
                                eprintln!("Architecture wrapping failed: {err}");
                                std::process::exit(1);
                            }
                        };
                        if let Some(err) = wrapped.recoverable_error.as_deref() {
                            eprintln!("{err}");
                            std::process::exit(1);
                        }
                        wrapped.code
                    } else {
                        c_code
                    };
                    emit_output(&rendered, cli.output.as_ref());
                    if cli.dump_json {
                        let output = require_companion_output_path(&cli);
                        let compile_options = compile_options_json_string(Some("c"), cli.double);
                        match compile_fixture_to_json_text(
                            &store,
                            module,
                            compile_options,
                            cli.double,
                        ) {
                            Ok(json) => emit_json_companion_output(&json, output),
                            Err(err) => {
                                eprintln!("JSON fixture generation failed: {err}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("C fixture codegen failed: {err}");
                    std::process::exit(1);
                }
            }
            return;
        }

        print_global_usage_and_exit();
    }

    let Some(input_path) = cli.input.as_ref() else {
        print_global_usage_and_exit();
    };

    if cli.dump_cpp_from_fbc {
        let text = match std::fs::read_to_string(input_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Cannot read .fbc file '{}': {e}", input_path.display());
                std::process::exit(1);
            }
        };
        let mut reader = std::io::BufReader::new(text.as_bytes());
        let factory = match read_fbc::<f32>(&mut reader) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to parse .fbc: {e}");
                std::process::exit(1);
            }
        };
        let opts = FbcCppOptions {
            class_name: cli.cpp_class_name.clone(),
            pragma_once: true,
            namespace: None,
        };
        match generate_cpp_from_fbc(&factory, &opts) {
            Ok(cpp) => emit_output(&cpp, cli.output.as_ref()),
            Err(e) => {
                eprintln!("Native C++ generation from FBC failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.golden {
        if !cli.import_dir.is_empty() {
            eprintln!("--import-dir is not supported with --golden");
            std::process::exit(2);
        }
        match golden_snapshot_from_file(input_path) {
            Ok(snapshot) => {
                emit_output(&snapshot, cli.output.as_ref());
            }
            Err(err) => {
                eprintln!("Failed to create golden snapshot: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.parse {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default(input_path)
        } else {
            compiler.compile_file(input_path, &cli.import_dir)
        };
        timer.phase("parse");

        match result {
            Ok(out) => {
                println!(
                    "Parsed OK: root={:?} parse_errors={} recoveries={}",
                    out.root,
                    out.errors.len(),
                    out.state.ctx.recovery_count()
                );
            }
            Err(err) => {
                eprintln!("Parse failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_box {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default(input_path)
        } else {
            compiler.compile_file(input_path, &cli.import_dir)
        };
        timer.phase("parse");

        match result {
            Ok(out) => {
                let Some(root) = out.root else {
                    eprintln!("Parse failed: no root node produced");
                    std::process::exit(1);
                };
                let rendered = format!("{}\n", dump_box(&out.state.arena, root));
                timer.phase("dump-box");
                emit_output(&rendered, cli.output.as_ref());
            }
            Err(err) => {
                eprintln!("Parse failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.svg {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        // Use eval+propagate to get the evaluated process box (post-eval form).
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_signals(input_path)
        } else {
            compiler.compile_file_to_signals(input_path, &cli.import_dir)
        };
        timer.phase("eval");

        match result {
            Ok(out) => {
                // Derive output directory name from input stem: "<name>-svg/"
                let stem = input_path
                    .file_stem()
                    .unwrap_or(std::ffi::OsStr::new("process"))
                    .to_string_lossy();
                let dir = std::path::PathBuf::from(format!("{stem}-svg"));
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    eprintln!("SVG: cannot create output directory {}: {e}", dir.display());
                    std::process::exit(1);
                }
                timer.phase("svg-setup");

                let draw_config = draw::DrawConfig {
                    shadow_blur: cli.shadow_blur,
                    scaled_svg: cli.scaled_svg,
                    draw_route_frame: cli.draw_route_frame,
                    max_name_size: cli.max_name_size,
                    fold_threshold: cli.fold,
                    fold_complexity: cli.fold_complexity,
                };
                if let Err(e) = draw::draw_schema(
                    &out.parse.state.arena,
                    out.process_box,
                    &cli.process_name,
                    &dir,
                    &draw_config,
                    &out.def_names,
                ) {
                    eprintln!("SVG generation failed: {e}");
                    std::process::exit(1);
                }
                timer.phase("svg-render");
                eprintln!("SVG written to {}", dir.display());
            }
            Err(err) => {
                eprintln!("SVG: compile failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_sig {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_signals(input_path)
        } else {
            compiler.compile_file_to_signals(input_path, &cli.import_dir)
        };
        timer.phase("signals");

        match result {
            Ok(out) => {
                let mut rendered = format!(
                    "Signals OK: inputs={} outputs={}",
                    out.process_arity.inputs, out.process_arity.outputs
                );
                for (index, sig) in out.signals.iter().enumerate() {
                    rendered.push('\n');
                    rendered.push_str(&format!(
                        "[{index}] {}",
                        dump_sig_readable(&out.parse.state.arena, *sig)
                    ));
                }
                rendered.push('\n');
                emit_output(&rendered, cli.output.as_ref());
            }
            Err(err) => {
                eprintln!("Signal pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_fir_verify {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = Compiler::new()
            .with_fir_verify_options(FirVerifyOptions {
                enabled: false,
                strict: false,
            })
            .with_process_name(cli.process_name.clone())
            .with_real_type(selected_real_type(&cli))
            .with_cancel(std::sync::Arc::clone(&cancel));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_fir_with_lane(
                input_path,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_fir_with_lane(
                input_path,
                &cli.import_dir,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("FIR");

        match result {
            Ok(out) => {
                let rendered =
                    render_fir_verify_report(&out.store, out.module, cli.fir_verify_strict);
                let report = verify_fir_module(&out.store, out.module);
                let fatal = report.has_errors()
                    || (cli.fir_verify_strict && report.warnings().next().is_some());
                timer.phase("verify");
                emit_output(&rendered, cli.output.as_ref());
                if fatal {
                    std::process::exit(1);
                }
            }
            Err(err) => {
                eprintln!("FIR pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_fir || matches!(cli.lang, Some(CliLang::Fir)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_fir_with_lane(
                input_path,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_fir_with_lane(
                input_path,
                &cli.import_dir,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("FIR");

        match result {
            Ok(out) => {
                let mut rendered = dump_fir(&out.store, out.module);
                if !rendered.ends_with('\n') {
                    rendered.push('\n');
                }
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, &cli, input_path, CliLang::Fir);
                }
            }
            Err(err) => {
                eprintln!("FIR pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_json && cli.lang.is_none() {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_json_with_lane_and_compile_options(
                input_path,
                selected_codegen_lane(&cli).into_compiler_lane(),
                compile_options_json_string(None, cli.double),
            )
        } else {
            compiler.compile_file_to_json_with_compile_options(
                input_path,
                &cli.import_dir,
                selected_codegen_lane(&cli).into_compiler_lane(),
                compile_options_json_string(None, cli.double),
            )
        };
        timer.phase("json");

        match result {
            Ok(json) => emit_output(&json, cli.output.as_ref()),
            Err(err) => {
                eprintln!("JSON pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_interp || matches!(cli.lang, Some(CliLang::Interp)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let options = InterpOptions::default();
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_interp_with_lane(
                input_path,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_interp_with_lane(
                input_path,
                &cli.import_dir,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("interp");

        match result {
            Ok(fbc_text) => {
                emit_output(&fbc_text, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(
                        &compiler,
                        &cli,
                        input_path,
                        CliLang::Interp,
                    );
                }
            }
            Err(err) => {
                eprintln!("Interp pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_cranelift || matches!(cli.lang, Some(CliLang::Cranelift)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_fir_with_lane(
                input_path,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_fir_with_lane(
                input_path,
                &cli.import_dir,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("FIR");

        match result {
            Ok(out) => {
                let subset_gap = diagnose_cranelift_compute_subset_gap(&out.store, out.module)
                    .map_err(|err| err.to_string());
                let compiled = match generate_cranelift_module(
                    &out.store,
                    out.module,
                    &CraneliftOptions::default(),
                ) {
                    Ok(compiled) => compiled,
                    Err(err) => {
                        eprintln!("Cranelift pipeline failed: {err}");
                        std::process::exit(1);
                    }
                };
                timer.phase("cranelift-codegen");
                let rendered =
                    render_cranelift_report(&compiled, subset_gap.ok().flatten().as_deref());
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(
                        &compiler,
                        &cli,
                        input_path,
                        CliLang::Cranelift,
                    );
                }
            }
            Err(err) => {
                eprintln!("Cranelift FIR pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Asc)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_fir_with_lane(
                input_path,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_fir_with_lane(
                input_path,
                &cli.import_dir,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("asc-codegen");

        match result {
            Ok(out) => {
                let options = AscOptions {
                    class_name: selected_class_name(&cli),
                    ..AscOptions::default()
                };
                match generate_asc_module(&out.store, out.module, &options) {
                    Ok(asc) => {
                        emit_output(&asc, cli.output.as_ref());
                        if cli.dump_json {
                            emit_cli_json_companion_for_backend(
                                &compiler,
                                &cli,
                                input_path,
                                CliLang::Asc,
                            );
                        }
                    }
                    Err(err) => {
                        eprintln!("AssemblyScript codegen failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Err(err) => {
                eprintln!("AssemblyScript pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Julia)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let options = JuliaOptions {
            class_name: selected_class_name(&cli),
            real_type: selected_julia_real_type(&cli),
        };
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_julia_with_lane(
                input_path,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_julia_with_lane(
                input_path,
                &cli.import_dir,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("julia-codegen");

        match result {
            Ok(julia) => {
                let rendered = wrap_backend_with_architecture(&julia, &cli);
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(
                        &compiler,
                        &cli,
                        input_path,
                        CliLang::Julia,
                    );
                }
            }
            Err(err) => {
                eprintln!("Julia pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Wasm)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let options = WasmOptions {
            double_precision: cli.double,
            ..WasmOptions::default()
        };
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_wasm_with_lane(
                input_path,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_wasm_with_lane(
                input_path,
                &cli.import_dir,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("wasm-codegen");

        match result {
            Ok(wasm) => {
                if cli.dump_json {
                    let output = require_companion_output_path(&cli);
                    emit_wasm_output(&wasm.wasm_binary, &wasm.dsp_json, Some(output));
                } else {
                    emit_wasm_output(&wasm.wasm_binary, &wasm.dsp_json, cli.output.as_ref());
                }
            }
            Err(err) => {
                eprintln!("WASM pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Wast)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let options = WasmOptions {
            double_precision: cli.double,
            ..WasmOptions::default()
        };
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_wasm_with_lane(
                input_path,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_wasm_with_lane(
                input_path,
                &cli.import_dir,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("wast-codegen");

        match result {
            Ok(wasm) => {
                let wast = render_wast_output(&wasm.wasm_binary);
                emit_output(&wast, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, &cli, input_path, CliLang::Wast);
                }
            }
            Err(err) => {
                eprintln!("WAST pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_cpp || matches!(cli.lang, Some(CliLang::Cpp)) || mode_count == 0 {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let options = CppOptions {
            class_name: selected_class_name(&cli),
            super_class_name: selected_super_class_name(&cli),
            ..CppOptions::default()
        };
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_cpp_with_lane(
                input_path,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_cpp_with_lane(
                input_path,
                &cli.import_dir,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("cpp-codegen");

        match result {
            Ok(cpp) => {
                let rendered = if let Some(architecture_file) = cli.architecture.as_ref() {
                    let mut options = EnrobageOptions::new(architecture_file.clone());
                    options.architecture_dirs = cli.architecture_dir.clone();
                    options.inline_arch_files = cli.inline_architecture_files;
                    if let Some(class_name) = selected_class_name(&cli) {
                        options.class_name = class_name;
                    }
                    if let Some(super_class_name) = selected_super_class_name(&cli) {
                        options.super_class_name = super_class_name;
                    }
                    let wrapped = match wrap_cpp_with_architecture(&cpp, &options) {
                        Ok(wrapped) => wrapped,
                        Err(err) => {
                            eprintln!("Architecture wrapping failed: {err}");
                            std::process::exit(1);
                        }
                    };
                    if let Some(err) = wrapped.recoverable_error.as_deref() {
                        eprintln!("{err}");
                        std::process::exit(1);
                    }
                    wrapped.code
                } else {
                    cpp
                };
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, &cli, input_path, CliLang::Cpp);
                }
            }
            Err(err) => {
                eprintln!("C++ pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    if cli.dump_c || matches!(cli.lang, Some(CliLang::C)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(&cli, Some(std::sync::Arc::clone(&cancel)));
        let options = COptions {
            class_name: selected_class_name(&cli),
            ..COptions::default()
        };
        let result = if cli.import_dir.is_empty() {
            compiler.compile_file_default_to_c_with_lane(
                input_path,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        } else {
            compiler.compile_file_to_c_with_lane(
                input_path,
                &cli.import_dir,
                &options,
                selected_codegen_lane(&cli).into_compiler_lane(),
            )
        };
        timer.phase("c-codegen");

        match result {
            Ok(c_code) => {
                let rendered = if let Some(architecture_file) = cli.architecture.as_ref() {
                    let mut options = EnrobageOptions::new(architecture_file.clone());
                    options.architecture_dirs = cli.architecture_dir.clone();
                    options.inline_arch_files = cli.inline_architecture_files;
                    if let Some(class_name) = selected_class_name(&cli) {
                        options.class_name = class_name;
                    }
                    let wrapped = match wrap_cpp_with_architecture(&c_code, &options) {
                        Ok(wrapped) => wrapped,
                        Err(err) => {
                            eprintln!("Architecture wrapping failed: {err}");
                            std::process::exit(1);
                        }
                    };
                    if let Some(err) = wrapped.recoverable_error.as_deref() {
                        eprintln!("{err}");
                        std::process::exit(1);
                    }
                    wrapped.code
                } else {
                    c_code
                };
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, &cli, input_path, CliLang::C);
                }
            }
            Err(err) => {
                eprintln!("C pipeline failed: {err}");
                print_structured_diagnostics(&err, cli.error_format, cli.error_verbosity);
                std::process::exit(1);
            }
        }
        timer.total();
        return;
    }

    print_global_usage_and_exit();
}
