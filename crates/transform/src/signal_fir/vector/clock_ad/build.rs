//! Producer derivations for the clock/AD plan: islands, transport
//! policies, and reverse-AD fallbacks. `check.rs` never calls into this
//! module: it independently re-derives every island, transport, and
//! reverse-fallback obligation from the same sources (arena, domain table,
//! decoration certificate, vector plan) using its own checker-owned
//! helpers. The shared terminal verify
//! (`verify_vector_clock_ad_plan_after_vector_plan`) then runs on both this
//! producer path and the standalone checker path (plan provenance: §4.8), so it is now
//! a genuine producer-vs-checker cross-check rather than a replay.

use super::check::{verify_source_alignment, verify_vector_clock_ad_plan_after_vector_plan};
use super::model::*;
use crate::signal_fir::vector::analysis::{EffectAtom, StateCell, StateResource};
use crate::signal_fir::vector::decoration_verify::{
    CanonicalSigType, VerifiedDecorationCertificate,
};
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::verify::{LoopKind, Placement, VectorPlan};
use crate::signal_prepare::VerifiedPreparedSignals;
use propagate::{ClockDomainKind, ClockDomainTable};
use signals::{SigId, SigMatch, match_sig};
use sigtype::Nature;
use std::collections::BTreeMap;

/// Builds and independently checks the clock-island/AD policy.
pub fn build_vector_clock_ad_plan(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VerifiedVectorPlan,
) -> Result<VerifiedVectorClockAdPlan, VectorClockAdError> {
    let plan = vector_plan.plan();
    verify_source_alignment(prepared, domains, decorations, plan)?;
    let clock_islands = derive_clock_islands(prepared, domains, decorations, plan)?;
    let transports = derive_transport_policies(prepared, plan)?;
    let reverse_ad_fallbacks = derive_reverse_fallbacks(prepared, decorations, plan)?;
    let clock_ad_plan = VectorClockAdPlan {
        schema_version: VECTOR_CLOCK_AD_PLAN_VERSION,
        vec_size: plan.vec_size,
        clock_islands,
        transports,
        forward_ad: ForwardAdPolicy::ExpandedSignalGraph,
        reverse_ad_fallbacks,
    };
    verify_vector_clock_ad_plan_after_vector_plan(
        prepared,
        domains,
        decorations,
        plan,
        &clock_ad_plan,
    )?;
    Ok(VerifiedVectorClockAdPlan {
        plan: clock_ad_plan,
        vector_plan: plan.clone(),
    })
}
pub(super) fn derive_clock_islands(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
) -> Result<Vec<ClockIsland>, VectorClockAdError> {
    let ids = crate::signal_fir::vector::common::ids::prepared_signal_ids(prepared);
    let records = decorations
        .certificate()
        .records
        .iter()
        .map(|record| (record.signal_id, record))
        .collect::<BTreeMap<_, _>>();
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let loops = plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record))
        .collect::<BTreeMap<_, _>>();

    let mut wrappers = BTreeMap::<u64, Vec<(u64, u32, ClockDomainKind)>>::new();
    let mut clock_state = BTreeMap::<u64, Vec<u64>>::new();
    for record in records.values() {
        let sig = ids[&record.signal_id];
        if let Some((domain, clock, kind)) = wrapper_domain_and_clock(prepared, sig) {
            wrappers.entry(domain).or_default().push((
                u64::from(record.signal_id),
                clock.as_u32(),
                kind,
            ));
        }
        if record
            .effects
            .iter()
            .any(|effect| is_owned_clock_effect(effect, record.signal_id))
        {
            if matches!(match_sig(prepared.arena(), sig), SigMatch::Clocked(token, _)
                if prepared.arena().is_nil(token))
            {
                continue;
            }
            let domain = clock_state_domain(prepared, sig).ok_or(
                VectorClockAdError::ClockStateDomainUnknown {
                    signal_id: u64::from(record.signal_id),
                },
            )?;
            clock_state
                .entry(domain)
                .or_default()
                .push(u64::from(record.signal_id));
        }
    }

    let mut islands = Vec::new();
    for (domain_id, domain) in domains.iter() {
        let raw = u64::from(domain_id.as_u32());
        let domain_wrappers = wrappers.get(&raw).cloned().unwrap_or_default();
        if domain_wrappers.len() != 1 {
            return Err(VectorClockAdError::WrapperCoverageMismatch { domain_id: raw });
        }
        let (wrapper_signal_id, clock_signal, signal_kind) = domain_wrappers[0];
        if signal_kind != domain.kind {
            return Err(VectorClockAdError::WrapperKindMismatch {
                domain_id: raw,
                table: domain.kind,
                signal: signal_kind,
            });
        }
        let clock_signal_id = u64::from(clock_signal);
        let clock_record =
            records
                .get(&clock_signal)
                .ok_or(VectorClockAdError::ClockSignalUnknown {
                    domain_id: raw,
                    signal_id: clock_signal_id,
                })?;
        let guard = guard_for(raw, domain.kind, &clock_record.sig_type)?;
        let wrapper = signals[&wrapper_signal_id];
        let Placement::Owned(boundary_loop_id) = wrapper.placement else {
            return Err(VectorClockAdError::BoundaryNotOwned {
                domain_id: raw,
                signal_id: wrapper_signal_id,
            });
        };
        if !matches!(loops[&boundary_loop_id].kind, LoopKind::Island(_)) {
            return Err(VectorClockAdError::BoundaryNotSerial {
                domain_id: raw,
                loop_id: boundary_loop_id,
            });
        }
        let signal_ids = records
            .values()
            .filter(|record| record.clock_domain == Some(domain_id.as_u32()))
            .map(|record| u64::from(record.signal_id))
            .collect::<Vec<_>>();
        let domain_clock_id = raw + 1;
        let nested_loop_ids = plan
            .loops
            .iter()
            .filter(|loop_record| {
                loop_record
                    .roots
                    .iter()
                    .any(|root| signals[root].clock_id == domain_clock_id)
            })
            .map(|loop_record| loop_record.loop_id)
            .collect::<Vec<_>>();
        islands.push(ClockIsland {
            domain_id: raw,
            parent_domain: domain.parent.map(|parent| u64::from(parent.as_u32())),
            kind: domain.kind,
            clock_signal_id,
            wrapper_signal_id,
            boundary_loop_id,
            guard,
            signal_ids,
            clock_state_signal_ids: clock_state.remove(&raw).unwrap_or_default(),
            nested_loop_ids,
        });
    }
    if !clock_state.is_empty()
        || wrappers
            .keys()
            .any(|domain| *domain >= domains.len() as u64)
    {
        return Err(VectorClockAdError::IslandCoverageMismatch);
    }
    Ok(islands)
}
pub(super) fn derive_transport_policies(
    prepared: &VerifiedPreparedSignals,
    plan: &VectorPlan,
) -> Result<Vec<ClockTransportPolicy>, VectorClockAdError> {
    let ids = crate::signal_fir::vector::common::ids::prepared_signal_ids(prepared);
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let fused_transports = plan
        .fused_serial_groups
        .iter()
        .flat_map(|group| {
            group
                .internal_transport_ids
                .iter()
                .map(move |&transport_id| (transport_id, group.group_id))
        })
        .collect::<BTreeMap<_, _>>();
    plan.transports
        .iter()
        .map(|transport| {
            let signal = signals.get(&transport.signal_id).ok_or(
                VectorClockAdError::SignalCoverageMismatch {
                    signal_id: transport.signal_id,
                },
            )?;
            let mode = if let Some(&group_id) = fused_transports.get(&transport.transport_id) {
                ClockTransportMode::FusedScalar { group_id }
            } else if signal.clock_id == 0 {
                ClockTransportMode::OuterChunk
            } else if ids
                .get(&u32::try_from(signal.signal_id).map_err(|_| {
                    VectorClockAdError::SignalCoverageMismatch {
                        signal_id: signal.signal_id,
                    }
                })?)
                .is_some_and(|&sig| {
                    matches!(match_sig(prepared.arena(), sig), SigMatch::PermVar(_))
                })
            {
                ClockTransportMode::HeldOutput {
                    domain_id: signal.clock_id - 1,
                }
            } else {
                ClockTransportMode::IslandScalar {
                    domain_id: signal.clock_id - 1,
                }
            };
            Ok(ClockTransportPolicy {
                transport_id: transport.transport_id,
                mode,
            })
        })
        .collect()
}
pub(super) fn derive_reverse_fallbacks(
    prepared: &VerifiedPreparedSignals,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
) -> Result<Vec<ReverseAdFallback>, VectorClockAdError> {
    let ids = crate::signal_fir::vector::common::ids::prepared_signal_ids(prepared);
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut fallbacks = Vec::new();
    for record in &decorations.certificate().records {
        let sig = ids[&record.signal_id];
        let kind = match match_sig(prepared.arena(), sig) {
            SigMatch::ReverseTimeRec(_) => ReverseAdKind::ReverseTimeRec,
            SigMatch::BlockReverseAD { .. } => ReverseAdKind::BlockReverseAd,
            _ => continue,
        };
        let signal_id = u64::from(record.signal_id);
        let Placement::Owned(owner_loop_id) = signals[&signal_id].placement else {
            return Err(VectorClockAdError::ReverseCarrierNotOwned { signal_id });
        };
        fallbacks.push(ReverseAdFallback {
            signal_id,
            owner_loop_id,
            kind,
            epochs: vec![AdEpoch::Forward, AdEpoch::Reverse],
            diagnostic: ReverseAdDiagnostic::ScalarReverseWindowRequired,
        });
    }
    Ok(fallbacks)
}
fn wrapper_domain_and_clock(
    prepared: &VerifiedPreparedSignals,
    sig: SigId,
) -> Option<(u64, SigId, ClockDomainKind)> {
    let (children, kind) = match match_sig(prepared.arena(), sig) {
        SigMatch::OnDemand(children) => (children, ClockDomainKind::OnDemand),
        SigMatch::Upsampling(children) => (children, ClockDomainKind::Upsampling),
        SigMatch::Downsampling(children) => (children, ClockDomainKind::Downsampling),
        _ => return None,
    };
    let first = children.first().copied()?;
    let SigMatch::Clocked(token, clock) = match_sig(prepared.arena(), first) else {
        return None;
    };
    let SigMatch::ClockEnvToken(domain) = match_sig(prepared.arena(), token) else {
        return None;
    };
    Some((u64::from(domain), clock, kind))
}
fn clock_state_domain(prepared: &VerifiedPreparedSignals, sig: SigId) -> Option<u64> {
    match match_sig(prepared.arena(), sig) {
        SigMatch::Clocked(token, _) => {
            let SigMatch::ClockEnvToken(domain) = match_sig(prepared.arena(), token) else {
                return None;
            };
            Some(u64::from(domain))
        }
        SigMatch::OnDemand(_) | SigMatch::Upsampling(_) | SigMatch::Downsampling(_) => {
            wrapper_domain_and_clock(prepared, sig).map(|(domain, _, _)| domain)
        }
        _ => None,
    }
}
fn is_owned_clock_effect(effect: &EffectAtom, signal_id: u32) -> bool {
    matches!(
        effect,
        EffectAtom::ReadState(StateResource::Signal {
            owner,
            cell: StateCell::Clock,
        }) | EffectAtom::WriteState(StateResource::Signal {
            owner,
            cell: StateCell::Clock,
        })
        if *owner == signal_id
    )
}
fn guard_for(
    domain_id: u64,
    kind: ClockDomainKind,
    ty: &CanonicalSigType,
) -> Result<ClockGuard, VectorClockAdError> {
    let (nature, boolean_interval) = scalar_clock_facts(ty)
        .ok_or(VectorClockAdError::UnsupportedClockType { domain_id, kind })?;
    match kind {
        ClockDomainKind::OnDemand if boolean_interval => Ok(ClockGuard::BooleanOnDemand),
        ClockDomainKind::OnDemand if nature == Nature::Int => Ok(ClockGuard::CountedOnDemand),
        ClockDomainKind::Upsampling if nature == Nature::Int => Ok(ClockGuard::CountedUpsampling),
        ClockDomainKind::Downsampling if nature == Nature::Int => Ok(ClockGuard::DownsampleModulo),
        _ => Err(VectorClockAdError::UnsupportedClockType { domain_id, kind }),
    }
}
fn scalar_clock_facts(ty: &CanonicalSigType) -> Option<(Nature, bool)> {
    match ty {
        CanonicalSigType::Sound => None,
        CanonicalSigType::Simple {
            nature, interval, ..
        } => {
            let lo = f64::from_bits(interval.lo_bits);
            let hi = f64::from_bits(interval.hi_bits);
            Some((
                *nature,
                !lo.is_nan() && !hi.is_nan() && lo >= 0.0 && hi <= 1.0,
            ))
        }
        CanonicalSigType::Table { .. } | CanonicalSigType::Tuplet { .. } => None,
    }
}
