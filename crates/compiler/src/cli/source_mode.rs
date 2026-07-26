//! The normal mode: compile a `.dsp` source and emit the selected output.
//!
//! This is the per-backend emission ladder reached from a DSP file, the
//! counterpart of [`super::fixture_mode`].

use boxes::dump_box;
use codegen::backends::asc::AscOptions;
use codegen::backends::c::COptions;
use codegen::backends::cpp::CppOptions;
use codegen::backends::cranelift::CraneliftOptions;
use codegen::backends::interp::{FbcCppOptions, InterpOptions, generate_cpp_from_fbc, read_fbc};
use codegen::backends::julia::JuliaOptions;
use codegen::backends::rust::{RustOptions, generate_rust_module};
use codegen::backends::wasm::WasmOptions;
use compiler::{
    Compiler, FirVerifyOptions, compile_options_json_string,
    enrobage::{EnrobageOptions, wrap_cpp_with_architecture},
    golden_snapshot_from_file,
};
use fir::{checker::verify_fir_module, dump_fir};
use signals::dump_sig_readable;

use super::args::{CliArgs, CliLang};
use super::runner::*;
use super::timer::CompilationTimer;

/// Runs the normal mode: compile the DSP source at `input_path` and emit
/// whatever the selected output mode asks for.
pub(crate) fn run_source_mode(
    cli: &CliArgs,
    input_path: &std::path::Path,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    mode_count: usize,
) {
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
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file(input_path, &cli.import_dir);
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
            Err(err) => report_pipeline_failure("Parse failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_box {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file(input_path, &cli.import_dir);
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
            Err(err) => report_pipeline_failure("Parse failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.svg {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        // Use eval+propagate to get the evaluated process box (post-eval form).
        let result = compiler.compile_file_to_signals(input_path, &cli.import_dir);
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
            Err(err) => report_pipeline_failure("SVG: compile failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_sig {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file_to_signals(input_path, &cli.import_dir);
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
            Err(err) => report_pipeline_failure("Signal pipeline failed", &err, cli),
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
            .with_real_type(selected_real_type(cli))
            .with_cancel(std::sync::Arc::clone(cancel));
        let result = compiler.compile_file_to_fir_with_lane(
            input_path,
            &cli.import_dir,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
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
            Err(err) => report_pipeline_failure("FIR pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.check {
        // D2: full front-end (parse → eval → propagate → type) plus FIR
        // verification, no codegen. `compiler_from_cli` wires FIR-verify
        // from `--no-fir-verify`/`--fir-verify-strict`, and the validation
        // block above rejects `--check --no-fir-verify`, so verification
        // always actually runs here -- unlike `--dump-fir-verify`, which
        // disables the built-in verify to report it manually.
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file_to_fir_with_lane(
            input_path,
            &cli.import_dir,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("check");

        match result {
            Ok(_) => emit_check_success(cli.error_format, cli.error_verbosity),
            Err(err) => report_pipeline_failure("Check failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_fir || matches!(cli.lang, Some(CliLang::Fir)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file_to_fir_with_lane(
            input_path,
            &cli.import_dir,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("FIR");

        match result {
            Ok(out) => {
                let mut rendered = dump_fir(&out.store, out.module);
                if !rendered.ends_with('\n') {
                    rendered.push('\n');
                }
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, cli, input_path, CliLang::Fir);
                }
            }
            Err(err) => report_pipeline_failure("FIR pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_json && cli.lang.is_none() {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file_to_json_with_compile_options(
            input_path,
            &cli.import_dir,
            selected_codegen_lane(cli).into_compiler_lane(),
            compile_options_json_string(None, cli.double),
        );
        timer.phase("json");

        match result {
            Ok(json) => emit_output(&json, cli.output.as_ref()),
            Err(err) => report_pipeline_failure("JSON pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_interp || matches!(cli.lang, Some(CliLang::Interp)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        // Honor `-cn`/`--class-name` like every other textual backend; this
        // used to be a hardcoded default, so the flag was silently ignored.
        let options = InterpOptions {
            module_name: selected_class_name(cli).or_else(|| Some("mydsp".to_owned())),
            ..InterpOptions::default()
        };
        let result = compiler.compile_file_to_interp_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("interp");

        match result {
            Ok(fbc_text) => {
                emit_output(&fbc_text, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(
                        &compiler,
                        cli,
                        input_path,
                        CliLang::Interp,
                    );
                }
            }
            Err(err) => report_pipeline_failure("Interp pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_cranelift || matches!(cli.lang, Some(CliLang::Cranelift)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let lane = selected_codegen_lane(cli).into_compiler_lane();
        let options = CraneliftOptions::default();
        let result = compiler.compile_file_to_cranelift_report_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            lane,
        );
        timer.phase("cranelift-codegen");

        match result {
            Ok(rendered) => {
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(
                        &compiler,
                        cli,
                        input_path,
                        CliLang::Cranelift,
                    );
                }
            }
            Err(err) => report_pipeline_failure("Cranelift FIR pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Asc)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        // Route through the facade helper, exactly like the Julia branch below.
        // The previous code lowered to FIR generically and called
        // `generate_asc_module` directly, which named the FIR module after the
        // source file — so `-lang asc foo.dsp` emitted `class foo` and a
        // `// name: foo` header while every other backend emitted `mydsp`.
        // Sharing one route makes CLI and facade output identical by
        // construction rather than by convention.
        let options = AscOptions {
            class_name: selected_class_name(cli).or_else(|| Some("mydsp".to_owned())),
            double_precision: cli.double,
            ..AscOptions::default()
        };
        let result = compiler.compile_file_to_asc_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("asc-codegen");

        match result {
            Ok(asc) => {
                emit_output(&asc, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, cli, input_path, CliLang::Asc);
                }
            }
            Err(err) => report_pipeline_failure("AssemblyScript pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Julia)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let options = JuliaOptions {
            class_name: selected_class_name(cli),
            real_type: selected_julia_real_type(cli),
        };
        let result = compiler.compile_file_to_julia_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("julia-codegen");

        match result {
            Ok(julia) => {
                let rendered = wrap_backend_with_architecture(&julia, cli);
                emit_output(&rendered, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, cli, input_path, CliLang::Julia);
                }
            }
            Err(err) => report_pipeline_failure("Julia pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Rust)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let result = compiler.compile_file_to_fir_with_lane(
            input_path,
            &cli.import_dir,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("rust-codegen");

        match result {
            Ok(out) => {
                let options = RustOptions {
                    class_name: selected_class_name(cli).or_else(|| Some("mydsp".to_owned())),
                    faust_float_type: selected_rust_real_type(cli),
                };
                match generate_rust_module(&out.store, out.module, &options) {
                    Ok(rust) => {
                        emit_output(&rust, cli.output.as_ref());
                        if cli.dump_json {
                            emit_cli_json_companion_for_backend(
                                &compiler,
                                cli,
                                input_path,
                                CliLang::Rust,
                            );
                        }
                    }
                    Err(err) => {
                        eprintln!("Rust codegen failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Err(err) => report_pipeline_failure("Rust pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Wasm)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let options = WasmOptions {
            double_precision: cli.double,
            ..WasmOptions::default()
        };
        let result = compiler.compile_file_to_wasm_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("wasm-codegen");

        match result {
            Ok(wasm) => {
                if cli.dump_json {
                    let output = require_companion_output_path(cli);
                    emit_wasm_output(&wasm.wasm_binary, &wasm.dsp_json, Some(output));
                } else {
                    emit_wasm_output(&wasm.wasm_binary, &wasm.dsp_json, cli.output.as_ref());
                }
            }
            Err(err) => report_pipeline_failure("WASM pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if matches!(cli.lang, Some(CliLang::Wast)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let options = WasmOptions {
            double_precision: cli.double,
            ..WasmOptions::default()
        };
        let result = compiler.compile_file_to_wasm_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("wast-codegen");

        match result {
            Ok(wasm) => {
                let wast = render_wast_output(&wasm.wasm_binary);
                emit_output(&wast, cli.output.as_ref());
                if cli.dump_json {
                    emit_cli_json_companion_for_backend(&compiler, cli, input_path, CliLang::Wast);
                }
            }
            Err(err) => report_pipeline_failure("WAST pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_cpp || matches!(cli.lang, Some(CliLang::Cpp)) || mode_count == 0 {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let options = CppOptions {
            class_name: selected_class_name(cli),
            super_class_name: selected_super_class_name(cli),
            ..CppOptions::default()
        };
        let result = compiler.compile_file_to_cpp_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("cpp-codegen");

        match result {
            Ok(cpp) => {
                let rendered = if let Some(architecture_file) = cli.architecture.as_ref() {
                    let mut options = EnrobageOptions::new(architecture_file.clone());
                    options.architecture_dirs = cli.architecture_dir.clone();
                    options.inline_arch_files = cli.inline_architecture_files;
                    if let Some(class_name) = selected_class_name(cli) {
                        options.class_name = class_name;
                    }
                    if let Some(super_class_name) = selected_super_class_name(cli) {
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
                    emit_cli_json_companion_for_backend(&compiler, cli, input_path, CliLang::Cpp);
                }
            }
            Err(err) => report_pipeline_failure("C++ pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    if cli.dump_c || matches!(cli.lang, Some(CliLang::C)) {
        let mut timer = CompilationTimer::new(cli.timeout, cli.compilation_time);
        let compiler = compiler_from_cli(cli, Some(std::sync::Arc::clone(cancel)));
        let options = COptions {
            class_name: selected_class_name(cli),
            ..COptions::default()
        };
        let result = compiler.compile_file_to_c_with_lane(
            input_path,
            &cli.import_dir,
            &options,
            selected_codegen_lane(cli).into_compiler_lane(),
        );
        timer.phase("c-codegen");

        match result {
            Ok(c_code) => {
                let rendered = if let Some(architecture_file) = cli.architecture.as_ref() {
                    let mut options = EnrobageOptions::new(architecture_file.clone());
                    options.architecture_dirs = cli.architecture_dir.clone();
                    options.inline_arch_files = cli.inline_architecture_files;
                    if let Some(class_name) = selected_class_name(cli) {
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
                    emit_cli_json_companion_for_backend(&compiler, cli, input_path, CliLang::C);
                }
            }
            Err(err) => report_pipeline_failure("C pipeline failed", &err, cli),
        }
        timer.total();
        return;
    }

    print_global_usage_and_exit();
}
