//! Process-level orchestration for the `faust-rs` CLI.
//!
//! This module contains the code that turns parsed [`CliArgs`] into compiler
//! operations: parse/dump modes, backend dispatch, FIR fixture handling,
//! architecture wrapping, companion JSON generation, binary/text output, and
//! process exit behavior.  It deliberately remains a binary-facing layer; the
//! reusable compilation API stays in the `compiler` library crate.

use clap::Parser;
use std::path::{Path, PathBuf};

use codegen::backends::interp::{InterpOptions, generate_interp_module, write_fbc};
use codegen::backends::julia::JuliaRealType;
use codegen::backends::rust::RustRealType;
use codegen::backends::wasm::WasmOptions;
use codegen::fixtures::backend_test_fixtures;
use compiler::{
    Compiler, CompilerError, ComputeMode, ControlRateMode, FaustInstallPaths, FirVerifyOptions,
    ProcessingApi, RealType, SchedulingStrategy, compile_options_json_string,
    enrobage::{EnrobageOptions, wrap_cpp_with_architecture},
};
use diagnostics::DiagnosticBundle;
use fir::checker::verify_fir_module;

use super::args::{
    CliArgs, CliLang, CliSignalFirLane, ErrorFormat, ErrorVerbosity, normalize_legacy_args,
};
use super::diagnostics::{format_diagnostics_json_with_verbosity, print_bundle};
use super::validate::{
    handle_early_exit_modes, handle_fixture_listing, spawn_timeout_watchdog, validate_cli_arguments,
};
use super::{fixture_mode::run_fir_fixture_mode, source_mode::run_source_mode};

/// Prints top-level usage and exits the process.
pub fn print_global_usage_and_exit() -> ! {
    eprintln!("Usage:");
    eprintln!(
        "  cargo run -p compiler -- -lang asc|c|cpp|fir|julia|rust|wast <input.dsp> [-o <file>] [-I <dir> ...] [--class-name <name>] [--super-class-name <name>] [--signal-fir-lane fast] [--error-format human|json] [--error-verbosity standard|debug]"
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
        "  cargo run -p compiler -- --check <input.dsp> [-I <dir> ...] [--signal-fir-lane fast] [--fir-verify-strict] [--error-format human|json] [--error-verbosity standard|debug]"
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

/// Renders the first requested Faust directory-info flag, following the C++
/// precedence order in `global::printDirectories()`.
pub fn render_directory_info(cli: &CliArgs, paths: &FaustInstallPaths) -> Option<String> {
    if cli.libdir {
        Some(paths.render_lib_dir())
    } else if cli.includedir {
        Some(paths.render_include_dir())
    } else if cli.archdir {
        Some(paths.render_arch_dir())
    } else if cli.dspdir {
        Some(paths.render_dsp_dir())
    } else if cli.pathslist {
        Some(paths.render_paths_list())
    } else {
        None
    }
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
        CliLang::Cmajor => "cmajor",
        CliLang::Cpp => "cpp",
        CliLang::Fir => "fir",
        CliLang::Interp => "interp",
        CliLang::Cranelift => "cranelift",
        CliLang::Asc => "asc",
        CliLang::Codebox => "codebox",
        CliLang::CodeboxTest => "codebox-test",
        CliLang::Julia => "julia",
        CliLang::Rust => "rust",
        CliLang::Wasm => "wasm",
        CliLang::Wast => "wast",
    }
}

/// Maps a [`CliLang`] to the backend identifier the capability table is keyed
/// by ([`compiler::execution::backend_execution_caps`]).
///
/// Almost always the `-lang` token itself, but not always: `codebox-test` is a
/// second spelling of the codebox backend that changes parameter naming and
/// nothing else, so it must resolve to the same row. Looking a row up under
/// `codebox-test` would fail closed and reject a perfectly valid command line.
pub fn cli_backend_id(lang: CliLang) -> &'static str {
    match lang {
        CliLang::CodeboxTest => "codebox",
        other => cli_lang_name(other),
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
///
/// Delegates to the facade renderer so the FIR-fixture path below and
/// [`Compiler::compile_file_default_to_cranelift_report`] cannot drift apart.
pub fn render_cranelift_report(
    compiled: &codegen::backends::cranelift::JitDspModule,
    subset_gap: Option<&str>,
) -> String {
    compiler::render_cranelift_module_report(compiled, subset_gap)
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

/// Maps the `-vec`/`-vs`/`-lv` switches to a [`ComputeMode`] (roadmap P6, V1).
pub fn selected_compute_mode(cli: &CliArgs) -> ComputeMode {
    if cli.vec {
        ComputeMode::Vector {
            vec_size: cli.vs,
            loop_variant: cli.lv,
        }
    } else {
        ComputeMode::Scalar
    }
}

/// Maps `-ss`/`--scheduling-strategy` to a [`SchedulingStrategy`] (vectorization
/// port plan P2). Reuses [`SchedulingStrategy::decode`]'s total `0/1/2/n>=3`
/// split; `clap`'s `u32` parsing already rejects missing, non-integer, and
/// negative values before this function ever runs.
pub fn selected_scheduling_strategy(cli: &CliArgs) -> SchedulingStrategy {
    SchedulingStrategy::decode(cli.scheduling_strategy)
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

/// Maps CLI precision switches to the Rust backend's `FaustFloat` alias,
/// mirroring [`selected_real_type`] for the Rust code generator.
pub fn selected_rust_real_type(cli: &CliArgs) -> RustRealType {
    if cli.double {
        RustRealType::Float64
    } else {
        RustRealType::Float32
    }
}

/// Maps `-ec` to a [`ControlRateMode`].
pub fn selected_control_rate_mode(cli: &CliArgs) -> ControlRateMode {
    if cli.external_control {
        ControlRateMode::External
    } else {
        ControlRateMode::InlinePerBlock
    }
}

/// Maps `-os` to a [`ProcessingApi`].
pub fn selected_processing_api(cli: &CliArgs) -> ProcessingApi {
    if cli.one_sample {
        ProcessingApi::OneSample
    } else {
        ProcessingApi::Block
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
        .with_dlt(cli.dlt)
        .with_compute_mode(selected_compute_mode(cli))
        .with_scheduling_strategy(selected_scheduling_strategy(cli))
        .with_control_rate_mode(selected_control_rate_mode(cli))
        .with_processing_api(selected_processing_api(cli));
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
        Err(err) => report_pipeline_failure("JSON companion pipeline failed", &err, cli),
    }
}

/// Reports one compiler-pipeline failure honoring the CLI-selected diagnostic
/// format (D1: "make the machine channel clean"), then exits the process
/// with status 1.
///
/// `--error-format human` preserves the pre-D1 behavior byte for byte: a
/// short `"<prefix>: <err>"` line goes to stderr, immediately followed by the
/// human-rendered diagnostic bundle (also stderr; see
/// [`print_bundle`]).
///
/// `--error-format json` suppresses the human prefix line entirely --
/// [`print_bundle`] is the sole writer of stdout content in
/// that mode, and it writes exactly one well-formed JSON document with no
/// leading or trailing non-JSON bytes, which is the contract the P0 phase of
/// `porting/mcp-server-analysis-and-plan-2026-07-21-en.md` (§1.4.2, Part 4)
/// exists to guarantee. All pipeline dispatch sites in this module funnel
/// their `Err` arm through this one function so the human/json split is
/// enforced in one place instead of once per backend.
pub(crate) fn report_pipeline_failure(prefix: &str, err: &CompilerError, cli: &CliArgs) -> ! {
    if matches!(cli.error_format, ErrorFormat::Human) {
        eprintln!("{prefix}: {err}");
    }
    print_bundle(
        err.diagnostic_bundle(),
        cli.error_format,
        cli.error_verbosity,
        cli.diagnostic_paths,
    );
    std::process::exit(1);
}

/// Prints the `--check` (D2) success payload.
///
/// Human mode prints a one-line `"Check OK: 0 diagnostics"` summary. JSON
/// mode prints an envelope with an empty `diagnostics` array, deliberately
/// reusing the exact same renderer as the failure path
/// ([`print_bundle`]) so success and failure share one
/// schema -- a consumer never needs a second parser for `--check`.
pub(crate) fn emit_check_success(format: ErrorFormat, verbosity: ErrorVerbosity) {
    match format {
        ErrorFormat::Human => println!("Check OK: 0 diagnostics"),
        ErrorFormat::Json => println!(
            "{}",
            format_diagnostics_json_with_verbosity(&DiagnosticBundle::new(), verbosity)
        ),
    }
}

/// Prints the non-blocking semantic warnings for `input_path` under `--warn`.
///
/// # Why a separate front-end pass
///
/// Warnings are produced by the front end but every output mode returns its own
/// artifact type (FIR module, generated source, JSON), so surfacing them
/// in-band would mean threading a diagnostic bundle through each one. Running
/// the front end once more is confined to this opt-in flag and keeps all modes
/// behaving identically.
///
/// # Stream contract
///
/// Warnings always go to stderr, in both formats. On success stdout carries
/// generated output, and under `--error-format json` it is reserved for the one
/// diagnostics document the D1 contract promises; a warning must not compete
/// with either.
///
/// A failure here is silent on purpose: the real compilation that follows will
/// report the same failure through the normal diagnostic path.
pub(crate) fn report_semantic_warnings(
    cli: &CliArgs,
    input_path: &Path,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    if !cli.warn {
        return;
    }
    let compiler =
        compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel))).with_semantic_warnings(true);
    let Ok(output) = compiler.compile_file_to_signals(input_path, &cli.import_dir) else {
        return;
    };
    if output.warnings.is_empty() {
        return;
    }
    match cli.error_format {
        ErrorFormat::Human => eprint!(
            "{}",
            super::human::format_bundle(
                &output.warnings,
                super::human::HumanRenderOptions {
                    verbosity: cli.error_verbosity,
                    path_style: cli.diagnostic_paths,
                },
            )
        ),
        ErrorFormat::Json => eprintln!(
            "{}",
            format_diagnostics_json_with_verbosity(&output.warnings, cli.error_verbosity)
        ),
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

    if handle_early_exit_modes(&cli) {
        return;
    }
    let cancel = spawn_timeout_watchdog(&cli);
    if handle_fixture_listing(&cli) {
        return;
    }
    let Some(mode_count) = validate_cli_arguments(&cli) else {
        return;
    };

    if let Some(fixture_name) = cli.fir_fixture.as_deref() {
        run_fir_fixture_mode(&cli, fixture_name, mode_count);
        return;
    }

    let Some(input_path) = cli.input.as_ref() else {
        print_global_usage_and_exit();
    };
    run_source_mode(&cli, input_path, &cancel, mode_count);
}
