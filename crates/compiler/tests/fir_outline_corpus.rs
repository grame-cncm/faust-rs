//! The outlining pass over the corpus (plan
//! `porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §8.2).
//!
//! Every hermetic program of `tests/corpus` and `tests/impulse-tests/dsp` (no
//! `import`/`library`, so no installed Faust is needed, AGENTS.md §3; the
//! clocked programs with `If` blocks are in the second), and a few inline
//! `ondemand` programs shaped like the coopiano voice, go through
//! `fir::outline` with a
//! policy that outlines every `then`/`else` block it can (`budget` and
//! `min_block` 1). For each program:
//!
//! - the pass succeeds, so its independent checker accepted the output;
//! - the FIR verifier reports the same errors before and after (none, in
//!   practice);
//! - the Mojo backend emits the outlined program, scalar and batch, whenever
//!   it emits the program at all.
//!
//! The numeric check is the opt-in impulse lane `make mojo-outline` (Mojo
//! needed), which runs the same policy against the C++ oracle.

use codegen::backends::mojo::MojoOptions;
use compiler::{Compiler, SignalFirLane};
use fir::checker::{Severity, verify_fir_module};
use fir::outline::{OutlinePolicy, outline_large_blocks};
use fir::{FirId, FirStore};

const EVERY_BLOCK: OutlinePolicy = OutlinePolicy {
    budget: 1,
    min_block: 1,
    split_budget: usize::MAX,
};

/// Phase 2 as well (plan §3.4): every block over 8 statements cut into runs
/// of at most 8, escapes through spill arrays.
const EVERY_RUN: OutlinePolicy = OutlinePolicy {
    budget: 8,
    min_block: 1,
    split_budget: 8,
};

/// Programs with many small `ondemand` blocks inside the sample loop, as
/// `cp.on_change` builds them, and sibling domains sharing a payload.
const INLINE: &[(&str, &str)] = &[
    (
        "on_change_bank",
        r#"on_change(chg, x, f) = (chg, x) : ondemand(f);
key = hslider("key", 60, 21, 108, 1);
vel = hslider("vel", 0.5, 0, 1, 0.01);
chg = (key != key') | (vel != vel');
c(i) = on_change(chg, key, \(k).(0.9 + 0.09 * sin(k * 0.01 * (i + 1))));
g(i) = on_change(chg, key, \(k).(vel * cos(k * 0.02 * (i + 1))));
process = _ <: par(i, 8, *(g(i)) : + ~ *(c(i))) :> _;"#,
    ),
    (
        "sibling_phases",
        r#"decimal(x) = x - floor(x);
startup = 1 - (1');
phase(inc) = (select2(startup, +(inc), 0) : decimal) ~ _;
process = (button("a") : ondemand(phase(0.01))), (button("b") : ondemand(phase(0.02)));"#,
    ),
    (
        "held_gate",
        r#"gate = button("gate");
env = (gate, (gate : + ~ *(0.99))) : ondemand(\(x).(x * x + 1));
process = _ * env;"#,
    ),
];

/// The flattened module of `source`, as the Mojo backend sees it.
fn flat_module(name: &str, source: &str) -> Option<(FirStore, FirId)> {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(name, source, SignalFirLane::TransformFastLane)
        .ok()?;
    if fir::subcontainer::has_sub_modules(&fir.store, fir.module) {
        fir::subcontainer::flatten_sub_modules_owned(
            &fir.store,
            fir.module,
            fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
        )
        .ok()
    } else {
        let mut store = FirStore::new();
        let root = store.import_from(&fir.store, fir.module);
        Some((store, root))
    }
}

fn error_codes(store: &FirStore, root: FirId) -> Vec<String> {
    let mut codes: Vec<String> = verify_fir_module(store, root)
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code.to_string())
        .collect();
    codes.sort();
    codes
}

fn mojo(name: &str, source: &str, batch: bool, outline: Option<OutlinePolicy>) -> Option<String> {
    let options = MojoOptions {
        batch,
        outline,
        ..MojoOptions::default()
    };
    Compiler::new()
        .compile_source_to_mojo(name, source, &options)
        .ok()
}

/// Runs the checks of the module doc on one program, under `policy`;
/// returns the number of functions outlined, `None` when the program does
/// not lower to FIR.
fn check_with(name: &str, source: &str, policy: OutlinePolicy) -> Option<usize> {
    let (store, root) = flat_module(name, source)?;
    let (dst, droot, report) = outline_large_blocks(&store, root, policy)
        .unwrap_or_else(|e| panic!("{name}: outlining failed: {e}"));
    assert_eq!(
        error_codes(&store, root),
        error_codes(&dst, droot),
        "{name}: the FIR verifier sees the outlined module differently"
    );
    for batch in [false, true] {
        match mojo(name, source, batch, Some(policy)) {
            Some(text) => {
                if !report.outlined.is_empty() && !batch {
                    assert!(
                        text.contains("_outline0(mut "),
                        "{name}: no outlined method in the Mojo output"
                    );
                }
            }
            None => assert!(
                mojo(name, source, batch, None).is_none(),
                "{name}: the Mojo backend (batch {batch}) emits the program but not its \
                 outlined form"
            ),
        }
    }
    Some(report.outlined.len())
}

/// `(blocks, runs)`: the functions outlined under [`EVERY_BLOCK`] and
/// under [`EVERY_RUN`].
fn check(name: &str, source: &str) -> Option<(usize, usize)> {
    let blocks = check_with(name, source, EVERY_BLOCK)?;
    let runs = check_with(name, source, EVERY_RUN)?;
    Some((blocks, runs))
}

/// `(lowered, with blocks, block functions, run functions)` over the
/// hermetic programs of a directory of the repository.
fn check_directory(dir: &str) -> (usize, usize, usize, usize) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(dir);
    let mut paths: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dsp"))
        .collect();
    paths.sort();
    let (mut lowered, mut with_blocks, mut functions, mut runs) = (0, 0, 0, 0);
    for path in paths {
        let source = std::fs::read_to_string(&path).expect("corpus file");
        if source.contains("import(") || source.contains("library(") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if let Some((n, r)) = check(&name, &source) {
            lowered += 1;
            with_blocks += usize::from(n > 0);
            functions += n;
            runs += r;
        }
    }
    eprintln!(
        "{dir}: {lowered} programs lowered, {with_blocks} with blocks, {functions} block \
         functions, {runs} functions with runs"
    );
    (lowered, with_blocks, functions, runs)
}

#[test]
fn every_block_of_the_hermetic_corpus_outlines_exactly() {
    // Kept from going vacuous: the counts of 2026-11-09, rounded down.
    let (lowered, _, _, runs) = check_directory("tests/corpus");
    assert!(lowered >= 230, "{lowered} corpus programs lowered");
    assert!(runs >= 100, "{runs} functions with runs");
}

#[test]
fn every_block_of_the_hermetic_impulse_programs_outlines_exactly() {
    let (lowered, with_blocks, functions, runs) = check_directory("tests/impulse-tests/dsp");
    assert!(runs >= 50, "{runs} functions with runs");
    assert!(lowered >= 50, "{lowered} impulse programs lowered");
    assert!(
        with_blocks >= 5,
        "{with_blocks} impulse programs with an outlined block"
    );
    assert!(functions >= 6, "{functions} functions outlined");
}

#[test]
fn every_block_of_the_inline_ondemand_programs_outlines_exactly() {
    for &(name, source) in INLINE {
        let (n, _) = check(name, source).unwrap_or_else(|| panic!("{name} does not lower"));
        assert!(n > 0, "{name}: nothing outlined");
    }
}

/// Under the default policy the corpus is unchanged: no program of it is
/// near the budget.
#[test]
fn the_default_policy_leaves_the_inline_programs_whole() {
    for &(name, source) in INLINE {
        let (store, root) = flat_module(name, source).expect("lowers");
        let (_, _, report) =
            outline_large_blocks(&store, root, OutlinePolicy::default()).expect("outlining");
        assert!(report.outlined.is_empty(), "{name}");
    }
}
