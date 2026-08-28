//! End-to-end tests for the `-ct`/`--check-table` option
//! (`porting/table-clamp-signal-promotion-plan-2026-08-28-en.md`, gates G3/G4).
//!
//! The clamp itself is a signal-level rewrite (`normalize::table_promote`,
//! staged as `signal_prepare` steps 2.10a/2.10b); these tests lock the
//! user-visible contract: the generated C++ shape under both `-ct` values —
//! byte-comparable to the reference compiler's `std::max<int>(0,
//! std::min<int>(...))` form — and the `--warn`-surfaced messages.

use codegen::backends::cpp::CppOptions;
use compiler::Compiler;

/// `rdtable` read whose slider index interval `[0, 100]` exceeds the table
/// size 16 — the motivating program (`tests/lean/table_unclamped.dsp`).
const TABLE_UNCLAMPED: &str = r#"process = rdtable(16, 1.0, int(hslider("i",0,0,100,1)));"#;

/// `rwtable` with unprovable write *and* read indexes.
const RWTABLE_UNCLAMPED: &str =
    r#"process = rwtable(16, 0.0, int(hslider("w",0,0,100,1)), _, int(hslider("r",0,0,100,1)));"#;

/// `rdtable` read whose index interval `[0, 15]` is provably in-bounds.
const TABLE_PROVABLE: &str = r#"process = rdtable(16, 1.0, int(hslider("i",0,0,15,1)));"#;

fn compile_cpp(source: &str, check_table: bool) -> String {
    Compiler::new()
        .with_check_table(check_table)
        .compile_source_to_cpp("check_table_test.dsp", source, &CppOptions::default())
        .expect("cpp compilation must succeed")
}

#[test]
fn unprovable_read_is_clamped_by_default() {
    let cpp = compile_cpp(TABLE_UNCLAMPED, true);
    // Reference faust emits the full pair:
    //   std::max<int>(0, std::min<int>(<cast index>, 15))
    assert!(
        cpp.contains("std::max<int>(0, std::min<int>("),
        "expected the reference-shaped full clamp, got:\n{cpp}"
    );
    assert!(cpp.contains(", 15))"), "clamp upper bound must be size-1");
}

#[test]
fn check_table_off_generates_the_raw_access() {
    let cpp = compile_cpp(TABLE_UNCLAMPED, false);
    assert!(
        !cpp.contains("std::max<int>") && !cpp.contains("std::min<int>"),
        "-ct 0 must generate the raw unclamped access, got:\n{cpp}"
    );
    assert!(
        !cpp.contains("%"),
        "-ct 0 must not fall back to modular wrapping either"
    );
}

#[test]
fn provable_read_is_never_clamped() {
    // The interval proves the access in-bounds: no clamp in either mode.
    for check_table in [true, false] {
        let cpp = compile_cpp(TABLE_PROVABLE, check_table);
        assert!(
            !cpp.contains("std::max<int>") && !cpp.contains("std::min<int>"),
            "provably safe access must stay direct (check_table={check_table}):\n{cpp}"
        );
    }
}

#[test]
fn rwtable_write_and_read_are_both_clamped() {
    let cpp = compile_cpp(RWTABLE_UNCLAMPED, true);
    let clamps = cpp.matches("std::max<int>(0, std::min<int>(").count();
    assert_eq!(
        clamps, 2,
        "write index and read index must each get the full clamp:\n{cpp}"
    );
    assert!(
        !cpp.contains("% 16"),
        "the write index must be clamped, not modular-wrapped"
    );
}

#[test]
fn warn_surface_reports_the_clamped_accesses() {
    let compiler = Compiler::new();
    let signals = compiler
        .compile_source_to_signals("check_table_test.dsp", RWTABLE_UNCLAMPED)
        .expect("signal compilation must succeed");
    let messages = compiler.table_range_warning_messages(&signals);
    assert_eq!(messages.len(), 2, "one warning per clamped access");
    assert!(
        messages
            .iter()
            .any(|m| m == "WARNING : WRTbl write index [0:100] is outside of table size (16)"),
        "unexpected messages: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m == "WARNING : RDTbl read index [0:100] is outside of table size (16)"),
        "unexpected messages: {messages:?}"
    );
}

#[test]
fn prepared_dump_shows_the_signal_level_clamp() {
    // The user-visible motivation for the whole plan: the clamp must be
    // observable at the signal level (`--dump-sig-dag-prepared`), not
    // invented invisibly at FIR lowering.
    let compiler = Compiler::new();
    let signals = compiler
        .compile_source_to_signals("check_table_test.dsp", TABLE_UNCLAMPED)
        .expect("signal compilation must succeed");
    let prepared = transform::signal_prepare::prepare_signals_for_fir_verified_with_options(
        &signals.parse.state.arena,
        &signals.signals,
        &signals.ui,
        &transform::signal_prepare::PrepareOptions { check_table: true },
    )
    .expect("preparation must succeed");
    let dump = signals::dump_sig_dag(prepared.arena(), prepared.outputs(), Some(&signals.ui));
    assert!(
        dump.contains("SIGMAX"),
        "clamp missing from dump:
{dump}"
    );
    assert!(
        dump.contains("SIGMIN"),
        "clamp missing from dump:
{dump}"
    );

    let raw = transform::signal_prepare::prepare_signals_for_fir_verified_with_options(
        &signals.parse.state.arena,
        &signals.signals,
        &signals.ui,
        &transform::signal_prepare::PrepareOptions { check_table: false },
    )
    .expect("preparation must succeed");
    let dump = signals::dump_sig_dag(raw.arena(), raw.outputs(), Some(&signals.ui));
    assert!(!dump.contains("SIGMAX") && !dump.contains("SIGMIN"));
}

#[test]
fn check_table_off_reports_no_warnings() {
    let compiler = Compiler::new().with_check_table(false);
    let signals = compiler
        .compile_source_to_signals("check_table_test.dsp", RWTABLE_UNCLAMPED)
        .expect("signal compilation must succeed");
    assert!(compiler.table_range_warning_messages(&signals).is_empty());
}
