//! Unit tests for extracted `xtask` workflow helpers.
//!
//! These tests stay in a separate module so `main.rs` can remain a small command
//! facade while still exercising option parsing, trace serialization, and ABI
//! export validation helpers.

use super::*;
use clap::{CommandFactory, Parser};

fn parse_xtask<I, T>(args: I) -> Result<XtaskCli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    XtaskCli::try_parse_from(
        std::iter::once(OsString::from("xtask")).chain(args.into_iter().map(Into::into)),
    )
}

fn parse_corpus_query<I, T>(args: I) -> Result<CorpusStatusQueryOptions, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let cli = parse_xtask(
        std::iter::once(OsString::from("corpus-status-query"))
            .chain(args.into_iter().map(Into::into)),
    )?;
    let XtaskCommand::CorpusStatusQuery(args) = cli.command else {
        unreachable!("requested corpus-status-query")
    };
    Ok(args.into())
}

#[test]
fn clap_definition_is_consistent() {
    XtaskCli::command().debug_assert();
}

#[test]
fn ci_command_names_are_accepted() {
    for command in [
        "cpp-backend-diff-report",
        "golden-check",
        "ffi-boundary-check",
        "structure-check",
        "vector-coverage-check",
        "vector-interp-opt-check",
        "compile-budget-check",
        "corpus-runtime-diff",
    ] {
        parse_xtask([command]).unwrap_or_else(|error| panic!("{command}: {error}"));
    }
    parse_xtask(["backend-align-smoke", "--skip-golden"]).unwrap();
}

#[test]
fn generated_reports_replace_checkout_local_paths() {
    let workspace_case = workspace_root().join("tests/corpus/example.dsp");
    let cpp_binary = Path::new(CPP_SOURCE_ROOT).join("build/bin/faust");
    let text = format!(
        "case={} cpp={} expr=clocked(0x12Ab90, IN[0])",
        workspace_case.display(),
        cpp_binary.display()
    );
    let portable = portable_report_text(&text);
    assert_eq!(
        portable,
        "case=tests/corpus/example.dsp cpp=../faust/build/bin/faust \
         expr=clocked(<ptr>, IN[0])"
    );
}

#[test]
fn generated_reports_normalize_windows_path_separators() {
    let portable =
        portable_report_text("case=tests\\corpus\\example.dsp cpp=..\\faust\\build\\bin\\faust");
    assert_eq!(
        portable,
        "case=tests/corpus/example.dsp cpp=../faust/build/bin/faust"
    );
}

#[test]
fn unknown_command_is_a_clap_error() {
    let error = parse_xtask(["definitely-unknown"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

#[test]
fn golden_gen_cpp_preserves_hyphenated_passthrough_arguments() {
    let cli = parse_xtask(["golden-gen-cpp", "--", "-vec", "-I", "some dir"]).unwrap();
    let XtaskCommand::GoldenGenCpp(args) = cli.command else {
        unreachable!("requested golden-gen-cpp")
    };
    assert_eq!(
        args.extra_args,
        [
            OsString::from("-vec"),
            OsString::from("-I"),
            OsString::from("some dir")
        ]
    );
}

#[cfg(unix)]
#[test]
fn golden_gen_cpp_passthrough_preserves_non_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let opaque = OsString::from_vec(vec![b'-', 0xff, b'x']);
    let cli = parse_xtask([
        OsString::from("golden-gen-cpp"),
        OsString::from("--"),
        opaque.clone(),
    ])
    .unwrap();
    let XtaskCommand::GoldenGenCpp(args) = cli.command else {
        unreachable!("requested golden-gen-cpp")
    };
    assert_eq!(args.extra_args, [opaque]);
}

#[test]
fn trace_scenarios_and_lane_aliases_are_clap_values() {
    for scenario in ["zeros", "impulse", "ramp", "sine"] {
        parse_xtask([
            "interp-trace-dump",
            "--case",
            "test.dsp",
            "--scenario",
            scenario,
            "--lane",
            "transform",
        ])
        .unwrap();
    }
}

#[test]
fn parse_interp_trace_dump_defaults_and_required_case() {
    let cli = parse_xtask([
        "interp-trace-dump",
        "--case",
        "tests/corpus/rep_31_extended_primitives.dsp",
    ])
    .unwrap();
    let XtaskCommand::InterpTraceDump(args) = cli.command else {
        unreachable!("requested interp-trace-dump")
    };
    let opts = InterpTraceDumpOptions::from(args);
    assert_eq!(opts.scenario, TraceScenario::Zeros);
    assert_eq!(opts.lane, TraceLane::Fast);
    assert_eq!(opts.sample_rate, 48_000);
    assert_eq!(opts.block_size, 64);
    assert_eq!(opts.num_blocks, 4);
    assert!(!opts.strict_fir_types);
}

#[test]
fn parse_interp_trace_dump_accepts_strict_fir_types_flag() {
    let cli = parse_xtask([
        "interp-trace-dump",
        "--case",
        "tests/runtime_corpus/trace_01_passthrough.dsp",
        "--strict-fir-types",
    ])
    .unwrap();
    let XtaskCommand::InterpTraceDump(args) = cli.command else {
        unreachable!("requested interp-trace-dump")
    };
    let opts = InterpTraceDumpOptions::from(args);
    assert!(opts.strict_fir_types);
}

#[test]
fn parse_interp_trace_batch_defaults() {
    let cli = parse_xtask(["interp-trace-gen"]).unwrap();
    let XtaskCommand::InterpTraceGen(args) = cli.command else {
        unreachable!("requested interp-trace-gen")
    };
    let opts = InterpTraceBatchOptions::from(args);
    assert_eq!(opts.case, None);
    assert_eq!(opts.lane, TraceLane::Fast);
    assert_eq!(opts.sample_rate, 48_000);
    assert_eq!(opts.block_size, 64);
    assert_eq!(opts.num_blocks, 4);
    assert!(!opts.strict_fir_types);
}

#[test]
fn parse_interp_trace_batch_accepts_strict_fir_types_flag() {
    let cli = parse_xtask(["interp-trace-check", "--strict-fir-types"]).unwrap();
    let XtaskCommand::InterpTraceCheck(args) = cli.command else {
        unreachable!("requested interp-trace-check")
    };
    let opts = InterpTraceBatchOptions::from(args);
    assert!(opts.strict_fir_types);
}

#[test]
fn corpus_runtime_diff_defaults_to_three_scenarios_and_accepts_bare_cases() {
    let cli = parse_xtask([
        "corpus-runtime-diff",
        "--case",
        "rep_01_passthrough.dsp",
        "--abs-tol",
        "0.000002",
    ])
    .unwrap();
    let XtaskCommand::CorpusRuntimeDiff(args) = cli.command else {
        unreachable!("requested corpus-runtime-diff")
    };
    let options = CorpusRuntimeDiffOptions::from(args);
    assert_eq!(
        options.scenarios,
        vec![
            TraceScenario::Impulse,
            TraceScenario::Ramp,
            TraceScenario::Sine
        ]
    );
    assert_eq!(options.tolerances.abs_tol, 2.0e-6);
    let cases = resolve_corpus_runtime_cases(&options.cases).unwrap();
    assert_eq!(cases.len(), 1);
    assert!(cases[0].ends_with("tests/corpus/rep_01_passthrough.dsp"));
}

#[test]
fn corpus_runtime_diff_rejects_negative_or_non_finite_tolerances() {
    for value in ["-1", "NaN", "inf"] {
        let error = parse_xtask(["corpus-runtime-diff", "--abs-tol", value]).unwrap_err();
        assert!(error.to_string().contains(value));
    }
}

#[test]
fn corpus_runtime_expectations_are_strict_and_strip_dsp_suffixes() {
    let entries = parse_corpus_runtime_expectations(
        "# known\nmismatch | rep_18_stream_wrappers.dsp | DIFF-GAP-001\n\
         oracle | rep_77_foreign_variable | unsupported by C++ interp\n",
    )
    .unwrap();
    assert_eq!(
        entries["rep_18_stream_wrappers"].kind,
        CorpusRuntimeExpectationKind::Mismatch
    );
    assert_eq!(
        entries["rep_77_foreign_variable"].kind,
        CorpusRuntimeExpectationKind::Oracle
    );

    for invalid in [
        "unknown | case | reason",
        "mismatch | case | free-form reason",
        "mismatch | case",
        "mismatch | case | one\noracle | case.dsp | two",
    ] {
        assert!(parse_corpus_runtime_expectations(invalid).is_err());
    }
}

#[test]
fn fir_type_diagnostic_code_filter_matches_expected_groups() {
    assert!(is_fir_type_diagnostic_code("FIR-B03"));
    assert!(is_fir_type_diagnostic_code("FIR-U02"));
    assert!(is_fir_type_diagnostic_code("FIR-C01"));
    assert!(is_fir_type_diagnostic_code("FIR-FC03"));
    assert!(is_fir_type_diagnostic_code("FIR-T02"));
    assert!(is_fir_type_diagnostic_code("FIR-MA04"));
    assert!(is_fir_type_diagnostic_code("FIR-L03"));
    assert!(is_fir_type_diagnostic_code("FIR-SW01"));
    assert!(!is_fir_type_diagnostic_code("FIR-M07"));
    assert!(!is_fir_type_diagnostic_code("FIR-SC01"));
}

#[test]
fn runtime_trace_scenario_mapping_for_typed_primitives() {
    let scenarios = trace_scenarios_for_runtime_case(Path::new(
        "tests/runtime_corpus/trace_31_extended_primitives_typed.dsp",
    ))
    .unwrap();
    assert_eq!(scenarios, vec![TraceScenario::Zeros]);
}

#[test]
fn runtime_trace_scenario_mapping_for_int_plus_one() {
    let scenarios = trace_scenarios_for_runtime_case(Path::new(
        "tests/runtime_corpus/trace_40_int_plus_one.dsp",
    ))
    .unwrap();
    assert_eq!(scenarios, vec![TraceScenario::Ramp]);
}

#[test]
fn runtime_trace_snapshot_path_uses_case_and_scenario() {
    let path = runtime_trace_snapshot_path("trace_01_passthrough", TraceScenario::Impulse);
    let expected = runtime_trace_snapshot_root()
        .join("trace_01_passthrough")
        .join("impulse.json");
    assert_eq!(path, expected);
}

#[test]
fn generate_impulse_inputs_sets_first_sample_only() {
    let inputs = generate_trace_inputs(TraceScenario::Impulse, 2, 5, 48_000);
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0], vec![1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(inputs[1], vec![1.0, 0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn render_runtime_trace_json_contains_expected_keys() {
    let trace = RuntimeTrace {
        dsp_path: "tests/corpus/example.dsp".into(),
        lane: "fast-lane".into(),
        scenario: "zeros".into(),
        sample_rate: 48_000,
        block_size: 64,
        num_blocks: 1,
        num_inputs: 1,
        num_outputs: 1,
        outputs: vec![vec![0.0, 1.0]],
    };
    let json = render_runtime_trace_json(&trace);
    assert!(json.contains("\"backend\": \"interp\""));
    assert!(json.contains("\"signal_fir_lane\": \"fast-lane\""));
    assert!(json.contains("\"scenario\""));
    assert!(json.contains("\"outputs\""));
}

#[test]
fn parse_runtime_trace_json_roundtrip() {
    let trace = RuntimeTrace {
        dsp_path: "tests/runtime_corpus/trace_01_passthrough.dsp".into(),
        lane: "fast-lane".into(),
        scenario: "impulse".into(),
        sample_rate: 48_000,
        block_size: 64,
        num_blocks: 1,
        num_inputs: 1,
        num_outputs: 1,
        outputs: vec![vec![1.0, 0.0]],
    };
    let parsed = parse_runtime_trace_json(&render_runtime_trace_json(&trace)).unwrap();
    assert_eq!(parsed, trace);
}

#[test]
fn compare_runtime_traces_tolerates_small_float_delta() {
    let a = RuntimeTrace {
        dsp_path: "x".into(),
        lane: "normalized".into(),
        scenario: "zeros".into(),
        sample_rate: 48_000,
        block_size: 64,
        num_blocks: 1,
        num_inputs: 0,
        num_outputs: 1,
        outputs: vec![vec![1.0]],
    };
    let mut b = a.clone();
    b.outputs[0][0] = 1.0 + 1.0e-7;
    assert!(compare_runtime_traces(&a, &b, TraceCompareTolerances::default()).is_ok());
}

#[test]
fn compare_runtime_traces_reports_large_float_delta() {
    let a = RuntimeTrace {
        dsp_path: "x".into(),
        lane: "normalized".into(),
        scenario: "zeros".into(),
        sample_rate: 48_000,
        block_size: 64,
        num_blocks: 1,
        num_inputs: 0,
        num_outputs: 1,
        outputs: vec![vec![1.0]],
    };
    let mut b = a.clone();
    b.outputs[0][0] = 1.1;
    let mismatch = compare_runtime_traces(&a, &b, TraceCompareTolerances::default()).unwrap_err();
    assert_eq!(mismatch.field, "outputs");
    assert_eq!(mismatch.channel, Some(0));
    assert_eq!(mismatch.sample, Some(0));
}

#[test]
fn cross_compiler_trace_comparison_ignores_only_the_lane() {
    let cpp = RuntimeTrace {
        dsp_path: "tests/corpus/x.dsp".into(),
        lane: "cpp-fbc".into(),
        scenario: "impulse".into(),
        sample_rate: 48_000,
        block_size: 64,
        num_blocks: 1,
        num_inputs: 1,
        num_outputs: 1,
        outputs: vec![vec![1.0, 0.0]],
    };
    let mut rust = cpp.clone();
    rust.lane = "fast-lane".into();
    assert!(compare_runtime_traces(&cpp, &rust, TraceCompareTolerances::default()).is_err());
    assert!(
        compare_cross_compiler_runtime_traces(&cpp, &rust, TraceCompareTolerances::default())
            .is_ok()
    );
    rust.num_outputs = 2;
    assert!(
        compare_cross_compiler_runtime_traces(&cpp, &rust, TraceCompareTolerances::default())
            .is_err()
    );
}

#[test]
fn interp_trace_opt_level_diff_matches_on_passthrough_case() {
    let case = workspace_root().join("tests/runtime_corpus/trace_01_passthrough.dsp");
    interp_trace_diff_opt_levels_cases(&[case], false).unwrap();
}

#[test]
fn parse_faustwasm_compiler_module_options_defaults_to_release() {
    let cli = parse_xtask(["build-faustwasm-compiler-module"]).unwrap();
    let XtaskCommand::BuildFaustwasmCompilerModule(args) = cli.command else {
        unreachable!("requested build-faustwasm-compiler-module")
    };
    let options = FaustwasmCompilerModuleOptions::from(args);
    assert!(options.release);
}

#[test]
fn parse_faustwasm_compiler_module_options_accepts_debug_flag() {
    let cli = parse_xtask(["build-faustwasm-compiler-module", "--debug"]).unwrap();
    let XtaskCommand::BuildFaustwasmCompilerModule(args) = cli.command else {
        unreachable!("requested build-faustwasm-compiler-module")
    };
    let options = FaustwasmCompilerModuleOptions::from(args);
    assert!(!options.release);
}

#[test]
fn publish_wasm_compiler_module_uses_distribution_name_and_replaces_old_output() {
    let root = std::env::temp_dir().join(format!(
        "faust-rs-xtask-wasm-publish-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let cargo_module = root.join("faust_wasm_ffi.wasm");
    let distributed_module = root.join("libfaust-rs.wasm");
    fs::write(&cargo_module, b"new module").unwrap();
    fs::write(&distributed_module, b"old module").unwrap();

    publish_wasm_compiler_module(&cargo_module, &distributed_module).unwrap();

    assert!(!cargo_module.exists());
    assert_eq!(fs::read(&distributed_module).unwrap(), b"new module");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn publish_wasm_compiler_module_accepts_an_already_published_module() {
    let root = std::env::temp_dir().join(format!(
        "faust-rs-xtask-wasm-existing-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let cargo_module = root.join("faust_wasm_ffi.wasm");
    let distributed_module = root.join("libfaust-rs.wasm");
    fs::write(&distributed_module, b"published module").unwrap();

    publish_wasm_compiler_module(&cargo_module, &distributed_module).unwrap();

    assert_eq!(fs::read(&distributed_module).unwrap(), b"published module");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn publish_libfaust_native_artifacts_uses_hyphenated_distribution_names() {
    let root = std::env::temp_dir().join(format!(
        "faust-rs-xtask-native-publish-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();

    let raw_static = root.join(native_static_library_name("faust_rs"));
    let raw_dynamic = root.join(native_dynamic_library_name("faust_rs"));
    fs::write(&raw_static, b"static").unwrap();
    fs::write(&raw_dynamic, b"dynamic").unwrap();

    let dynamic = publish_libfaust_native_artifacts(&root).unwrap();
    assert_eq!(
        dynamic.file_name().unwrap().to_string_lossy(),
        native_dynamic_library_name("faust-rs")
    );
    assert_eq!(
        fs::read(root.join(native_static_library_name("faust-rs"))).unwrap(),
        b"static"
    );
    assert_eq!(fs::read(dynamic).unwrap(), b"dynamic");
    assert!(!raw_static.exists());
    assert!(!raw_dynamic.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_wasm_ffi_exports_accepts_expected_surface() {
    let bytes = wat::parse_str(
        r#"
            (module
              (memory (export "memory") 1)
              (func (export "faust_wasm_alloc"))
              (func (export "faust_wasm_dealloc"))
              (func (export "faust_wasm_version_ptr"))
              (func (export "faust_wasm_version_len"))
              (func (export "faust_wasm_compile_dsp"))
              (func (export "faust_wasm_result_is_ok"))
              (func (export "faust_wasm_result_wasm_ptr"))
              (func (export "faust_wasm_result_wasm_len"))
              (func (export "faust_wasm_result_json_ptr"))
              (func (export "faust_wasm_result_json_len"))
              (func (export "faust_wasm_result_compile_options_ptr"))
              (func (export "faust_wasm_result_compile_options_len"))
              (func (export "faust_wasm_result_error_ptr"))
              (func (export "faust_wasm_result_error_len"))
              (func (export "faust_wasm_result_get_error_diagnostics"))
              (func (export "faust_wasm_result_get_diagnostics"))
              (func (export "faust_wasm_result_free"))
              (func (export "faust_wasm_get_info"))
              (func (export "faust_wasm_expand_dsp"))
              (func (export "faust_wasm_generate_aux_files"))
              (func (export "faust_wasm_generate_aux_files_json"))
              (func (export "faust_wasm_text_result_is_ok"))
              (func (export "faust_wasm_text_result_ptr"))
              (func (export "faust_wasm_text_result_len"))
              (func (export "faust_wasm_text_result_diagnostics_ptr"))
              (func (export "faust_wasm_text_result_diagnostics_len"))
              (func (export "faust_wasm_text_result_free"))
            )
            "#,
    )
    .unwrap();

    verify_wasm_ffi_exports(&bytes).unwrap();
}

#[test]
fn verify_wasm_ffi_exports_rejects_missing_exports() {
    let bytes = wat::parse_str(
        r#"
            (module
              (memory (export "memory") 1)
              (func (export "faust_wasm_alloc"))
            )
            "#,
    )
    .unwrap();

    let error = verify_wasm_ffi_exports(&bytes).unwrap_err().to_string();
    assert!(error.contains("faust_wasm_compile_dsp"));
    assert!(error.contains("faust_wasm_text_result_free"));
}

// ---------------------------------------------------------------------------
// corpus-status-query (C3): option parsing, pure classification, and full
// end-to-end checks against the real C++ reference binary.
// ---------------------------------------------------------------------------

#[test]
fn corpus_status_query_options_require_case_or_all() {
    let error = parse_corpus_query(std::iter::empty::<String>())
        .unwrap_err()
        .to_string();
    assert!(error.contains("--case") && error.contains("--all"));
}

#[test]
fn corpus_status_query_options_reject_case_and_all_together() {
    let error = parse_corpus_query(["--case", "tests/corpus/fad_basic.dsp", "--all"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot be used with"));
}

#[test]
fn corpus_status_query_options_accept_repeated_case_and_format() {
    let options = parse_corpus_query([
        "--case",
        "tests/corpus/fad_basic.dsp",
        "--case",
        "tests/corpus/rep_01_passthrough.dsp",
        "--format",
        "human",
    ])
    .unwrap();
    assert_eq!(options.cases.len(), 2);
    assert!(!options.all);
    assert_eq!(options.format, QueryFormat::Human);
}

#[test]
fn corpus_status_query_options_reject_unknown_format() {
    let error = parse_corpus_query(["--all", "--format", "yaml"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("yaml"));
}

/// The exact C++ reference wording for an unresolved `fad`/`rad` symbol,
/// confirmed against `porting/phases/phase-4-corpus-status-diff-report-en.md`
/// (e.g. the `fad_basic` row: `tests/corpus/fad_basic.dsp:1 : ERROR :
/// undefined symbol : fad`).
#[test]
fn is_expected_divergence_detects_fad_and_rad_undefined_symbol() {
    assert!(is_expected_divergence(
        "tests/corpus/fad_basic.dsp:1 : ERROR : undefined symbol : fad"
    ));
    assert!(is_expected_divergence(
        "tests/corpus/err_rad_delay_temporal_unsupported.dsp:5 : ERROR : undefined symbol : rad"
    ));
}

#[test]
fn is_expected_divergence_detects_ondemand_undefined_symbol() {
    // Found by measurement: every one of the 21 `real_divergence` cases in
    // the full 218-file corpus run was `undefined symbol : ondemand`, not a
    // genuine regression. See the doc comment on `EXPECTED_DIVERGENCE_SYMBOLS`.
    assert!(is_expected_divergence(
        "interleave.lib:90 : ERROR : undefined symbol : ondemand"
    ));
    assert!(is_expected_divergence(
        "tests/corpus/rep_18_stream_wrappers.dsp:1 : ERROR : undefined symbol : ondemand"
    ));
}

#[test]
fn is_expected_divergence_rejects_unrelated_undefined_symbols() {
    // A hypothetical symbol that merely starts with the same letters must not
    // match: the check stops at the first non-identifier character.
    assert!(!is_expected_divergence(
        "some.dsp:1 : ERROR : undefined symbol : radius"
    ));
    assert!(!is_expected_divergence(
        "some.dsp:1 : ERROR : undefined symbol : fadeout"
    ));
    assert!(!is_expected_divergence(
        "some.dsp:1 : ERROR : undefined symbol : ondemandish"
    ));
    assert!(!is_expected_divergence("some.dsp:1 : ERROR : syntax error"));
}

#[test]
fn classify_divergence_covers_all_four_buckets() {
    assert_eq!(classify_divergence(true, true, "ok"), DivergenceClass::OkOk);
    assert_eq!(
        classify_divergence(false, false, "some other error"),
        DivergenceClass::ErrErr
    );
    assert_eq!(
        classify_divergence(false, true, "undefined symbol : fad"),
        DivergenceClass::ExpectedDivergence
    );
    assert_eq!(
        classify_divergence(false, true, "undefined symbol : somethingelse"),
        DivergenceClass::RealDivergence
    );
    // C++ ok, Rust fails: always a real (Rust) regression, never "expected",
    // even if the C++ reason string happens to mention fad/rad incidentally.
    assert_eq!(
        classify_divergence(true, false, "undefined symbol : fad"),
        DivergenceClass::RealDivergence
    );
}

/// Best-effort guard for the end-to-end tests below: they need a working C++
/// reference binary (either `FAUST_CPP_BIN` or the checked-out build tree
/// `resolve_cpp_faust_bin` falls back to). If neither resolves to something
/// runnable, the tests are skipped rather than failed, mirroring how the
/// existing `xtask` report generators depend on an external checkout without
/// a bundled fixture binary.
fn cpp_reference_binary_available() -> bool {
    let (bin, is_fallback) = resolve_cpp_faust_bin();
    if is_fallback {
        return false;
    }
    bin.exists()
}

#[test]
fn corpus_status_query_json_response_carries_staleness_metadata() {
    if !cpp_reference_binary_available() {
        eprintln!("skipping: no C++ reference binary available (set FAUST_CPP_BIN)");
        return;
    }
    let options = parse_corpus_query(["--case", "tests/corpus/fad_basic.dsp"]).unwrap();
    let response = run_corpus_status_query(&options).unwrap();

    // Round-trip through JSON: the schema must actually parse, not merely
    // serialize.
    let json = serde_json::to_string(&response).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["schema_version"], CORPUS_STATUS_QUERY_SCHEMA_VERSION);
    assert!(parsed["generated_at_unix"].as_u64().unwrap() > 0);
    assert!(parsed["corpus_file_count_seen"].as_u64().unwrap() > 0);
    assert!(!parsed["cpp_binary"]["path"].as_str().unwrap().is_empty());
    assert!(
        !parsed["cpp_binary"]["resolved_via"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    // The corpus is much larger than the one requested case: proves the
    // staleness signal (corpus size actually seen) is independent of, and
    // does not collapse into, the query scope.
    assert!(response.corpus_file_count_seen > response.cases.len());
}

#[test]
fn corpus_status_query_classifies_fad_basic_as_expected_divergence() {
    if !cpp_reference_binary_available() {
        eprintln!("skipping: no C++ reference binary available (set FAUST_CPP_BIN)");
        return;
    }
    let options = parse_corpus_query(["--case", "tests/corpus/fad_basic.dsp"]).unwrap();
    let response = run_corpus_status_query(&options).unwrap();

    assert_eq!(response.cases.len(), 1);
    let case = &response.cases[0];
    assert_eq!(case.case, "fad_basic");
    assert_eq!(case.cpp_status, "ERR");
    assert_eq!(case.rust_status, "OK");
    assert_eq!(case.classification, DivergenceClass::ExpectedDivergence);
    assert_eq!(response.counts.expected_divergence, 1);
    assert_eq!(response.counts.real_divergence, 0);
}

#[test]
fn corpus_status_query_case_list_compiles_only_requested_cases() {
    if !cpp_reference_binary_available() {
        eprintln!("skipping: no C++ reference binary available (set FAUST_CPP_BIN)");
        return;
    }
    let requested = [
        "tests/corpus/fad_basic.dsp",
        "tests/corpus/rep_01_passthrough.dsp",
        "tests/corpus/rep_05_one_pole_lowpass.dsp",
    ];
    let mut args = Vec::new();
    for case in requested {
        args.push("--case".to_string());
        args.push(case.to_string());
    }
    let options = parse_corpus_query(args).unwrap();
    let response = run_corpus_status_query(&options).unwrap();

    assert_eq!(response.query_scope, QueryScope::Cases);
    assert_eq!(response.requested_cases.len(), requested.len());
    assert_eq!(response.cases.len(), requested.len());
    assert_eq!(response.counts.total, requested.len());
    // The corpus holds far more than 3 files; a query for 3 cases must not
    // silently expand to the whole corpus.
    assert!(response.corpus_file_count_seen > requested.len());
    let names: Vec<&str> = response.cases.iter().map(|c| c.case.as_str()).collect();
    assert_eq!(
        names,
        vec!["fad_basic", "rep_01_passthrough", "rep_05_one_pole_lowpass"]
    );
}

#[test]
fn corpus_status_query_counts_are_internally_consistent() {
    if !cpp_reference_binary_available() {
        eprintln!("skipping: no C++ reference binary available (set FAUST_CPP_BIN)");
        return;
    }
    let requested = [
        "tests/corpus/fad_basic.dsp",
        "tests/corpus/rep_01_passthrough.dsp",
        "tests/corpus/rep_05_one_pole_lowpass.dsp",
        "tests/corpus/err_rad_delay_temporal_unsupported.dsp",
    ];
    let mut args = Vec::new();
    for case in requested {
        args.push("--case".to_string());
        args.push(case.to_string());
    }
    let options = parse_corpus_query(args).unwrap();
    let response = run_corpus_status_query(&options).unwrap();

    let c = &response.counts;
    assert_eq!(
        c.total,
        c.ok_ok + c.err_err + c.expected_divergence + c.real_divergence
    );
    assert_eq!(c.total, response.cases.len());

    // The staleness field must reflect what this run actually observed, not
    // a cached or hardcoded figure: recompute the corpus size independently
    // (a fresh directory listing) and require the response to agree.
    let actual_corpus_file_count = corpus_files().unwrap().len();
    assert_eq!(response.corpus_file_count_seen, actual_corpus_file_count);
}
