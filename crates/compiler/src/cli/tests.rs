//! Unit tests for the CLI support modules.
//!
//! These tests intentionally live next to the extracted CLI implementation so
//! they can exercise parser normalization, diagnostic rendering helpers, and
//! output utilities without expanding the launcher in `main.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{CommandFactory, Parser};
use compiler::{Compiler, FaustInstallPaths};
use errors::{Diagnostic, DiagnosticBundle, DiagnosticCode, Severity, SourceSpan, Stage};
use serde_json::Value;
use signals::{SigMatch, match_sig};

use super::args::{CliArgs, CliLang, ErrorVerbosity, normalize_legacy_args};
use super::diagnostics::{
    format_diagnostics_human, format_diagnostics_human_with_verbosity, format_diagnostics_json,
    format_diagnostics_json_with_verbosity,
};
use super::runner::{
    emit_wasm_output, render_directory_info, render_version_text, render_wast_output,
};

#[test]
fn normalize_legacy_args_maps_dash_fir_to_lang_fir() {
    let args = vec![
        "faust-rs".to_owned(),
        "-lang".to_owned(),
        "-fir".to_owned(),
        "foo.dsp".to_owned(),
    ];
    let normalized = normalize_legacy_args(args);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--lang".to_owned(),
            "fir".to_owned(),
            "foo.dsp".to_owned()
        ]
    );
}

#[test]
fn vec_flags_map_to_compute_mode() {
    use super::runner::selected_compute_mode;
    use compiler::ComputeMode;

    // Single-dash Faust style `-vec -vs 64 -lv 1` normalizes then parses.
    let normalized = normalize_legacy_args(vec![
        "faust-rs".to_owned(),
        "-vec".to_owned(),
        "-vs".to_owned(),
        "64".to_owned(),
        "-lv".to_owned(),
        "1".to_owned(),
        "foo.dsp".to_owned(),
    ]);
    let cli = CliArgs::parse_from(normalized);
    assert!(cli.vec);
    assert_eq!(cli.vs, 64);
    assert_eq!(cli.lv, 1);
    assert_eq!(
        selected_compute_mode(&cli),
        ComputeMode::Vector {
            vec_size: 64,
            loop_variant: 1
        }
    );

    // No `-vec` → scalar (default), with the Faust default vector size (32).
    let cli = CliArgs::parse_from(["faust-rs", "foo.dsp"]);
    assert!(!cli.vec);
    assert_eq!(cli.vs, ComputeMode::DEFAULT_VEC_SIZE);
    assert_eq!(cli.vs, 32);
    assert_eq!(selected_compute_mode(&cli), ComputeMode::Scalar);
}

// ── `-ss` / `--scheduling-strategy` (vectorization port plan P2) ─────────────
//
// P2 is plumbing-only: these tests check parsing, defaulting, legacy `-ss`
// normalization, and `SchedulingStrategy::decode` mapping — not that
// scheduling is active (no compiled-output assertions here).

#[test]
fn scheduling_strategy_flag_decodes_all_documented_values() {
    use super::runner::selected_scheduling_strategy;
    use compiler::SchedulingStrategy;

    let cases: [(u32, SchedulingStrategy); 6] = [
        (0, SchedulingStrategy::DepthFirst),
        (1, SchedulingStrategy::BreadthFirst),
        (2, SchedulingStrategy::Special),
        (3, SchedulingStrategy::ReverseBreadthFirst),
        (4, SchedulingStrategy::ReverseBreadthFirst),
        (42, SchedulingStrategy::ReverseBreadthFirst),
    ];
    for (n, expected) in cases {
        let cli = CliArgs::parse_from([
            "faust-rs",
            "--scheduling-strategy",
            &n.to_string(),
            "foo.dsp",
        ]);
        assert_eq!(cli.scheduling_strategy, n);
        assert_eq!(
            selected_scheduling_strategy(&cli),
            expected,
            "-ss {n} should decode to {expected:?}"
        );
    }

    // `-ss 3` and `-ss 42` both decode to `ReverseBreadthFirst`.
    let cli3 = CliArgs::parse_from(["faust-rs", "--scheduling-strategy", "3", "foo.dsp"]);
    let cli42 = CliArgs::parse_from(["faust-rs", "--scheduling-strategy", "42", "foo.dsp"]);
    assert_eq!(
        selected_scheduling_strategy(&cli3),
        selected_scheduling_strategy(&cli42)
    );
}

#[test]
fn scheduling_strategy_defaults_to_depth_first_in_scalar_and_vector_modes() {
    use super::runner::selected_scheduling_strategy;
    use compiler::SchedulingStrategy;

    let scalar = CliArgs::parse_from(["faust-rs", "foo.dsp"]);
    assert_eq!(scalar.scheduling_strategy, 0);
    assert_eq!(
        selected_scheduling_strategy(&scalar),
        SchedulingStrategy::DepthFirst
    );

    // `-vec` must not alter the `-ss` default.
    let vector = CliArgs::parse_from(["faust-rs", "--vec", "foo.dsp"]);
    assert_eq!(vector.scheduling_strategy, 0);
    assert_eq!(
        selected_scheduling_strategy(&vector),
        SchedulingStrategy::DepthFirst
    );
}

#[test]
fn scheduling_strategy_flag_is_accepted_without_vec() {
    let cli = CliArgs::parse_from(["faust-rs", "--scheduling-strategy", "1", "foo.dsp"]);
    assert!(!cli.vec);
    assert_eq!(cli.scheduling_strategy, 1);
}

#[test]
fn scheduling_strategy_flag_rejects_missing_value() {
    // `--scheduling-strategy` as the trailing argument, with nothing after it.
    let result = CliArgs::try_parse_from(["faust-rs", "--scheduling-strategy"]);
    assert!(result.is_err());
}

#[test]
fn scheduling_strategy_flag_rejects_negative_value() {
    let result = CliArgs::try_parse_from(["faust-rs", "--scheduling-strategy", "-1", "foo.dsp"]);
    assert!(result.is_err());
}

#[test]
fn scheduling_strategy_flag_rejects_non_integer_value() {
    let result = CliArgs::try_parse_from(["faust-rs", "--scheduling-strategy", "abc", "foo.dsp"]);
    assert!(result.is_err());
}

#[test]
fn normalize_legacy_args_maps_dash_ss_to_scheduling_strategy() {
    let args = vec![
        "faust-rs".to_owned(),
        "-ss".to_owned(),
        "42".to_owned(),
        "foo.dsp".to_owned(),
    ];
    let normalized = normalize_legacy_args(args);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--scheduling-strategy".to_owned(),
            "42".to_owned(),
            "foo.dsp".to_owned(),
        ]
    );

    let cli = CliArgs::parse_from(normalize_legacy_args(vec![
        "faust-rs".to_owned(),
        "-ss".to_owned(),
        "3".to_owned(),
        "foo.dsp".to_owned(),
    ]));
    assert_eq!(cli.scheduling_strategy, 3);
}

#[test]
fn normalize_legacy_args_maps_dash_pn_to_process_name() {
    let args = vec![
        "faust-rs".to_owned(),
        "-pn".to_owned(),
        "dsp".to_owned(),
        "foo.dsp".to_owned(),
    ];
    let normalized = normalize_legacy_args(args);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--process-name".to_owned(),
            "dsp".to_owned(),
            "foo.dsp".to_owned()
        ]
    );
}

#[test]
fn cli_help_lists_lang_possible_values_alphabetically() {
    let help = CliArgs::command().render_long_help().to_string();
    assert!(
        help.contains(
            "possible values: asc, c, cpp, cranelift, fir, interp, julia, rust, wasm, wast"
        ),
        "{help}"
    );
}

#[test]
fn cli_parse_accepts_lang_fir() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "fir", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Fir)));
}

#[test]
fn cli_parse_accepts_lang_cranelift() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "cranelift", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Cranelift)));
}

#[test]
fn cli_parse_accepts_lang_rust() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "rust", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Rust)));
}

#[test]
fn cli_parse_accepts_lang_rs_alias() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "rs", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Rust)));
}

#[test]
fn cli_parse_accepts_lang_julia() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "julia", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Julia)));
}

#[test]
fn cli_parse_accepts_lang_wasm() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "wasm", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Wasm)));
}

#[test]
fn cli_parse_accepts_lang_wast() {
    let cli = CliArgs::parse_from(["faust-rs", "--lang", "wast", "foo.dsp"]);
    assert!(matches!(cli.lang, Some(CliLang::Wast)));
}

#[test]
fn cli_parse_accepts_json_flag() {
    let cli = CliArgs::parse_from(["faust-rs", "--json", "foo.dsp"]);
    assert!(cli.dump_json);
}

#[test]
fn cli_parse_accepts_json_with_lang() {
    let cli = CliArgs::parse_from(["faust-rs", "--json", "--lang", "cpp", "foo.dsp"]);
    assert!(cli.dump_json);
    assert!(matches!(cli.lang, Some(CliLang::Cpp)));
}

#[test]
fn normalize_legacy_args_maps_dash_json_to_json_flag() {
    let args = vec![
        "faust-rs".to_owned(),
        "-json".to_owned(),
        "foo.dsp".to_owned(),
    ];
    let normalized = normalize_legacy_args(args);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--json".to_owned(),
            "foo.dsp".to_owned()
        ]
    );
}

#[test]
fn emit_wasm_output_writes_companion_json_next_to_wasm_file() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "faust_rs_wasm_emit_test_{}_{}",
        std::process::id(),
        unique
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    let wasm_path = dir.join("voice.wasm");

    emit_wasm_output(
        b"\0asm\x01\0\0\0",
        "{\"backend\":\"wasm\"}",
        Some(&wasm_path),
    );

    let json_path = dir.join("voice.json");
    assert_eq!(
        fs::read(&wasm_path).expect("wasm output should exist"),
        b"\0asm\x01\0\0\0"
    );
    assert_eq!(
        fs::read_to_string(&json_path).expect("json output should exist"),
        "{\"backend\":\"wasm\"}"
    );

    fs::remove_dir_all(&dir).expect("temp dir should be removed");
}

#[test]
fn render_wast_output_prints_valid_text_module() {
    let wast = render_wast_output(b"\0asm\x01\0\0\0");
    assert!(wast.contains("(module"));
}

#[test]
fn cli_parse_accepts_dump_cranelift() {
    let cli = CliArgs::parse_from(["faust-rs", "--dump-cranelift", "foo.dsp"]);
    assert!(cli.dump_cranelift);
}

#[test]
fn cli_parse_accepts_process_name() {
    let cli = CliArgs::parse_from(["faust-rs", "--process-name", "dsp", "foo.dsp"]);
    assert_eq!(cli.process_name, "dsp");
}

#[test]
fn cli_parse_accepts_class_name() {
    let cli = CliArgs::parse_from(["faust-rs", "--class-name", "customdsp", "foo.dsp"]);
    assert_eq!(cli.class_name.as_deref(), Some("customdsp"));
}

#[test]
fn cli_parse_accepts_super_class_name() {
    let cli = CliArgs::parse_from(["faust-rs", "--super-class-name", "faust_dsp", "foo.dsp"]);
    assert_eq!(cli.super_class_name.as_deref(), Some("faust_dsp"));
}

#[test]
fn normalize_legacy_args_maps_dash_cn_to_class_name() {
    let normalized = normalize_legacy_args(vec![
        "faust-rs".to_owned(),
        "-cn".to_owned(),
        "customdsp".to_owned(),
        "foo.dsp".to_owned(),
    ]);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--class-name".to_owned(),
            "customdsp".to_owned(),
            "foo.dsp".to_owned(),
        ]
    );
}

#[test]
fn normalize_legacy_args_maps_dash_scn_to_super_class_name() {
    let normalized = normalize_legacy_args(vec![
        "faust-rs".to_owned(),
        "-scn".to_owned(),
        "faust_dsp".to_owned(),
        "foo.dsp".to_owned(),
    ]);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--super-class-name".to_owned(),
            "faust_dsp".to_owned(),
            "foo.dsp".to_owned(),
        ]
    );
}

#[test]
fn cli_parse_accepts_list_fir_fixtures() {
    let cli = CliArgs::parse_from(["faust-rs", "--list-fir-fixtures"]);
    assert!(cli.list_fir_fixtures);
}

#[test]
fn cli_parse_accepts_fir_fixture_with_lang() {
    let cli = CliArgs::parse_from(["faust-rs", "--fir-fixture", "sine_phasor", "--lang", "cpp"]);
    assert_eq!(cli.fir_fixture.as_deref(), Some("sine_phasor"));
    assert!(matches!(cli.lang, Some(CliLang::Cpp)));
}

#[test]
fn cli_parse_accepts_dump_cpp_from_fbc() {
    let cli = CliArgs::parse_from(["faust-rs", "--dump-cpp-from-fbc", "foo.fbc"]);
    assert!(cli.dump_cpp_from_fbc);
    assert_eq!(cli.input.as_deref(), Some(Path::new("foo.fbc")));
}

#[test]
fn cli_parse_accepts_cpp_class_name_with_dump_cpp_from_fbc() {
    let cli = CliArgs::parse_from([
        "faust-rs",
        "--dump-cpp-from-fbc",
        "--cpp-class-name",
        "my_dsp",
        "foo.fbc",
    ]);
    assert_eq!(cli.cpp_class_name.as_deref(), Some("my_dsp"));
}

#[test]
fn help_mentions_faust_naming_aliases() {
    let mut command = CliArgs::command();
    let rendered = command.render_long_help().to_string();
    assert!(rendered.contains("-cn <name>"));
    assert!(rendered.contains("-scn <name>"));
    assert!(rendered.contains("-pn <name>"));
}

#[test]
fn version_mentions_faust_copyright() {
    let rendered = render_version_text();
    assert!(rendered.starts_with("faust-rs "));
    assert!(rendered.contains(
        "Copyright (C) 2002-2026, GRAME - Centre National de Creation Musicale. All rights reserved."
    ));
}

#[test]
fn normalize_legacy_args_maps_directory_info_flags() {
    let normalized = normalize_legacy_args(vec![
        "faust-rs".to_owned(),
        "-includedir".to_owned(),
        "-libdir".to_owned(),
        "-dspdir".to_owned(),
        "-archdir".to_owned(),
        "-pathslist".to_owned(),
    ]);
    assert_eq!(
        normalized,
        vec![
            "faust-rs".to_owned(),
            "--includedir".to_owned(),
            "--libdir".to_owned(),
            "--dspdir".to_owned(),
            "--archdir".to_owned(),
            "--pathslist".to_owned(),
        ]
    );
}

#[test]
fn cli_parse_accepts_directory_info_flags() {
    let cli = CliArgs::parse_from(["faust-rs", "--includedir", "--pathslist"]);
    assert!(cli.includedir);
    assert!(cli.pathslist);
}

#[test]
fn render_directory_info_uses_cpp_precedence() {
    let cli = CliArgs::parse_from(["faust-rs", "--includedir", "--libdir"]);
    let paths = FaustInstallPaths::from_parts(
        Some(PathBuf::from("/opt/faust/bin/faust-rs")),
        Some("custom-dsp".into()),
        Some("custom-arch".into()),
    );
    assert_eq!(
        render_directory_info(&cli, &paths),
        Some(FaustInstallPaths::render_path(&paths.lib_dir))
    );
}

#[test]
fn render_directory_info_pathslist_matches_cpp_shape() {
    let cli = CliArgs::parse_from(["faust-rs", "--pathslist"]);
    let paths = FaustInstallPaths::from_parts(
        Some(PathBuf::from("/opt/faust/bin/faust-rs")),
        Some("custom-dsp".into()),
        Some("custom-arch".into()),
    );
    let rendered = render_directory_info(&cli, &paths).expect("pathslist should render");
    assert!(rendered.starts_with("FAUST dsp library paths:\ncustom-dsp\n"));
    assert!(rendered.contains("\nFAUST architectures paths:\ncustom-arch\n"));
    assert!(rendered.ends_with('\n'));
}

#[test]
fn diagnostics_human_renderer_keeps_code_and_location() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Eval,
            DiagnosticCode("FRS-EVAL-0001"),
            "missing process",
        )
        .with_label(errors::Label::new(
            errors::LabelStyle::Primary,
            SourceSpan::new("test.dsp", 3, 7, 3, 12),
            "here",
        )),
    );

    let rendered = format_diagnostics_human(&bundle);
    assert!(rendered.contains("test.dsp:3:7"));
    assert!(rendered.contains("[FRS-EVAL-0001]"));
    assert!(rendered.contains("missing process"));
}

#[test]
fn diagnostics_json_renderer_exposes_structured_fields() {
    let compiler = Compiler::new();
    let err = compiler
        .compile_source_to_signals("missing_process.dsp", "foo = _;")
        .expect_err("missing process should fail");
    let diagnostics = err
        .diagnostics()
        .expect("compiler errors should expose diagnostics");

    let rendered = format_diagnostics_json(diagnostics);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let first = &value["diagnostics"][0];
    assert_eq!(first["severity"], "error");
    assert_eq!(first["stage"], "eval");
    let code = first["code"].as_str().expect("code should be a string");
    assert!(code.starts_with("FRS-EVAL-"));
    assert!(first["message"].is_string());
    assert!(first["labels"].is_array());
}

#[test]
fn diagnostics_human_renderer_snapshot_with_snippet_and_caret() {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "faust_rs_diag_human_{}_{}.dsp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos()
    ));
    std::fs::write(&path, "process = _,_ <: _,_,_;\n").expect("fixture should be written");

    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Propagate,
            DiagnosticCode("FRS-PROP-0002"),
            "split composition mismatch",
        )
        .with_label(errors::Label::new(
            errors::LabelStyle::Primary,
            SourceSpan::new(&path, 1, 13, 1, 15),
            "related source",
        ))
        .with_note("rule: split(A, B) requires inputs(B) % outputs(A) == 0")
        .with_note("computed: 3 % 2 = 1")
        .with_help("make B input count a multiple of A output count"),
    );

    let rendered = format_diagnostics_human(&bundle);
    let path_text = path.to_string_lossy().to_string();
    let normalized = rendered.replace(&path_text, "$TMPFILE");
    let expected = "\
$TMPFILE:1:13: error [FRS-PROP-0002] split composition mismatch
  1 | process = _,_ <: _,_,_;
    |             ^^ related source
  = note: rule: split(A, B) requires inputs(B) % outputs(A) == 0
  = note: computed: 3 % 2 = 1
  = help: make B input count a multiple of A output count
";
    assert_eq!(normalized, expected);

    std::fs::remove_file(path).expect("fixture should be removed");
}

#[test]
fn diagnostics_json_renderer_snapshot_shape_stable() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Eval,
            DiagnosticCode("FRS-EVAL-0003"),
            "too many arguments",
        )
        .with_note("application accepts at most 1 argument(s), got 2")
        .with_help("remove one argument"),
    );

    let rendered = format_diagnostics_json(&bundle);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];

    assert_eq!(diag["severity"], "error");
    assert_eq!(diag["stage"], "eval");
    assert_eq!(diag["code"], "FRS-EVAL-0003");
    assert_eq!(diag["message"], "too many arguments");
    assert!(diag["labels"].is_array());
    assert_eq!(
        diag["notes"][0],
        "application accepts at most 1 argument(s), got 2"
    );
    assert_eq!(diag["help"][0], "remove one argument");
}

#[test]
fn diagnostics_json_renderer_debug_mode_exposes_internal_fields() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Propagate,
            DiagnosticCode("FRS-PROP-0002"),
            "split mismatch",
        )
        .with_note("node_id=42")
        .with_note("box_expr=3(1(), 1())"),
    );
    let rendered = format_diagnostics_json_with_verbosity(&bundle, ErrorVerbosity::Debug);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    assert_eq!(diag["debug"]["node_id"], 42);
    assert_eq!(diag["debug"]["box_expr"], "3(1(), 1())");
}

#[test]
fn diagnostics_json_renderer_standard_mode_omits_internal_debug_fields() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Propagate,
            DiagnosticCode("FRS-PROP-0002"),
            "split mismatch",
        )
        .with_note("node_id=42")
        .with_note("box_expr=3(1(), 1())"),
    );
    let rendered = format_diagnostics_json_with_verbosity(&bundle, ErrorVerbosity::Standard);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    assert!(diag["debug"].is_null());
}

#[test]
fn diagnostics_human_renderer_renders_cpp_style_a_b_block() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Propagate,
            DiagnosticCode("FRS-PROP-0002"),
            "split composition mismatch",
        )
        .with_note("A (split left) = (_, _)")
        .with_note("B (split right) = (_, (_, _))")
        .with_note("A arity: inputs=2 outputs=2")
        .with_note("B arity: inputs=3 outputs=3"),
    );

    let rendered = format_diagnostics_human(&bundle);
    assert!(rendered.contains("Here  A = (_, _)"));
    assert!(rendered.contains("while B = (_, (_, _))"));
    assert!(rendered.contains("has inputs=2 outputs=2"));
    assert!(rendered.contains("has inputs=3 outputs=3"));
    assert!(!rendered.contains("A (split left) = "));
}

#[test]
fn diagnostics_human_renderer_hides_internal_machine_notes() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Eval,
            DiagnosticCode("FRS-EVAL-0002"),
            "undefined symbol `x`",
        )
        .with_note("node_id=42")
        .with_note("box_expr=0(sym(\"x\"))")
        .with_note("expr=x"),
    );
    let rendered = format_diagnostics_human(&bundle);
    assert!(
        !rendered.contains("node_id=42"),
        "human mode should hide internal node ids"
    );
    assert!(
        !rendered.contains("box_expr=0(sym(\"x\"))"),
        "human mode should hide internal box previews"
    );
    assert!(
        rendered.contains("expr=x"),
        "human mode should keep readable expression context"
    );
}

#[test]
fn diagnostics_human_renderer_debug_mode_keeps_internal_machine_notes() {
    let mut bundle = DiagnosticBundle::new();
    bundle.push(
        Diagnostic::new(
            Severity::Error,
            Stage::Eval,
            DiagnosticCode("FRS-EVAL-0002"),
            "undefined symbol `x`",
        )
        .with_note("node_id=42")
        .with_note("box_expr=0(sym(\"x\"))")
        .with_note("expr=x"),
    );
    let rendered = format_diagnostics_human_with_verbosity(&bundle, ErrorVerbosity::Debug);
    assert!(rendered.contains("node_id=42"));
    assert!(rendered.contains("box_expr=0(sym(\"x\"))"));
    assert!(rendered.contains("expr=x"));
}

/// Resolves `file` against the workspace `tests/corpus` directory for snapshot tests.
fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

/// Returns the index of the first note starting with `prefix`, panicking with
/// a descriptive message if none matches.
fn note_index<'a>(notes: &'a [&'a str], prefix: &str) -> usize {
    notes
        .iter()
        .position(|note| note.starts_with(prefix))
        .unwrap_or_else(|| panic!("missing note prefix `{prefix}` in {:?}", notes))
}

#[test]
fn diagnostics_human_renderer_snapshots_cover_complex_phase4_failures() {
    let fixtures = [
        (
            "err_06_propagate_split_mismatch_chain.dsp",
            vec![
                "error [FRS-PROP-0002] split composition mismatch",
                "error originates from definition 'foo'",
                "binding_trace=process -> baz -> bar -> foo",
                "Here  A = (_, _)",
                "while B = (_, (_, _))",
                "suggested target: set inputs(B) to 4",
            ],
        ),
        (
            "err_07_propagate_rec_mismatch_alias.dsp",
            vec![
                "error [FRS-PROP-0003] recursive composition mismatch",
                "error originates from definition 'foo'",
                "binding_trace=process -> bar -> foo",
                "Here  A = _",
                "while B = (_, (_, _))",
                "suggested target: set outputs(A) >= 3 and inputs(A) >= 3",
            ],
        ),
        (
            "err_08_propagate_seq_ui_mismatch.dsp",
            vec![
                "error [FRS-PROP-0002] sequential composition mismatch",
                "cause: sequential composition bus widths do not match",
                "error originates from definition 'foo'",
                "binding_trace=process -> foo",
                "Here  A = hslider(\"gain\", 0.5, 0, 1, 0.01)",
                "while B = ",
                "suggested target: make outputs(A) and inputs(B) equal (common target: 2)",
            ],
        ),
        (
            "err_16_propagate_compound_with_letrec_split.dsp",
            vec![
                "error [FRS-PROP-0002] split composition mismatch",
                "cause: split composition divisibility rule is violated",
                "error originates from definition 'foo'",
                "binding_trace=process -> baz -> bar -> foo",
                "Here  A = (_, _)",
                "while B = (_, (_, _))",
                "template: process = A <: B; // inputs(B) % outputs(A) == 0",
            ],
        ),
    ];

    let compiler = Compiler::new();
    for (file, expected_lines) in fixtures {
        let path = corpus_path(file);
        let err = compiler
            .compile_file_default_to_signals(&path)
            .expect_err("fixture should fail in signal pipeline");
        let diagnostics = err
            .diagnostics()
            .expect("fixture error should expose diagnostics");
        let rendered = format_diagnostics_human(diagnostics);
        let path_text = path.to_string_lossy().to_string();
        let normalized = rendered.replace(&path_text, "$FIXTURE");
        for expected in expected_lines {
            assert!(
                normalized.contains(expected),
                "{file} human snapshot should contain: {expected}\nrendered:\n{normalized}"
            );
        }
        let source = fs::read_to_string(&path).expect("fixture source should be readable");
        let first_line = source
            .lines()
            .next()
            .expect("fixture should contain at least one line");
        assert!(
            normalized.contains(first_line),
            "{file} human snapshot should include source snippet line"
        );
    }
}

#[test]
fn diagnostics_json_renderer_snapshots_cover_complex_phase4_failures() {
    let fixtures = [
        (
            "err_06_propagate_split_mismatch_chain.dsp",
            "binding_trace=process -> baz -> bar -> foo",
            "A (split left) = ",
            "B (split right) = ",
            "error originates from definition 'foo'",
            "suggested target: set inputs(B) to 4",
        ),
        (
            "err_07_propagate_rec_mismatch_alias.dsp",
            "binding_trace=process -> bar -> foo",
            "A (rec left) = ",
            "B (rec right) = ",
            "error originates from definition 'foo'",
            "suggested target: set outputs(A) >= 3 and inputs(A) >= 3",
        ),
        (
            "err_08_propagate_seq_ui_mismatch.dsp",
            "binding_trace=process -> foo",
            "A (seq left) = ",
            "B (seq right) = ",
            "error originates from definition 'foo'",
            "suggested target: make outputs(A) and inputs(B) equal (common target: 2)",
        ),
        (
            "err_16_propagate_compound_with_letrec_split.dsp",
            "binding_trace=process -> baz -> bar -> foo",
            "A (split left) = ",
            "B (split right) = ",
            "error originates from definition 'foo'",
            "suggested target: set inputs(B) to 4",
        ),
    ];

    let compiler = Compiler::new();
    for (file, trace, left_prefix, right_prefix, owner_note, suggestion_note) in fixtures {
        let path = corpus_path(file);
        let err = compiler
            .compile_file_default_to_signals(&path)
            .expect_err("fixture should fail in signal pipeline");
        let diagnostics = err
            .diagnostics()
            .expect("fixture error should expose diagnostics");
        let rendered = format_diagnostics_json(diagnostics);
        let value: Value =
            serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
        let diag = &value["diagnostics"][0];
        let notes = diag["notes"]
            .as_array()
            .expect("notes should be an array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert!(
            notes.contains(&trace),
            "{file} json snapshot should contain trace note"
        );
        assert!(
            notes.iter().any(|note| note.starts_with(left_prefix)),
            "{file} json snapshot should contain left-side note"
        );
        assert!(
            notes.iter().any(|note| note.starts_with(right_prefix)),
            "{file} json snapshot should contain right-side note"
        );
        assert!(
            notes.contains(&owner_note),
            "{file} json snapshot should contain owner note"
        );
        assert!(
            notes.iter().any(|note| note.starts_with(suggestion_note)),
            "{file} json snapshot should contain numeric suggestion note"
        );
    }
}

#[test]
fn diagnostics_human_renderer_snapshot_for_eval_undefined_symbol() {
    let compiler = Compiler::new();
    let path = corpus_path("err_09_eval_undefined_symbol.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_human(diagnostics);
    let path_text = path.to_string_lossy().to_string();
    let normalized = rendered.replace(&path_text, "$FIXTURE");

    assert!(normalized.contains("error [FRS-EVAL-0002] undefined symbol `bar`"));
    assert!(normalized.contains("error originates from definition 'foo'"));
    assert!(normalized.contains("binding_trace=process -> foo"));
    assert!(normalized.contains("expr=bar"));
    assert!(normalized.contains("define the symbol in scope or fix the identifier name"));
}

#[test]
fn diagnostics_human_renderer_snapshot_for_eval_undefined_symbol_alias_chain() {
    let compiler = Compiler::new();
    let path = corpus_path("err_13_eval_undefined_symbol_alias_chain_nested.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_human(diagnostics);
    let path_text = path.to_string_lossy().to_string();
    let normalized = rendered.replace(&path_text, "$FIXTURE");

    assert!(normalized.contains("error [FRS-EVAL-0002] undefined symbol `z`"));
    assert!(normalized.contains("cause: unresolved identifier in current lexical scope"));
    assert!(normalized.contains("error originates from definition 'foo'"));
    assert!(normalized.contains("binding_trace=process -> baz -> bar -> foo"));
    assert!(
        normalized.contains("rule: referenced identifier must be present in visible lexical scope")
    );
    assert!(normalized.contains("template: z = ...; // define before use"));
}

#[test]
fn diagnostics_json_renderer_snapshot_for_eval_undefined_symbol() {
    let compiler = Compiler::new();
    let path = corpus_path("err_09_eval_undefined_symbol.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_json(diagnostics);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    let notes = diag["notes"]
        .as_array()
        .expect("notes should be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert_eq!(diag["code"], "FRS-EVAL-0002");
    assert!(notes.iter().any(|n| n.starts_with("expr=")));
    assert!(notes.contains(&"error originates from definition 'foo'"));
    assert!(notes.contains(&"binding_trace=process -> foo"));
}

#[test]
fn diagnostics_json_renderer_snapshot_for_eval_undefined_symbol_alias_chain() {
    let compiler = Compiler::new();
    let path = corpus_path("err_13_eval_undefined_symbol_alias_chain_nested.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_json(diagnostics);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    let labels = diag["labels"]
        .as_array()
        .expect("labels should be an array");
    let notes = diag["notes"]
        .as_array()
        .expect("notes should be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();

    assert_eq!(diag["code"], "FRS-EVAL-0002");
    assert_eq!(labels[0]["role"], "definition_site");
    if labels.len() >= 2 {
        assert_eq!(labels[1]["role"], "call_site");
    }
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("cause: unresolved identifier"))
    );
    assert!(notes.contains(&"binding_trace=process -> baz -> bar -> foo"));

    let cause_i = note_index(&notes, "cause:");
    let rule_i = note_index(&notes, "rule:");
    let computed_i = note_index(&notes, "computed:");
    let context_i = note_index(&notes, "scope.local=");
    assert!(
        cause_i < rule_i && rule_i < computed_i && computed_i < context_i,
        "eval JSON note ordering should be cause -> rule -> computed -> context"
    );
}

#[test]
fn diagnostics_json_renderer_note_order_for_propagate_split_compound() {
    let compiler = Compiler::new();
    let path = corpus_path("err_16_propagate_compound_with_letrec_split.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in propagate stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_json(diagnostics);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    let notes = diag["notes"]
        .as_array()
        .expect("notes should be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let cause_i = note_index(&notes, "cause:");
    let rule_i = note_index(&notes, "rule:");
    let computed_i = note_index(&notes, "computed:");
    let context_i = note_index(&notes, "A (split left) = ");
    assert!(
        cause_i < rule_i && rule_i < computed_i && computed_i < context_i,
        "propagate JSON note ordering should be cause -> rule -> computed -> context"
    );
}

#[test]
fn diagnostics_json_renderer_note_order_for_propagate_merge_alias() {
    let compiler = Compiler::new();
    let path = corpus_path("err_05_propagate_merge_mismatch_alias.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in propagate stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_json(diagnostics);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    let notes = diag["notes"]
        .as_array()
        .expect("notes should be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let cause_i = note_index(&notes, "cause:");
    let rule_i = note_index(&notes, "rule:");
    let computed_i = note_index(&notes, "computed:");
    let context_i = note_index(&notes, "A (merge left) = ");
    assert!(cause_i < rule_i && rule_i < computed_i && computed_i < context_i);
}

#[test]
fn diagnostics_json_renderer_note_order_for_propagate_rec_alias() {
    let compiler = Compiler::new();
    let path = corpus_path("err_07_propagate_rec_mismatch_alias.dsp");
    let err = compiler
        .compile_file_default_to_signals(&path)
        .expect_err("fixture should fail in propagate stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_json(diagnostics);
    let value: Value =
        serde_json::from_str(&rendered).expect("JSON diagnostics output should be valid");
    let diag = &value["diagnostics"][0];
    let notes = diag["notes"]
        .as_array()
        .expect("notes should be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let cause_i = note_index(&notes, "cause:");
    let rule_i = note_index(&notes, "rule:");
    let computed_i = note_index(&notes, "computed:");
    let context_i = note_index(&notes, "A (rec left) = ");
    assert!(cause_i < rule_i && rule_i < computed_i && computed_i < context_i);
}

#[test]
fn diagnostics_human_renderer_snapshot_for_pipeline_origin_fallback() {
    let compiler = Compiler::new();
    let source = fs::read_to_string(corpus_path("err_17_origin_fallback_missing_props_eval.dsp"))
        .expect("fixture should be readable");
    let mut parsed =
        parser::parse_program(&source, "err_17_origin_fallback_missing_props_eval.dsp");
    parsed.state.ctx = parser::ParserCtx::new();
    let err = compiler
        .compile_parsed_to_signals("err_17_origin_fallback_missing_props_eval.dsp", parsed)
        .expect_err("fixture should fail in eval stage");
    let diagnostics = err
        .diagnostics()
        .expect("fixture error should expose diagnostics");
    let rendered = format_diagnostics_human(diagnostics);
    assert!(rendered.contains("origin span unavailable; pointing to nearest call/owner site"));
}

#[test]
fn diagnostics_human_renderer_compound_case_fixture_now_compiles() {
    let compiler = Compiler::new();
    let path = corpus_path("err_15_eval_compound_with_letrec_case_arity.dsp");
    let out = compiler
        .compile_file_default_to_signals(&path)
        .expect("fixture should now compile to signals");
    assert_eq!(out.process_arity.inputs, 1);
    assert_eq!(out.process_arity.outputs, 1);
    assert_eq!(out.signals.len(), 1);
    assert_eq!(
        match_sig(&out.parse.state.arena, out.signals[0]),
        SigMatch::Int(1)
    );
}
