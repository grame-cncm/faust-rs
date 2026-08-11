//! The `--fir-fixture` mode: emit from a built-in FIR fixture.
//!
//! The FIR module comes from a fixture instead of a compiled DSP source, then
//! goes through the same per-backend emission ladder as [`super::source_mode`].

use codegen::backends::asc::{AscOptions, generate_asc_module};
use codegen::backends::c::COptions;
use codegen::backends::c::generate_c_module;
use codegen::backends::cmajor::{CmajorOptions, CmajorRealType, generate_cmajor_module};
use codegen::backends::codebox::{CodeboxOptions, generate_codebox_module};
use codegen::backends::cpp::CppOptions;
use codegen::backends::cpp::generate_cpp_module;
use codegen::backends::cranelift::{
    CraneliftOptions, diagnose_cranelift_compute_subset_gap, generate_cranelift_module,
};
use codegen::backends::interp::InterpOptions;
use codegen::backends::julia::{JuliaOptions, generate_julia_module};
use codegen::backends::rust::{RustOptions, generate_rust_module};
use codegen::backends::wasm::{WasmOptions, generate_wasm_module};
use fir::{checker::verify_fir_module, dump_fir};

use super::args::{CliArgs, CliLang};
use super::runner::*;

/// Runs the `--fir-fixture` mode: the FIR module comes from a built-in fixture
/// instead of a compiled DSP source, then goes through the same per-backend
/// emission ladder.
pub(crate) fn run_fir_fixture_mode(cli: &CliArgs, fixture_name: &str, mode_count: usize) {
    let Some(build_fixture) = find_fir_fixture(fixture_name) else {
        eprintln!("Unknown FIR fixture: {fixture_name}");
        eprintln!("{}", render_fir_fixture_list());
        std::process::exit(2);
    };
    let (store, module) = build_fixture();

    if cli.dump_fir_verify {
        let rendered = render_fir_verify_report(&store, module, cli.fir_verify_strict);
        let report = verify_fir_module(&store, module);
        let fatal =
            report.has_errors() || (cli.fir_verify_strict && report.warnings().next().is_some());
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
            emit_fixture_json_companion(cli, &store, module, "fir");
        }
        return;
    }

    if cli.dump_interp || matches!(cli.lang, Some(CliLang::Interp)) {
        let options = InterpOptions {
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::Interp)),
            )),
            ..InterpOptions::default()
        };
        match compile_fixture_to_interp_text(&store, module, &options) {
            Ok(fbc_text) => {
                emit_output(&fbc_text, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "interp");
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
        let subset_gap =
            diagnose_cranelift_compute_subset_gap(&store, module).map_err(|err| err.to_string());
        let compiled = match generate_cranelift_module(&store, module, &CraneliftOptions::default())
        {
            Ok(compiled) => compiled,
            Err(err) => {
                eprintln!("Cranelift fixture codegen failed: {err}");
                std::process::exit(1);
            }
        };
        let rendered = render_cranelift_report(&compiled, subset_gap.ok().flatten().as_deref());
        emit_output(&rendered, cli.output.as_ref());
        if cli.dump_json {
            emit_fixture_json_companion(cli, &store, module, "cranelift");
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
                    let output = require_companion_output_path(cli);
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
                    emit_fixture_json_companion(cli, &store, module, "wast");
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
            // Default to `mydsp` like every other backend. Passing `None`
            // here made the generator fall back to the FIR module name,
            // which on this path carries the source file stem — so
            // `-lang asc foo.dsp` emitted `class foo` while `-lang cpp`,
            // `-lang julia` and `-lang rust` all emitted `mydsp`.
            class_name: selected_class_name(cli).or_else(|| Some("mydsp".to_owned())),
            double_precision: cli.double,
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::Asc)),
            )),
            ..AscOptions::default()
        };
        match generate_asc_module(&store, module, &options) {
            Ok(asc) => {
                emit_output(&asc, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "asc");
                }
            }
            Err(err) => {
                eprintln!("AssemblyScript fixture codegen failed: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Some(lang @ (CliLang::Codebox | CliLang::CodeboxTest)) = cli.lang {
        // Unlike the source path, nothing here can force the lowering: the FIR
        // module arrives already lowered from the fixture file. So the shape is
        // not imposed but *checked* — a fixture lowered for block processing
        // has no `frame` function, and the emitter says so with
        // `FRS-CGEN-CBOX-0002` rather than emitting something plausible.
        let options = CodeboxOptions {
            double_precision: cli.double,
            test_labels: lang == CliLang::CodeboxTest,
            compile_options: Some(compile_options_full_string(cli, Some(cli_lang_name(lang)))),
        };
        match generate_codebox_module(&store, module, &options) {
            Ok(codebox) => {
                emit_output(&codebox, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "codebox");
                }
            }
            Err(err) => {
                eprintln!("Codebox fixture codegen failed: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    if matches!(cli.lang, Some(CliLang::Cmajor)) {
        let options = CmajorOptions {
            class_name: selected_class_name(cli).unwrap_or_else(|| "mydsp".to_owned()),
            real_type: if cli.double {
                CmajorRealType::Float64
            } else {
                CmajorRealType::Float32
            },
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::Cmajor)),
            )),
        };
        match generate_cmajor_module(&store, module, &options) {
            Ok(cmajor) => {
                let rendered = wrap_backend_with_architecture(&cmajor, cli);
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "cmajor");
                }
            }
            Err(err) => {
                eprintln!("Cmajor fixture codegen failed: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    if matches!(cli.lang, Some(CliLang::Julia)) {
        let options = JuliaOptions {
            class_name: selected_class_name(cli),
            real_type: selected_julia_real_type(cli),
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::Julia)),
            )),
        };
        match generate_julia_module(&store, module, &options) {
            Ok(julia) => {
                let rendered = wrap_backend_with_architecture(&julia, cli);
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "julia");
                }
            }
            Err(err) => {
                eprintln!("Julia fixture codegen failed: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    if matches!(cli.lang, Some(CliLang::Rust)) {
        let options = RustOptions {
            class_name: selected_class_name(cli).or_else(|| Some("mydsp".to_owned())),
            faust_float_type: selected_rust_real_type(cli),
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::Rust)),
            )),
        };
        match generate_rust_module(&store, module, &options) {
            Ok(rust) => {
                emit_output(&rust, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "rust");
                }
            }
            Err(err) => {
                eprintln!("Rust fixture codegen failed: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.dump_json {
        let compile_options = compile_options_full_string(cli, cli.lang.map(cli_lang_name));
        match compile_fixture_to_json_text(&store, module, compile_options, cli.double) {
            Ok(json) => {
                if cli.lang.is_some() {
                    let output = require_companion_output_path(cli);
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
            class_name: selected_class_name(cli),
            super_class_name: selected_super_class_name(cli),
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::Cpp)),
            )),
            ..CppOptions::default()
        };
        match generate_cpp_module(&store, module, &options) {
            Ok(cpp) => {
                let rendered = wrap_backend_with_architecture(&cpp, cli);
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "cpp");
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
            class_name: selected_class_name(cli),
            compile_options: Some(compile_options_full_string(
                cli,
                Some(cli_lang_name(CliLang::C)),
            )),
            ..COptions::default()
        };
        match generate_c_module(&store, module, &options) {
            Ok(c_code) => {
                let rendered = if cli.architecture.is_some() {
                    wrap_backend_with_architecture(&c_code, cli)
                } else {
                    c_code
                };
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_fixture_json_companion(cli, &store, module, "c");
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

/// Emits the `--json` companion for a fixture-mode run.
///
/// The counterpart of [`emit_cli_json_companion_for_backend`] for this ladder:
/// the JSON is rebuilt from the already-lowered fixture module instead of by
/// compiling a source file, so `backend` only names the emission in the
/// `compile_options` provenance string.
///
/// Two companion sites deliberately do NOT go through here: the WASM branch
/// emits the module and its matched JSON together, and the standalone `--json`
/// branch already holds the rendered text.
fn emit_fixture_json_companion(
    cli: &CliArgs,
    store: &fir::FirStore,
    module: fir::FirId,
    backend: &str,
) {
    let output = require_companion_output_path(cli);
    let compile_options = compile_options_full_string(cli, Some(backend));
    match compile_fixture_to_json_text(store, module, compile_options, cli.double) {
        Ok(json) => emit_json_companion_output(&json, output),
        Err(err) => {
            eprintln!("JSON fixture generation failed: {err}");
            std::process::exit(1);
        }
    }
}
