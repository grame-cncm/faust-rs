//! Command-line validation and the flags that short-circuit compilation.
//!
//! Everything here runs before any parsing or lowering: the informational
//! flags that print and exit, the timeout watchdog, and the rejection of
//! unusable command lines.

use compiler::{Compiler, FaustInstallPaths};

use super::args::{CliArgs, CliLang};
use super::runner::*;

/// Handles the flags that print information and exit before any compilation:
/// `--version`, `--help-error-format`, and the directory queries.
///
/// Returns `true` when one of them ran, meaning the caller must return.
pub(crate) fn handle_early_exit_modes(cli: &CliArgs) -> bool {
    if cli.version {
        println!("{}", render_version_text());
        return true;
    }
    maybe_print_error_format_help(cli.help_error_format);
    if let Some(info) = render_directory_info(cli, &FaustInstallPaths::from_environment()) {
        print!("{info}");
        return true;
    }
    false
}

/// Arms the cooperative cancellation flag and, when `--timeout` is non-zero,
/// the CLI watchdog thread that enforces it.
pub(crate) fn spawn_timeout_watchdog(
    cli: &CliArgs,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    // Cooperative cancellation flag + CLI watchdog.
    //
    // Two-pronged timeout approach:
    // 1. The cooperative cancel flag is checked by the evaluator on every
    //    recursive call and returns `EvalError::Cancelled`. This is safe for
    //    library (libfaust-rs) usage because it never calls `process::exit`.
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
    cancel
}

/// Handles `--list-fir-fixtures`. Returns `true` when it ran.
pub(crate) fn handle_fixture_listing(cli: &CliArgs) -> bool {
    if cli.list_fir_fixtures {
        if cli.fir_fixture.is_some() || cli.input.is_some() {
            eprintln!("--list-fir-fixtures does not accept --fir-fixture or input file");
            std::process::exit(2);
        }
        emit_output(&render_fir_fixture_list(), cli.output.as_ref());
        return true;
    }
    false
}

/// Rejects every unusable command line before any compilation starts, and
/// returns how many output modes were selected.
///
/// `None` means the run is already over: the "no mode, no input" case prints
/// the scaffold banner and there is nothing left to compile. A count of zero
/// means "no explicit mode", so the caller falls back to the default C++
/// emission.
///
/// Every rejection here exits with status 2, so the order of the checks decides
/// which message a doubly-invalid command line reports — keep it stable.
pub(crate) fn validate_cli_arguments(cli: &CliArgs) -> Option<usize> {
    #[cfg(not(all(feature = "network-imports", not(target_arch = "wasm32"))))]
    if cli.allow_network_imports {
        eprintln!(
            "--allow-network-imports requires a native build with the `network-imports` Cargo feature"
        );
        std::process::exit(2);
    }

    // Execution-option validation happens before any parsing or lowering
    // (plan §4.2): when `-lang` names the backend, consult the capability
    // table now; backend paths selected without `-lang` are enforced by the
    // same validation at the lowering dispatch.
    //
    // `cli.vec` is part of the trigger because a backend may reject `-vec` on
    // its own, with neither `-ec` nor `-os` in play (codebox does).
    if cli.external_control || cli.one_sample || cli.vec {
        if let Some(lang) = cli.lang {
            if let Err(error) = compiler::execution::validate_execution_options(
                cli_backend_id(lang),
                selected_control_rate_mode(cli),
                selected_processing_api(cli),
                selected_compute_mode(cli),
            ) {
                eprintln!("ERROR : {error}");
                std::process::exit(1);
            }
        } else if cli.one_sample && cli.vec {
            eprintln!(
                "ERROR : {}",
                compiler::execution::ExecutionOptionsError::OneSampleWithVectorMode
            );
            std::process::exit(1);
        }
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
        cli.check,
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
        return None;
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
        || cli.check
        || matches!(
            cli.lang,
            Some(
                CliLang::Fir
                    | CliLang::Interp
                    | CliLang::Cranelift
                    | CliLang::Wasm
                    | CliLang::Wast
                    | CliLang::Asc
                    | CliLang::Rust
            )
        ))
        && cli.architecture.is_some()
    {
        eprintln!("--architecture is currently supported only for C/C++/Cmajor/Julia output");
        std::process::exit(2);
    }
    if cli.no_fir_verify && (cli.dump_fir_verify || cli.check) {
        eprintln!("--no-fir-verify is incompatible with --dump-fir-verify/--check");
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
        if cli.golden || cli.parse || cli.dump_box || cli.dump_sig || cli.check {
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
    Some(mode_count)
}
