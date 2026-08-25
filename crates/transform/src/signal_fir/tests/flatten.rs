//! Flattening of generator sub-modules (`fir::subcontainer`).
//!
//! These run the real sub-module producer and then flatten its output,
//! rather than
//! hand-building a module: the pass exists to consume what the producer emits,
//! and a hand-built fixture would only prove the pass agrees with my idea of
//! that shape.

use super::fixtures::*;
use crate::signal_fir::{SignalFirOptions, TableInitMode};
use fir::subcontainer::{SubModuleStatePolicy, flatten_sub_modules, verify_flattened};
use fir::{FirMatch, match_fir};
use signals::SigBuilder;
use tlib::TreeArena;

/// Lowers `rdtable(size, content, 0)` with the sub-module producer enabled.
fn runtime_table_module(size: i32) -> crate::signal_fir::SignalFirOutput {
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        let content = b.real(0.5);
        let size_sig = b.int(size);
        let idx = b.int(0);
        b.read_only_table(size_sig, content, idx)
    };
    let options = SignalFirOptions {
        table_init_mode: TableInitMode::Runtime,
        ..SignalFirOptions::default()
    };
    compile_fastlane_without_ui(&arena, &[sig], 0, 1, &options)
        .expect("runtime table init should lower")
}

/// Lowers a nested generated table: the generator of the outer table reads
/// another generated table.
fn nested_table_module() -> crate::signal_fir::SignalFirOutput {
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        let inner_content = b.real(0.25);
        let inner_size = b.int(4);
        let inner_idx = b.int(0);
        let inner = b.read_only_table(inner_size, inner_content, inner_idx);
        let outer_size = b.int(4);
        let outer_idx = b.int(0);
        b.read_only_table(outer_size, inner, outer_idx)
    };
    let options = SignalFirOptions {
        table_init_mode: TableInitMode::Runtime,
        ..SignalFirOptions::default()
    };
    compile_fastlane_without_ui(&arena, &[sig], 0, 1, &options)
        .expect("nested runtime table init should lower")
}

/// Counts the sub-modules a module still declares.
fn sub_module_count(out: &crate::signal_fir::SignalFirOutput, module: fir::FirId) -> usize {
    let FirMatch::Module { sub_modules, .. } = match_fir(&out.store, module) else {
        panic!("module expected");
    };
    match match_fir(&out.store, sub_modules) {
        FirMatch::Block(items) => items.len(),
        _ => 0,
    }
}

#[test]
fn flattening_removes_the_sub_module_under_both_policies() {
    for policy in [
        SubModuleStatePolicy::StackLocals,
        SubModuleStatePolicy::MergedStructFields,
    ] {
        let mut out = runtime_table_module(8);
        assert_eq!(
            sub_module_count(&out, out.module),
            1,
            "producer precondition"
        );

        let flattened = flatten_sub_modules(&mut out.store, out.module, policy)
            .unwrap_or_else(|err| panic!("flattening failed under {policy:?}: {err}"));

        assert_eq!(
            sub_module_count(&out, flattened),
            0,
            "{policy:?} must leave no sub-module declared"
        );
        let report = verify_flattened(&out.store, flattened);
        assert!(
            report.is_clean(),
            "{policy:?} produced a structurally broken module: {:?}",
            report.problems
        );
    }
}

#[test]
fn flattened_module_still_passes_fir_verification() {
    // Flattening rewrites lifecycle bodies; the ordinary FIR checker must
    // still accept the result, including its variable declarations and types.
    for policy in [
        SubModuleStatePolicy::StackLocals,
        SubModuleStatePolicy::MergedStructFields,
    ] {
        let mut out = runtime_table_module(8);
        let flattened = flatten_sub_modules(&mut out.store, out.module, policy)
            .expect("flattening should succeed");
        let report = fir::checker::verify_fir_module(&out.store, flattened);
        let errors: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.severity == fir::checker::Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "{policy:?} flattened module rejected by the FIR checker: {errors:?}"
        );
    }
}

#[test]
fn nested_generators_are_flattened_too() {
    // The inner generator lives inside the outer sub-module, so a pass that
    // only walked the top level would leave a `fill` call with no callee.
    for policy in [
        SubModuleStatePolicy::StackLocals,
        SubModuleStatePolicy::MergedStructFields,
    ] {
        let mut out = nested_table_module();
        let flattened = flatten_sub_modules(&mut out.store, out.module, policy)
            .unwrap_or_else(|err| panic!("nested flattening failed under {policy:?}: {err}"));
        let report = verify_flattened(&out.store, flattened);
        assert!(
            report.is_clean(),
            "{policy:?} left the nested generator unflattened: {:?}",
            report.problems
        );
    }
}

#[test]
fn a_module_without_generators_is_returned_unchanged() {
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        b.input(0)
    };
    let mut out = compile_fastlane_without_ui(&arena, &[sig], 1, 1, &SignalFirOptions::default())
        .expect("passthrough should lower");
    let before = out.module;
    let after = flatten_sub_modules(&mut out.store, before, SubModuleStatePolicy::StackLocals)
        .expect("flattening a module without generators must succeed");
    assert_eq!(
        before, after,
        "a module with no sub-modules must not be rebuilt"
    );
}
