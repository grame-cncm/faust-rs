use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
use super::CraneliftBackendError;
use super::{
    AscCodegenError, CCodegenError, CodeboxCodegenError, Compiler, CompilerError, ComputeMode,
    CppCodegenError, DiagnosticBundle, ExpandDspRequest, GenerateAuxFilesRequest, InferenceError,
    InterpCodegenError, InterpCodegenErrorCode, JuliaCodegenError, PropagateError, RealType,
    RustCodegenError, SchedulingStrategy, SignalFirError, SignalFirErrorCode, SignalFirLane,
    SourceReaderError, WasmArtifactRequest, WasmBackendError, build_import_search_paths,
    compile_options_json_string, default_import_search_paths, golden_snapshot, resolve_module_name,
    resolve_ui_root_label,
};
use codegen::backends::wasm::WasmOptions;
use parser::VirtualSourceMap;
use serde_json::Value;

/// Creates a unique temporary directory for one test, keyed by `test_name`,
/// process id and a nanosecond timestamp to avoid collisions across runs.
fn temp_root(test_name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock drift")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "faust_rs_compiler_{test_name}_{}_{}",
        std::process::id(),
        stamp
    ));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

/// Extracts the `include_pathnames` array from a DSP JSON string as paths.
fn json_include_pathnames(dsp_json: &str) -> Vec<PathBuf> {
    let parsed: Value = serde_json::from_str(dsp_json).expect("valid DSP JSON");
    parsed["include_pathnames"]
        .as_array()
        .expect("include_pathnames array")
        .iter()
        .map(|value| {
            PathBuf::from(
                value
                    .as_str()
                    .expect("include_pathnames entries should be strings"),
            )
        })
        .collect()
}

/// Extracts the `library_list` array from a DSP JSON string as paths.
fn json_library_list(dsp_json: &str) -> Vec<PathBuf> {
    let parsed: Value = serde_json::from_str(dsp_json).expect("valid DSP JSON");
    parsed["library_list"]
        .as_array()
        .expect("library_list array")
        .iter()
        .map(|value| {
            PathBuf::from(
                value
                    .as_str()
                    .expect("library_list entries should be strings"),
            )
        })
        .collect()
}

/// Returns the `filename` field of a DSP JSON string, if present.
fn json_filename(dsp_json: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(dsp_json).expect("valid DSP JSON");
    parsed["filename"].as_str().map(str::to_owned)
}

// ── golden helpers ────────────────────────────────────────────────────────

#[test]
fn golden_snapshot_is_stable_for_lf_vs_crlf() {
    let lf = "process = _;\n";
    let crlf = "process = _;\r\n";
    assert_eq!(
        golden_snapshot("pass_through.dsp", lf),
        golden_snapshot("pass_through.dsp", crlf)
    );
}

// ── default_import_search_paths ───────────────────────────────────────────

#[test]
fn default_import_search_paths_starts_with_parent_directory() {
    let path = PathBuf::from("/some/dir/file.dsp");
    let paths = build_import_search_paths(&path, &[], None, None);
    assert_eq!(paths.first(), Some(&PathBuf::from("/some/dir")));
    assert!(paths.contains(&PathBuf::from("/usr/local/share/faust")));
    assert!(paths.contains(&PathBuf::from("/usr/share/faust")));
}

#[test]
fn default_import_search_paths_use_dot_for_bare_filename() {
    let path = PathBuf::from("file.dsp");
    let paths = build_import_search_paths(&path, &[], None, None);
    assert!(
        matches!(paths.first(), Some(first) if first == &PathBuf::from(".") || first == &PathBuf::from("")),
        "expected first search path to stay local for bare filename, got {paths:?}"
    );
}

#[test]
fn import_search_paths_place_explicit_dirs_before_cpp_defaults() {
    let path = PathBuf::from("/project/main.dsp");
    let explicit = [PathBuf::from("/custom/a"), PathBuf::from("/custom/b")];
    let paths = build_import_search_paths(
        &path,
        &explicit,
        Some(OsString::from("/env/faust")),
        Some(PathBuf::from("/opt/faust/bin/faust-rs")),
    );

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/custom/a"),
            PathBuf::from("/custom/b"),
            PathBuf::from("/project"),
            PathBuf::from("/env/faust"),
            PathBuf::from("/opt/faust/share/faust"),
            PathBuf::from("/usr/local/share/faust"),
            PathBuf::from("/usr/share/faust"),
        ]
    );
}

#[test]
fn import_search_paths_expand_native_path_lists_in_order() {
    let path = PathBuf::from("/project/main.dsp");
    let library_dirs = [
        PathBuf::from("/libraries/current"),
        PathBuf::from("/libraries/dx7"),
        PathBuf::from("/libraries/old"),
    ];
    let faust_lib_path = std::env::join_paths(&library_dirs).expect("native path list");
    let paths = build_import_search_paths(&path, &[], Some(faust_lib_path), None);

    assert_eq!(
        &paths[..4],
        [
            PathBuf::from("/project"),
            PathBuf::from("/libraries/current"),
            PathBuf::from("/libraries/dx7"),
            PathBuf::from("/libraries/old"),
        ]
    );
}

#[test]
fn import_search_paths_deduplicate_repeated_entries() {
    let path = PathBuf::from("/project/main.dsp");
    let explicit = [
        PathBuf::from("/project"),
        PathBuf::from("/usr/local/share/faust"),
    ];
    let paths = build_import_search_paths(
        &path,
        &explicit,
        Some(OsString::from("/usr/local/share/faust")),
        Some(PathBuf::from("/usr/local/bin/faust-rs")),
    );

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/project"),
            PathBuf::from("/usr/local/share/faust"),
            PathBuf::from("/usr/share/faust"),
        ]
    );
}

#[test]
fn public_default_import_search_paths_never_return_empty() {
    let path = PathBuf::from("file.dsp");
    let paths = default_import_search_paths(&path);
    assert!(!paths.is_empty());
}

// ── resolve_module_name ───────────────────────────────────────────────────

#[test]
fn resolve_module_name_uses_explicit_class_name() {
    let name = resolve_module_name(Some("MyDsp"), "ignored.dsp");
    assert_eq!(name, "MyDsp");
}

#[test]
fn resolve_module_name_defaults_to_mydsp() {
    let name = resolve_module_name(None, "sine_phasor.dsp");
    assert_eq!(name, "mydsp");
}

#[test]
fn resolve_ui_root_label_prefers_declared_name_metadata() {
    let store = parser::CompilationMetadataStore::new("root.dsp");
    store.declare_top_level("root.dsp", "name", "main");
    let name = resolve_ui_root_label("root.dsp", &store.snapshot());
    assert_eq!(name, "main");
}

#[test]
fn resolve_ui_root_label_falls_back_to_source_stem() {
    let name = resolve_ui_root_label(
        "nested/path/sine_phasor.dsp",
        &parser::CompilationMetadataSnapshot::default(),
    );
    assert_eq!(name, "sine_phasor");
}

// ── Compiler::with_scheduling_strategy (vectorization port plan P2) ──────────
//
// `-ss` / `--scheduling-strategy` travels through the `Compiler` builder into
// `SignalLoweringContext` (and, from there, into `SignalFirOptions`; see
// `transform::signal_fir::tests::signal_fir_options_default_scheduling_strategy_is_depth_first`
// for the receiving end). Scope: these tests check only that the selected
// strategy reaches the lowering context. That it then reorders emitted
// statements is covered by the transform-level scheduling tests.

#[test]
fn compiler_default_scheduling_strategy_is_depth_first() {
    let compiler = Compiler::new();
    let ctx = compiler.lowering_ctx(SignalFirLane::TransformFastLane);
    assert_eq!(ctx.scheduling_strategy, SchedulingStrategy::DepthFirst);
}

#[test]
fn compiler_with_scheduling_strategy_reaches_lowering_context() {
    let compiler =
        Compiler::new().with_scheduling_strategy(SchedulingStrategy::ReverseBreadthFirst);
    let ctx = compiler.lowering_ctx(SignalFirLane::TransformFastLane);
    assert_eq!(
        ctx.scheduling_strategy,
        SchedulingStrategy::ReverseBreadthFirst
    );
}

#[test]
fn compiler_scheduling_strategy_is_independent_of_compute_mode() {
    // Selecting `-vec` (a `ComputeMode`) must not perturb the default
    // scheduling strategy, and selecting a non-default scheduling strategy
    // must not perturb the default (scalar) compute mode.
    let vector_compiler = Compiler::new().with_compute_mode(ComputeMode::Vector {
        vec_size: 64,
        loop_variant: 1,
    });
    let vector_ctx = vector_compiler.lowering_ctx(SignalFirLane::TransformFastLane);
    assert_eq!(
        vector_ctx.scheduling_strategy,
        SchedulingStrategy::DepthFirst
    );

    let scheduled_compiler =
        Compiler::new().with_scheduling_strategy(SchedulingStrategy::BreadthFirst);
    let scheduled_ctx = scheduled_compiler.lowering_ctx(SignalFirLane::TransformFastLane);
    assert_eq!(scheduled_ctx.compute_mode, ComputeMode::Scalar);
}

// ── Compiler::compile_source ──────────────────────────────────────────────

#[test]
fn compiler_compile_source_accepts_valid_dsp() {
    let compiler = Compiler::new();
    let out = compiler
        .compile_source("valid.dsp", "process = _;")
        .expect("valid source should parse");
    assert!(out.root.is_some());
    assert!(out.errors.is_empty());
}

#[test]
fn compiler_double_precision_selects_doubleprecision_library_variant() {
    let compiler = Compiler::new().with_real_type(RealType::Float64);
    let cpp = compiler
        .compile_source_to_cpp(
            "precision_variant.dsp",
            "singleprecision value = 1.0;\ndoubleprecision value = 2.0;\nprocess = value;\n",
            &codegen::backends::cpp::CppOptions::default(),
        )
        .expect("double-precision variant should compile");
    assert!(
        cpp.contains("2.0") || cpp.contains("2."),
        "selected C++ should use the double variant: {cpp}"
    );
    assert!(
        !cpp.contains("1.0"),
        "single-precision variant leaked into double mode: {cpp}"
    );
}

#[test]
fn c_family_source_identity_matches_cpp_and_stays_independent_from_class_name() {
    let compiler = Compiler::new();
    let source = "declare name \"Display DSP\";\nprocess = 0;\n";
    let cpp_options = codegen::backends::cpp::CppOptions {
        class_name: Some("CustomClass".to_owned()),
        ..codegen::backends::cpp::CppOptions::default()
    };
    let cpp = compiler
        .compile_source_to_cpp("nested/source-file.dsp", source, &cpp_options)
        .expect("C++ source identity should compile");
    assert!(cpp.contains("name: \"Display DSP\""));
    assert!(cpp.contains("class CustomClass"));
    assert!(cpp.contains("m->declare(\"filename\", \"source-file.dsp\");"));
    assert!(cpp.contains("m->declare(\"name\", \"Display DSP\");"));

    let c_options = codegen::backends::c::COptions {
        class_name: Some("CustomClass".to_owned()),
        ..codegen::backends::c::COptions::default()
    };
    let c = compiler
        .compile_source_to_c("nested/source-file.dsp", source, &c_options)
        .expect("C source identity should compile");
    assert!(c.contains("name: \"Display DSP\""));
    assert!(c.contains("} CustomClass;"));
    assert!(c.contains("m->declare(m->metaInterface, \"filename\", \"source-file.dsp\");"));
    assert!(c.contains("m->declare(m->metaInterface, \"name\", \"Display DSP\");"));
}

#[test]
fn c_family_source_identity_defaults_to_the_source_basename() {
    let compiler = Compiler::new();
    let cpp = compiler
        .compile_source_to_cpp(
            "nested/plain-name.dsp",
            "process = 0;",
            &codegen::backends::cpp::CppOptions::default(),
        )
        .expect("default C++ identity should compile");
    assert!(cpp.contains("name: \"plain-name\""));
    assert!(cpp.contains("m->declare(\"filename\", \"plain-name.dsp\");"));
    assert!(cpp.contains("m->declare(\"name\", \"plain-name\");"));
}

#[test]
fn compiler_rejects_conflicting_zero_arity_redefinition() {
    let compiler = Compiler::new();
    let err = compiler
        .compile_source_to_signals("multi.dsp", "foo = 1;\nfoo = 2;\nprocess = foo;\n")
        .expect_err("conflicting zero-arity redefinition should fail");
    let CompilerError::Parse { diagnostics, .. } = err else {
        panic!("expected parse error for duplicate zero-arity definition, got {err}");
    };
    assert!(
        diagnostics.as_slice().iter().any(|diag| diag
            .message
            .contains("multiple definitions of symbol 'foo'")),
        "expected multiple-definitions diagnostic, got {diagnostics:?}"
    );
}

#[test]
fn compiler_compile_source_rejects_malformed_dsp() {
    let compiler = Compiler::new();
    let err = compiler
        .compile_source("invalid.dsp", "process = ;")
        .expect_err("malformed source should fail compile facade");
    match err {
        CompilerError::Parse {
            parse_errors,
            diagnostics,
            ..
        } => {
            assert!(parse_errors >= 1);
            assert!(!diagnostics.is_empty());
        }
        other => panic!("expected CompilerError::Parse, got {other:?}"),
    }
}

#[test]
fn compiler_compile_source_to_signals_accepts_custom_entrypoint_name() {
    let compiler = Compiler::new().with_process_name("dsp");
    let out = compiler
        .compile_source_to_signals("custom_entry.dsp", "dsp = _;")
        .expect("custom entrypoint should evaluate and propagate");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
}

#[test]
fn compiler_compile_file_to_signals_loads_component_through_eval_context() {
    let root = temp_root("component_eval_context");
    let entry = root.join("main.dsp");
    let child = root.join("child.dsp");
    fs::write(&entry, "process = component(\"child.dsp\");\n").expect("write entry");
    fs::write(&child, "process = _;\n").expect("write child");

    let compiler = Compiler::new();
    let output = compiler
        .compile_file_default_to_signals(&entry)
        .expect("file-backed compile should load component");

    assert_eq!(output.process_arity.inputs, 1);
    assert_eq!(output.process_arity.outputs, 1);
}

#[test]
fn compiler_preserves_parser_diagnostics_from_a_loaded_component() {
    let root = temp_root("component_parse_diagnostics");
    let entry = root.join("main.dsp");
    let child = root.join("child.dsp");
    fs::write(&entry, "process = component(\"child.dsp\");\n").expect("write entry");
    fs::write(&child, "process = _\n").expect("write malformed child");

    let error = Compiler::new()
        .compile_file_default_to_signals(&entry)
        .expect_err("malformed component must fail");
    let diagnostics = error.diagnostic_bundle();
    let child = child.canonicalize().expect("child should canonicalize");
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .any(|diagnostic| diagnostic.code.0 == "FRS-PARSE-0001"
                && diagnostic
                    .labels
                    .iter()
                    .any(|label| label.span.file == child)),
        "the original parser code and child-source label must survive: {diagnostics:?}"
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn compiler_compile_file_to_signals_aggregates_component_metadata() {
    let root = temp_root("component_metadata");
    let entry = root.join("main.dsp");
    let child = root.join("child.dsp");
    fs::write(&entry, "process = component(\"child.dsp\");\n").expect("write entry");
    fs::write(&child, "declare author \"child-author\";\nprocess = _;\n").expect("write child");

    let compiler = Compiler::new();
    let output = compiler
        .compile_file_default_to_signals(&entry)
        .expect("file-backed compile should aggregate metadata");

    let key = parser::CompilationMetadataKey::scoped(
        child
            .canonicalize()
            .expect("child should canonicalize")
            .to_string_lossy()
            .into_owned(),
        "author",
    );
    let values = output
        .compilation_metadata
        .entries()
        .get(&key)
        .expect("component metadata should exist in final compiler output");
    assert!(values.contains("child-author"));
}

#[test]
fn c_family_callbacks_replay_master_and_component_metadata_with_relative_keys() {
    let root = temp_root("c_family_metadata");
    let entry = root.join("main.dsp");
    let child = root.join("child.dsp");
    fs::write(
        &entry,
        "declare license \"MIT\";\nprocess = component(\"child.dsp\");\n",
    )
    .expect("write entry");
    fs::write(&child, "declare author \"child-author\";\nprocess = _;\n").expect("write child");

    let compiler = Compiler::new();
    let cpp = compiler
        .compile_file_default_to_cpp(&entry, &codegen::backends::cpp::CppOptions::default())
        .expect("C++ metadata callback should compile");
    assert!(cpp.contains("m->declare(\"child.dsp/author\", \"child-author\");"));
    assert!(cpp.contains("m->declare(\"license\", \"MIT\");"));
    assert!(!cpp.contains(&root.to_string_lossy().into_owned()));

    let c = compiler
        .compile_file_default_to_c(&entry, &codegen::backends::c::COptions::default())
        .expect("C metadata callback should compile");
    assert!(c.contains("m->declare(m->metaInterface, \"child.dsp/author\", \"child-author\");"));
    assert!(c.contains("m->declare(m->metaInterface, \"license\", \"MIT\");"));
    assert!(!c.contains(&root.to_string_lossy().into_owned()));

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn compiler_compile_source_to_wasm_emits_magic_header() {
    let compiler = Compiler::new();
    let out = compiler
        .compile_source_to_wasm("zero.dsp", "process = 0;", &WasmOptions::default())
        .expect("WASM scaffold should compile from source");
    assert!(out.wasm_binary.starts_with(b"\0asm"));
    assert!(out.dsp_json.contains("\"size\": "));
    assert!(out.dsp_json.contains("\"ui\": ["));
    assert!(out.dsp_json.contains("\"filename\": \"zero.dsp\""));
    assert!(
        out.dsp_json
            .contains(&format!("\"version\": \"{}\"", Compiler::version()))
    );
    assert!(out.dsp_json.contains(&format!(
        "\"compile_options\": \"{}\"",
        compile_options_json_string(Some("wasm"), false)
    )));
}

#[test]
fn compiler_compile_wasm_artifact_returns_matched_wasm_and_json_pair() {
    let compiler = Compiler::new();
    let request = WasmArtifactRequest::new("zero.dsp", "process = 0;");
    let out = compiler
        .compile_wasm_artifact(&request)
        .expect("artifact compile should succeed");

    assert!(out.wasm_bytes.starts_with(b"\0asm"));
    assert_eq!(json_filename(&out.dsp_json).as_deref(), Some("zero.dsp"));
    assert_eq!(
        out.compile_options,
        compile_options_json_string(Some("wasm"), false)
    );
    assert!(
        out.dsp_json
            .contains(&format!("\"compile_options\": \"{}\"", out.compile_options))
    );
}

#[test]
fn timing_helper_without_sink_runs_without_measuring() {
    let mut called = false;
    let value = super::time_phase_with_sink(None, "test-phase", || {
        called = true;
        42
    });

    assert!(called);
    assert_eq!(value, 42);
}

#[test]
fn wasm_artifact_request_defaults_to_transform_fastlane() {
    let request = WasmArtifactRequest::new("zero.dsp", "process = 0;");
    assert_eq!(request.lane, SignalFirLane::TransformFastLane);
}

#[test]
fn compiler_compile_wasm_artifact_supports_memory_source_import_dirs() {
    let root = temp_root("wasm_artifact_memory_import_dirs");
    let child = root.join("child.lib");
    fs::write(&child, "process = _;\n").expect("write child");

    let compiler = Compiler::new();
    let mut request = WasmArtifactRequest::new("main.dsp", "process = component(\"child.lib\");");
    request.import_dirs.push(root.clone());
    let out = compiler
        .compile_wasm_artifact(&request)
        .expect("artifact compile with import dirs should succeed");

    assert!(out.wasm_bytes.starts_with(b"\0asm"));
    assert!(out.dsp_json.contains("child.lib"));
    let include_pathnames = json_include_pathnames(&out.dsp_json);
    assert!(include_pathnames.contains(&root), "{include_pathnames:?}");
}

#[test]
fn compiler_compile_wasm_artifact_supports_virtual_faust_library_bundle() {
    let compiler = Compiler::new();
    let mut request = WasmArtifactRequest::new(
        "main.dsp",
        "import(\"stdfaust.lib\");\nprocess = os.freq;\n",
    );
    request.virtual_sources = VirtualSourceMap::new([
        (
            PathBuf::from("stdfaust.lib"),
            "os = library(\"osc.lib\");\n".to_owned(),
        ),
        (PathBuf::from("osc.lib"), "freq = 440;\n".to_owned()),
    ]);
    let out = compiler
        .compile_wasm_artifact(&request)
        .expect("artifact compile with virtual libraries should succeed");

    assert!(out.wasm_bytes.starts_with(b"\0asm"));
    let library_list = json_library_list(&out.dsp_json);
    assert!(library_list.contains(&PathBuf::from("stdfaust.lib")));
    assert!(library_list.contains(&PathBuf::from("osc.lib")));
}

#[test]
fn compiler_compile_wasm_artifact_keeps_ui_for_memory_source_without_extension() {
    let compiler = Compiler::new();
    let source = "process = *(hslider(\"gain\", 0.5, 0.0, 1.0, 0.01));";
    let strict_json = compiler
        .compile_source_to_json("gain", source)
        .expect("strict JSON should preserve UI controls");
    let request = WasmArtifactRequest::new("gain", source);
    let out = compiler
        .compile_wasm_artifact(&request)
        .expect("artifact compile should preserve UI controls");

    assert!(strict_json.contains("\"filename\": \"gain\""));
    assert!(strict_json.contains("\"label\": \"gain\""));
    assert!(strict_json.contains("\"type\": \"hslider\""));
    assert!(strict_json.contains("\"address\": \"/gain/gain\""));
    assert!(out.wasm_bytes.starts_with(b"\0asm"));
    assert_eq!(json_filename(&out.dsp_json).as_deref(), Some("gain"));
    assert!(out.dsp_json.contains("\"label\": \"gain\""));
    assert!(out.dsp_json.contains("\"type\": \"hslider\""));
    assert!(out.dsp_json.contains("\"address\": \"/gain/gain\""));
}

#[test]
fn compiler_memory_eval_source_context_preserves_ui_widgets() {
    let compiler = Compiler::new();
    let source = "process = *(hslider(\"gain\", 0.5, 0.0, 1.0, 0.01));";
    let store_without_ctx = parser::CompilationMetadataStore::new("gain");
    let store_with_ctx = parser::CompilationMetadataStore::new("gain");
    let output_without_ctx =
        parser::parse_program_with_metadata(source, "gain", store_without_ctx.clone());
    let output_with_ctx =
        parser::parse_program_with_metadata(source, "gain", store_with_ctx.clone());

    let without_ctx = compiler
        .pipeline_to_signals("gain", output_without_ctx, None)
        .expect("pipeline without source context should succeed");
    let with_ctx = compiler
        .pipeline_to_signals(
            "gain",
            output_with_ctx,
            Some(eval::EvalSourceContext::memory_with_metadata(
                store_with_ctx,
            )),
        )
        .expect("pipeline with memory source context should succeed");

    assert!(
        !without_ctx.ui.controls.is_empty(),
        "pipeline without source context should preserve widget UI"
    );
    assert_eq!(
        with_ctx.ui.controls.len(),
        without_ctx.ui.controls.len(),
        "memory source context should not change widget UI extraction"
    );
}

#[test]
fn compiler_compile_file_to_wasm_emits_file_provenance_fields() {
    let root = temp_root("wasm_json_provenance");
    let entry = root.join("main.dsp");
    let child = root.join("child.lib");
    fs::write(
        &entry,
        "declare name \"Main DSP\";\nprocess = component(\"child.lib\");\n",
    )
    .expect("write entry");
    fs::write(&child, "process = _;\n").expect("write child");

    let compiler = Compiler::new();
    let out = compiler
        .compile_file_default_to_wasm(&entry, &WasmOptions::default())
        .expect("file-backed WASM compile should succeed");

    assert!(out.dsp_json.contains("\"name\": \"Main DSP\""));
    assert_eq!(json_filename(&out.dsp_json).as_deref(), Some("main.dsp"));
    assert!(
        out.dsp_json
            .contains(&format!("\"version\": \"{}\"", Compiler::version()))
    );
    let library_list = json_library_list(&out.dsp_json);
    assert!(
        library_list.contains(&child),
        "library_list should include the imported file: {library_list:?}"
    );
    let include_pathnames = json_include_pathnames(&out.dsp_json);
    assert!(
        include_pathnames.contains(&root),
        "include_pathnames should include the source directory: {include_pathnames:?}"
    );
}

#[test]
fn compiler_compile_file_to_wasm_artifact_preserves_file_provenance_and_options() {
    let root = temp_root("wasm_artifact_file_provenance");
    let entry = root.join("main.dsp");
    let child = root.join("child.lib");
    fs::write(
        &entry,
        "declare name \"Main DSP\";\nprocess = component(\"child.lib\");\n",
    )
    .expect("write entry");
    fs::write(&child, "process = _;\n").expect("write child");

    let compiler = Compiler::new();
    let out = compiler
        .compile_file_default_to_wasm_artifact(&entry, &WasmOptions::default())
        .expect("file-backed artifact compile should succeed");

    assert!(out.wasm_bytes.starts_with(b"\0asm"));
    assert_eq!(
        out.compile_options,
        compile_options_json_string(Some("wasm"), false)
    );
    assert_eq!(json_filename(&out.dsp_json).as_deref(), Some("main.dsp"));
    let library_list = json_library_list(&out.dsp_json);
    assert!(library_list.contains(&child), "{library_list:?}");
    assert!(
        out.dsp_json
            .contains(&format!("\"compile_options\": \"{}\"", out.compile_options))
    );
}

#[test]
fn compiler_compile_source_to_json_emits_strict_json_without_widget_indices() {
    let compiler = Compiler::new();
    let json = compiler
            .compile_source_to_json(
                "gain.dsp",
                "declare name \"Gain\";\ngain = hslider(\"gain\", 0.5, 0, 1, 0.01);\nprocess = _ * gain;\n",
            )
            .expect("strict JSON should compile from source");

    assert!(json.contains("\"name\": \"Gain\""));
    assert!(json.contains("\"filename\": \"gain.dsp\""));
    assert!(json.contains("\"ui\": ["));
    assert!(json.contains(&format!(
        "\"compile_options\": \"{}\"",
        compile_options_json_string(None, false)
    )));
    assert!(!json.contains("\"index\":"));
}

#[test]
fn compile_options_json_string_tracks_lang_and_float_mode() {
    assert_eq!(
        compile_options_json_string(Some("wasm"), false),
        "-lang wasm -single"
    );
    assert_eq!(
        compile_options_json_string(Some("wasm"), true),
        "-lang wasm -double"
    );
    assert_eq!(
        compile_options_json_string(Some("cpp"), false),
        "-lang cpp -single"
    );
    assert_eq!(compile_options_json_string(None, false), "-single");
    assert_eq!(compile_options_json_string(None, true), "-double");
}

#[test]
fn compiler_get_faustwasm_info_supports_cpp_directory_keys() {
    let compiler = Compiler::new();

    assert_eq!(
        compiler
            .get_faustwasm_info("version")
            .expect("version should be supported"),
        Compiler::version()
    );
    let help = compiler
        .get_faustwasm_info("help")
        .expect("help should be supported");
    assert!(help.contains("supported keys"));
    assert!(help.contains("- libdir"));
    assert!(help.contains("- pathslist"));

    let libdir = compiler
        .get_faustwasm_info("libdir")
        .expect("libdir should be supported");
    assert!(libdir.ends_with("/lib\n") || libdir.ends_with("\\lib\n"));

    let includedir = compiler
        .get_faustwasm_info("includedir")
        .expect("includedir should be supported");
    assert!(includedir.ends_with("/include\n") || includedir.ends_with("\\include\n"));

    let archdir = compiler
        .get_faustwasm_info("archdir")
        .expect("archdir should be supported");
    assert!(archdir.contains("share"));
    assert!(archdir.ends_with("/faust\n") || archdir.ends_with("\\faust\n"));

    let dspdir = compiler
        .get_faustwasm_info("dspdir")
        .expect("dspdir should be supported");
    assert_eq!(dspdir, archdir);

    let pathslist = compiler
        .get_faustwasm_info("pathslist")
        .expect("pathslist should be supported");
    assert!(pathslist.contains("FAUST dsp library paths:"));
    assert!(pathslist.contains("FAUST architectures paths:"));

    let invalid = compiler
        .get_faustwasm_info("wat")
        .expect_err("unknown keys should be rejected");
    assert!(invalid.message.contains("incorrect argument"));
}

#[test]
fn compiler_expand_dsp_returns_source_when_valid() {
    let compiler = Compiler::new();
    let source = "process = 0;".to_owned();
    let expanded = compiler
        .expand_dsp(&ExpandDspRequest {
            source_name: "zero.dsp".to_owned(),
            source: source.clone(),
            args: String::new(),
        })
        .expect("expand_dsp should succeed for valid source");
    assert_eq!(expanded, source);
}

#[test]
fn compiler_expand_dsp_fails_for_invalid_source() {
    let compiler = Compiler::new();
    let err = compiler
        .expand_dsp(&ExpandDspRequest {
            source_name: "bad.dsp".to_owned(),
            source: "process = undefined_symbol;".to_owned(),
            args: String::new(),
        })
        .expect_err("expand_dsp should fail for invalid source");
    assert_eq!(err.code, crate::FaustwasmServiceErrorCode::Unsupported);
}

#[test]
fn compiler_cranelift_report_matches_the_cli_contract() {
    // The facade entry point must reproduce the report the CLI has always
    // printed for `--lang cranelift`, including the module name derived from
    // the source (NOT the "mydsp" class-name default) — the FIR dump and the
    // report name the same module, so the two must agree.
    let compiler = Compiler::new();
    let report = compiler
        .compile_source_to_cranelift_report(
            "gain.dsp",
            "process = _ * 0.5;",
            &codegen::backends::cranelift::CraneliftOptions::default(),
        )
        .expect("cranelift report generation should succeed");

    assert!(report.starts_with("backend: cranelift (experimental)\n"));
    assert!(report.contains("\nmodule: gain\n"), "{report}");
    assert!(
        report.contains("\ncompute_symbol: gain::compute\n"),
        "{report}"
    );
    assert!(report.contains("\ncompute_entry_addr: 0x"), "{report}");
    assert!(report.contains("\ndsp_struct_layout: "), "{report}");

    // A DSP source that does not compile surfaces as a normal CompilerError,
    // not a panic out of the JIT layer.
    let err = compiler
        .compile_source_to_cranelift_report(
            "bad.dsp",
            "process = undefined_thing;",
            &codegen::backends::cranelift::CraneliftOptions::default(),
        )
        .expect_err("an undefined symbol must fail the cranelift report");
    assert!(err.to_string().contains("undefined symbol"), "{err}");
}

#[test]
fn compiler_generate_aux_files_no_flags_returns_empty() {
    let compiler = Compiler::new();
    let artifacts = compiler
        .generate_aux_files(&GenerateAuxFilesRequest {
            source_name: "zero.dsp".to_owned(),
            source: "process = 0;".to_owned(),
            args: String::new(),
            ..Default::default()
        })
        .expect("generate_aux_files should succeed with no flags");
    assert!(artifacts.is_empty());
}

#[test]
fn compiler_generate_aux_files_json_flag_produces_json_artifact() {
    let compiler = Compiler::new();
    let artifacts = compiler
        .generate_aux_files(&GenerateAuxFilesRequest {
            source_name: "zero.dsp".to_owned(),
            source: "process = 0;".to_owned(),
            args: "-json".to_owned(),
            ..Default::default()
        })
        .expect("generate_aux_files with -json should succeed");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].path, "zero.json");
    assert!(!artifacts[0].binary);
    let text = std::str::from_utf8(&artifacts[0].content).expect("json must be utf-8");
    assert!(text.contains("\"name\""));
}

#[test]
fn compiler_generate_aux_files_cpp_flag_produces_cpp_artifact() {
    let compiler = Compiler::new();
    let artifacts = compiler
        .generate_aux_files(&GenerateAuxFilesRequest {
            source_name: "zero.dsp".to_owned(),
            source: "process = 0;".to_owned(),
            args: "-cpp".to_owned(),
            ..Default::default()
        })
        .expect("generate_aux_files with -cpp should succeed");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].path, "zero.cpp");
    assert!(!artifacts[0].binary);
}

#[test]
fn compiler_generate_aux_files_never_emits_two_artifacts_at_one_path() {
    // Every requested output owns a distinct path. `-wasm` already emits the
    // companion `<stem>.json` matched to the module, so combining it with
    // `-json` must NOT produce a second `zero.json`: a host writing the
    // artifacts out in order would otherwise silently overwrite one file.
    let artifacts = Compiler::new()
        .generate_aux_files(&GenerateAuxFilesRequest {
            source_name: "zero.dsp".to_owned(),
            source: "process = 0;".to_owned(),
            args: "-cpp -c -wasm -json".to_owned(),
            ..Default::default()
        })
        .expect("generate_aux_files with every output flag should succeed");

    let mut paths: Vec<&str> = artifacts.iter().map(|a| a.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["zero.c", "zero.cpp", "zero.json", "zero.wasm"]);

    // The one JSON present is the wasm companion, so it must describe a wasm
    // build rather than the standalone strict-JSON output.
    let json = artifacts
        .iter()
        .find(|a| a.path == "zero.json")
        .expect("json artifact");
    let text = std::str::from_utf8(&json.content).expect("json must be utf-8");
    assert!(
        text.contains("\"compile_options\": \"-lang wasm"),
        "expected the wasm companion JSON, got:\n{text}"
    );
}

#[test]
fn compiler_generate_aux_files_applies_long_double_flag_to_the_wasm_backend() {
    // `--double` is the CLI-normalized spelling of `-double`. It must reach
    // the WASM backend options too, not only the internal real type, or the
    // emitted module would compute in double while its JSON advertises
    // single precision.
    let json_of = |args: &str| {
        let artifacts = Compiler::new()
            .generate_aux_files(&GenerateAuxFilesRequest {
                source_name: "zero.dsp".to_owned(),
                source: "process = 0;".to_owned(),
                args: args.to_owned(),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("generate_aux_files({args}) must succeed: {e}"));
        let json = artifacts
            .into_iter()
            .find(|a| a.path == "zero.json")
            .expect("wasm companion json");
        String::from_utf8(json.content).expect("json must be utf-8")
    };

    assert!(json_of("-wasm").contains("-single"));
    for spelling in ["-wasm -double", "-wasm --double"] {
        let json = json_of(spelling);
        assert!(
            json.contains("-double") && !json.contains("-single"),
            "{spelling} must select double precision:\n{json}"
        );
    }
}

#[test]
fn compiler_generate_aux_files_emits_assemblyscript_for_lang_asc() {
    let compiler = Compiler::new();
    let files = compiler
        .generate_aux_files(&GenerateAuxFilesRequest {
            source_name: "gain.dsp".to_owned(),
            source: "process = _ * 0.5;".to_owned(),
            args: "-lang asc -cn Probe -o /Probe.ts".to_owned(),
            virtual_sources: VirtualSourceMap::default(),
        })
        .expect("asc aux-file generation should succeed");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "/Probe.ts");
    assert!(!files[0].binary);
    let text = String::from_utf8(files[0].content.clone()).expect("utf-8 asc source");
    assert!(text.contains("export class Probe"));
    assert!(text.contains("compute(count: i32"));
}

#[test]
fn identical_widgets_under_distinct_groups_stay_distinct() {
    // Regression: the same widget box reached under DIFFERENT interpolated
    // group labels must produce distinct controls (C++ threads the group
    // path into widget signals). A box-id-only UI dedupe collapses all three
    // sliders into one (observed as DX7 operators losing their parameter
    // groups).
    let compiler = Compiler::new();
    let json = compiler
        .compile_source_to_json(
            "lbl.dsp",
            "process = par(i, 3, vgroup(\"Op %i\", hslider(\"g\", 0, 0, 1, 0.1))) :> _;",
        )
        .expect("compiles");
    for address in ["/lbl/Op 0/g", "/lbl/Op 1/g", "/lbl/Op 2/g"] {
        assert!(
            json.contains(&format!("\"address\": \"{address}\"")),
            "missing widget address {address} in JSON:\n{json}"
        );
    }
}

#[test]
fn ui_children_sort_lexicographically_by_raw_label() {
    // C++ parity: group children are ordered by the RAW label including the
    // "[n] " ordering prefix, using plain byte-wise comparison ("[10]" sorts
    // before "[2]"; unnumbered labels interleave by their own spelling).
    // Reference: C++ faust JSON for this exact source.
    let compiler = Compiler::new();
    let json = compiler
        .compile_source_to_json(
            "ord.dsp",
            "process = hgroup(\"top\", hslider(\"[10] b\",0,0,1,0.1) + hslider(\"[2] a\",0,0,1,0.1) + hslider(\"zz\",0,0,1,0.1) + hslider(\"Aa\",0,0,1,0.1));",
        )
        .expect("compiles");
    let order: Vec<usize> = [
        "\"label\": \"Aa\"",
        "\"label\": \"b\"",
        "\"label\": \"a\"",
        "\"label\": \"zz\"",
    ]
    .iter()
    .map(|needle| json.find(needle).expect("label present"))
    .collect();
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "expected Aa, b, a, zz order in JSON:\n{json}"
    );
}

#[test]
fn replicated_widgets_wire_distinct_dsp_fields() {
    // Regression for the widget-identity collapse: UI extraction listed every
    // replicated control, but the SIGNAL path aliased same-box widgets across
    // group contexts to one control — a DX7 voice wired only 34 of 147
    // parameters (envelopes stuck at init values; a piano patch degenerated
    // into a percussive thump). UI-level assertions cannot catch this, so
    // check the generated code: three group instances must produce three
    // DSP fields, like the C++ compiler.
    let compiler = Compiler::new();
    let cpp = compiler
        .compile_source_to_cpp(
            "rep.dsp",
            "process = par(i, 3, vgroup(\"Op %i\", hslider(\"g\", 0, 0, 1, 0.1))) :> _;",
            &codegen::backends::cpp::CppOptions::default(),
        )
        .expect("compiles");
    for field in ["fHslider0", "fHslider1", "fHslider2"] {
        assert!(
            cpp.contains(&format!("FAUSTFLOAT {field};")),
            "missing DSP field {field} — replicated widgets collapsed:\n{cpp}"
        );
    }
}

#[test]
fn ad_seed_references_unify_to_one_control() {
    // Counterpart guard: a widget referenced from a `fad(...)` SEED is a
    // differentiation parameter — every reference (body and seed, in any
    // group context) must resolve to the SAME control, even though the body
    // reference sits inside a group and the seed is walked context-free.
    let compiler = Compiler::new();
    let cpp = compiler
        .compile_source_to_cpp(
            "seed.dsp",
            "g = hslider(\"g\", 0.5, 0, 1, 0.01);\nprocess = +~vgroup(\"fb\", fad(*(g), g));",
            &codegen::backends::cpp::CppOptions::default(),
        )
        .expect("compiles");
    assert!(cpp.contains("FAUSTFLOAT fHslider0;"));
    assert!(
        !cpp.contains("FAUSTFLOAT fHslider1;"),
        "fad seed reference forked into a second control:\n{cpp}"
    );
}

#[test]
fn compiler_error_source_classification_covers_every_variant() {
    fn assert_source_is<T>(error: CompilerError)
    where
        T: std::error::Error + 'static,
    {
        let source = std::error::Error::source(&error).expect("typed variant must expose a source");
        assert!(
            source.is::<T>(),
            "unexpected source type for compiler error: {error:?}"
        );
        let _ = error.diagnostic_bundle();
    }

    fn assert_has_no_source(error: CompilerError) {
        assert!(
            std::error::Error::source(&error).is_none(),
            "aggregate-only variant must not invent a source: {error:?}"
        );
        let _ = error.diagnostic_bundle();
    }

    let empty = || DiagnosticBundle::new();
    assert_source_is::<SourceReaderError>(CompilerError::import(SourceReaderError::ImportCycle {
        path: PathBuf::from("cycle.dsp"),
        cycle: Vec::new(),
    }));
    assert_source_is::<eval::EvalError>(CompilerError::Eval {
        source: "eval.dsp".into(),
        error: Box::new(eval::EvalError::NegativeIterationCount { value: -1 }),
        diagnostics: empty(),
    });
    assert_source_is::<PropagateError>(CompilerError::Propagate {
        source: "propagate.dsp".into(),
        error: PropagateError::NegativeIntegerValue {
            field: "index",
            value: -1,
        },
        diagnostics: empty(),
    });
    assert_source_is::<InferenceError>(CompilerError::Type {
        source: "type.dsp".into(),
        error: Box::new(InferenceError::SignalStructure {
            signal: None,
            context: "type failure".into(),
        }),
        diagnostics: empty(),
    });
    assert_source_is::<crate::execution::ExecutionOptionsError>(CompilerError::ExecutionOptions {
        source: "options.dsp".into(),
        error: crate::execution::ExecutionOptionsError::OneSampleWithVectorMode,
        diagnostics: empty(),
    });
    assert_source_is::<SignalFirError>(CompilerError::Transform {
        source: "transform.dsp".into(),
        error: Box::new(SignalFirError::new(
            SignalFirErrorCode::EmptySignalList,
            "empty",
        )),
        diagnostics: empty(),
    });
    assert_source_is::<CppCodegenError>(CompilerError::CodegenCpp {
        source: "cpp.dsp".into(),
        error: CppCodegenError::new(
            codegen::backends::cpp::CodegenErrorCode::RootNotModule,
            "root",
        ),
        diagnostics: empty(),
    });
    assert_source_is::<CCodegenError>(CompilerError::CodegenC {
        source: "c.dsp".into(),
        error: CCodegenError::new(
            codegen::backends::c::CodegenErrorCode::RootNotModule,
            "root",
        ),
        diagnostics: empty(),
    });
    assert_source_is::<JuliaCodegenError>(CompilerError::CodegenJulia {
        source: "julia.dsp".into(),
        error: JuliaCodegenError::new(
            codegen::backends::julia::CodegenErrorCode::RootNotModule,
            "root",
        ),
        diagnostics: empty(),
    });
    assert_source_is::<AscCodegenError>(CompilerError::CodegenAsc {
        source: "asc.dsp".into(),
        error: AscCodegenError::new(
            codegen::backends::asc::CodegenErrorCode::RootNotModule,
            "root",
        ),
        diagnostics: empty(),
    });
    assert_source_is::<CodeboxCodegenError>(CompilerError::CodegenCodebox {
        source: "codebox.dsp".into(),
        error: CodeboxCodegenError::new(
            codegen::backends::codebox::CodegenErrorCode::RootNotModule,
            "root",
        ),
        diagnostics: empty(),
    });
    assert_source_is::<RustCodegenError>(CompilerError::CodegenRust {
        source: "rust.dsp".into(),
        error: RustCodegenError::new(
            codegen::backends::rust::CodegenErrorCode::RootNotModule,
            "root",
        ),
        diagnostics: empty(),
    });
    assert_source_is::<InterpCodegenError>(CompilerError::CodegenInterp {
        source: "interp.dsp".into(),
        error: InterpCodegenError {
            code: InterpCodegenErrorCode::RootNotModule,
            message: "root".to_owned(),
        },
        diagnostics: empty(),
    });
    #[cfg(not(target_arch = "wasm32"))]
    assert_source_is::<CraneliftBackendError>(CompilerError::CodegenCranelift {
        source: "cranelift.dsp".into(),
        error: CraneliftBackendError {
            code: codegen::backends::cranelift::CraneliftBackendErrorCode::UnsupportedModuleShape,
            message: "root".to_owned(),
        },
        diagnostics: empty(),
    });
    assert_source_is::<WasmBackendError>(CompilerError::CodegenWasm {
        source: "wasm.dsp".into(),
        error: WasmBackendError::new(
            codegen::backends::wasm::WasmBackendErrorCode::UnsupportedModuleShape,
            "root",
        ),
        diagnostics: empty(),
    });

    assert_has_no_source(CompilerError::missing_root("missing.dsp"));
    assert_has_no_source(CompilerError::Parse {
        source: "parse.dsp".into(),
        parse_errors: 1,
        recoveries: 0,
        diagnostics: empty(),
    });
    assert_has_no_source(CompilerError::FirVerify {
        source: "fir.dsp".into(),
        strict: false,
        diagnostics: empty(),
    });
}

/// `CompilerError` is returned by value from every facade entry point, so its
/// width is a real cost and `clippy::result_large_err` rejects it past 128
/// bytes. That check runs per platform, and `PathBuf` is eight bytes wider on
/// Windows than on Unix — which is how this enum once passed CI on Linux and
/// macOS while failing on Windows at exactly the threshold.
///
/// The bound below leaves headroom for that difference so the failure surfaces
/// here, on any developer machine, instead of only on one CI runner. If a new
/// variant trips it, box the payload rather than raising the number: the
/// diagnostic bundle every variant already carries is the floor, and widening
/// past it costs every `Result` in the pipeline.
#[test]
fn compiler_error_stays_narrow_enough_for_every_platform() {
    const MAX_BYTES: usize = 112;
    let actual = std::mem::size_of::<CompilerError>();
    assert!(
        actual <= MAX_BYTES,
        "CompilerError grew to {actual} bytes (budget {MAX_BYTES}); \
         box the widest variant's payload instead of raising the budget"
    );
}
