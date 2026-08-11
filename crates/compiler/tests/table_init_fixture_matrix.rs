//! Runs the frozen §8.1 fixture matrix in both `--table-init` modes.
//!
//! The 13 fixtures in `porting/generated/siggen-table-init-s0/dsp/` were frozen
//! at plan phase S0 and used by hand throughout the port; nothing executed them
//! afterwards. §5.10 of the plan requires both modes to carry this matrix,
//! because `const` is a permanent supported mode rather than a scaffold, and a
//! mode nobody exercises is a mode that quietly stops working.
//!
//! Two properties, and the split between them is the point:
//!
//! - **every** fixture compiles under `runtime` — that is what the port bought;
//! - exactly the three §2.3 fixtures are rejected under `const`, with
//!   `FRS-SFIR-0004`. Pinning *which* three matters in both directions: a
//!   fourth would be a regression in the folding path, and a missing one would
//!   mean the compile-time interpreter silently grew a capability the plan says
//!   it does not have.

use std::path::{Path, PathBuf};

use compiler::{Compiler, SignalFirLane, TableInitMode};

/// Fixtures requiring an explicit compile-time SR or unsupported because they
/// call a foreign function in the fill body.
const CONST_MODE_UNFOLDABLE: [&str; 3] =
    ["f02_subcontainer1", "f03_sr_dependent", "f09_ffunction_gen"];

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../porting/generated/siggen-table-init-s0/dsp")
        .canonicalize()
        .expect("the frozen S0 fixture directory must exist")
}

fn fixtures() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(fixture_dir())
        .expect("fixture directory must be readable")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "dsp" {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some((name, path))
        })
        .collect();
    out.sort();
    assert_eq!(
        out.len(),
        13,
        "the S0 baseline is 13 fixtures; the matrix and its expectations were \
         frozen together, so a change in count needs the plan updated too"
    );
    out
}

/// Compiles on a worker thread with the compiler's stack contract.
///
/// Library expansion and structural lowering recurse deeply even when the final
/// FIR is small — `f01` imports `stdfaust.lib` — so the default test-thread
/// stack overflows. The CLI spawns the same 64 MiB thread for the same reason.
fn compile(path: &Path, mode: TableInitMode) -> Result<(), String> {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let search = vec![fixture_dir(), PathBuf::from("/usr/local/share/faust")];
            Compiler::new()
                .with_table_init_mode(mode)
                .compile_file_to_fir_with_lane(&path, &search, SignalFirLane::TransformFastLane)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
        .expect("worker thread must spawn")
        .join()
        .expect("compilation thread must not panic")
}

#[test]
fn every_fixture_compiles_in_runtime_mode() {
    for (name, path) in fixtures() {
        if let Err(error) = compile(&path, TableInitMode::Runtime) {
            panic!("{name} must compile with --table-init runtime: {error}");
        }
    }
}

#[test]
fn const_mode_rejects_exactly_the_three_documented_fixtures() {
    for (name, path) in fixtures() {
        let expected_unfoldable = CONST_MODE_UNFOLDABLE.contains(&name.as_str());
        match compile(&path, TableInitMode::Const) {
            Ok(()) => assert!(
                !expected_unfoldable,
                "{name} is documented in plan §2.3 as unfoldable, but const mode \
                 accepted it; either the interpreter grew a capability the plan \
                 does not record, or the fixture stopped exercising its gap"
            ),
            Err(error) => {
                assert!(
                    expected_unfoldable,
                    "{name} used to fold at compile time and no longer does; \
                     const mode is a permanent supported mode, so this is a \
                     regression, not an expected outcome: {error}"
                );
                assert!(
                    error.contains("FRS-SFIR-0004"),
                    "{name} must be rejected with the documented FRS-SFIR-0004 \
                     code so `--table-init runtime` can be suggested: {error}"
                );
            }
        }
    }
}

#[test]
fn const_mode_folds_sample_rate_generators_with_an_explicit_rate() {
    for name in ["f02_subcontainer1", "f03_sr_dependent"] {
        let path = fixture_dir().join(format!("{name}.dsp"));
        let result = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                Compiler::new()
                    .with_table_init_mode(TableInitMode::Const)
                    .with_table_init_sample_rate(48_000)
                    .compile_file_to_fir_with_lane(
                        &path,
                        &[fixture_dir(), PathBuf::from("/usr/local/share/faust")],
                        SignalFirLane::TransformFastLane,
                    )
            })
            .expect("worker thread must spawn")
            .join()
            .expect("compilation thread must not panic");
        assert!(
            result.is_ok(),
            "{name} must fold with an explicit SR: {result:?}"
        );
    }
}
