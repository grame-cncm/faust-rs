//! Checked final-module integration for the signal-level vector pipeline.
//!
//! # C++ provenance and adaptation
//! The compute drivers mirror `VectorCodeContainer::processFIR` in the C++
//! compiler: `-lv 0` emits fixed-size chunks plus one remainder, while `-lv 1`
//! emits one min-bounded chunk loop. Lifecycle placement follows the common
//! Faust contract implemented by `CodeContainer`: persistent fields belong to
//! the DSP struct, constants to `instanceConstants`, resettable signal state to
//! `instanceClear`, and all chunk-local buffers to `compute`.
//!
//! Rust keeps this integration behind the complete P4/P5/P6 producer/checker
//! chain. The final FIR module is independently checked for lifecycle shape,
//! output coverage, inclusion of the accepted P6.3b body, and generic FIR type
//! and scope correctness before it can replace the transitional vector module.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use fir::checker::verify_fir_module;
use fir::{
    AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir,
};
use propagate::ClockDomainTable;
use signals::SigId;

use crate::schedule::SchedulingStrategy;
use crate::signal_prepare::VerifiedPreparedSignals;

use super::super::module::{INT_FUN_PROTO_ORDER, MATH_PROTO_ORDER};
use super::super::{
    ComputeMode, SignalFirOutput, VectorEffectiveMode, VectorFallbackReason, VectorPipelineStatus,
};
use super::decoration_verify::{VerifiedDecorationCertificate, certify_decorations};
use super::vector_analysis::{DepKind, EffectAtom};
use super::vector_assemble::{
    VectorClockOutputStore, VectorFirAssembly, VectorLoopFirInput, assemble_vector_fir,
    fir_reachable,
};
use super::vector_clock_ad::build_vector_clock_ad_plan;
use super::vector_events::{
    DEFAULT_EVENT_LIMITS, build_state_event_order_certificate, precheck_state_event_bound,
};
use super::vector_lower::{VectorLoweringContext, lower_vector_program};
use super::vector_plan::build_vector_plan_with_lockstep;
use super::vector_route::{VectorRegion, VerifiedRoutedFir};
use super::vector_state::build_vector_state_plan_with_clock;
use super::vector_ui::{VectorUiFir, build_vector_ui_fir};
use super::vector_verify::VectorPlan;

/// Failure stage retained by the production selector as an observable fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VectorModuleFailure {
    pub reason: VectorFallbackReason,
    pub detail: String,
}

impl VectorModuleFailure {
    fn new(reason: VectorFallbackReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for VectorModuleFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.reason.code(), self.detail)
    }
}

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
    if assembly.schema_version != super::vector_assemble::VECTOR_FIR_ASSEMBLY_VERSION
        || output_stores.len() != context.num_outputs
    {
        return Err(module_shape("verified module evidence lost final coverage"));
    }
    Ok(output)
}

struct BuiltVectorModule {
    output: SignalFirOutput,
    assembly: VectorFirAssembly,
    output_stores: Vec<FirId>,
}

fn build_verified_vector_module_with_evidence(
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

fn reject_cross_loop_delay_read_transports(
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

struct OutputMaterialization<'a> {
    routed: &'a VerifiedRoutedFir,
    loop_inputs: &'a mut [VectorLoopFirInput],
    control_statements: &'a mut Vec<FirId>,
    control_output_stores: &'a mut Vec<FirId>,
    clock_output_stores: &'a mut Vec<VectorClockOutputStore>,
    clock_plan: &'a super::vector_clock_ad::VerifiedVectorClockAdPlan,
    store: &'a mut FirStore,
}

fn materialize_outputs(
    outputs: &[SigId],
    num_outputs: usize,
    context: &mut OutputMaterialization<'_>,
) -> Result<Vec<FirId>, VectorModuleFailure> {
    if outputs.len() != num_outputs {
        return Err(VectorModuleFailure::new(
            VectorFallbackReason::OutputAssembly,
            "prepared output count does not match the module contract",
        ));
    }
    let loop_index = context
        .loop_inputs
        .iter()
        .enumerate()
        .map(|(index, body)| (body.loop_id, index))
        .collect::<BTreeMap<_, _>>();
    let mut stores = Vec::with_capacity(outputs.len());
    for (channel, output) in outputs.iter().enumerate() {
        let signal_id = u64::from(output.as_u32());
        let signal = context
            .routed
            .plan()
            .signals
            .iter()
            .find(|signal| signal.signal_id == signal_id)
            .ok_or_else(|| {
                VectorModuleFailure::new(
                    VectorFallbackReason::OutputAssembly,
                    format!("output signal {signal_id} is absent from the vector plan"),
                )
            })?;
        let definition_region = match signal.placement {
            super::vector_verify::Placement::Owned(loop_id) => VectorRegion::Loop(loop_id),
            super::vector_verify::Placement::Control => VectorRegion::Control,
            super::vector_verify::Placement::Inline => {
                return Err(VectorModuleFailure::new(
                    VectorFallbackReason::OutputAssembly,
                    format!("output signal {signal_id} remains inline at final assembly"),
                ));
            }
        };
        let value = context
            .routed
            .trace()
            .definitions()
            .iter()
            .find(|definition| {
                definition.signal_id == signal_id && definition.region == definition_region
            })
            .map(|definition| definition.value)
            .ok_or_else(|| {
                VectorModuleFailure::new(
                    VectorFallbackReason::OutputAssembly,
                    format!("output signal {signal_id} has no routed FIR definition"),
                )
            })?;
        let value_is_faust_float = context.store.value_type(value) == Some(FirType::FaustFloat);
        let mut builder = FirBuilder::new(context.store);
        let channel_i32 = i32::try_from(channel).map_err(|_| {
            VectorModuleFailure::new(
                VectorFallbackReason::OutputAssembly,
                "output channel index exceeds FIR i32",
            )
        })?;
        let channel_value = builder.int32(channel_i32);
        let pointer_type = FirType::Ptr(Box::new(FirType::FaustFloat));
        let pointer = builder.load_table(
            "outputs",
            AccessType::FunArgs,
            channel_value,
            pointer_type.clone(),
        );
        context.control_statements.push(builder.declare_var(
            format!("output{channel}"),
            pointer_type,
            AccessType::Stack,
            Some(pointer),
        ));
        let value = if value_is_faust_float {
            value
        } else {
            builder.cast(FirType::FaustFloat, value)
        };
        let sample = builder.load_var("i0", AccessType::Loop, FirType::Int32);
        let output_store =
            builder.store_table(format!("output{channel}"), AccessType::Stack, sample, value);
        match definition_region {
            VectorRegion::Loop(loop_id) => {
                let is_clock_owned = context
                    .clock_plan
                    .plan()
                    .clock_islands
                    .iter()
                    .any(|island| island.nested_loop_ids.contains(&loop_id));
                if is_clock_owned {
                    context.clock_output_stores.push(VectorClockOutputStore {
                        owner_loop_id: loop_id,
                        statement: output_store,
                    });
                } else {
                    let region_index = *loop_index.get(&loop_id).ok_or_else(|| {
                        VectorModuleFailure::new(
                            VectorFallbackReason::OutputAssembly,
                            format!("output loop {loop_id} has no final region body"),
                        )
                    })?;
                    context.loop_inputs[region_index]
                        .statements
                        .push(output_store);
                }
            }
            VectorRegion::Control => context.control_output_stores.push(output_store),
        }
        stores.push(output_store);
    }
    Ok(stores)
}

struct FinalModuleContext<'a> {
    module_name: &'a str,
    num_inputs: usize,
    num_outputs: usize,
    real_type: &'a FirType,
    vec_size: u32,
    loop_variant: u8,
    control_statements: &'a [FirId],
    table_declarations: &'a [FirId],
    table_init_statements: &'a [FirId],
    math_ops: &'a std::collections::HashSet<fir::FirMathOp>,
    int_helpers: &'a std::collections::BTreeSet<&'static str>,
    assembly: &'a VectorFirAssembly,
    control_output_stores: &'a [FirId],
    ui_fir: &'a VectorUiFir,
    static_declarations: &'a [FirId],
}

fn assemble_module(
    store: &mut FirStore,
    context: &FinalModuleContext<'_>,
) -> Result<FirId, VectorModuleFailure> {
    let module_name = context.module_name;
    let num_inputs = context.num_inputs;
    let num_outputs = context.num_outputs;
    let real_type = context.real_type.clone();
    let vec_size = context.vec_size;
    let loop_variant = context.loop_variant;
    let control_statements = context.control_statements;
    let table_declarations = context.table_declarations;
    let table_init_statements = context.table_init_statements;
    let math_ops = context.math_ops;
    let int_helpers = context.int_helpers;
    let assembly = context.assembly;
    let control_output_stores = context.control_output_stores;
    let ui_fir = context.ui_fir;
    let static_declarations = context.static_declarations;
    let dsp_arg_type = FirType::Ptr(Box::new(FirType::Obj));
    let dsp_arg = NamedType {
        name: "dsp".to_owned(),
        typ: dsp_arg_type.clone(),
    };
    let empty = FirBuilder::new(store).block(&[]);
    let metadata = FirBuilder::new(store).declare_fun(
        "metadata",
        FirType::Fun {
            args: vec![dsp_arg_type.clone(), FirType::Meta],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "m".to_owned(),
                typ: FirType::Meta,
            },
        ],
        Some(empty),
        false,
    );

    let sample_rate =
        FirBuilder::new(store).load_var("sample_rate", AccessType::FunArgs, FirType::Int32);
    let sample_rate_store =
        FirBuilder::new(store).store_var("fSampleRate", AccessType::Struct, sample_rate);
    // Mutable-table content is written once at init and persists across
    // compute calls; emitting it in the per-block control section would reset
    // runtime writes every block.
    let mut constants_statements = vec![sample_rate_store];
    constants_statements.extend(table_init_statements.iter().copied());
    let constants_body = FirBuilder::new(store).block(&constants_statements);
    let instance_constants = FirBuilder::new(store).declare_fun(
        "instanceConstants",
        FirType::Fun {
            args: vec![dsp_arg_type.clone(), FirType::Int32],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "sample_rate".to_owned(),
                typ: FirType::Int32,
            },
        ],
        Some(constants_body),
        false,
    );

    let reset_body = FirBuilder::new(store).block(&ui_fir.reset_statements);
    let instance_reset_ui =
        lifecycle_function(store, "instanceResetUserInterface", &dsp_arg, reset_body);
    let clear_body = FirBuilder::new(store).block(&assembly.clear_statements);
    let instance_clear = lifecycle_function(store, "instanceClear", &dsp_arg, clear_body);
    let ui_body = FirBuilder::new(store).block(&ui_fir.build_statements);
    let build_ui = FirBuilder::new(store).declare_fun(
        "buildUserInterface",
        FirType::Fun {
            args: vec![dsp_arg_type.clone(), FirType::UI],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "ui_interface".to_owned(),
                typ: FirType::UI,
            },
        ],
        Some(ui_body),
        false,
    );

    let chunk = if control_output_stores.is_empty() {
        assembly.top_level_statement
    } else {
        let fill = sample_loop_for_statements(store, control_output_stores);
        FirBuilder::new(store).block(&[assembly.top_level_statement, fill])
    };
    let driver = build_chunk_driver(store, chunk, vec_size, loop_variant)?;
    let mut compute_statements = control_statements.to_vec();
    compute_statements.extend(assembly.local_declarations.iter().copied());
    compute_statements.extend(driver);
    let compute_body = FirBuilder::new(store).block(&compute_statements);
    let audio_ptr = FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat))));
    let compute = FirBuilder::new(store).declare_fun(
        "compute",
        FirType::Fun {
            args: vec![
                dsp_arg_type.clone(),
                FirType::Int32,
                audio_ptr.clone(),
                audio_ptr.clone(),
            ],
            ret: Box::new(FirType::Void),
        },
        &[
            dsp_arg.clone(),
            NamedType {
                name: "count".to_owned(),
                typ: FirType::Int32,
            },
            NamedType {
                name: "inputs".to_owned(),
                typ: audio_ptr.clone(),
            },
            NamedType {
                name: "outputs".to_owned(),
                typ: audio_ptr,
            },
        ],
        Some(compute_body),
        false,
    );

    let globals = build_prototypes(store, real_type, math_ops, int_helpers);
    let functions = FirBuilder::new(store).block(&[
        metadata,
        instance_constants,
        instance_reset_ui,
        instance_clear,
        build_ui,
        compute,
    ]);
    let sample_rate_field =
        FirBuilder::new(store).declare_var("fSampleRate", FirType::Int32, AccessType::Struct, None);
    let mut fields = vec![sample_rate_field];
    fields.extend(ui_fir.struct_declarations.iter().copied());
    fields.extend(assembly.state_declarations.iter().copied());
    fields.extend(table_declarations.iter().copied());
    let dsp_struct = FirBuilder::new(store).block(&fields);
    let static_declarations = FirBuilder::new(store).block(static_declarations);
    Ok(FirBuilder::new(store).module(
        num_inputs,
        num_outputs,
        module_name,
        dsp_struct,
        globals,
        functions,
        static_declarations,
    ))
}

fn sample_loop_for_statements(store: &mut FirStore, statements: &[FirId]) -> FirId {
    let body = FirBuilder::new(store).block(statements);
    let mut builder = FirBuilder::new(store);
    let start = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let init = builder.declare_var("i0", FirType::Int32, AccessType::Loop, Some(start));
    let start = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
    let end = builder.binop(FirBinOp::Add, start, count, FirType::Int32);
    let step = builder.int32(1);
    builder.for_loop("i0", init, end, step, body, false)
}

fn lifecycle_function(store: &mut FirStore, name: &str, dsp_arg: &NamedType, body: FirId) -> FirId {
    FirBuilder::new(store).declare_fun(
        name,
        FirType::Fun {
            args: vec![dsp_arg.typ.clone()],
            ret: Box::new(FirType::Void),
        },
        std::slice::from_ref(dsp_arg),
        Some(body),
        false,
    )
}

fn build_prototypes(
    store: &mut FirStore,
    real_type: FirType,
    math_ops: &std::collections::HashSet<fir::FirMathOp>,
    int_helpers: &std::collections::BTreeSet<&'static str>,
) -> FirId {
    let mut prototypes = Vec::new();
    for op in MATH_PROTO_ORDER {
        if !math_ops.contains(op) {
            continue;
        }
        let arity = match op {
            fir::FirMathOp::Pow
            | fir::FirMathOp::Min
            | fir::FirMathOp::Max
            | fir::FirMathOp::Atan2
            | fir::FirMathOp::Fmod
            | fir::FirMathOp::Remainder => 2,
            _ => 1,
        };
        let args = (0..arity)
            .map(|index| NamedType {
                name: format!("arg{index}"),
                typ: real_type.clone(),
            })
            .collect::<Vec<_>>();
        prototypes.push(FirBuilder::new(store).declare_fun(
            op.symbol(),
            FirType::Fun {
                args: vec![real_type.clone(); arity],
                ret: Box::new(real_type.clone()),
            },
            &args,
            None,
            false,
        ));
    }
    for name in INT_FUN_PROTO_ORDER {
        if !int_helpers.contains(name) {
            continue;
        }
        let arity = usize::from(*name != "abs") + 1;
        let args = (0..arity)
            .map(|index| NamedType {
                name: format!("arg{index}"),
                typ: FirType::Int32,
            })
            .collect::<Vec<_>>();
        prototypes.push(FirBuilder::new(store).declare_fun(
            *name,
            FirType::Fun {
                args: vec![FirType::Int32; arity],
                ret: Box::new(FirType::Int32),
            },
            &args,
            None,
            false,
        ));
    }
    FirBuilder::new(store).block(&prototypes)
}

fn build_chunk_driver(
    store: &mut FirStore,
    chunk: FirId,
    vec_size: u32,
    loop_variant: u8,
) -> Result<Vec<FirId>, VectorModuleFailure> {
    let vec_size = i32::try_from(vec_size).map_err(|_| {
        VectorModuleFailure::new(
            VectorFallbackReason::ModuleVerification,
            "vector size exceeds FIR i32",
        )
    })?;
    match loop_variant {
        0 => Ok(build_fast_driver(store, chunk, vec_size)),
        1 => Ok(vec![build_simple_driver(store, chunk, vec_size)]),
        _ => Err(VectorModuleFailure::new(
            VectorFallbackReason::ModuleVerification,
            format!("unsupported vector loop variant {loop_variant}"),
        )),
    }
}

fn build_simple_driver(store: &mut FirStore, chunk: FirId, vec_size: i32) -> FirId {
    let mut builder = FirBuilder::new(store);
    let index = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let remaining = builder.binop(FirBinOp::Sub, count, index, FirType::Int32);
    let width = builder.int32(vec_size);
    let smaller = builder.binop(FirBinOp::Lt, remaining, width, FirType::Bool);
    let vcount = builder.select2(smaller, remaining, width, FirType::Int32);
    let vcount = builder.declare_var("vcount", FirType::Int32, AccessType::Stack, Some(vcount));
    let body = builder.block(&[vcount, chunk]);
    let zero = builder.int32(0);
    let init = builder.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(zero));
    let end = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let step = builder.int32(vec_size);
    builder.for_loop("vindex", init, end, step, body, false)
}

fn build_fast_driver(store: &mut FirStore, chunk: FirId, vec_size: i32) -> Vec<FirId> {
    let mut builder = FirBuilder::new(store);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let width = builder.int32(vec_size);
    let rem = builder.binop(FirBinOp::Rem, count, width, FirType::Int32);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let limit = builder.binop(FirBinOp::Sub, count, rem, FirType::Int32);

    let width_value = builder.int32(vec_size);
    let main_vcount = builder.declare_var(
        "vcount",
        FirType::Int32,
        AccessType::Stack,
        Some(width_value),
    );
    let main_body = builder.block(&[main_vcount, chunk]);
    let zero = builder.int32(0);
    let main_init = builder.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(zero));
    let main_step = builder.int32(vec_size);
    let main_loop = builder.for_loop("vindex", main_init, limit, main_step, main_body, false);
    let zero = builder.int32(0);
    let has_main = builder.binop(FirBinOp::Gt, limit, zero, FirType::Bool);
    let main_then = builder.block(&[main_loop]);
    let guarded_main = builder.if_(has_main, main_then, None);

    let rem_init = builder.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(limit));
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let remaining = builder.binop(FirBinOp::Sub, count, limit, FirType::Int32);
    let rem_vcount =
        builder.declare_var("vcount", FirType::Int32, AccessType::Stack, Some(remaining));
    let rem_body = builder.block(&[rem_init, rem_vcount, chunk]);
    let count = builder.load_var("count", AccessType::FunArgs, FirType::Int32);
    let has_rem = builder.binop(FirBinOp::Lt, limit, count, FirType::Bool);
    let guarded_rem = builder.if_(has_rem, rem_body, None);
    vec![guarded_main, guarded_rem]
}

/// Rejects any compute-time store into a `Sound`-typed DSP-struct field.
///
/// Soundfile data is loaded at lifecycle time and immutable during `compute`;
/// admitting soundfile reads widened what compute may contain, so the
/// read-only claim must be carried by the emitted FIR itself. The field set is
/// derived from the emitted struct declarations alone, exactly as the table
/// checks derive theirs.
fn verify_sound_field_immutability(
    store: &FirStore,
    compute: FirId,
    struct_fields: &[FirId],
) -> Result<(), VectorModuleFailure> {
    let sound_fields = struct_fields
        .iter()
        .filter_map(|field| match match_fir(store, *field) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Sound,
                ..
            } => Some(name),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if sound_fields.is_empty() {
        return Ok(());
    }
    for node in fir_reachable(store, compute) {
        let FirMatch::StoreVar { name, .. } = match_fir(store, node) else {
            continue;
        };
        if sound_fields.contains(&name) {
            return Err(module_shape(format!(
                "compute stores into soundfile field {name}"
            )));
        }
    }
    Ok(())
}

/// Verifies mutable-table writes and initialization against the emitted FIR.
///
/// The mutable-table set is read from the emitted DSP-struct declarations, not
/// from the effect model or the lowerer registry, so a misprojection on either
/// side surfaces as a count mismatch here. Three obligations: every claimed
/// direct `WriteTable` performer is backed by exactly one physical `StoreTable`
/// on its table's field in `compute` and vice versa (the table analogue of the
/// UI-write attribution check), and `instanceConstants` initializes every cell
/// of every mutable table exactly once. Initial values themselves are numeric
/// claims the certificate does not carry; the native oracle matrix is their
/// arbiter.
fn verify_mutable_table_attribution(
    store: &FirStore,
    compute: FirId,
    instance_constants: FirId,
    table_declarations: &[FirId],
    plan: &VectorPlan,
) -> Result<(), VectorModuleFailure> {
    let mut declared = BTreeMap::<String, usize>::new();
    for declaration in table_declarations {
        let FirMatch::DeclareVar {
            name,
            typ: FirType::Array(_, length),
            ..
        } = match_fir(store, *declaration)
        else {
            return Err(module_shape(
                "mutable table declaration is not a struct array field",
            ));
        };
        declared.insert(name, length);
    }
    let mut claimed = BTreeMap::<String, usize>::new();
    for signal in &plan.signals {
        for effect in &signal.direct_effects {
            let EffectAtom::WriteTable(table) = effect else {
                continue;
            };
            let table_id = u64::from(*table);
            let name = [FirType::Int32, FirType::Float32]
                .iter()
                .map(|elem| super::vector_lower::mutable_table_name(table_id, elem))
                .chain(std::iter::once(super::vector_lower::mutable_table_name(
                    table_id,
                    &FirType::Float64,
                )))
                .find(|name| declared.contains_key(name));
            let Some(name) = name else {
                return Err(module_shape(format!(
                    "claimed mutable table writer {table_id} has no declared struct field"
                )));
            };
            *claimed.entry(name).or_default() += 1;
        }
    }
    let mut physical = BTreeMap::<String, usize>::new();
    for node in fir_reachable(store, compute) {
        let FirMatch::StoreTable { name, .. } = match_fir(store, node) else {
            continue;
        };
        if declared.contains_key(&name) {
            *physical.entry(name).or_default() += 1;
        }
    }
    for (name, count) in &claimed {
        let emitted = physical.get(name).copied().unwrap_or(0);
        if emitted != *count {
            return Err(module_shape(format!(
                "mutable table {name} has {count} claimed writers but compute stores it {emitted} times"
            )));
        }
    }
    for (name, emitted) in &physical {
        if claimed.get(name).copied().unwrap_or(0) != *emitted {
            return Err(module_shape(format!(
                "compute stores mutable table {name} {emitted} times with no matching claimed writer"
            )));
        }
    }
    let FirMatch::Block(init_statements) = match_fir(store, instance_constants) else {
        return Err(module_shape("instanceConstants is not a block"));
    };
    let mut covered = BTreeMap::<String, BTreeSet<i32>>::new();
    for statement in &init_statements {
        let FirMatch::StoreTable { name, index, .. } = match_fir(store, *statement) else {
            continue;
        };
        if !declared.contains_key(&name) {
            return Err(module_shape(format!(
                "instanceConstants stores unknown table {name}"
            )));
        }
        let FirMatch::Int32 { value, .. } = match_fir(store, index) else {
            return Err(module_shape(format!(
                "mutable table {name} initialization index is not a constant"
            )));
        };
        if !covered.entry(name.clone()).or_default().insert(value) {
            return Err(module_shape(format!(
                "mutable table {name} cell {value} initialized twice"
            )));
        }
    }
    for (name, length) in &declared {
        let cells = covered.get(name).map_or(0, BTreeSet::len);
        let complete = cells == *length
            && covered.get(name).is_some_and(|cells| {
                cells.first().is_some_and(|&first| first == 0)
                    && cells
                        .last()
                        .is_some_and(|&last| last + 1 == i32::try_from(*length).unwrap_or(i32::MAX))
            });
        if !complete {
            return Err(module_shape(format!(
                "mutable table {name} initialization covers {cells} of {length} cells"
            )));
        }
    }
    Ok(())
}

/// Rejects a UI write event attributed to a signal that performs no such write.
///
/// The event model now attributes an effect operation to the signal performing
/// it, read from the plan's `direct_effects`. Producer and checker both derive
/// that projection from the same analysis, so their agreement cannot catch a
/// misprojection. This counts the physical zone stores in the emitted `compute`
/// body instead and requires one per claimed performer: attributing a zone
/// write to every transitive carrier of it is exactly what made `mixer` report
/// 38 operations on a zone the compiler stores once.
///
/// FIR nodes are interned, so a body duplicated across chunk drivers yields one
/// node per physical store rather than one per driver.
fn verify_ui_write_attribution(
    store: &FirStore,
    compute: FirId,
    ui: &ui::UiProgram,
    plan: &VectorPlan,
) -> Result<(), VectorModuleFailure> {
    let mut claimed = BTreeMap::<u32, usize>::new();
    for signal in &plan.signals {
        for effect in &signal.direct_effects {
            if let EffectAtom::WriteUi(control) = effect {
                *claimed.entry(*control).or_default() += 1;
            }
        }
    }
    let mut physical = BTreeMap::<String, usize>::new();
    for node in fir_reachable(store, compute) {
        if let FirMatch::StoreVar { name, .. } = match_fir(store, node) {
            *physical.entry(name).or_default() += 1;
        }
    }
    let mut zones = BTreeMap::<String, u32>::new();
    for spec in &ui.controls {
        zones.insert(super::vector_ui::zone_name(spec.kind, spec.id), spec.id);
    }
    for (control, count) in &claimed {
        let Some(spec) = ui.controls.iter().find(|spec| spec.id == *control) else {
            return Err(module_shape(format!(
                "event certificate writes unknown UI control {control}"
            )));
        };
        let zone = super::vector_ui::zone_name(spec.kind, spec.id);
        let emitted = physical.get(&zone).copied().unwrap_or(0);
        if emitted != *count {
            return Err(module_shape(format!(
                "UI control {control} has {count} claimed writers but compute stores its zone {emitted} times"
            )));
        }
    }
    for (zone, emitted) in &physical {
        let Some(control) = zones.get(zone) else {
            continue;
        };
        if claimed.get(control).copied().unwrap_or(0) != *emitted {
            return Err(module_shape(format!(
                "compute stores UI zone {zone} {emitted} times with no matching claimed writer"
            )));
        }
    }
    Ok(())
}

/// Rejects any store into a table the lowering declared read-only.
///
/// The signal-level effect model and the pure lowerer share one read-only
/// predicate, so their agreement cannot catch a misclassification of one. This
/// reads the emitted body instead: a table whose content is declared once with
/// initializers must carry no `StoreTable` anywhere in `compute`, whatever the
/// signal-level model believed. Every table reaching a checked vector module is
/// read-only today because `lower_readonly_table_definition` rejects live write
/// ports; admitting mutable tables must extend this check rather than drop it.
fn verify_readonly_table_stores(
    store: &FirStore,
    compute: FirId,
    static_declarations: &[FirId],
) -> Result<(), VectorModuleFailure> {
    let readonly = static_declarations
        .iter()
        .filter_map(|declaration| match match_fir(store, *declaration) {
            FirMatch::DeclareTable { name, .. } => Some(name),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if readonly.is_empty() {
        return Ok(());
    }
    for node in fir_reachable(store, compute) {
        let FirMatch::StoreTable { name, .. } = match_fir(store, node) else {
            continue;
        };
        if readonly.contains(&name) {
            return Err(module_shape(format!(
                "compute stores into read-only table {name}"
            )));
        }
    }
    Ok(())
}

/// Everything the final independent module check compares the emitted FIR
/// against.
struct FinalModuleExpectations<'a> {
    assembly: &'a VectorFirAssembly,
    output_stores: &'a [FirId],
    ui_fir: &'a VectorUiFir,
    static_declarations: &'a [FirId],
    table_declarations: &'a [FirId],
    ui: &'a ui::UiProgram,
    plan: &'a VectorPlan,
}

fn verify_final_module(
    store: &FirStore,
    module: FirId,
    expected: &FinalModuleExpectations<'_>,
) -> Result<(), VectorModuleFailure> {
    let FinalModuleExpectations {
        assembly,
        output_stores,
        ui_fir,
        static_declarations: expected_static_declarations,
        table_declarations,
        ui,
        plan,
    } = *expected;
    let report = verify_fir_module(store, module);
    if report.has_errors() {
        let detail = report
            .errors()
            .map(|diagnostic| format!("{} {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(VectorModuleFailure::new(
            VectorFallbackReason::ModuleVerification,
            detail,
        ));
    }
    let FirMatch::Module {
        dsp_struct,
        functions,
        static_decls,
        ..
    } = match_fir(store, module)
    else {
        return Err(module_shape("root is not a FIR module"));
    };
    if match_fir(store, static_decls) != FirMatch::Block(expected_static_declarations.to_vec()) {
        return Err(module_shape(
            "module does not contain the exact checked static declarations",
        ));
    }
    let FirMatch::Block(fields) = match_fir(store, dsp_struct) else {
        return Err(module_shape("DSP struct is not a block"));
    };
    if assembly
        .state_declarations
        .iter()
        .any(|declaration| !fields.contains(declaration))
    {
        return Err(module_shape("P6 state declaration missing from DSP struct"));
    }
    if ui_fir
        .struct_declarations
        .iter()
        .any(|declaration| !fields.contains(declaration))
    {
        return Err(module_shape("UI zone declaration missing from DSP struct"));
    }
    if table_declarations
        .iter()
        .any(|declaration| !fields.contains(declaration))
    {
        return Err(module_shape(
            "mutable table declaration missing from DSP struct",
        ));
    }
    let FirMatch::Block(functions) = match_fir(store, functions) else {
        return Err(module_shape("function section is not a block"));
    };
    let bodies = functions
        .iter()
        .filter_map(|function| match match_fir(store, *function) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } => Some((name, body)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    for required in [
        "metadata",
        "instanceConstants",
        "instanceResetUserInterface",
        "instanceClear",
        "buildUserInterface",
        "compute",
    ] {
        if !bodies.contains_key(required) {
            return Err(module_shape(format!(
                "missing lifecycle function {required}"
            )));
        }
    }
    if match_fir(store, bodies["instanceClear"])
        != FirMatch::Block(assembly.clear_statements.clone())
    {
        return Err(module_shape(
            "instanceClear does not contain exact P6 clears",
        ));
    }
    if match_fir(store, bodies["instanceResetUserInterface"])
        != FirMatch::Block(ui_fir.reset_statements.clone())
    {
        return Err(module_shape(
            "instanceResetUserInterface does not contain exact UI resets",
        ));
    }
    if match_fir(store, bodies["buildUserInterface"])
        != FirMatch::Block(ui_fir.build_statements.clone())
    {
        return Err(module_shape(
            "buildUserInterface does not contain exact grouped UI program",
        ));
    }
    let compute = bodies["compute"];
    if !contains_statement(store, compute, assembly.top_level_statement) {
        return Err(module_shape(
            "compute does not contain the accepted P6.3b body",
        ));
    }
    verify_readonly_table_stores(store, compute, expected_static_declarations)?;
    verify_sound_field_immutability(store, compute, &fields)?;
    verify_ui_write_attribution(store, compute, ui, plan)?;
    verify_mutable_table_attribution(
        store,
        compute,
        bodies["instanceConstants"],
        table_declarations,
        plan,
    )?;
    for output in output_stores {
        if !contains_statement(store, compute, *output) {
            return Err(module_shape("compute does not cover every output store"));
        }
    }
    Ok(())
}

fn contains_statement(store: &FirStore, root: FirId, target: FirId) -> bool {
    if root == target {
        return true;
    }
    match match_fir(store, root) {
        FirMatch::Block(body) => body
            .into_iter()
            .any(|child| contains_statement(store, child, target)),
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => {
            contains_statement(store, then_block, target)
                || else_block.is_some_and(|body| contains_statement(store, body, target))
        }
        FirMatch::Control { stmt, .. } => contains_statement(store, stmt, target),
        FirMatch::ForLoop { body, .. }
        | FirMatch::SimpleForLoop { body, .. }
        | FirMatch::IteratorForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. } => contains_statement(store, body, target),
        FirMatch::Switch { cases, default, .. } => {
            cases
                .into_iter()
                .any(|(_, body)| contains_statement(store, body, target))
                || default.is_some_and(|body| contains_statement(store, body, target))
        }
        _ => false,
    }
}

fn module_shape(detail: impl Into<String>) -> VectorModuleFailure {
    VectorModuleFailure::new(VectorFallbackReason::ModuleVerification, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fir::checker::verify_fir_module;
    use signals::SigBuilder;
    use tlib::TreeArena;

    use crate::signal_prepare::prepare_signals_for_fir_verified;

    use super::super::vector_verify::{
        Placement, Rate, SignalRecord, VECTOR_PLAN_SCHEMA_VERSION, ValueType, Vectorability,
    };

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
        let name = super::super::vector_lower::mutable_table_name(table_id, &FirType::Float64);
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

        let missing_store = verify_mutable_table_attribution(
            &store,
            compute_empty,
            full_init,
            &[declaration],
            &writer,
        )
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
                assert!(!verify_fir_module(&output.store, output.module).has_errors());
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
}
