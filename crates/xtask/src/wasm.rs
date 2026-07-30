//! `faustwasm` compiler-module build and ABI verification workflow.
//!
//! This module owns the `build-faustwasm-compiler-module` command. It builds
//! the raw `wasm-ffi` artifact and verifies the exported ABI surface expected by
//! the embedded compiler adapter.

use super::*;

// ---------------------------------------------------------------------------
// `build-faustwasm-compiler-module`
// ---------------------------------------------------------------------------

const CARGO_WASM_MODULE_NAME: &str = "faust_wasm_ffi.wasm";
const DISTRIBUTED_WASM_MODULE_NAME: &str = "libfaust-rs.wasm";

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

impl From<FaustwasmCompilerModuleArgs> for FaustwasmCompilerModuleOptions {
    fn from(args: FaustwasmCompilerModuleArgs) -> Self {
        Self {
            release: !args.debug,
        }
    }
}

/// Builds the raw Rust compiler module consumed by the future embedded
/// `faustwasm` path and verifies that its exported ABI matches the documented
/// `wasm-ffi` contract.
pub(crate) fn build_faustwasm_compiler_module(
    args: FaustwasmCompilerModuleArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = FaustwasmCompilerModuleOptions::from(args);
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

    let artifact_dir = root
        .join("target")
        .join("wasm32-unknown-unknown")
        .join(profile);
    let cargo_module_path = artifact_dir.join(CARGO_WASM_MODULE_NAME);
    let module_path = artifact_dir.join(DISTRIBUTED_WASM_MODULE_NAME);
    publish_wasm_compiler_module(&cargo_module_path, &module_path)?;
    let bytes = fs::read(&module_path)?;
    verify_wasm_ffi_exports(&bytes)?;
    println!(
        "faustwasm compiler module ready: {}",
        workspace_relative_path(&module_path)
    );
    Ok(())
}

/// Publishes Cargo's crate-normalized artifact under the stable distribution
/// name consumed by WebAssembly hosts.
///
/// Cargo derives the linked filename from the Rust library target and emits
/// `faust_wasm_ffi.wasm`; Rust target names cannot directly express the
/// hyphenated `libfaust-rs.wasm` distribution contract. Re-running the workflow
/// is supported when Cargo considers the raw artifact fresh and only the
/// already-published module is present.
pub(crate) fn publish_wasm_compiler_module(
    cargo_module_path: &Path,
    distributed_module_path: &Path,
) -> Result<(), io::Error> {
    if cargo_module_path.is_file() {
        if distributed_module_path.exists() {
            fs::remove_file(distributed_module_path)?;
        }
        fs::rename(cargo_module_path, distributed_module_path)?;
        return Ok(());
    }
    if distributed_module_path.is_file() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "Cargo WebAssembly artifact not found at {}",
            cargo_module_path.display()
        ),
    ))
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
        "faust_wasm_result_get_error_diagnostics",
        "faust_wasm_result_get_diagnostics",
        "faust_wasm_result_free",
        "faust_wasm_get_info",
        "faust_wasm_expand_dsp",
        "faust_wasm_generate_aux_files",
        "faust_wasm_generate_aux_files_json",
        "faust_wasm_text_result_is_ok",
        "faust_wasm_text_result_ptr",
        "faust_wasm_text_result_len",
        "faust_wasm_text_result_diagnostics_ptr",
        "faust_wasm_text_result_diagnostics_len",
        "faust_wasm_text_result_free",
    ]
}

/// Verifies that a compiled `wasm-ffi` module exports the documented raw ABI.
pub(crate) fn verify_wasm_ffi_exports(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut exported_functions = BTreeSet::new();
    let mut has_memory_export = false;

    for payload in WasmParser::new(0).parse_all(bytes) {
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
