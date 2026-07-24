//! Top-level vector module orchestration (producer): runs the full
//! checked pipeline stage chain and carries the fail-closed fallback
//! evidence. Its terminal step calls `check::verify_final_module`, so
//! every admission guard there also binds the producer (plan §4.8).
//! `reject_cross_loop_delay_read_transports` is a producer-side
//! admission guard with its single call site here.

use super::check::{FinalModuleExpectations, module_shape, verify_final_module};
use super::lifecycle::{FinalModuleContext, assemble_module};
use super::model::VectorModuleFailure;
use super::outputs::{OutputMaterialization, materialize_outputs};
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::vector::analysis::DepKind;
use crate::signal_fir::vector::assemble::{
    VectorFirAssembly, VectorLoopFirInput, assemble_vector_fir,
};
use crate::signal_fir::vector::clock_ad::build_vector_clock_ad_plan;
use crate::signal_fir::vector::decoration_verify::{
    VerifiedDecorationCertificate, certify_decorations,
};
use crate::signal_fir::vector::events::{
    DEFAULT_EVENT_LIMITS, build_state_event_order_certificate, precheck_state_event_bound,
};
use crate::signal_fir::vector::lower::{VectorLoweringContext, lower_vector_program};
use crate::signal_fir::vector::plan::build_vector_plan_with_lockstep;
use crate::signal_fir::vector::state::build_vector_state_plan_with_clock;
use crate::signal_fir::vector::ui::build_vector_ui_fir;
use crate::signal_fir::vector::verify::VectorPlan;
use crate::signal_fir::{
    ComputeMode, SignalFirOutput, VectorEffectiveMode, VectorFallbackReason, VectorPipelineStatus,
};
use crate::signal_prepare::VerifiedPreparedSignals;
use fir::inliner::is_obviously_side_effect_free_value;
use fir::{FirId, FirMatch, FirStore, FirType, match_fir};
use propagate::ClockDomainTable;

/// Runs the complete checked vector path for the supported P6.5 subset and
/// returns a final FIR module.
pub(crate) struct VectorModuleContext<'a> {
    pub domains: &'a ClockDomainTable,
    pub ui: &'a ui::UiProgram,
    pub num_inputs: usize,
    pub num_outputs: usize,
    pub module_name: &'a str,
    pub real_type: FirType,
    pub max_copy_delay: u32,
    pub compute_mode: ComputeMode,
    pub strategy: SchedulingStrategy,
}
pub(crate) fn build_verified_vector_module(
    prepared: &VerifiedPreparedSignals,
    context: &VectorModuleContext<'_>,
) -> Result<SignalFirOutput, VectorModuleFailure> {
    let built = build_verified_vector_module_with_evidence(prepared, context)?;
    let BuiltVectorModule {
        output,
        assembly,
        output_stores,
    } = built;
    if assembly.schema_version != crate::signal_fir::vector::assemble::VECTOR_FIR_ASSEMBLY_VERSION
        || output_stores.len() != context.num_outputs
    {
        return Err(module_shape("verified module evidence lost final coverage"));
    }
    Ok(output)
}
pub(super) struct BuiltVectorModule {
    pub(super) output: SignalFirOutput,
    pub(super) assembly: VectorFirAssembly,
    pub(super) output_stores: Vec<FirId>,
}
pub(super) fn build_verified_vector_module_with_evidence(
    prepared: &VerifiedPreparedSignals,
    context: &VectorModuleContext<'_>,
) -> Result<BuiltVectorModule, VectorModuleFailure> {
    let domains = context.domains;
    let ui = context.ui;
    let num_inputs = context.num_inputs;
    let num_outputs = context.num_outputs;
    let module_name = context.module_name;
    let real_type = context.real_type.clone();
    let max_copy_delay = context.max_copy_delay;
    let compute_mode = context.compute_mode;
    let strategy = context.strategy;
    let ComputeMode::Vector {
        vec_size,
        loop_variant,
    } = compute_mode
    else {
        return Err(VectorModuleFailure::new(
            VectorFallbackReason::VectorPlan,
            "checked vector module requested for scalar compute mode",
        ));
    };
    let timing_enabled = std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some();
    let mut stage_started = std::time::Instant::now();
    let mut trace_stage = |stage: &str| {
        if timing_enabled {
            eprintln!(
                "[vector-stage] {stage}: {:.3}s",
                stage_started.elapsed().as_secs_f64()
            );
        }
        stage_started = std::time::Instant::now();
    };

    let clocks = crate::clk_env::annotate(prepared.arena(), domains, prepared.outputs()).map_err(
        |error| VectorModuleFailure::new(VectorFallbackReason::ClockAnalysis, error.to_string()),
    )?;
    trace_stage("clock-analysis");
    let decorations = certify_decorations(prepared, &clocks).map_err(|error| {
        VectorModuleFailure::new(VectorFallbackReason::Decorations, error.to_string())
    })?;
    trace_stage("decorations");
    let vector_plan = build_vector_plan_with_lockstep(prepared, &decorations, u64::from(vec_size))
        .map_err(|error| {
            VectorModuleFailure::new(VectorFallbackReason::VectorPlan, error.to_string())
        })?;
    trace_stage("vector-plan");
    reject_cross_loop_delay_read_transports(&decorations, vector_plan.plan())?;
    trace_stage("recursive-transport-guard");
    let clock_plan = build_vector_clock_ad_plan(prepared, domains, &decorations, &vector_plan)
        .map_err(|error| {
            VectorModuleFailure::new(VectorFallbackReason::ClockAdPlan, error.to_string())
        })?;
    if let Some(fallback) = clock_plan.plan().reverse_ad_fallbacks.first() {
        return Err(VectorModuleFailure::new(
            VectorFallbackReason::ReverseAd,
            format!(
                "{}: signal {} requires fixed {:?} epochs",
                fallback.diagnostic.message(),
                fallback.signal_id,
                fallback.epochs
            ),
        ));
    }
    trace_stage("clock-ad-plan");
    let state_plan = build_vector_state_plan_with_clock(
        prepared,
        &decorations,
        &vector_plan,
        &clock_plan,
        u64::from(max_copy_delay),
    )
    .map_err(|error| {
        VectorModuleFailure::new(VectorFallbackReason::StatePlan, error.to_string())
    })?;
    trace_stage("state-plan");
    precheck_state_event_bound(&vector_plan, &state_plan, DEFAULT_EVENT_LIMITS).map_err(
        |error| VectorModuleFailure::new(VectorFallbackReason::EventCertificate, error.to_string()),
    )?;
    trace_stage("event-bound-precheck");
    let mut program = lower_vector_program(
        prepared,
        &vector_plan,
        &state_plan,
        &clock_plan,
        &VectorLoweringContext {
            ui,
            strategy,
            real_type: real_type.clone(),
            num_inputs,
        },
    )
    .map_err(|error| {
        VectorModuleFailure::new(VectorFallbackReason::PureLowering, error.to_string())
    })?;
    trace_stage("vector-lowering");
    build_state_event_order_certificate(
        &vector_plan,
        program.routed(),
        &state_plan,
        DEFAULT_EVENT_LIMITS,
    )
    .map_err(|error| {
        VectorModuleFailure::new(VectorFallbackReason::EventCertificate, error.to_string())
    })?;
    trace_stage("event-certificate");

    let routed = program.routed().clone();
    let mut loop_inputs = program
        .regions()
        .iter()
        .map(|region| VectorLoopFirInput {
            loop_id: region.loop_id(),
            statements: region.statements().to_vec(),
        })
        .collect::<Vec<_>>();
    let mut control_statements = program.control_statements().to_vec();
    let math_ops = program.math_ops().clone();
    let int_helpers = program.int_helpers().clone();
    let mut control_output_stores = Vec::new();
    let mut clock_output_stores = Vec::new();
    let output_stores = materialize_outputs(
        prepared.outputs(),
        num_outputs,
        &mut OutputMaterialization {
            routed: &routed,
            loop_inputs: &mut loop_inputs,
            control_statements: &mut control_statements,
            control_output_stores: &mut control_output_stores,
            clock_output_stores: &mut clock_output_stores,
            clock_plan: &clock_plan,
            store: program.store_mut(),
        },
    )?;
    trace_stage("output-materialization");
    remove_pure_drop_roots(program.store(), &mut control_statements);
    for input in &mut loop_inputs {
        remove_pure_drop_roots(program.store(), &mut input.statements);
    }
    let assembly = assemble_vector_fir(
        &routed,
        Some(&state_plan),
        Some(&clock_plan),
        &loop_inputs,
        &clock_output_stores,
        real_type.clone(),
        program.store_mut(),
    )
    .map_err(|error| {
        VectorModuleFailure::new(VectorFallbackReason::FirAssembly, error.to_string())
    })?;
    trace_stage("fir-assembly");

    let assembly = assembly.into_assembly();
    let ui_fir = build_vector_ui_fir(ui, &real_type, program.store_mut())
        .map_err(|error| VectorModuleFailure::new(VectorFallbackReason::UiProgram, error))?;
    trace_stage("ui-lifecycle");
    let static_declarations = program.static_declarations().to_vec();
    let table_declarations = program.table_declarations().to_vec();
    let table_init_statements = program.table_init_statements().to_vec();
    let module_context = FinalModuleContext {
        module_name,
        num_inputs,
        num_outputs,
        real_type: &real_type,
        vec_size,
        loop_variant,
        control_statements: &control_statements,
        table_declarations: &table_declarations,
        table_init_statements: &table_init_statements,
        math_ops: &math_ops,
        int_helpers: &int_helpers,
        assembly: &assembly,
        control_output_stores: &control_output_stores,
        ui_fir: &ui_fir,
        static_declarations: &static_declarations,
    };
    let module = assemble_module(program.store_mut(), &module_context)?;
    verify_final_module(
        program.store(),
        module,
        &FinalModuleExpectations {
            assembly: &assembly,
            output_stores: &output_stores,
            ui_fir: &ui_fir,
            static_declarations: &static_declarations,
            table_declarations: &table_declarations,
            ui,
            plan: routed.plan(),
        },
    )?;
    trace_stage("module-assembly-verification");

    Ok(BuiltVectorModule {
        output: SignalFirOutput {
            store: program.into_store(),
            module,
            emission_order: Vec::new(),
            shadow_report: None,
            vector_pipeline_status: VectorPipelineStatus::Certified,
            vector_effective_mode: VectorEffectiveMode::CertifiedVector,
            vector_pipeline_detail: None,
        },
        assembly,
        output_stores,
    })
}

/// Removes the pure `Drop` roots used to seed CSE after the checked lowering
/// evidence and concrete output stores have consumed them.
pub(super) fn remove_pure_drop_roots(store: &FirStore, statements: &mut Vec<FirId>) {
    statements.retain(|&statement| {
        !matches!(
            match_fir(store, statement),
            FirMatch::Drop(value) if is_obviously_side_effect_free_value(store, value)
        )
    });
}

pub(super) fn reject_cross_loop_delay_read_transports(
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
) -> Result<(), VectorModuleFailure> {
    let recursive_delayed_carriers = decorations
        .certificate()
        .records
        .iter()
        .filter(|record| record.max_delay > 0 && record.recursive_projection.is_some())
        .map(|record| u64::from(record.signal_id))
        .collect::<std::collections::BTreeSet<_>>();
    for transport in &plan.transports {
        for dependency in decorations.certificate().dependencies.iter() {
            let carrier_signal_id = u64::from(dependency.to);
            if u64::from(dependency.from) != transport.signal_id
                || !recursive_delayed_carriers.contains(&carrier_signal_id)
                || !matches!(dependency.kind, DepKind::Delayed { .. })
            {
                continue;
            }
            let covered = plan.fused_serial_groups.iter().any(|group| {
                group
                    .internal_transport_ids
                    .binary_search(&transport.transport_id)
                    .is_ok()
                    && group
                        .delayed_read_signal_ids
                        .binary_search(&transport.signal_id)
                        .is_ok()
                    && group
                        .state_write_signal_ids
                        .binary_search(&carrier_signal_id)
                        .is_ok()
                    && group
                        .member_loop_ids
                        .binary_search(&transport.producer_loop)
                        .is_ok()
                    && group
                        .member_loop_ids
                        .binary_search(&transport.consumer_loop)
                        .is_ok()
            });
            if covered {
                continue;
            }
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                eprintln!(
                    "[vector-fused-uncovered] transport={:?} carrier={} groups={:?}",
                    transport, carrier_signal_id, plan.fused_serial_groups
                );
            }
            return Err(VectorModuleFailure::new(
                VectorFallbackReason::VectorPlan,
                format!(
                    "delayed recursive signal {} crosses vector loops {} -> {}; scalar fallback preserves recursive delay semantics",
                    transport.signal_id, transport.producer_loop, transport.consumer_loop
                ),
            ));
        }
    }
    Ok(())
}
