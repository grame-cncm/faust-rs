//! Independent checker and the shared terminal verify path.
//!
//! `verify_routed_fir_with_policies_after_plan` is called by BOTH the
//! producer session's terminal verification (`session.rs`) and the
//! standalone checker entries below, so its admission guards remain on
//! both paths (the shared-guard rule of the producer/checker doctrine in
//! `vector/mod.rs`).

use super::model::*;
use crate::signal_fir::vector::clock_ad::{
    ClockTransportMode, ClockTransportPolicy, VerifiedVectorClockAdPlan,
};
use crate::signal_fir::vector::verify::{
    Placement, TransportRecord, ValueType, VectorPlan, verify_vector_plan,
};
use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, match_fir};
use std::collections::{BTreeMap, BTreeSet};

/// Independently checks a complete routed-FIR trace.
pub fn verify_routed_fir(
    plan: &VectorPlan,
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let policies = plan
        .transports
        .iter()
        .map(|transport| ClockTransportPolicy {
            transport_id: transport.transport_id,
            mode: fused_transport_group(plan, transport.transport_id)
                .map_or(ClockTransportMode::OuterChunk, |group_id| {
                    ClockTransportMode::FusedScalar { group_id }
                }),
        })
        .collect::<Vec<_>>();
    verify_routed_fir_with_policies(plan, &policies, trace, real_type, store)
}
/// Independently checks routed FIR against the exact clock-plan transport policy.
pub fn verify_routed_fir_with_clock_plan(
    plan: &VectorPlan,
    clock_plan: &VerifiedVectorClockAdPlan,
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    if clock_plan.vector_plan() != plan {
        return Err(VectorRouteError::ClockPlanMismatch);
    }
    verify_routed_fir_with_policies(plan, &clock_plan.plan().transports, trace, real_type, store)
}
fn verify_routed_fir_with_policies(
    plan: &VectorPlan,
    policies: &[ClockTransportPolicy],
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    verify_vector_plan(plan)?;
    verify_routed_fir_with_policies_after_plan(plan, policies, trace, real_type, store)
}
/// Checks route evidence relative to an already accepted opaque vector plan.
/// Public raw-plan checkers retain the full plan verification above; the
/// production session uses this boundary to avoid rechecking the same global
/// plan at route construction and route closure.
pub(super) fn verify_routed_fir_with_policies_after_plan(
    plan: &VectorPlan,
    policies: &[ClockTransportPolicy],
    trace: &RoutedFirTrace,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    verify_policy_coverage(plan, policies)?;
    let signal_by_id = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let loop_ids = plan
        .loops
        .iter()
        .map(|record| record.loop_id)
        .collect::<BTreeSet<_>>();

    let mut seen_definitions = BTreeSet::new();
    let mut signals_with_definition = BTreeSet::new();
    for definition in &trace.definitions {
        let signal = signal_by_id.get(&definition.signal_id).copied().ok_or(
            VectorRouteError::UnknownSignal {
                signal_id: definition.signal_id,
            },
        )?;
        if !seen_definitions.insert((definition.region, definition.signal_id)) {
            return Err(VectorRouteError::DuplicateDefinition {
                signal_id: definition.signal_id,
                region: definition.region,
            });
        }
        let legal = match (signal.placement, definition.region) {
            (Placement::Control, VectorRegion::Control) => true,
            (Placement::Inline, VectorRegion::Loop(loop_id)) => loop_ids.contains(&loop_id),
            (Placement::Owned(owner), VectorRegion::Loop(loop_id)) => owner == loop_id,
            _ => false,
        };
        if !legal {
            return Err(VectorRouteError::WrongRegion {
                signal_id: definition.signal_id,
                expected: signal.placement,
                actual: definition.region,
            });
        }
        check_value_type(
            definition.signal_id,
            &signal.value_type,
            real_type,
            definition.value,
            store,
        )?;
        signals_with_definition.insert(definition.signal_id);
    }
    for signal in &plan.signals {
        if !signals_with_definition.contains(&signal.signal_id) {
            // Tuple-valued inline records are structural recursion/group
            // carriers. Their scalar projections carry all executable FIR
            // definitions; the tuple identity itself has no runtime value.
            if signal.structural {
                continue;
            }
            return match signal.placement {
                Placement::Inline => Err(VectorRouteError::MissingInlineDefinition {
                    signal_id: signal.signal_id,
                }),
                Placement::Control | Placement::Owned(_) => {
                    Err(VectorRouteError::DefinitionCoverage {
                        signal_id: signal.signal_id,
                    })
                }
            };
        }
    }

    if trace.transports.len() != plan.transports.len() {
        let transport_id = plan
            .transports
            .get(trace.transports.len())
            .map_or(u64::MAX, |transport| transport.transport_id);
        return Err(VectorRouteError::TransportCoverage { transport_id });
    }
    for ((transport, policy), routed) in plan.transports.iter().zip(policies).zip(&trace.transports)
    {
        if routed.transport_id != transport.transport_id {
            return Err(VectorRouteError::TransportCoverage {
                transport_id: transport.transport_id,
            });
        }
        if routed.mode != policy.mode {
            return Err(VectorRouteError::TransportPolicyCoverage {
                transport_id: transport.transport_id,
            });
        }
        verify_transport(transport, policy.mode, routed, real_type, store)?;
        let producer_value_is_declared = trace.definitions.iter().any(|definition| {
            definition.signal_id == transport.signal_id
                && definition.region == VectorRegion::Loop(transport.producer_loop)
                && Some(definition.value) == routed.producer_value
        });
        if !producer_value_is_declared {
            return Err(VectorRouteError::TransportStore {
                transport_id: transport.transport_id,
            });
        }
        let load_is_consumed = trace.uses.iter().any(|routed_use| {
            routed_use.signal_id == transport.signal_id
                && routed_use.consumer_loop == transport.consumer_loop
                && routed_use.source == RoutedUseSource::Transport(transport.transport_id)
                && Some(routed_use.value) == routed.load
        });
        if !load_is_consumed {
            return Err(VectorRouteError::TransportLoad {
                transport_id: transport.transport_id,
            });
        }
    }

    for routed_use in &trace.uses {
        let signal = signal_by_id.get(&routed_use.signal_id).copied().ok_or(
            VectorRouteError::UnknownSignal {
                signal_id: routed_use.signal_id,
            },
        )?;
        if !loop_ids.contains(&routed_use.consumer_loop) {
            return Err(VectorRouteError::UnknownLoop {
                loop_id: routed_use.consumer_loop,
            });
        }
        let valid = match routed_use.source {
            RoutedUseSource::Direct(VectorRegion::Control) => {
                signal.placement == Placement::Control
                    && trace.definitions.iter().any(|definition| {
                        definition.signal_id == routed_use.signal_id
                            && definition.region == VectorRegion::Control
                            && definition.value == routed_use.value
                    })
            }
            RoutedUseSource::Direct(VectorRegion::Loop(source)) => {
                source == routed_use.consumer_loop
                    && (signal.placement == Placement::Inline
                        || signal.placement == Placement::Owned(source))
                    && trace.definitions.iter().any(|definition| {
                        definition.signal_id == routed_use.signal_id
                            && definition.region == VectorRegion::Loop(source)
                            && definition.value == routed_use.value
                    })
            }
            RoutedUseSource::Transport(transport_id) => plan.transports.iter().any(|transport| {
                transport.transport_id == transport_id
                    && transport.signal_id == routed_use.signal_id
                    && transport.consumer_loop == routed_use.consumer_loop
                    && trace.transports.iter().any(|routed| {
                        routed.transport_id == transport_id && routed.load == Some(routed_use.value)
                    })
            }),
        };
        if !valid {
            return Err(VectorRouteError::InvalidUse {
                signal_id: routed_use.signal_id,
                consumer_loop: routed_use.consumer_loop,
            });
        }
    }
    Ok(())
}
pub(super) fn verify_transport(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    routed: &RoutedTransport,
    real_type: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let elem = transport_fir_type(transport, real_type.clone())?;
    verify_transport_declaration(transport, mode, routed.declaration, &elem, store)?;
    let Some(producer_value) = routed.producer_value else {
        return Err(VectorRouteError::TransportStore {
            transport_id: transport.transport_id,
        });
    };
    verify_transport_store(transport, mode, routed.store, producer_value, &elem, store)?;
    verify_transport_load(transport, mode, routed.load, &elem, store)
}
pub(super) fn verify_policy_coverage(
    plan: &VectorPlan,
    policies: &[ClockTransportPolicy],
) -> Result<(), VectorRouteError> {
    if policies.len() != plan.transports.len() {
        let transport_id = plan
            .transports
            .get(policies.len())
            .map_or(u64::MAX, |transport| transport.transport_id);
        return Err(VectorRouteError::TransportPolicyCoverage { transport_id });
    }
    for (transport, policy) in plan.transports.iter().zip(policies) {
        if transport.transport_id != policy.transport_id {
            return Err(VectorRouteError::TransportPolicyCoverage {
                transport_id: transport.transport_id,
            });
        }
        let fused_group = fused_transport_group(plan, transport.transport_id);
        let fused_policy_group = match policy.mode {
            ClockTransportMode::FusedScalar { group_id } => Some(group_id),
            _ => None,
        };
        if fused_group != fused_policy_group {
            return Err(VectorRouteError::TransportPolicyCoverage {
                transport_id: transport.transport_id,
            });
        }
    }
    Ok(())
}
pub(super) fn fused_transport_group(plan: &VectorPlan, transport_id: u64) -> Option<u64> {
    plan.fused_serial_groups.iter().find_map(|group| {
        group
            .internal_transport_ids
            .binary_search(&transport_id)
            .is_ok()
            .then_some(group.group_id)
    })
}
pub(super) fn verify_transport_declaration(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    declaration: FirId,
    elem: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let valid = match (mode, match_fir(store, declaration)) {
        (
            ClockTransportMode::OuterChunk,
            FirMatch::DeclareVar {
                name,
                typ: FirType::Array(actual_elem, actual_length),
                access: AccessType::Stack,
                init: None,
            },
        ) => {
            usize::try_from(transport.length) == Ok(actual_length)
                && name == transport.stable_name
                && *actual_elem == *elem
        }
        (
            ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. },
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Stack,
                init: None,
            },
        ) => name == transport.stable_name && typ == *elem,
        (
            ClockTransportMode::HeldOutput { .. },
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Struct,
                init: None,
            },
        ) => name == transport.stable_name && typ == *elem,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(VectorRouteError::TransportDeclaration {
            transport_id: transport.transport_id,
        })
    }
}
pub(super) fn verify_transport_store(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    statement: Option<FirId>,
    producer_value: FirId,
    elem: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let valid = match (mode, statement.map(|id| match_fir(store, id))) {
        (
            ClockTransportMode::OuterChunk,
            Some(FirMatch::StoreTable {
                name,
                access: AccessType::Stack,
                index,
                value,
            }),
        ) => {
            name == transport.stable_name && value == producer_value && is_chunk_index(store, index)
        }
        (
            ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. },
            Some(FirMatch::StoreVar {
                name,
                access: AccessType::Stack,
                value,
            }),
        )
        | (
            ClockTransportMode::HeldOutput { .. },
            Some(FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            }),
        ) => name == transport.stable_name && value == producer_value,
        _ => false,
    };
    if valid && store.value_type(producer_value) == Some(elem.clone()) {
        Ok(())
    } else {
        Err(VectorRouteError::TransportStore {
            transport_id: transport.transport_id,
        })
    }
}
pub(super) fn verify_transport_load(
    transport: &TransportRecord,
    mode: ClockTransportMode,
    value: Option<FirId>,
    elem: &FirType,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let valid = match (mode, value.map(|id| (id, match_fir(store, id)))) {
        (
            ClockTransportMode::OuterChunk,
            Some((
                id,
                FirMatch::LoadTable {
                    name,
                    access: AccessType::Stack,
                    index,
                    typ,
                },
            )),
        ) => {
            name == transport.stable_name
                && typ == *elem
                && is_chunk_index(store, index)
                && store.value_type(id) == Some(elem.clone())
        }
        (
            ClockTransportMode::FusedScalar { .. } | ClockTransportMode::IslandScalar { .. },
            Some((
                id,
                FirMatch::LoadVar {
                    name,
                    access: AccessType::Stack,
                    typ,
                },
            )),
        )
        | (
            ClockTransportMode::HeldOutput { .. },
            Some((
                id,
                FirMatch::LoadVar {
                    name,
                    access: AccessType::Struct,
                    typ,
                },
            )),
        ) => {
            name == transport.stable_name
                && typ == *elem
                && store.value_type(id) == Some(elem.clone())
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(VectorRouteError::TransportLoad {
            transport_id: transport.transport_id,
        })
    }
}
pub(super) fn transport_fir_type(
    transport: &TransportRecord,
    real_type: FirType,
) -> Result<FirType, VectorRouteError> {
    if matches!(transport.element_type, ValueType::Tuple(_)) {
        Err(VectorRouteError::UnsupportedTupleTransport {
            signal_id: transport.signal_id,
        })
    } else {
        Ok(value_fir_type(&transport.element_type, real_type))
    }
}
pub(in crate::signal_fir::vector) fn value_fir_type(
    value_type: &ValueType,
    real_type: FirType,
) -> FirType {
    match value_type {
        ValueType::Int => FirType::Int32,
        ValueType::Real => real_type,
        ValueType::Sound => FirType::Sound,
        ValueType::Tuple(components) => {
            let fields = components
                .iter()
                .map(|component| value_fir_type(component, real_type.clone()))
                .collect::<Vec<_>>();
            FirType::Struct(tuple_type_name(value_type, &real_type), fields)
        }
    }
}
pub(super) fn tuple_type_name(value_type: &ValueType, real_type: &FirType) -> String {
    fn append_component(name: &mut String, value_type: &ValueType, real_type: &FirType) {
        match value_type {
            ValueType::Int => name.push_str("_i32"),
            ValueType::Sound => name.push_str("_sound"),
            ValueType::Real => match real_type {
                FirType::Float32 => name.push_str("_f32"),
                FirType::Float64 => name.push_str("_f64"),
                _ => name.push_str("_real"),
            },
            ValueType::Tuple(components) => {
                name.push_str("_t");
                name.push_str(&components.len().to_string());
                for component in components {
                    append_component(name, component, real_type);
                }
            }
        }
    }

    let mut name = String::from("frs_vec_tuple");
    append_component(&mut name, value_type, real_type);
    name
}
pub(super) fn check_value_type(
    signal_id: u64,
    value_type: &ValueType,
    real_type: &FirType,
    value: FirId,
    store: &FirStore,
) -> Result<(), VectorRouteError> {
    let expected = value_fir_type(value_type, real_type.clone());
    let actual = store.value_type(value);
    if actual != Some(expected.clone()) {
        Err(VectorRouteError::ValueTypeMismatch {
            signal_id,
            expected,
            actual,
        })
    } else if value_shape_matches(value_type, real_type, value, store) {
        Ok(())
    } else {
        Err(VectorRouteError::TupleValueShape { signal_id })
    }
}
pub(super) fn value_shape_matches(
    value_type: &ValueType,
    real_type: &FirType,
    value: FirId,
    store: &FirStore,
) -> bool {
    let ValueType::Tuple(components) = value_type else {
        return true;
    };
    let FirMatch::ValueArray {
        values,
        typ: FirType::Struct(_, fields),
    } = match_fir(store, value)
    else {
        return false;
    };
    if values.len() != components.len() || fields.len() != components.len() {
        return false;
    }
    components.iter().zip(values).all(|(component, value)| {
        store.value_type(value) == Some(value_fir_type(component, real_type.clone()))
            && value_shape_matches(component, real_type, value, store)
    })
}
pub(super) fn is_chunk_index(store: &FirStore, value: FirId) -> bool {
    let FirMatch::BinOp {
        op: FirBinOp::Sub,
        lhs,
        rhs,
        typ: FirType::Int32,
    } = match_fir(store, value)
    else {
        return false;
    };
    matches!(
        match_fir(store, lhs),
        FirMatch::LoadVar {
            name,
            access: AccessType::Loop,
            typ: FirType::Int32,
        } if name == "i0"
    ) && matches!(
        match_fir(store, rhs),
        FirMatch::LoadVar {
            name,
            access: AccessType::Loop,
            typ: FirType::Int32,
        } if name == "vindex"
    )
}
