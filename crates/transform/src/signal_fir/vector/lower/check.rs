//! Boundary and body verification for lowered pure-vector programs.
//! Called from the producer's terminal step in `signal.rs` (plan provenance: §4.8).

use super::program::*;
use super::tables::{mutable_table_signal, readonly_table_signal};
use crate::signal_fir::vector::analysis::EffectAtom;
use crate::signal_fir::vector::clock_ad::VerifiedVectorClockAdPlan;
use crate::signal_fir::vector::recursion::decode_symbolic_group_bodies;
use crate::signal_fir::vector::route::{RoutedUseSource, VectorRegion, VerifiedRoutedFir};
use crate::signal_fir::vector::state::VerifiedVectorStatePlan;
use crate::signal_fir::vector::verify::{Placement, ValueType, VectorPlan};
use crate::signal_prepare::{SimpleSigType, VerifiedPreparedSignals};
use fir::{FirId, FirMatch, FirStore, match_fir};
use signals::{SigId, SigMatch, dump_sig_readable, match_sig};
use std::collections::{BTreeMap, BTreeSet};
use tlib::match_sym_ref;

pub(super) fn collect_prepared_ids(prepared: &VerifiedPreparedSignals) -> BTreeMap<u64, SigId> {
    let mut ids = BTreeMap::new();
    let mut stack = prepared.outputs().to_vec();
    while let Some(id) = stack.pop() {
        if ids.insert(u64::from(id.as_u32()), id).is_some() {
            continue;
        }
        if let Some(children) = prepared.arena().children(id) {
            stack.extend(children.iter().copied());
        }
    }
    ids
}
pub(super) fn verify_plan_prepared_boundary(
    prepared: &VerifiedPreparedSignals,
    ui: &ui::UiProgram,
    plan: &VectorPlan,
    ids: &BTreeMap<u64, SigId>,
    state_plan: Option<&VerifiedVectorStatePlan>,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
) -> Result<(), PureVectorLowerError> {
    let mut managed_resources = state_plan
        .map(VerifiedVectorStatePlan::managed_resources)
        .unwrap_or_default();
    if let Some(clock_plan) = clock_plan {
        managed_resources.extend(clock_plan.managed_state_resources());
    }
    let output_channels = ids
        .values()
        .filter_map(|&sig| match match_sig(prepared.arena(), sig) {
            SigMatch::Output(channel, _) if channel >= 0 => {
                Some(u32::try_from(channel).expect("nonnegative output channel fits u32"))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for record in &plan.signals {
        let sig = ids.get(&record.signal_id).copied().ok_or(
            PureVectorLowerError::MissingPreparedSignal {
                signal_id: record.signal_id,
            },
        )?;
        let prepared_type = prepared.ty(sig);
        let matches = match (&record.value_type, prepared_type) {
            (ValueType::Int, Some(SimpleSigType::Int))
            | (ValueType::Real, Some(SimpleSigType::Real))
            | (ValueType::Sound, Some(SimpleSigType::Sound)) => true,
            (ValueType::Tuple(_), _) => {
                decode_symbolic_group_bodies(prepared.arena(), sig).is_some()
                    || match_sym_ref(prepared.arena(), sig).is_some()
            }
            _ => false,
        };
        if !matches {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                eprintln!(
                    "[vector-lower-type-mismatch] signal={} planned={:?} prepared={prepared_type:?} expr={}",
                    record.signal_id,
                    record.value_type,
                    dump_sig_readable(prepared.arena(), sig)
                );
            }
            return Err(PureVectorLowerError::PlannedTypeMismatch {
                signal_id: record.signal_id,
                planned: record.value_type.clone(),
                prepared: prepared_type,
            });
        }
        let effects_supported = record.effects.iter().all(|effect| match effect {
            EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource) => {
                managed_resources.contains(resource)
                    // Recursive projection records inherit the full `SYMREC`
                    // effect set, including unreferenced identity body slots.
                    // The state plan independently proves that every reachable
                    // projection has a transition; an unmanaged recursion
                    // resource at this later lowering boundary is therefore
                    // aggregate metadata, not a missing physical state word.
                    || matches!(resource, crate::signal_fir::vector::analysis::StateResource::Recursion { .. })
            }
            EffectAtom::ReadTable(table) | EffectAtom::WriteTable(table) => {
                readonly_table_signal(prepared, ids, u64::from(*table))
                    || mutable_table_signal(prepared, ids, u64::from(*table))
            }
            EffectAtom::WriteOutput(channel) => output_channels.contains(channel),
            EffectAtom::WriteUi(control) => ui.control(*control).is_some_and(|spec| {
                matches!(
                    spec.kind,
                    ui::ControlKind::VBargraph | ui::ControlKind::HBargraph
                ) && spec.id == *control
            }),
            _ => false,
        });
        if !record.effects.is_empty() && !effects_supported {
            return Err(PureVectorLowerError::EffectfulSignal {
                signal_id: record.signal_id,
                expression: dump_sig_readable(prepared.arena(), sig),
                effects: record.effects.to_vec(),
            });
        }
    }
    Ok(())
}
/// Independently reconnects final region bodies to the accepted route.
pub fn verify_pure_vector_bodies(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    transport_declarations: &[FirId],
    control_statements: &[FirId],
    regions: &[PureVectorRegionBody],
    state_plan: Option<&VerifiedVectorStatePlan>,
    store: &FirStore,
) -> Result<(), PureVectorLowerError> {
    // Prefix writes are assembled from the accepted state plan after this
    // region-body check. Their routed input therefore has an explicit checked
    // consumer even though it is not reachable from the prefix read value.
    let state_consumed_uses = state_plan
        .into_iter()
        .flat_map(|state| state.plan().prefixes.iter())
        .map(|transition| (transition.loop_id, transition.value_signal_id))
        .collect::<BTreeSet<_>>();
    let expected_order = routed
        .layout()
        .loops()
        .iter()
        .map(|region| region.loop_id)
        .collect::<Vec<_>>();
    let actual_order = regions
        .iter()
        .map(PureVectorRegionBody::loop_id)
        .collect::<Vec<_>>();
    if actual_order != expected_order {
        return Err(PureVectorLowerError::BodyEvidence {
            detail: "region order differs from the verified schedule".to_owned(),
        });
    }
    let routed_transports = routed.trace().transports();
    if transport_declarations.len() != plan.transports.len()
        || routed_transports.len() != plan.transports.len()
    {
        return Err(PureVectorLowerError::BodyEvidence {
            detail: "transport declaration coverage differs from the plan".to_owned(),
        });
    }
    for (index, transport) in plan.transports.iter().enumerate() {
        if transport_declarations[index] != routed_transports[index].declaration {
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "transport {} declaration is not authoritative",
                    transport.transport_id
                ),
            });
        }
        let producer = region_by_id(regions, transport.producer_loop)?;
        let consumer = region_by_id(regions, transport.consumer_loop)?;
        let store_id =
            routed_transports[index]
                .store
                .ok_or_else(|| PureVectorLowerError::BodyEvidence {
                    detail: format!("transport {} has no producer store", transport.transport_id),
                })?;
        let load_id =
            routed_transports[index]
                .load
                .ok_or_else(|| PureVectorLowerError::BodyEvidence {
                    detail: format!("transport {} has no consumer load", transport.transport_id),
                })?;
        if producer
            .statements
            .iter()
            .filter(|&&id| id == store_id)
            .count()
            != 1
        {
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "transport {} store is not emitted exactly once",
                    transport.transport_id
                ),
            });
        }
        if !body_contains(store, &consumer.statements, load_id)
            && !body_contains_equivalent_table_load(store, &consumer.statements, load_id)
            && !body_contains_equivalent_scalar_load(store, &consumer.statements, load_id)
            && !state_consumed_uses.contains(&(transport.consumer_loop, transport.signal_id))
        {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                for statement in &consumer.statements {
                    eprintln!(
                        "[vector-lower-consumer-body] loop={} {}",
                        transport.consumer_loop,
                        fir::dump_fir(store, *statement)
                    );
                }
            }
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "transport {} for signal {} load is absent from its consumer body",
                    transport.transport_id, transport.signal_id
                ),
            });
        }
    }
    for definition in routed.trace().definitions() {
        let visible = match definition.region {
            VectorRegion::Control => body_contains(store, control_statements, definition.value),
            VectorRegion::Loop(loop_id) => body_contains(
                store,
                &region_by_id(regions, loop_id)?.statements,
                definition.value,
            ),
        };
        let structural_tuple = plan
            .signals
            .iter()
            .find(|signal| signal.signal_id == definition.signal_id)
            .is_some_and(|signal| {
                matches!(signal.value_type, ValueType::Tuple(_))
                    && !plan
                        .transports
                        .iter()
                        .any(|transport| transport.signal_id == definition.signal_id)
            });
        // Region-local CSE may rebuild an inline expression around temporary
        // loads, so its pre-CSE FIR id need not remain reachable. It is safe to
        // accept only non-transported `Inline` values here: owned and routed
        // definitions retain exact independent visibility requirements.
        let cse_local_inline = plan
            .signals
            .iter()
            .find(|signal| signal.signal_id == definition.signal_id)
            .is_some_and(|signal| signal.placement == Placement::Inline)
            && !plan
                .transports
                .iter()
                .any(|transport| transport.signal_id == definition.signal_id);
        if !visible && !structural_tuple && !cse_local_inline {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                eprintln!(
                    "[vector-lower-missing-definition] signal={} region={:?} fir={}",
                    definition.signal_id,
                    definition.region,
                    fir::dump_fir(store, definition.value)
                );
            }
            return Err(PureVectorLowerError::BodyEvidence {
                detail: format!(
                    "signal {} definition is absent from {:?}",
                    definition.signal_id, definition.region
                ),
            });
        }
    }
    for routed_use in routed.trace().uses() {
        if let RoutedUseSource::Transport(transport_id) = routed_use.source {
            let consumer = region_by_id(regions, routed_use.consumer_loop)?;
            let transport_load = routed
                .trace()
                .transports()
                .iter()
                .find(|transport| transport.transport_id == transport_id)
                .and_then(|transport| transport.load);
            if !body_contains(store, &consumer.statements, routed_use.value)
                && transport_load.is_none_or(|load| {
                    !body_contains_equivalent_table_load(store, &consumer.statements, load)
                        && !body_contains_equivalent_scalar_load(store, &consumer.statements, load)
                })
                && !state_consumed_uses.contains(&(routed_use.consumer_loop, routed_use.signal_id))
            {
                return Err(PureVectorLowerError::BodyEvidence {
                    detail: format!(
                        "signal {} routed load is absent from loop {}",
                        routed_use.signal_id, routed_use.consumer_loop
                    ),
                });
            }
        }
    }
    Ok(())
}
pub(super) fn region_by_id(
    regions: &[PureVectorRegionBody],
    loop_id: u64,
) -> Result<&PureVectorRegionBody, PureVectorLowerError> {
    regions
        .iter()
        .find(|region| region.loop_id == loop_id)
        .ok_or_else(|| PureVectorLowerError::BodyEvidence {
            detail: format!("missing loop body {loop_id}"),
        })
}
pub(super) fn body_contains(store: &FirStore, roots: &[FirId], needle: FirId) -> bool {
    let mut stack = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        if value == needle {
            return true;
        }
        if seen.insert(value) {
            stack.extend(fir_children(store, value));
        }
    }
    false
}
pub(super) fn body_contains_equivalent_table_load(
    store: &FirStore,
    roots: &[FirId],
    expected: FirId,
) -> bool {
    let FirMatch::LoadTable {
        name: expected_name,
        access: expected_access,
        typ: expected_type,
        ..
    } = match_fir(store, expected)
    else {
        return false;
    };
    let mut stack = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        if !seen.insert(value) {
            continue;
        }
        if matches!(
            match_fir(store, value),
            FirMatch::LoadTable { name, access, typ, .. }
                if name == expected_name && access == expected_access && typ == expected_type
        ) {
            return true;
        }
        stack.extend(fir_children(store, value));
    }
    false
}
pub(super) fn body_contains_equivalent_scalar_load(
    store: &FirStore,
    roots: &[FirId],
    expected: FirId,
) -> bool {
    let FirMatch::LoadVar {
        name: expected_name,
        access: expected_access,
        typ: expected_type,
    } = match_fir(store, expected)
    else {
        return false;
    };
    let mut stack = roots.to_vec();
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        if !seen.insert(value) {
            continue;
        }
        if matches!(
            match_fir(store, value),
            FirMatch::LoadVar { name, access, typ }
                if name == expected_name && access == expected_access && typ == expected_type
        ) {
            return true;
        }
        stack.extend(fir_children(store, value));
    }
    false
}
/// Complete child listing for the routed-body name search.
///
/// Delegates to the exhaustive [`fir::fir_match_children`] primitive so an
/// unclassified `FirMatch` variant fails at compile time in `fir` instead of
/// silently truncating this verifier's search (the failure mode that once hid
/// a transport during E2).
pub(super) fn fir_children(store: &FirStore, value: FirId) -> Vec<FirId> {
    fir::fir_match_children(store, value)
}
