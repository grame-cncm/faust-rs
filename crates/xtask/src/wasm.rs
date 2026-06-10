//! `faustwasm` compiler-module build and ABI verification workflow.
//!
//! This module owns the `build-faustwasm-compiler-module` command. It builds
//! the raw `wasm-ffi` artifact and verifies the exported ABI surface expected by
//! the embedded compiler adapter.

use super::*;

// ---------------------------------------------------------------------------
// `build-faustwasm-compiler-module`
// ---------------------------------------------------------------------------

/// Parsed options for the `build-faustwasm-compiler-module` workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FaustwasmCompilerModuleOptions {
    /// `true` builds the release artifact; `false` builds the debug artifact.
    pub(crate) release: bool,
}

impl Default for FaustwasmCompilerModuleOptions {
    fn default() -> Self {
        Self { release: true }
    }
}

/// Parses flags for the `build-faustwasm-compiler-module` workflow.
pub(crate) fn parse_faustwasm_compiler_module_options(
    args: impl Iterator<Item = String>,
) -> Result<FaustwasmCompilerModuleOptions, Box<dyn std::error::Error>> {
    let mut options = FaustwasmCompilerModuleOptions::default();
    for arg in args {
        match arg.as_str() {
            "--debug" => options.release = false,
            other => {
                return Err(format!(
                    "usage: cargo run -p xtask -- build-faustwasm-compiler-module [--debug]\nunknown option: {other}"
                )
                .into());
            }
        }
    }
    Ok(options)
}

/// Builds the raw Rust compiler module consumed by the future embedded
/// `faustwasm` path and verifies that its exported ABI matches the documented
/// `wasm-ffi` contract.
pub(crate) fn build_faustwasm_compiler_module(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_faustwasm_compiler_module_options(args)?;
    let root = workspace_root();
    let profile = if options.release { "release" } else { "debug" };

    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(&root)
        .arg("build")
        .arg("-p")
        .arg("wasm-ffi")
        .arg("--target")
        .arg("wasm32-unknown-unknown");
    if options.release {
        cargo.arg("--release");
    }
    // The Rust default wasm shadow stack is 1 MiB, which deep evaluator /
    // propagation recursion overflows on large real-world DSPs (for example
    // master_me-class mastering chains). Match a desktop-like 16 MiB stack.
    let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        rustflags.push(' ');
    }
    rustflags.push_str("-C link-arg=-zstack-size=16777216");
    cargo.env("RUSTFLAGS", rustflags);

    let status = cargo.status()?;
    if !status.success() {
        return Err(
            "failed to build `wasm-ffi` for `wasm32-unknown-unknown`; ensure the target is installed (for example with `rustup target add wasm32-unknown-unknown`) and try again"
                .into(),
        );
    }

    let module_path = root
        .join("target")
        .join("wasm32-unknown-unknown")
        .join(profile)
        .join("faust_wasm_ffi.wasm");
    let bytes = fs::read(&module_path)?;
    verify_wasm_ffi_exports(&bytes)?;
    println!(
        "faustwasm compiler module ready: {}",
        workspace_relative_path(&module_path)
    );
    Ok(())
}

/// Lists the minimum raw export surface expected by the `faustwasm`
/// embedded-compiler adapter.
///
/// The verifier checks a freshly built `.wasm` module against this list so ABI
/// regressions are caught in the same workflow that produces the artifact.
pub(crate) fn required_wasm_ffi_exports() -> &'static [&'static str] {
    &[
        "memory",
        "faust_wasm_alloc",
        "faust_wasm_dealloc",
        "faust_wasm_version_ptr",
        "faust_wasm_version_len",
        "faust_wasm_compile_dsp",
        "faust_wasm_result_is_ok",
        "faust_wasm_result_wasm_ptr",
        "faust_wasm_result_wasm_len",
        "faust_wasm_result_json_ptr",
        "faust_wasm_result_json_len",
        "faust_wasm_result_compile_options_ptr",
        "faust_wasm_result_compile_options_len",
        "faust_wasm_result_error_ptr",
        "faust_wasm_result_error_len",
        "faust_wasm_result_free",
        "faust_wasm_get_info",
        "faust_wasm_expand_dsp",
        "faust_wasm_generate_aux_files",
        "faust_wasm_generate_aux_files_json",
        "faust_wasm_text_result_is_ok",
        "faust_wasm_text_result_ptr",
        "faust_wasm_text_result_len",
        "faust_wasm_text_result_free",
    ]
}

/// Verifies that a compiled `wasm-ffi` module exports the documented raw ABI.
pub(crate) fn verify_wasm_ffi_exports(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut exported_functions = BTreeSet::new();
    let mut has_memory_export = false;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export?;
                    match export.kind {
                        ExternalKind::Memory if export.name == "memory" => {
                            has_memory_export = true;
                        }
                        ExternalKind::Func => {
                            exported_functions.insert(export.name.to_owned());
                        }
                        _ => {}
                    }
                }
            }
            Payload::End(_) => break,
            _ => {}
        }
    }

    let mut missing = Vec::new();
    if !has_memory_export {
        missing.push("memory".to_owned());
    }
    for export in required_wasm_ffi_exports() {
        if *export != "memory" && !exported_functions.contains(*export) {
            missing.push((*export).to_owned());
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "`wasm-ffi` module is missing required exports: {}",
            missing.join(", ")
        )
        .into())
    }
}
