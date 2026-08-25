//! Producer materialization of the vector FIR assembly (state storage,
//! loops, clock islands, top level). The terminal step calls
//! `check::verify_vector_fir_assembly`, so every admission guard there
//! also binds the producer (plan provenance: §4.8). Checker re-derivations live in
//! `check.rs` and must NOT be merged with these producer paths (plan provenance: §3.2).

use super::check::verify_vector_fir_assembly;
use super::model::*;
use crate::signal_fir::vector::clock_ad::{
    ClockGuard, ClockIsland, ClockTransportMode, VerifiedVectorClockAdPlan,
};
use crate::signal_fir::vector::route::{RoutedDefinition, VectorRegion, VerifiedRoutedFir};
use crate::signal_fir::vector::state::{
    DelayTransition, PrefixTransition, RecursionTransition, VectorDelayStorage, VectorStateAction,
    VectorStateInitialValue, VerifiedVectorStatePlan, WaveformTransition,
};
use crate::signal_fir::vector::verify::{ValueType, VectorPlan};
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};
use std::collections::{BTreeMap, BTreeSet};

/// Materializes checked state-plan phases and clock-plan serial islands into
/// concrete FIR.
pub fn assemble_vector_fir(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
    inputs: &[VectorLoopFirInput],
    clock_output_stores: &[VectorClockOutputStore],
    real_type: FirType,
    store: &mut FirStore,
) -> Result<VerifiedVectorFirAssembly, VectorFirAssemblyError> {
    if let Some(state) = state_plan {
        require_same_plan(routed.plan(), state.vector_plan(), "P6.1 state plan")?;
    }
    if let Some(clock) = clock_plan {
        require_same_plan(routed.plan(), clock.vector_plan(), "P6.2 clock/AD plan")?;
        if let Some(fallback) = clock.plan().reverse_ad_fallbacks.first() {
            return Err(VectorFirAssemblyError::ReverseAdRequiresScalar {
                signal_id: fallback.signal_id,
            });
        }
    }

    let input_map = check_loop_inputs(routed, inputs)?;
    let transport_declarations = inspect_transport_declarations(routed, store)?;
    let assembly = {
        let mut builder = FirBuilder::new(store);
        let mut local_declarations = Vec::new();
        let mut state_declarations = Vec::new();
        let mut clear_statements = Vec::new();
        let mut island_declarations = BTreeMap::new();
        classify_transport_declarations(
            &transport_declarations,
            &mut builder,
            &mut local_declarations,
            &mut state_declarations,
            &mut clear_statements,
            &mut island_declarations,
        )?;

        let mut delays = BTreeMap::new();
        let mut recursions = BTreeMap::new();
        let mut prefixes = BTreeMap::new();
        let mut waveforms = BTreeMap::new();
        if let Some(state) = state_plan {
            materialize_state_storage(
                state,
                real_type.clone(),
                &mut builder,
                &mut state_declarations,
                &mut local_declarations,
                &mut clear_statements,
            )?;
            delays.extend(
                state
                    .plan()
                    .delays
                    .iter()
                    .map(|delay| (delay.signal_id, delay)),
            );
            recursions.extend(
                state
                    .plan()
                    .recursions
                    .iter()
                    .map(|recursion| (recursion.group, recursion)),
            );
            prefixes.extend(
                state
                    .plan()
                    .prefixes
                    .iter()
                    .map(|transition| (transition.signal_id, transition)),
            );
            waveforms.extend(
                state
                    .plan()
                    .waveforms
                    .iter()
                    .map(|transition| (transition.signal_id, transition)),
            );
        }

        let mut definitions = definition_map(routed.trace().definitions());
        // A state action may consume a value through an accepted cross-loop
        // route rather than a local definition (notably `prefix` writes).
        // Preserve the independently checked routed value for that consumer.
        for routed_use in routed.trace().uses() {
            definitions
                .entry((
                    VectorRegion::Loop(routed_use.consumer_loop),
                    routed_use.signal_id,
                ))
                .or_insert(routed_use.value);
        }
        let signal_types = routed
            .plan()
            .signals
            .iter()
            .map(|signal| (signal.signal_id, signal.value_type.clone()))
            .collect::<BTreeMap<_, _>>();
        let fused_members_by_loop = routed
            .plan()
            .fused_serial_groups
            .iter()
            .flat_map(|group| {
                group
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, group.member_loop_ids.as_slice()))
            })
            .collect::<BTreeMap<_, _>>();
        let materialization = StateMaterializationContext {
            delays: &delays,
            recursions: &recursions,
            prefixes: &prefixes,
            waveforms: &waveforms,
            definitions: &definitions,
            fused_members_by_loop: &fused_members_by_loop,
            signal_types: &signal_types,
            real_type: real_type.clone(),
        };
        let mut loops = Vec::with_capacity(routed.layout().loops().len());
        for region in routed.layout().loops() {
            let phases = state_plan.and_then(|state| {
                state
                    .plan()
                    .loops
                    .iter()
                    .find(|phases| phases.loop_id == region.loop_id)
            });
            loops.push(materialize_loop(
                region.loop_id,
                input_map[&region.loop_id],
                phases,
                &materialization,
                &mut builder,
            )?);
        }

        let clock_materialization = ClockIslandMaterializationContext {
            routed,
            state_plan,
            clock_plan,
            loops: &loops,
            definitions: &definitions,
            island_declarations: &island_declarations,
        };
        let islands = materialize_clock_islands(
            &clock_materialization,
            &mut builder,
            &mut state_declarations,
            &mut clear_statements,
        )?;
        let top_level_statement = materialize_top_level(
            routed,
            state_plan,
            &loops,
            &islands,
            clock_output_stores,
            &mut builder,
        )?;
        VectorFirAssembly {
            schema_version: VECTOR_FIR_ASSEMBLY_VERSION,
            local_declarations,
            state_declarations,
            clear_statements,
            loops,
            islands,
            clock_output_stores: clock_output_stores.to_vec(),
            top_level_statement,
        }
    };
    verify_vector_fir_assembly(routed, state_plan, clock_plan, &assembly, store)?;
    Ok(VerifiedVectorFirAssembly {
        assembly,
        vector_plan: routed.plan().clone(),
    })
}
pub(super) fn require_same_plan(
    routed: &VectorPlan,
    artifact: &VectorPlan,
    name: &'static str,
) -> Result<(), VectorFirAssemblyError> {
    if routed == artifact {
        Ok(())
    } else {
        Err(VectorFirAssemblyError::PlanMismatch { artifact: name })
    }
}
pub(super) fn check_loop_inputs<'a>(
    routed: &VerifiedRoutedFir,
    inputs: &'a [VectorLoopFirInput],
) -> Result<BTreeMap<u64, &'a [FirId]>, VectorFirAssemblyError> {
    let expected = routed
        .layout()
        .loops()
        .iter()
        .map(|region| region.loop_id)
        .collect::<BTreeSet<_>>();
    let mut result = BTreeMap::new();
    for input in inputs {
        if result
            .insert(input.loop_id, input.statements.as_slice())
            .is_some()
        {
            return Err(VectorFirAssemblyError::DuplicateLoopInput {
                loop_id: input.loop_id,
            });
        }
    }
    for loop_id in expected {
        if !result.contains_key(&loop_id) {
            return Err(VectorFirAssemblyError::LoopInputCoverage { loop_id });
        }
    }
    if let Some(extra) = result.keys().find(|loop_id| {
        !routed
            .layout()
            .loops()
            .iter()
            .any(|r| r.loop_id == **loop_id)
    }) {
        return Err(VectorFirAssemblyError::LoopInputCoverage { loop_id: *extra });
    }
    Ok(result)
}
pub(super) struct TransportDeclaration {
    mode: ClockTransportMode,
    declaration: FirId,
    held: Option<(String, FirType)>,
}
pub(super) fn inspect_transport_declarations(
    routed: &VerifiedRoutedFir,
    store: &FirStore,
) -> Result<Vec<TransportDeclaration>, VectorFirAssemblyError> {
    routed
        .trace()
        .transports()
        .iter()
        .map(|transport| {
            let held = if matches!(transport.mode, ClockTransportMode::HeldOutput { .. }) {
                match match_fir(store, transport.declaration) {
                    FirMatch::DeclareVar {
                        name,
                        typ,
                        access: AccessType::Struct,
                        init: None,
                    } => Some((name, typ)),
                    _ => {
                        return Err(VectorFirAssemblyError::DeclarationShape {
                            name: format!("transport_{}", transport.transport_id),
                        });
                    }
                }
            } else {
                None
            };
            Ok(TransportDeclaration {
                mode: transport.mode,
                declaration: transport.declaration,
                held,
            })
        })
        .collect()
}
pub(super) fn classify_transport_declarations(
    transports: &[TransportDeclaration],
    builder: &mut FirBuilder<'_>,
    local: &mut Vec<FirId>,
    state: &mut Vec<FirId>,
    clear: &mut Vec<FirId>,
    island_declarations: &mut BTreeMap<u64, Vec<FirId>>,
) -> Result<(), VectorFirAssemblyError> {
    for transport in transports {
        match transport.mode {
            ClockTransportMode::OuterChunk => {
                local.push(transport.declaration);
            }
            ClockTransportMode::FusedScalar { .. } => {
                local.push(transport.declaration);
            }
            ClockTransportMode::IslandScalar { domain_id } => {
                island_declarations
                    .entry(domain_id)
                    .or_default()
                    .push(transport.declaration);
            }
            ClockTransportMode::HeldOutput { .. } => {
                let (name, typ) = transport
                    .held
                    .as_ref()
                    .expect("held transport was inspected before FIR construction");
                state.push(transport.declaration);
                let zero = zero_value(builder, typ);
                clear.push(builder.store_var(name, AccessType::Struct, zero));
            }
        }
    }
    Ok(())
}
pub(super) fn materialize_state_storage(
    state_plan: &VerifiedVectorStatePlan,
    real_type: FirType,
    builder: &mut FirBuilder<'_>,
    state: &mut Vec<FirId>,
    local: &mut Vec<FirId>,
    clear: &mut Vec<FirId>,
) -> Result<(), VectorFirAssemblyError> {
    let mut clock_cursors = BTreeSet::new();
    for delay in &state_plan.plan().delays {
        let typ = state_fir_type(delay, real_type.clone())?;
        match &delay.storage {
            VectorDelayStorage::Register {
                local_name,
                persistent_name,
                ..
            } => {
                local.push(builder.declare_var(local_name, typ.clone(), AccessType::Stack, None));
                state.push(builder.declare_var(
                    persistent_name,
                    typ.clone(),
                    AccessType::Struct,
                    None,
                ));
                let zero = zero_value(builder, &typ);
                clear.push(builder.store_var(persistent_name, AccessType::Struct, zero));
            }
            VectorDelayStorage::Copy {
                temporary_name,
                permanent_name,
                history_length,
                temporary_length,
            } => {
                let temporary_length_u64 = *temporary_length;
                let history_length_u64 = *history_length;
                let temporary_length = usize_value("temporary delay length", temporary_length_u64)?;
                let history_length = usize_value("permanent delay length", history_length_u64)?;
                local.push(builder.declare_var(
                    temporary_name,
                    FirType::Array(Box::new(typ.clone()), temporary_length),
                    AccessType::Stack,
                    None,
                ));
                state.push(builder.declare_var(
                    permanent_name,
                    FirType::Array(Box::new(typ.clone()), history_length),
                    AccessType::Struct,
                    None,
                ));
                clear.push(clear_table(
                    builder,
                    permanent_name,
                    AccessType::Struct,
                    &typ,
                    history_length_u64,
                )?);
            }
            VectorDelayStorage::Ring {
                buffer_name,
                index_name,
                index_save_name,
                capacity,
                ..
            } => {
                let capacity_u64 = *capacity;
                let capacity = usize_value("ring capacity", capacity_u64)?;
                state.push(builder.declare_var(
                    buffer_name,
                    FirType::Array(Box::new(typ.clone()), capacity),
                    AccessType::Struct,
                    None,
                ));
                state.push(builder.declare_var(
                    index_name,
                    FirType::Int32,
                    AccessType::Struct,
                    None,
                ));
                state.push(builder.declare_var(
                    index_save_name,
                    FirType::Int32,
                    AccessType::Struct,
                    None,
                ));
                clear.push(clear_table(
                    builder,
                    buffer_name,
                    AccessType::Struct,
                    &typ,
                    capacity_u64,
                )?);
                let zero = builder.int32(0);
                clear.push(builder.store_var(index_name, AccessType::Struct, zero));
                clear.push(builder.store_var(index_save_name, AccessType::Struct, zero));
            }
            VectorDelayStorage::ClockRing {
                buffer_name,
                cursor_name,
                capacity,
                ..
            } => {
                let capacity_u64 = *capacity;
                let capacity = usize_value("clock-ring capacity", capacity_u64)?;
                state.push(builder.declare_var(
                    buffer_name,
                    FirType::Array(Box::new(typ.clone()), capacity),
                    AccessType::Struct,
                    None,
                ));
                clear.push(clear_table(
                    builder,
                    buffer_name,
                    AccessType::Struct,
                    &typ,
                    capacity_u64,
                )?);
                if clock_cursors.insert(cursor_name.clone()) {
                    state.push(builder.declare_var(
                        cursor_name,
                        FirType::Int32,
                        AccessType::Struct,
                        None,
                    ));
                    let zero = builder.int32(0);
                    clear.push(builder.store_var(cursor_name, AccessType::Struct, zero));
                }
            }
        }
    }
    for prefix in &state_plan.plan().prefixes {
        let typ = value_type_to_fir(&prefix.value_type, real_type.clone(), prefix.signal_id)?;
        state.push(builder.declare_var(&prefix.state_name, typ.clone(), AccessType::Struct, None));
        let initial = match prefix.initial {
            VectorStateInitialValue::Int(value) => builder.int32(value),
            VectorStateInitialValue::RealBits(bits) => match typ {
                FirType::Float64 => builder.float64(f64::from_bits(bits)),
                _ => builder.float32(f64::from_bits(bits) as f32),
            },
            VectorStateInitialValue::Zero => zero_value(builder, &typ),
        };
        clear.push(builder.store_var(&prefix.state_name, AccessType::Struct, initial));
    }
    for waveform in &state_plan.plan().waveforms {
        state.push(builder.declare_var(
            &waveform.index_name,
            FirType::Int32,
            AccessType::Struct,
            None,
        ));
        let zero = builder.int32(0);
        clear.push(builder.store_var(&waveform.index_name, AccessType::Struct, zero));
    }
    Ok(())
}
pub(super) struct StateMaterializationContext<'a> {
    delays: &'a BTreeMap<u64, &'a DelayTransition>,
    recursions: &'a BTreeMap<u64, &'a RecursionTransition>,
    prefixes: &'a BTreeMap<u64, &'a PrefixTransition>,
    waveforms: &'a BTreeMap<u64, &'a WaveformTransition>,
    definitions: &'a BTreeMap<(VectorRegion, u64), FirId>,
    fused_members_by_loop: &'a BTreeMap<u64, &'a [u64]>,
    signal_types: &'a BTreeMap<u64, ValueType>,
    real_type: FirType,
}
pub(super) fn fused_member_definition(
    loop_id: u64,
    signal_id: u64,
    context: &StateMaterializationContext<'_>,
) -> Option<FirId> {
    if let Some(value) = context
        .definitions
        .get(&(VectorRegion::Loop(loop_id), signal_id))
        .copied()
    {
        return Some(value);
    }
    let members = context.fused_members_by_loop.get(&loop_id)?;
    let mut values = context
        .definitions
        .iter()
        .filter_map(|((region, id), &value)| {
            (*id == signal_id
            && matches!(region, VectorRegion::Loop(owner) if members.binary_search(owner).is_ok()))
        .then_some(value)
        });
    let value = values.next()?;
    values.all(|candidate| candidate == value).then_some(value)
}
pub(super) fn materialize_loop(
    loop_id: u64,
    inputs: &[FirId],
    phases: Option<&crate::signal_fir::vector::state::LoopStatePhases>,
    context: &StateMaterializationContext<'_>,
    builder: &mut FirBuilder<'_>,
) -> Result<AssembledVectorLoop, VectorFirAssemblyError> {
    let mut recursion_values = BTreeMap::new();
    let mut pre = Vec::new();
    let mut exec_actions = Vec::new();
    let mut post = Vec::new();
    if let Some(phases) = phases {
        for action in &phases.pre {
            pre.push(materialize_action(
                loop_id,
                action,
                context,
                &mut recursion_values,
                builder,
            )?);
        }
        for action in &phases.exec {
            exec_actions.push(materialize_action(
                loop_id,
                action,
                context,
                &mut recursion_values,
                builder,
            )?);
        }
        for action in &phases.post {
            post.push(materialize_action(
                loop_id,
                action,
                context,
                &mut recursion_values,
                builder,
            )?);
        }
    }
    let mut exec = inputs.to_vec();
    for action in &exec_actions {
        exec.extend(action.execution_statements.iter().copied());
    }
    let iteration_statement = builder.block(&exec);
    let sample_loop = sample_loop(builder, iteration_statement);
    let mut chunk = pre
        .iter()
        .map(|action| action.statement)
        .collect::<Vec<_>>();
    chunk.push(sample_loop);
    chunk.extend(post.iter().map(|action| action.statement));
    let chunk_statement = builder.block(&chunk);
    Ok(AssembledVectorLoop {
        loop_id,
        pre,
        exec,
        exec_actions,
        post,
        chunk_statement,
        iteration_statement,
    })
}
/// One recursion step: declare each projection's stack value from its fused
/// member definition, register the loads for later delay writes, and return
/// the declarations in projection order.
fn materialize_recursion_step(
    loop_id: u64,
    group: u64,
    context: &StateMaterializationContext<'_>,
    recursion_values: &mut BTreeMap<u64, FirId>,
    builder: &mut FirBuilder<'_>,
) -> Result<Vec<FirId>, VectorFirAssemblyError> {
    let recursions = context.recursions;
    let signal_types = context.signal_types;
    let real_type = context.real_type.clone();
    let group = &group;
    let recursion = recursions[group];
    let mut declarations = Vec::with_capacity(recursion.projections.len());
    for projection in &recursion.projections {
        let value = projection
            .signal_ids
            .iter()
            .find_map(|signal_id| fused_member_definition(loop_id, *signal_id, context))
            .or_else(|| fused_member_definition(loop_id, projection.value_signal_id, context));
        let value = value.ok_or(VectorFirAssemblyError::MissingRecursionProjection {
            group: *group,
            index: projection.index,
        })?;
        let signal_id = projection
            .signal_ids
            .first()
            .copied()
            .unwrap_or(projection.value_signal_id);
        let typ = value_type_to_fir(
            signal_types.get(&signal_id).ok_or(
                VectorFirAssemblyError::MissingRecursionProjection {
                    group: *group,
                    index: projection.index,
                },
            )?,
            real_type.clone(),
            signal_id,
        )
        .map_err(|_| VectorFirAssemblyError::MissingRecursionProjection {
            group: *group,
            index: projection.index,
        })?;
        let name = recursion_name(*group, projection.index);
        declarations.push(builder.declare_var(&name, typ.clone(), AccessType::Stack, Some(value)));
        let load = builder.load_var(name, AccessType::Stack, typ);
        for signal_id in &projection.signal_ids {
            recursion_values.insert(*signal_id, load);
        }
    }
    Ok(declarations)
}

/// One per-sample delay write into the planned storage shape: register
/// store, copy-history slot, masked ring slot, or clock-ring cursor slot.
fn materialize_delay_write(
    loop_id: u64,
    signal_id: u64,
    context: &StateMaterializationContext<'_>,
    recursion_values: &BTreeMap<u64, FirId>,
    builder: &mut FirBuilder<'_>,
) -> Result<FirId, VectorFirAssemblyError> {
    let delays = context.delays;
    let definitions = context.definitions;
    let signal_id = &signal_id;
    let delay = delays[signal_id];
    let value = recursion_values.get(signal_id).copied().or_else(|| {
        definitions
            .get(&(VectorRegion::Loop(loop_id), *signal_id))
            .copied()
    });
    let value = value.ok_or(VectorFirAssemblyError::MissingDefinition {
        signal_id: *signal_id,
        loop_id,
    })?;
    let local = local_index(builder);
    Ok(match &delay.storage {
        VectorDelayStorage::Register { local_name, .. } => {
            builder.store_var(local_name, AccessType::Stack, value)
        }
        VectorDelayStorage::Copy {
            temporary_name,
            history_length,
            ..
        } => {
            let history = fir_i32(builder, "copy history", *history_length)?;
            let index = builder.binop(FirBinOp::Add, history, local, FirType::Int32);
            builder.store_table(temporary_name, AccessType::Stack, index, value)
        }
        VectorDelayStorage::Ring {
            buffer_name,
            index_name,
            mask,
            ..
        } => {
            let index = builder.load_var(index_name, AccessType::Struct, FirType::Int32);
            let added = builder.binop(FirBinOp::Add, index, local, FirType::Int32);
            let mask = fir_i32(builder, "ring mask", *mask)?;
            let masked = builder.binop(FirBinOp::And, added, mask, FirType::Int32);
            builder.store_table(buffer_name, AccessType::Struct, masked, value)
        }
        VectorDelayStorage::ClockRing {
            buffer_name,
            cursor_name,
            mask,
            ..
        } => {
            let cursor = builder.load_var(cursor_name, AccessType::Struct, FirType::Int32);
            let mask = fir_i32(builder, "clock-ring mask", *mask)?;
            let index = builder.binop(FirBinOp::And, cursor, mask, FirType::Int32);
            builder.store_table(buffer_name, AccessType::Struct, index, value)
        }
    })
}

pub(super) fn materialize_action(
    loop_id: u64,
    action: &VectorStateAction,
    context: &StateMaterializationContext<'_>,
    recursion_values: &mut BTreeMap<u64, FirId>,
    builder: &mut FirBuilder<'_>,
) -> Result<VectorStateFirAction, VectorFirAssemblyError> {
    let delays = context.delays;
    let prefixes = context.prefixes;
    let waveforms = context.waveforms;
    let definitions = context.definitions;
    let real_type = context.real_type.clone();
    let mut execution_statements = None;
    let statement = match action {
        VectorStateAction::DelayRegisterLoad { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Register {
                local_name,
                persistent_name,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let value = builder.load_var(persistent_name, AccessType::Struct, typ);
            builder.store_var(local_name, AccessType::Stack, value)
        }
        VectorStateAction::DelayCopyIn { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Copy {
                temporary_name,
                permanent_name,
                history_length,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let index = builder.load_var("vdelay_copy", AccessType::Loop, FirType::Int32);
            let value = builder.load_table(permanent_name, AccessType::Struct, index, typ);
            let store = builder.store_table(temporary_name, AccessType::Stack, index, value);
            let upper = fir_i32(builder, "copy history", *history_length)?;
            let body = builder.block(&[store]);
            builder.simple_for_loop("vdelay_copy", upper, body, false)
        }
        VectorStateAction::DelayRingAdvance { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Ring {
                index_name,
                index_save_name,
                mask,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let index = builder.load_var(index_name, AccessType::Struct, FirType::Int32);
            let saved = builder.load_var(index_save_name, AccessType::Struct, FirType::Int32);
            let added = builder.binop(FirBinOp::Add, index, saved, FirType::Int32);
            let mask = fir_i32(builder, "ring mask", *mask)?;
            let masked = builder.binop(FirBinOp::And, added, mask, FirType::Int32);
            builder.store_var(index_name, AccessType::Struct, masked)
        }
        VectorStateAction::RecursionStep { group } => {
            let declarations =
                materialize_recursion_step(loop_id, *group, context, recursion_values, builder)?;
            execution_statements = Some(declarations.clone());
            builder.block(&declarations)
        }
        VectorStateAction::DelayWrite { signal_id } => {
            materialize_delay_write(loop_id, *signal_id, context, recursion_values, builder)?
        }
        VectorStateAction::PrefixWrite { signal_id } => {
            let transition = prefixes[signal_id];
            let value = definitions
                .get(&(VectorRegion::Loop(loop_id), transition.value_signal_id))
                .copied()
                .ok_or(VectorFirAssemblyError::MissingDefinition {
                    signal_id: transition.value_signal_id,
                    loop_id,
                })?;
            builder.store_var(&transition.state_name, AccessType::Struct, value)
        }
        VectorStateAction::WaveformAdvance { signal_id } => {
            let transition = waveforms[signal_id];
            let index =
                builder.load_var(&transition.index_name, AccessType::Struct, FirType::Int32);
            let one = builder.int32(1);
            let next = builder.binop(FirBinOp::Add, index, one, FirType::Int32);
            let length = fir_i32(builder, "waveform length", transition.length)?;
            let wrapped = builder.binop(FirBinOp::Rem, next, length, FirType::Int32);
            builder.store_var(&transition.index_name, AccessType::Struct, wrapped)
        }
        VectorStateAction::DelayRegisterStore { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Register {
                local_name,
                persistent_name,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let value = builder.load_var(local_name, AccessType::Stack, typ);
            builder.store_var(persistent_name, AccessType::Struct, value)
        }
        VectorStateAction::DelayCopyOut { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Copy {
                temporary_name,
                permanent_name,
                history_length,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let index = builder.load_var("vdelay_copy", AccessType::Loop, FirType::Int32);
            let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
            let source_index = builder.binop(FirBinOp::Add, count, index, FirType::Int32);
            let value = builder.load_table(temporary_name, AccessType::Stack, source_index, typ);
            let store = builder.store_table(permanent_name, AccessType::Struct, index, value);
            let upper = fir_i32(builder, "copy history", *history_length)?;
            let body = builder.block(&[store]);
            builder.simple_for_loop("vdelay_copy", upper, body, false)
        }
        VectorStateAction::DelayRingSaveAdvance { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Ring {
                index_save_name, ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
            builder.store_var(index_save_name, AccessType::Struct, count)
        }
    };
    Ok(VectorStateFirAction {
        action: action.clone(),
        statement,
        execution_statements: execution_statements.unwrap_or_else(|| vec![statement]),
    })
}
pub(super) struct ClockIslandMaterializationContext<'a> {
    routed: &'a VerifiedRoutedFir,
    state_plan: Option<&'a VerifiedVectorStatePlan>,
    clock_plan: Option<&'a VerifiedVectorClockAdPlan>,
    loops: &'a [AssembledVectorLoop],
    definitions: &'a BTreeMap<(VectorRegion, u64), FirId>,
    island_declarations: &'a BTreeMap<u64, Vec<FirId>>,
}
pub(super) fn materialize_clock_islands(
    context: &ClockIslandMaterializationContext<'_>,
    builder: &mut FirBuilder<'_>,
    state_declarations: &mut Vec<FirId>,
    clear_statements: &mut Vec<FirId>,
) -> Result<Vec<AssembledClockIsland>, VectorFirAssemblyError> {
    let routed = context.routed;
    let state_plan = context.state_plan;
    let clock_plan = context.clock_plan;
    let loops = context.loops;
    let definitions = context.definitions;
    let island_declarations = context.island_declarations;
    let Some(clock_plan) = clock_plan else {
        return Ok(Vec::new());
    };
    let mut owner = BTreeMap::new();
    for island in &clock_plan.plan().clock_islands {
        for loop_id in &island.nested_loop_ids {
            if owner.insert(*loop_id, island.domain_id).is_some() {
                return Err(VectorFirAssemblyError::ClockLoopOwnership { loop_id: *loop_id });
            }
        }
    }
    let loop_by_id = loops
        .iter()
        .map(|assembled| (assembled.loop_id, assembled))
        .collect::<BTreeMap<_, _>>();
    let island_by_id = clock_plan
        .plan()
        .clock_islands
        .iter()
        .map(|island| (island.domain_id, island))
        .collect::<BTreeMap<_, _>>();
    for island in &clock_plan.plan().clock_islands {
        if let Some(parent) = island.parent_domain
            && !island_by_id.contains_key(&parent)
        {
            return Err(VectorFirAssemblyError::MissingClockParent {
                domain_id: island.domain_id,
                parent_id: parent,
            });
        }
    }
    let mut statements = BTreeMap::new();
    let mut state_cursor_advances = BTreeMap::new();
    let mut pending = clock_plan.plan().clock_islands.len();
    while statements.len() < pending {
        let before = statements.len();
        for island in &clock_plan.plan().clock_islands {
            if statements.contains_key(&island.domain_id) {
                continue;
            }
            let children = clock_plan
                .plan()
                .clock_islands
                .iter()
                .filter(|child| child.parent_domain == Some(island.domain_id))
                .collect::<Vec<_>>();
            if children
                .iter()
                .any(|child| !statements.contains_key(&child.domain_id))
            {
                continue;
            }
            let local_declarations = island_declarations
                .get(&island.domain_id)
                .cloned()
                .unwrap_or_default();
            let scheduled_loop_ids = scheduled_island_loop_ids(routed, island);
            let mut body = local_declarations.clone();
            body.extend(
                scheduled_loop_ids
                    .iter()
                    .map(|loop_id| loop_by_id[loop_id].iteration_statement),
            );
            body.extend(children.iter().map(|child| statements[&child.domain_id]));
            if let Some(cursor_name) = clock_cursor_for_domain(state_plan, island.domain_id)? {
                let cursor = builder.load_var(&cursor_name, AccessType::Struct, FirType::Int32);
                let one = builder.int32(1);
                let next = builder.binop(FirBinOp::Add, cursor, one, FirType::Int32);
                let advance = builder.store_var(&cursor_name, AccessType::Struct, next);
                body.push(advance);
                state_cursor_advances.insert(island.domain_id, advance);
            }
            let body = builder.block(&body);
            let clock = resolve_clock_value(routed, definitions, island)?;
            let guarded = build_guard(
                island,
                clock,
                body,
                builder,
                state_declarations,
                clear_statements,
            );
            statements.insert(island.domain_id, guarded);
        }
        if statements.len() == before {
            let island = clock_plan
                .plan()
                .clock_islands
                .iter()
                .find(|island| !statements.contains_key(&island.domain_id))
                .expect("pending island count is nonzero");
            return Err(VectorFirAssemblyError::MissingClockParent {
                domain_id: island.domain_id,
                parent_id: island.parent_domain.unwrap_or(island.domain_id),
            });
        }
        pending = clock_plan.plan().clock_islands.len();
    }
    Ok(clock_plan
        .plan()
        .clock_islands
        .iter()
        .map(|island| AssembledClockIsland {
            domain_id: island.domain_id,
            parent_domain: island.parent_domain,
            guard: island.guard,
            nested_loop_ids: scheduled_island_loop_ids(routed, island),
            local_declarations: island_declarations
                .get(&island.domain_id)
                .cloned()
                .unwrap_or_default(),
            state_cursor_advance: state_cursor_advances.get(&island.domain_id).copied(),
            statement: statements[&island.domain_id],
        })
        .collect())
}
pub(super) fn clock_cursor_for_domain(
    state_plan: Option<&VerifiedVectorStatePlan>,
    domain_id: u64,
) -> Result<Option<String>, VectorFirAssemblyError> {
    let Some(state_plan) = state_plan else {
        return Ok(None);
    };
    let names = state_plan
        .plan()
        .delays
        .iter()
        .filter_map(|delay| match &delay.storage {
            VectorDelayStorage::ClockRing {
                cursor_name,
                domain_id: delay_domain,
                ..
            } if *delay_domain == domain_id => Some(cursor_name.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if names.len() > 1 {
        return Err(VectorFirAssemblyError::IslandShape { domain_id });
    }
    Ok(names.into_iter().next())
}
pub(super) fn materialize_top_level(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    loops: &[AssembledVectorLoop],
    islands: &[AssembledClockIsland],
    clock_output_stores: &[VectorClockOutputStore],
    builder: &mut FirBuilder<'_>,
) -> Result<FirId, VectorFirAssemblyError> {
    let owned = islands
        .iter()
        .flat_map(|island| island.nested_loop_ids.iter().copied())
        .collect::<BTreeSet<_>>();
    let direct_owner = islands
        .iter()
        .flat_map(|island| {
            island
                .nested_loop_ids
                .iter()
                .map(move |loop_id| (*loop_id, island.domain_id))
        })
        .collect::<BTreeMap<_, _>>();
    let island_by_id = islands
        .iter()
        .map(|island| (island.domain_id, island))
        .collect::<BTreeMap<_, _>>();
    let mut stores_by_root = BTreeMap::<u64, Vec<FirId>>::new();
    for output in clock_output_stores {
        let mut domain = *direct_owner.get(&output.owner_loop_id).ok_or(
            VectorFirAssemblyError::ClockLoopOwnership {
                loop_id: output.owner_loop_id,
            },
        )?;
        while let Some(parent) = island_by_id[&domain].parent_domain {
            domain = parent;
        }
        stores_by_root
            .entry(domain)
            .or_default()
            .push(output.statement);
    }
    let mut roots = Vec::new();
    for island in islands
        .iter()
        .filter(|island| island.parent_domain.is_none())
    {
        let mut sample_body = vec![island.statement];
        sample_body.extend(stores_by_root.remove(&island.domain_id).unwrap_or_default());
        let statement = builder.block(&sample_body);
        roots.push((island.nested_loop_ids.first().copied(), statement));
    }
    let loop_by_id = loops
        .iter()
        .map(|assembled| (assembled.loop_id, assembled))
        .collect::<BTreeMap<_, _>>();
    let fused_group_by_member = routed
        .plan()
        .fused_serial_groups
        .iter()
        .flat_map(|group| {
            group
                .member_loop_ids
                .iter()
                .map(move |&loop_id| (loop_id, group.group_id))
        })
        .collect::<BTreeMap<_, _>>();
    let fused_members_by_group = routed
        .plan()
        .fused_serial_groups
        .iter()
        .map(|group| {
            let members = routed
                .layout()
                .loops()
                .iter()
                .filter_map(|region| {
                    group
                        .member_loop_ids
                        .binary_search(&region.loop_id)
                        .is_ok()
                        .then_some(region.loop_id)
                })
                .collect::<Vec<_>>();
            (group.group_id, members)
        })
        .collect::<BTreeMap<_, _>>();
    let lockstep_bundle_by_member = routed
        .plan()
        .lockstep_bundles
        .iter()
        .flat_map(|bundle| {
            bundle
                .member_loop_ids
                .iter()
                .map(move |&loop_id| (loop_id, bundle.bundle_id))
        })
        .collect::<BTreeMap<_, _>>();
    let lockstep_members_by_bundle = routed
        .plan()
        .lockstep_bundles
        .iter()
        .map(|bundle| (bundle.bundle_id, bundle.member_loop_ids.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let register_bundles = state_plan
        .into_iter()
        .flat_map(|state| &state.plan().lockstep_register_bundles)
        .map(|bundle| bundle.bundle_id)
        .collect::<BTreeSet<_>>();
    let mut body = Vec::new();
    for region in routed.layout().loops() {
        if !owned.contains(&region.loop_id) {
            body.extend(
                loop_by_id[&region.loop_id]
                    .pre
                    .iter()
                    .map(|action| action.statement),
            );
        }
    }
    for region in routed.layout().loops() {
        if !owned.contains(&region.loop_id) {
            if let Some(group_id) = fused_group_by_member.get(&region.loop_id) {
                let members = &fused_members_by_group[group_id];
                if members.first() == Some(&region.loop_id) {
                    let iterations = members
                        .iter()
                        .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                        .collect::<Vec<_>>();
                    let fused_body = builder.block(&iterations);
                    body.push(sample_loop(builder, fused_body));
                }
            } else if let Some(bundle_id) = lockstep_bundle_by_member.get(&region.loop_id) {
                let members = lockstep_members_by_bundle[bundle_id];
                if members.first() == Some(&region.loop_id) {
                    let statements = if !register_bundles.contains(bundle_id) {
                        members
                            .iter()
                            .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                            .collect::<Vec<_>>()
                    } else {
                        let width = members
                            .first()
                            .map(|loop_id| loop_by_id[loop_id].exec.len())
                            .unwrap_or(0);
                        if members
                            .iter()
                            .all(|loop_id| loop_by_id[loop_id].exec.len() == width)
                        {
                            let mut transposed = Vec::with_capacity(width * members.len());
                            for index in 0..width {
                                for loop_id in members {
                                    transposed.push(loop_by_id[loop_id].exec[index]);
                                }
                            }
                            transposed
                        } else {
                            members
                                .iter()
                                .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                                .collect()
                        }
                    };
                    let lockstep_body = builder.block(&statements);
                    body.push(sample_loop(builder, lockstep_body));
                }
            } else {
                body.push(sample_loop(
                    builder,
                    loop_by_id[&region.loop_id].iteration_statement,
                ));
            }
        }
        for (first_loop, statement) in &roots {
            if *first_loop == Some(region.loop_id) {
                body.push(sample_loop(builder, *statement));
            }
        }
    }
    for region in routed.layout().loops() {
        if !owned.contains(&region.loop_id) {
            body.extend(
                loop_by_id[&region.loop_id]
                    .post
                    .iter()
                    .map(|action| action.statement),
            );
        }
    }
    if body.is_empty() && !roots.is_empty() {
        body.extend(
            roots
                .into_iter()
                .map(|(_, statement)| sample_loop(builder, statement)),
        );
    }
    Ok(builder.block(&body))
}
pub(super) fn resolve_clock_value(
    routed: &VerifiedRoutedFir,
    definitions: &BTreeMap<(VectorRegion, u64), FirId>,
    island: &ClockIsland,
) -> Result<FirId, VectorFirAssemblyError> {
    routed
        .trace()
        .uses()
        .iter()
        .find(|use_| {
            use_.signal_id == island.clock_signal_id
                && use_.consumer_loop == island.boundary_loop_id
        })
        .map(|use_| use_.value)
        .or_else(|| {
            definitions
                .get(&(VectorRegion::Control, island.clock_signal_id))
                .copied()
        })
        .or_else(|| {
            definitions
                .get(&(
                    VectorRegion::Loop(island.boundary_loop_id),
                    island.clock_signal_id,
                ))
                .copied()
        })
        .ok_or(VectorFirAssemblyError::MissingClockValue {
            domain_id: island.domain_id,
            signal_id: island.clock_signal_id,
        })
}
pub(super) fn build_guard(
    island: &ClockIsland,
    clock: FirId,
    body: FirId,
    builder: &mut FirBuilder<'_>,
    state_declarations: &mut Vec<FirId>,
    clear_statements: &mut Vec<FirId>,
) -> FirId {
    match island.guard {
        ClockGuard::BooleanOnDemand => {
            let zero = builder.int32(0);
            let cond = builder.binop(FirBinOp::Ne, clock, zero, FirType::Bool);
            builder.if_(cond, body, None)
        }
        ClockGuard::CountedOnDemand | ClockGuard::CountedUpsampling => builder.simple_for_loop(
            format!("vclock_d{}_fire", island.domain_id),
            clock,
            body,
            false,
        ),
        ClockGuard::DownsampleModulo => {
            let name = format!("vclock_d{}_counter", island.domain_id);
            state_declarations.push(builder.declare_var(
                &name,
                FirType::Int32,
                AccessType::Struct,
                None,
            ));
            let zero = builder.int32(0);
            clear_statements.push(builder.store_var(&name, AccessType::Struct, zero));
            let counter = builder.load_var(&name, AccessType::Struct, FirType::Int32);
            let zero = builder.int32(0);
            let cond = builder.binop(FirBinOp::Eq, counter, zero, FirType::Bool);
            let guarded = builder.if_(cond, body, None);
            let counter = builder.load_var(&name, AccessType::Struct, FirType::Int32);
            let one = builder.int32(1);
            let next = builder.binop(FirBinOp::Add, counter, one, FirType::Int32);
            let modulo = builder.binop(FirBinOp::Rem, next, clock, FirType::Int32);
            let update = builder.store_var(name, AccessType::Struct, modulo);
            builder.block(&[guarded, update])
        }
    }
}
pub(super) fn definition_map(
    definitions: &[RoutedDefinition],
) -> BTreeMap<(VectorRegion, u64), FirId> {
    definitions
        .iter()
        .map(|definition| ((definition.region, definition.signal_id), definition.value))
        .collect()
}
pub(super) fn state_fir_type(
    delay: &DelayTransition,
    real_type: FirType,
) -> Result<FirType, VectorFirAssemblyError> {
    match delay.value_type {
        ValueType::Int => Ok(FirType::Int32),
        ValueType::Real => Ok(real_type),
        ValueType::Sound | ValueType::Tuple(_) => {
            Err(VectorFirAssemblyError::UnsupportedValueType {
                signal_id: delay.signal_id,
            })
        }
    }
}
pub(super) fn value_type_to_fir(
    value_type: &ValueType,
    real_type: FirType,
    signal_id: u64,
) -> Result<FirType, VectorFirAssemblyError> {
    match value_type {
        ValueType::Int => Ok(FirType::Int32),
        ValueType::Real => Ok(real_type),
        ValueType::Sound | ValueType::Tuple(_) => {
            Err(VectorFirAssemblyError::UnsupportedValueType { signal_id })
        }
    }
}
pub(super) fn zero_value(builder: &mut FirBuilder<'_>, typ: &FirType) -> FirId {
    match typ {
        FirType::Float32 | FirType::FaustFloat => builder.float32(0.0),
        FirType::Float64 => builder.float64(0.0),
        FirType::Int64 => builder.int64(0),
        FirType::Bool => builder.bool_(false),
        _ => builder.int32(0),
    }
}
pub(super) fn clear_table(
    builder: &mut FirBuilder<'_>,
    name: &str,
    access: AccessType,
    typ: &FirType,
    length: u64,
) -> Result<FirId, VectorFirAssemblyError> {
    let index = builder.load_var("vclear", AccessType::Loop, FirType::Int32);
    let zero = zero_value(builder, typ);
    let store = builder.store_table(name, access, index, zero);
    let upper = fir_i32(builder, "clear length", length)?;
    let body = builder.block(&[store]);
    Ok(builder.simple_for_loop("vclear", upper, body, false))
}
pub(super) fn sample_loop(builder: &mut FirBuilder<'_>, body: FirId) -> FirId {
    let start = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let init = builder.declare_var("i0", FirType::Int32, AccessType::Loop, Some(start));
    let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
    let end = builder.binop(FirBinOp::Add, start, count, FirType::Int32);
    let one = builder.int32(1);
    builder.for_loop("i0", init, end, one, body, false)
}
pub(super) fn local_index(builder: &mut FirBuilder<'_>) -> FirId {
    let index = builder.load_var("i0", AccessType::Loop, FirType::Int32);
    let base = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    builder.binop(FirBinOp::Sub, index, base, FirType::Int32)
}
pub(super) fn fir_i32(
    builder: &mut FirBuilder<'_>,
    what: &'static str,
    value: u64,
) -> Result<FirId, VectorFirAssemblyError> {
    let value = i32::try_from(value)
        .map_err(|_| VectorFirAssemblyError::ArithmeticOverflow { what, value })?;
    Ok(builder.int32(value))
}
pub(super) fn usize_value(what: &'static str, value: u64) -> Result<usize, VectorFirAssemblyError> {
    usize::try_from(value).map_err(|_| VectorFirAssemblyError::ArithmeticOverflow { what, value })
}
