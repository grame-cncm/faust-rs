//! Tests for `vector::module` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use crate::signal_fir::ControlRateMode;
use fir::{AccessType, FirBuilder, FirStore, FirType};

use super::check::{
    FinalModuleExpectations, verify_final_module, verify_mutable_table_attribution,
    verify_readonly_table_stores, verify_sound_field_immutability, verify_ui_write_attribution,
};
use super::model::VectorModuleFailure;
use super::*;
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::vector::analysis::EffectAtom;
use crate::signal_fir::vector::ui::build_vector_ui_fir;
use crate::signal_fir::vector::verify::VectorPlan;
use crate::signal_fir::{ComputeMode, VectorFallbackReason, VectorPipelineStatus};
use crate::signal_prepare::VerifiedPreparedSignals;
use fir::checker::verify_fir_module;
use propagate::ClockDomainTable;
use signals::SigBuilder;
use signals::SigId;
use tlib::TreeArena;

use crate::signal_prepare::prepare_signals_for_fir_verified;

use super::super::verify::{
    Placement, Rate, SignalRecord, VECTOR_PLAN_SCHEMA_VERSION, ValueType, Vectorability,
};

#[test]
fn final_materialization_removes_pure_drops_but_retains_foreign_calls() {
    let mut store = FirStore::new();
    let (pure_drop, foreign_drop) = {
        let mut builder = FirBuilder::new(&mut store);
        let base = builder.float32(2.0);
        let exponent = builder.float32(3.0);
        let pure_math = builder.math_call(fir::FirMathOp::Pow, &[base, exponent], FirType::Float32);
        let foreign = builder.fun_call("observable", &[], FirType::Float32);
        (builder.drop_(pure_math), builder.drop_(foreign))
    };
    let mut statements = vec![pure_drop, foreign_drop];

    remove_pure_drop_roots(&store, &mut statements);

    assert_eq!(statements, vec![foreign_drop]);
}

/// A plan claiming no UI writes, for fixtures whose programs have no UI.
fn empty_ui_plan() -> VectorPlan {
    VectorPlan {
        schema_version: VECTOR_PLAN_SCHEMA_VERSION,
        vec_size: 32,
        signals: Vec::new(),
        loops: Vec::new(),
        epochs: Vec::new(),
        transports: Vec::new(),
        data_edges: Vec::new(),
        effect_edges: Vec::new(),
        vec_safe_witnesses: Vec::new(),
        fused_serial_groups: Vec::new(),
        lockstep_bundles: Vec::new(),
    }
}

#[test]
fn a_store_into_a_sound_field_is_rejected() {
    let mut store = FirStore::new();
    let (field, clean_compute, storing_compute) = {
        let mut b = FirBuilder::new(&mut store);
        let field = b.declare_var("fSound0", FirType::Sound, AccessType::Struct, None);
        let load = b.load_var("fSound0", AccessType::Struct, FirType::Sound);
        let drop_load = b.drop_(load);
        let clean_compute = b.block(&[drop_load]);
        let value = b.float64(0.0);
        let forged = b.store_var("fSound0", AccessType::Struct, value);
        let storing_compute = b.block(&[forged]);
        (field, clean_compute, storing_compute)
    };
    verify_sound_field_immutability(&store, clean_compute, &[field])
        .expect("reading a soundfile field is accepted");
    let rejected = verify_sound_field_immutability(&store, storing_compute, &[field])
        .expect_err("a store into a soundfile field must be rejected");
    assert!(rejected.detail.contains("fSound0"), "{rejected}");
}

#[test]
fn mutable_table_attribution_must_match_the_emitted_stores_and_init() {
    let mut store = FirStore::new();
    let table_id = 55_u64;
    let name = super::super::lower::mutable_table_name(table_id, &FirType::Float64);
    let (declaration, compute_with_store, compute_empty, full_init, partial_init) = {
        let mut b = FirBuilder::new(&mut store);
        let declaration = b.declare_var(
            name.clone(),
            FirType::Array(Box::new(FirType::Float64), 3),
            AccessType::Struct,
            None,
        );
        let index = b.int32(1);
        let value = b.float64(0.5);
        let write = b.store_table(name.clone(), AccessType::Struct, index, value);
        let compute_with_store = b.block(&[write]);
        let compute_empty = b.block(&[]);
        let inits = (0..3)
            .map(|cell| {
                let position = b.int32(cell);
                let content = b.float64(f64::from(cell));
                b.store_table(name.clone(), AccessType::Struct, position, content)
            })
            .collect::<Vec<_>>();
        let full_init = b.block(&inits);
        let partial_init = b.block(&inits[..2]);
        (
            declaration,
            compute_with_store,
            compute_empty,
            full_init,
            partial_init,
        )
    };
    let signal = |signal_id: u64, direct: Vec<EffectAtom>| SignalRecord {
        signal_id,
        value_type: ValueType::Real,
        structural: false,
        rate: Rate::Samp,
        vectorability: Vectorability::Scal,
        clock_id: 0,
        effects: direct.clone(),
        direct_effects: direct,
        placement: Placement::Owned(0),
        duplicable: false,
    };
    let plan = |signals: Vec<SignalRecord>| VectorPlan {
        signals,
        ..empty_ui_plan()
    };
    let writer = plan(vec![signal(
        table_id,
        vec![EffectAtom::WriteTable(u32::try_from(table_id).unwrap())],
    )]);

    verify_mutable_table_attribution(
        &store,
        compute_with_store,
        full_init,
        &[declaration],
        &writer,
    )
    .expect("one claimed writer, one store, complete init");

    let missing_store =
        verify_mutable_table_attribution(&store, compute_empty, full_init, &[declaration], &writer)
            .expect_err("a claimed writer with no emitted store must be rejected");
    assert!(
        missing_store.detail.contains("stores it 0 times"),
        "{missing_store}"
    );

    let unclaimed = verify_mutable_table_attribution(
        &store,
        compute_with_store,
        full_init,
        &[declaration],
        &plan(Vec::new()),
    )
    .expect_err("an emitted store with no claimed writer must be rejected");
    assert!(
        unclaimed.detail.contains("no matching claimed writer"),
        "{unclaimed}"
    );

    let double_claim = verify_mutable_table_attribution(
        &store,
        compute_with_store,
        full_init,
        &[declaration],
        &plan(vec![
            signal(
                table_id,
                vec![EffectAtom::WriteTable(u32::try_from(table_id).unwrap())],
            ),
            signal(
                table_id + 1,
                vec![EffectAtom::WriteTable(u32::try_from(table_id).unwrap())],
            ),
        ]),
    )
    .expect_err("two claimed writers for one store must be rejected");
    assert!(
        double_claim.detail.contains("2 claimed writers"),
        "{double_claim}"
    );

    let incomplete = verify_mutable_table_attribution(
        &store,
        compute_with_store,
        partial_init,
        &[declaration],
        &writer,
    )
    .expect_err("an init that skips one cell must be rejected");
    assert!(incomplete.detail.contains("covers 2 of 3"), "{incomplete}");
}

#[test]
fn ui_write_attribution_must_match_the_emitted_zone_stores() {
    let mut ui = ui::UiProgram::empty();
    ui.controls.push(ui::ControlSpec {
        id: 2,
        kind: ui::ControlKind::VBargraph,
        label: "meter".to_owned(),
        metadata: ui::UiMetadata::default(),
        range: None,
    });
    let mut store = FirStore::new();
    let compute = {
        let mut b = FirBuilder::new(&mut store);
        let value = b.float64(1.0);
        let write = b.store_var("fVbargraph2", AccessType::Struct, value);
        b.block(&[write])
    };
    let signal = |signal_id: u64, direct: Vec<EffectAtom>| SignalRecord {
        signal_id,
        value_type: ValueType::Real,
        structural: false,
        rate: Rate::Samp,
        vectorability: Vectorability::Scal,
        clock_id: 0,
        effects: direct.clone(),
        direct_effects: direct,
        placement: Placement::Owned(0),
        duplicable: false,
    };
    let plan = |signals: Vec<SignalRecord>| VectorPlan {
        schema_version: VECTOR_PLAN_SCHEMA_VERSION,
        vec_size: 32,
        signals,
        loops: Vec::new(),
        epochs: Vec::new(),
        transports: Vec::new(),
        data_edges: Vec::new(),
        effect_edges: Vec::new(),
        vec_safe_witnesses: Vec::new(),
        fused_serial_groups: Vec::new(),
        lockstep_bundles: Vec::new(),
    };

    // One performer, one physical store.
    verify_ui_write_attribution(
        &store,
        compute,
        &ui,
        &plan(vec![signal(1, vec![EffectAtom::WriteUi(2)])]),
    )
    .expect("one claimed writer matches one emitted zone store");

    // A carrier promoted to a performer: two claims, one store.
    let promoted = verify_ui_write_attribution(
        &store,
        compute,
        &ui,
        &plan(vec![
            signal(1, vec![EffectAtom::WriteUi(2)]),
            signal(2, vec![EffectAtom::WriteUi(2)]),
        ]),
    )
    .expect_err("a carrier claiming a zone write it does not perform must be rejected");
    assert!(promoted.detail.contains("2 claimed writers"), "{promoted}");

    // The performer demoted to a carrier: no claim, one store.
    let demoted = verify_ui_write_attribution(&store, compute, &ui, &plan(Vec::new()))
        .expect_err("an emitted zone store with no claimed writer must be rejected");
    assert!(
        demoted.detail.contains("no matching claimed writer"),
        "{demoted}"
    );
}

#[test]
fn a_store_into_a_declared_readonly_table_is_rejected() {
    let mut store = FirStore::new();
    let (declaration, clean_compute, storing_compute) = {
        let mut b = FirBuilder::new(&mut store);
        let first = b.int32(10);
        let second = b.int32(20);
        let declaration = b.declare_table(
            "iTbl0",
            AccessType::Static,
            FirType::Int32,
            &[first, second],
        );
        let index = b.int32(1);
        let value = b.int32(99);
        let load = b.load_table("iTbl0", AccessType::Static, index, FirType::Int32);
        let drop_load = b.drop_(load);
        let clean_compute = b.block(&[drop_load]);
        let forged = b.store_table("iTbl0", AccessType::Static, index, value);
        let storing_compute = b.block(&[forged]);
        (declaration, clean_compute, storing_compute)
    };
    verify_readonly_table_stores(&store, clean_compute, &[declaration])
        .expect("reading a read-only table is accepted");
    let rejected = verify_readonly_table_stores(&store, storing_compute, &[declaration])
        .expect_err("a store into a declared read-only table must be rejected");
    assert!(
        rejected.detail.contains("iTbl0"),
        "the rejection must name the table, got {}",
        rejected.detail
    );
}

fn raw_pure_fixture() -> (TreeArena, Vec<SigId>) {
    let mut arena = TreeArena::new();
    let roots = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let gain = builder.real(0.5);
        let value = builder.binop(signals::BinOp::Mul, input, gain);
        vec![builder.output(0, value)]
    };
    (arena, roots)
}

fn pure_fixture() -> VerifiedPreparedSignals {
    let (arena, roots) = raw_pure_fixture();
    prepare_signals_for_fir_verified(&arena, &roots, &ui::UiProgram::empty())
        .expect("prepare pure fixture")
}

fn delay_fixture() -> VerifiedPreparedSignals {
    let mut arena = TreeArena::new();
    let roots = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let amount = builder.int(2);
        let delayed = builder.delay(input, amount);
        vec![builder.output(0, delayed)]
    };
    prepare_signals_for_fir_verified(&arena, &roots, &ui::UiProgram::empty())
        .expect("prepare delay fixture")
}

fn recursion_fixture() -> VerifiedPreparedSignals {
    let mut arena = TreeArena::new();
    let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
    let body = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let feedback = builder.proj(0, self_ref);
        let previous = builder.delay1(feedback);
        builder.binop(signals::BinOp::Add, input, previous)
    };
    let nil = arena.nil();
    let bodies = arena.cons(body, nil);
    let group = tlib::de_bruijn_rec(&mut arena, bodies);
    let root = {
        let mut builder = SigBuilder::new(&mut arena);
        let projection = builder.proj(0, group);
        builder.output(0, projection)
    };
    prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty())
        .expect("prepare recursion fixture")
}

fn module_context<'a>(
    domains: &'a ClockDomainTable,
    ui: &'a ui::UiProgram,
    module_name: &'a str,
    loop_variant: u8,
    strategy: SchedulingStrategy,
) -> VectorModuleContext<'a> {
    VectorModuleContext {
        domains,
        ui,
        num_inputs: 1,
        num_outputs: 1,
        module_name,
        real_type: FirType::Float32,
        max_copy_delay: 16,
        compute_mode: ComputeMode::Vector {
            vec_size: 8,
            loop_variant,
        },
        strategy,
        control_rate_mode: ControlRateMode::InlinePerBlock,
    }
}

#[test]
fn final_module_covers_lifecycle_outputs_and_both_chunk_drivers() {
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        for loop_variant in [0, 1] {
            let prepared = pure_fixture();
            let domains = ClockDomainTable::new();
            let ui = ui::UiProgram::empty();
            let output = build_verified_vector_module(
                &prepared,
                &module_context(&domains, &ui, "mydsp", loop_variant, strategy),
            )
            .expect("verified module");
            assert_eq!(
                output.vector_pipeline_status,
                VectorPipelineStatus::Certified
            );
            let report = verify_fir_module(&output.store, output.module);
            assert!(!report.has_errors());
            assert!(
                !report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "FIR-D01"),
                "certified vector FIR retained a pure Drop root: {report:?}"
            );
        }
    }
}

#[test]
fn production_selector_certifies_pure_and_fixed_delay() {
    let options = super::super::super::SignalFirOptions {
        compute_mode: ComputeMode::Vector {
            vec_size: 8,
            loop_variant: 0,
        },
        ..super::super::super::SignalFirOptions::default()
    };
    let (arena, roots) = raw_pure_fixture();
    let pure = super::super::super::compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &roots,
        1,
        1,
        &ui::UiProgram::empty(),
        &options,
    )
    .expect("production pure vector compile");
    assert_eq!(pure.vector_pipeline_status, VectorPipelineStatus::Certified);

    let mut arena = TreeArena::new();
    let roots = {
        let mut builder = SigBuilder::new(&mut arena);
        let input = builder.input(0);
        let amount = builder.int(2);
        let delayed = builder.delay(input, amount);
        vec![builder.output(0, delayed)]
    };
    let stateful = super::super::super::compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &roots,
        1,
        1,
        &ui::UiProgram::empty(),
        &options,
    )
    .expect("transitional stateful vector compile");
    assert_eq!(
        stateful.vector_pipeline_status,
        VectorPipelineStatus::Certified
    );
}

#[test]
fn fixed_delay_enters_checked_final_module() {
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        for loop_variant in [0, 1] {
            let prepared = delay_fixture();
            let domains = ClockDomainTable::new();
            let ui = ui::UiProgram::empty();
            let output = build_verified_vector_module(
                &prepared,
                &module_context(&domains, &ui, "delaydsp", loop_variant, strategy),
            )
            .expect("fixed delay verified module");
            assert_eq!(
                output.vector_pipeline_status,
                VectorPipelineStatus::Certified
            );
        }
    }
}

#[test]
fn recursion_enters_checked_final_module() {
    for strategy in [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ] {
        for loop_variant in [0, 1] {
            let prepared = recursion_fixture();
            let domains = ClockDomainTable::new();
            let ui = ui::UiProgram::empty();
            build_verified_vector_module(
                &prepared,
                &module_context(&domains, &ui, "recdsp", loop_variant, strategy),
            )
            .expect("recursion verified module");
        }
    }
}

#[test]
fn final_checker_rejects_forged_output_coverage() {
    let prepared = pure_fixture();
    let domains = ClockDomainTable::new();
    let ui = ui::UiProgram::empty();
    let mut built = build_verified_vector_module_with_evidence(
        &prepared,
        &module_context(&domains, &ui, "mydsp", 0, SchedulingStrategy::DepthFirst),
    )
    .expect("verified module with evidence");
    assert_eq!(built.output_stores.len(), 1);
    let forged = FirBuilder::new(&mut built.output.store).int32(0);
    let ui_fir = build_vector_ui_fir(
        &ui::UiProgram::empty(),
        &FirType::Float32,
        &mut built.output.store,
    )
    .expect("empty UI evidence");
    assert!(matches!(
        verify_final_module(
            &built.output.store,
            built.output.module,
            &FinalModuleExpectations {
                assembly: &built.assembly,
                output_stores: &[forged],
                ui_fir: &ui_fir,
                static_declarations: &[],
                table_declarations: &[],
                ui: &ui::UiProgram::empty(),
                plan: &empty_ui_plan(),
            },
        ),
        Err(VectorModuleFailure {
            reason: VectorFallbackReason::ModuleVerification,
            ..
        })
    ));
}

#[test]
fn final_checker_rejects_forged_static_declaration_coverage() {
    let prepared = pure_fixture();
    let domains = ClockDomainTable::new();
    let ui = ui::UiProgram::empty();
    let mut built = build_verified_vector_module_with_evidence(
        &prepared,
        &module_context(&domains, &ui, "mydsp", 0, SchedulingStrategy::DepthFirst),
    )
    .expect("verified module with evidence");
    let output_stores = built.output_stores.clone();
    let forged = FirBuilder::new(&mut built.output.store).declare_table(
        "forged",
        AccessType::Static,
        FirType::Float32,
        &[],
    );
    let ui_fir = build_vector_ui_fir(
        &ui::UiProgram::empty(),
        &FirType::Float32,
        &mut built.output.store,
    )
    .expect("empty UI evidence");
    assert!(matches!(
        verify_final_module(
            &built.output.store,
            built.output.module,
            &FinalModuleExpectations {
                assembly: &built.assembly,
                output_stores: &output_stores,
                ui_fir: &ui_fir,
                static_declarations: &[forged],
                table_declarations: &[],
                ui: &ui::UiProgram::empty(),
                plan: &empty_ui_plan(),
            },
        ),
        Err(VectorModuleFailure {
            reason: VectorFallbackReason::ModuleVerification,
            ..
        })
    ));
}
