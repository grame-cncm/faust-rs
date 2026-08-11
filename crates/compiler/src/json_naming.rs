//! JSON description construction, name utilities, and diagnostic bundle helpers.
//!
//! Covers the JSON-facing surface of the compiler:
//! - `source_name_to_class` / `source_name_to_filename` — derive the class and
//!   file name strings used in generated code and JSON from a source path;
//! - `faustwasm_info_help_text` — help text for the faustwasm info query;
//! - [`StrictJsonContext`] / `build_strict_json_description` — assemble the
//!   full [`JsonDescription`] from a lowered FIR module (used by WASM backend);
//! - `compile_options_json_string` — formats the `-lang`/`-single`/`-double`
//!   string stored in JSON metadata;
//! - `wasm_json_context_for_*` / `json_meta_entries_from_snapshot` — build the
//!   [`WasmJsonContext`] for memory-backed and file-backed compilation sessions;
//! - `parse_search_paths_from_argv` / `sanitize_cpp_ident` / `resolve_ui_root_label` —
//!   miscellaneous name/path helpers;
//! - `bundle_from_diagnostic` / `eval_error_node` / `propagate_error_node` —
//!   diagnostic packaging and error-node extraction helpers.

use super::*;

// ─── Diagnostic helpers ───────────────────────────────────────────────────────

/// Converts a FIR verifier report into the workspace diagnostic bundle format.
pub(crate) fn fir_verify_bundle_from_report(report: &FirVerifyReport) -> DiagnosticBundle {
    let mut bundle = DiagnosticBundle::new();
    for d in &report.diagnostics {
        let code = match d.severity {
            FirVerifySeverity::Error => diagnostics::codes::FIR_VERIFY_ERROR,
            FirVerifySeverity::Warning => diagnostics::codes::FIR_VERIFY_WARNING,
        };
        let severity = match d.severity {
            FirVerifySeverity::Error => diagnostics::Severity::Error,
            FirVerifySeverity::Warning => diagnostics::Severity::Warning,
        };
        let mut diag = Diagnostic::new(severity, diagnostics::Stage::Fir, code, d.message.clone())
            .with_detail_code(d.code)
            .with_fact("fir_code", d.code)
            .with_fact("fir_node_id", u64::from(d.node.as_u32()))
            .with_debug_fact("fir_node_id", u64::from(d.node.as_u32()));
        if let Some(fun) = d.context.function_name.as_deref() {
            diag = diag.with_fact("fir_function", fun);
        }
        if let Some(var) = d.context.variable_name.as_deref() {
            diag = diag.with_fact("fir_variable", var);
        }
        bundle.push(diag);
    }
    bundle
}

/// Converts a `signal_fir` lowering error into a structured compiler diagnostic.
pub(crate) fn signal_fir_diagnostic(error: &SignalFirError) -> Diagnostic {
    let code = match error.code() {
        SignalFirErrorCode::InvalidOptions => diagnostics::codes::SFIR_INVALID_OPTIONS,
        SignalFirErrorCode::EmptySignalList => diagnostics::codes::SFIR_EMPTY_SIGNAL_LIST,
        SignalFirErrorCode::OutputArityMismatch => diagnostics::codes::SFIR_OUTPUT_ARITY_MISMATCH,
        SignalFirErrorCode::UnsupportedSignalNode => {
            diagnostics::codes::SFIR_UNSUPPORTED_SIGNAL_NODE
        }
        SignalFirErrorCode::UnsupportedBinOp => diagnostics::codes::SFIR_UNSUPPORTED_BINOP,
        SignalFirErrorCode::InputIndexOutOfRange => {
            diagnostics::codes::SFIR_INPUT_INDEX_OUT_OF_RANGE
        }
        SignalFirErrorCode::ClockedNotLowered => diagnostics::codes::SFIR_CLOCKED_NOT_LOWERED,
        SignalFirErrorCode::ClockAnalysis => diagnostics::codes::SFIR_CLOCK_ANALYSIS,
        SignalFirErrorCode::ForeignCountInExecutionMode => {
            diagnostics::codes::SFIR_FOREIGN_COUNT_IN_EXECUTION_MODE
        }
        SignalFirErrorCode::BlockSensitiveOneSample => {
            diagnostics::codes::SFIR_BLOCK_SENSITIVE_ONE_SAMPLE
        }
    };
    // `error.to_string()` renders as "[<the SFIR code>] <message>", and the
    // `Diagnostic` already carries the code, so using Display here printed it
    // twice: "error [FRS-SFIR-0004] [FRS-SFIR-0004] signal preparation
    // failed: ...". Take the bare message and let the diagnostic own the code.
    let category = match error.code() {
        SignalFirErrorCode::InvalidOptions => DiagnosticCategory::InvalidOptions,
        SignalFirErrorCode::ClockAnalysis => DiagnosticCategory::UserCode,
        SignalFirErrorCode::UnsupportedSignalNode
        | SignalFirErrorCode::UnsupportedBinOp
        | SignalFirErrorCode::ClockedNotLowered
        | SignalFirErrorCode::ForeignCountInExecutionMode
        | SignalFirErrorCode::BlockSensitiveOneSample => DiagnosticCategory::UnsupportedFeature,
        SignalFirErrorCode::EmptySignalList
        | SignalFirErrorCode::OutputArityMismatch
        | SignalFirErrorCode::InputIndexOutOfRange => DiagnosticCategory::CompilerBug,
    };
    let mut diagnostic = Diagnostic::new(
        diagnostics::Severity::Error,
        diagnostics::Stage::Transform,
        code,
        error.message(),
    )
    .with_category(category)
    .with_detail_code(error.code().as_str());
    if category == DiagnosticCategory::UnsupportedFeature {
        diagnostic = diagnostic
            .with_help("the selected lowering path does not support this Faust construct")
            .with_help("try a supported rewrite or another backend when available");
    } else if category == DiagnosticCategory::CompilerBug {
        diagnostic = diagnostic
            .with_help("this is an internal lowering invariant; report a minimal reproducer");
    }
    diagnostic
}

// ─── Name utilities ───────────────────────────────────────────────────────────

/// Derives the base class/module name from a source filename.
pub(crate) fn source_name_to_class(source_name: &str) -> String {
    Path::new(source_name)
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("faust_dsp")
        .to_owned()
}

/// Extracts the file-name component (with extension) from a source path string.
pub(crate) fn source_name_to_filename(source_name: &str) -> String {
    Path::new(source_name)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(source_name)
        .to_owned()
}

/// Returns the help text emitted for `faustwasm --info` queries.
///
/// Lists supported query keys so tooling can report capability.
pub(crate) fn faustwasm_info_help_text() -> String {
    let mut out = String::new();
    out.push_str("faust-rs faustwasm helper info\n");
    out.push_str("supported keys:\n");
    out.push_str("- version\n");
    out.push_str("- help\n");
    out.push_str("- libdir\n");
    out.push_str("- includedir\n");
    out.push_str("- archdir\n");
    out.push_str("- dspdir\n");
    out.push_str("- pathslist\n");
    out
}

/// Input bundle for [`build_strict_json_description`].
///
/// Separates the contextual metadata (filename, paths, options) from the FIR
/// store so callers can build the context independently of the lowering step.
pub(crate) struct StrictJsonContext {
    /// Source filename reported in the JSON `filename` field.
    pub(crate) filename: String,
    /// Import search paths included in the JSON `include_pathnames` array.
    pub(crate) include_pathnames: Vec<String>,
    /// Library files used during compilation, for the JSON `library_list` array.
    pub(crate) library_list: Vec<String>,
    /// Top-level `declare` metadata entries from the source.
    pub(crate) top_level_meta: Vec<JsonMetaEntry>,
    /// Formatted compile-options string (e.g. `"-lang wasm -single"`).
    pub(crate) compile_options: String,
    /// Whether the session targets double-precision output.
    pub(crate) double_precision: bool,
}

/// Builds the strict (Faust-compatible) JSON description of a compiled module.
///
/// Validates that `module` is a FIR `Module` whose functions section is a
/// `Block`, derives the WASM memory layout to obtain the struct size, then
/// assembles the [`JsonDescription`] from the FIR functions plus the contextual
/// metadata in `context`. Returns a [`WasmBackendError`] if the FIR root has an
/// unexpected shape.
pub(crate) fn build_strict_json_description(
    store: &FirStore,
    module: FirId,
    context: StrictJsonContext,
) -> Result<JsonDescription, WasmBackendError> {
    let fir::FirMatch::Module {
        name,
        functions,
        num_inputs,
        num_outputs,
        ..
    } = fir::match_fir(store, module)
    else {
        return Err(WasmBackendError::new(
            codegen::backends::wasm::WasmBackendErrorCode::UnsupportedModuleShape,
            "JSON generation expects a FIR Module root",
        ));
    };
    let fir::FirMatch::Block(function_items) = fir::match_fir(store, functions) else {
        return Err(WasmBackendError::new(
            codegen::backends::wasm::WasmBackendErrorCode::UnsupportedFirNode,
            "JSON generation expects the functions section to be a FIR Block",
        ));
    };
    let layout = WasmMemoryLayout::from_module(
        store,
        module,
        &WasmOptions {
            double_precision: context.double_precision,
            ..WasmOptions::default()
        },
        0,
    )?;
    build_json_description_from_fir(
        store,
        &function_items,
        JsonBuildOptions {
            name,
            filename: Some(context.filename),
            version: Some(Compiler::version().to_owned()),
            compile_options: Some(context.compile_options),
            library_list: context.library_list,
            include_pathnames: context.include_pathnames,
            top_level_meta: context.top_level_meta,
            size: Some(layout.struct_size),
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: None,
        },
        |_var| None,
    )
    .map_err(|error| {
        WasmBackendError::new(
            codegen::backends::wasm::WasmBackendErrorCode::UnsupportedFirNode,
            error.to_string(),
        )
    })
}

/// C++-parity baseline for the subset of `global::printCompilationOptions1()`
/// currently exposed by the Rust CLI/compiler path.
///
/// Mapping status: `adapted`.
/// - Included now: only the options that the Rust CLI actually exposes for the
///   selected flow (`-lang <backend>` when relevant, plus the float mode).
/// - Deferred: the rest of the C++ global option matrix until the
///   corresponding CLI/compiler knobs exist here.
pub fn compile_options_json_string(lang: Option<&str>, double_precision: bool) -> String {
    let float_mode = if double_precision {
        "-double"
    } else {
        "-single"
    };
    match lang {
        Some(lang) => format!("-lang {lang} {float_mode}"),
        None => float_mode.to_owned(),
    }
}

/// Builds a [`WasmJsonContext`] for an in-memory (string) compilation session.
pub(crate) fn wasm_json_context_for_memory_source(
    source_name: &str,
    signals: &SignalCompileOutput,
    compile_options: String,
) -> WasmJsonContext {
    WasmJsonContext {
        filename: Some(source_name_to_filename(source_name)),
        version: Some(Compiler::version().to_owned()),
        compile_options: Some(compile_options),
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
    }
}

/// Builds a [`WasmJsonContext`] for a file-backed compilation session.
///
/// Collects the library list from `signals.parse.used_files` (skipping the
/// primary source) and `signals.loaded_files`, deduplicating entries.
/// Include pathnames are derived by merging `path`'s parent with `search_paths`.
pub(crate) fn wasm_json_context_for_file(
    path: &Path,
    search_paths: &[PathBuf],
    signals: &SignalCompileOutput,
    compile_options: String,
) -> WasmJsonContext {
    let filename = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mut library_list: Vec<String> = signals
        .parse
        .used_files
        .iter()
        .skip(1)
        .map(|file| file.to_string_lossy().into_owned())
        .collect();
    for file in &signals.loaded_files {
        let file = file.to_string_lossy().into_owned();
        if !library_list.iter().any(|existing| existing == &file) {
            library_list.push(file);
        }
    }
    WasmJsonContext {
        filename: Some(filename),
        version: Some(Compiler::version().to_owned()),
        compile_options: Some(compile_options),
        library_list,
        include_pathnames: merge_import_search_paths(path, search_paths)
            .into_iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect(),
        top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
    }
}

/// Converts a compilation metadata snapshot into a flat list of JSON meta entries.
///
/// Each `(key, [v1, v2, ...])` pair in the snapshot becomes one `JsonMetaEntry`
/// per value.  Global keys (without a path prefix) and path-scoped keys are
/// both included.
pub(crate) fn json_meta_entries_from_snapshot(
    snapshot: &CompilationMetadataSnapshot,
) -> Vec<JsonMetaEntry> {
    let mut out = Vec::new();
    for (key, values) in snapshot.entries() {
        let mut values = values.iter();
        let Some(first_value) = values.next() else {
            continue;
        };
        let base_key = match key {
            CompilationMetadataKey::Global { key } => key.as_ref().to_owned(),
            CompilationMetadataKey::Scoped { source_file, key } => {
                format!("{source_file}/{}", key.as_ref())
            }
        };
        out.push(JsonMetaEntry {
            key: base_key.clone(),
            value: first_value.as_ref().to_owned(),
        });
        if base_key == "author" {
            for value in values {
                out.push(JsonMetaEntry {
                    key: "contributor".to_owned(),
                    value: value.as_ref().to_owned(),
                });
            }
        } else {
            for value in values {
                out.push(JsonMetaEntry {
                    key: base_key.clone(),
                    value: value.as_ref().to_owned(),
                });
            }
        }
    }
    out
}

/// Converts compilation metadata into the key/value stream used by the C and
/// C++ `metadata()` callbacks.
///
/// Identity keys are supplied separately by the backend options. Imported-file
/// keys are displayed relative to the master DSP directory when possible,
/// matching C++ `declareMetadata` pathname keys instead of leaking the Rust
/// resolver's canonical absolute path.
pub(crate) fn c_family_meta_entries_from_snapshot(
    source_name: &str,
    snapshot: &CompilationMetadataSnapshot,
) -> Vec<(String, String)> {
    let master_parent = Path::new(source_name)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let master_parent = master_parent
        .canonicalize()
        .unwrap_or_else(|_| master_parent.to_path_buf());
    let mut out = Vec::new();
    for (key, values) in snapshot.entries() {
        let key = match key {
            CompilationMetadataKey::Global { key }
                if matches!(key.as_ref(), "name" | "filename") =>
            {
                continue;
            }
            CompilationMetadataKey::Global { key } => {
                normalize_flat_metadata_key(&master_parent, key)
            }
            CompilationMetadataKey::Scoped { source_file, key } => {
                format!(
                    "{}/{}",
                    metadata_source_path(&master_parent, Path::new(source_file.as_ref())),
                    key.as_ref()
                )
            }
        };
        for value in values {
            out.push((key.clone(), value.as_ref().to_owned()));
        }
    }
    out
}

fn normalize_flat_metadata_key(master_parent: &Path, key: &str) -> String {
    let Some((source_file, suffix)) = key.rsplit_once('/') else {
        return key.to_owned();
    };
    let source_file = Path::new(source_file);
    if !source_file.is_absolute() {
        return key.to_owned();
    }
    format!(
        "{}/{}",
        metadata_source_path(master_parent, source_file),
        suffix
    )
}

fn metadata_source_path(master_parent: &Path, source_path: &Path) -> String {
    if let Ok(relative) = source_path.strip_prefix(master_parent) {
        return metadata_pathname(relative);
    }
    source_path
        .file_name()
        .map(PathBuf::from)
        .as_deref()
        .map(metadata_pathname)
        .unwrap_or_else(|| metadata_pathname(source_path))
}

/// Renders one metadata pathname with the slash separator used by Faust keys.
fn metadata_pathname(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// Returns the value following the first occurrence of any of `names` in a
/// whitespace-tokenized argv slice, e.g. `argv_value(argv, &["-cn"])` on
/// `["-cn", "Probe"]` returns `Some("Probe")`.
pub(crate) fn argv_value<'a>(argv: &'a [String], names: &[&str]) -> Option<&'a str> {
    argv.iter()
        .position(|arg| names.contains(&arg.as_str()))
        .and_then(|position| argv.get(position + 1))
        .map(String::as_str)
}

/// Like [`argv_value`], parsed as `T`. Returns `None` for a missing flag and
/// for an unparsable value alike: a hand-parsed argv string decodes on a
/// best-effort basis, since — unlike the CLI's `clap` parsing — these
/// helpers have no channel for reporting a hard error to the caller.
pub(crate) fn argv_value_parsed<T: std::str::FromStr>(
    argv: &[String],
    names: &[&str],
) -> Option<T> {
    argv_value(argv, names).and_then(|v| v.parse().ok())
}

/// Extracts `-I <path>` search paths from a whitespace-tokenized argv slice.
pub(crate) fn parse_search_paths_from_argv(argv: &[String]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "-I"
            && let Some(p) = argv.get(i + 1)
        {
            paths.push(PathBuf::from(p));
            i += 2;
            continue;
        }
        i += 1;
    }
    paths
}

/// Replaces non-identifier characters so the result is safe as a C/C++ identifier.
pub(crate) fn sanitize_cpp_ident(input: &str) -> String {
    let mut out = String::with_capacity(input.len().max(8));
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("faust_dsp");
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Resolves the canonical root UI label used when the top-level UI group is unnamed.
///
/// Source provenance (C++):
/// - `compiler/generator/compile.cpp`
/// - `compiler/generator/instructions_compiler.cpp`
///
/// Parity rule:
/// - prefer top-level `declare name "..."` metadata from the master document,
/// - otherwise fall back to the source filename stem,
/// - never use the backend class name for UI root labeling.
pub(crate) fn resolve_ui_root_label(
    source_name: &str,
    metadata: &CompilationMetadataSnapshot,
) -> String {
    metadata
        .entries()
        .get(&CompilationMetadataKey::global("name"))
        .and_then(|values| values.iter().next())
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| source_name_to_class(source_name))
}

/// Wraps a single diagnostic into a one-item bundle.
pub(crate) fn bundle_from_diagnostic(diagnostic: Diagnostic) -> DiagnosticBundle {
    let mut diagnostics = DiagnosticBundle::new();
    diagnostics.push(diagnostic);
    diagnostics
}

// ─── Error node extraction ────────────────────────────────────────────────────

/// Returns the offending node id for eval errors that carry one.
pub(crate) fn eval_error_node(error: &eval::EvalError) -> Option<BoxId> {
    match error {
        eval::EvalError::MissingProcessDefinition {
            definitions: node, ..
        }
        | eval::EvalError::UndefinedSymbol { node, .. }
        | eval::EvalError::MalformedDefinitionNode { node }
        | eval::EvalError::MalformedListNode { node }
        | eval::EvalError::MalformedCaseNode { node }
        | eval::EvalError::EmptyArgumentList { node }
        | eval::EvalError::NonIdentifierParameter { node }
        | eval::EvalError::NonIdentifierIterationVariable { node }
        | eval::EvalError::IterationCountNotInt { node }
        | eval::EvalError::PatternArityMismatch { node, .. }
        | eval::EvalError::PatternMatchFailed { node, .. }
        | eval::EvalError::TooManyArguments { node, .. }
        | eval::EvalError::LoopDetected { node } => Some(*node),
        _ => None,
    }
}

/// Returns the offending node id for propagate errors that carry one.
pub(crate) fn propagate_error_node(error: &PropagateError) -> Option<BoxId> {
    match error {
        PropagateError::UnsupportedBox { node, .. }
        | PropagateError::InvalidIntegerValue { node, .. }
        | PropagateError::InputArityMismatch { node, .. }
        | PropagateError::OutputArityMismatch { node, .. }
        | PropagateError::SeqArityMismatch { node, .. }
        | PropagateError::SplitArityMismatch { node, .. }
        | PropagateError::MergeArityMismatch { node, .. }
        | PropagateError::RecArityMismatch { node, .. } => Some(*node),
        _ => None,
    }
}
