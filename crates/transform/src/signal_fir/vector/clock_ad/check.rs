//! Independent checker and the shared terminal verify path.
//!
//! Every obligation below is re-derived from the sources (the `prepared`
//! arena, the `ClockDomainTable`, the decoration certificate, the vector
//! plan) by this module's own code. `check.rs` imports nothing from
//! `build.rs`: the `checker_*` leaf helpers near the bottom of this file are
//! deliberate duplicates of `build.rs`'s small derivation primitives, kept
//! under separate names. That duplication is the assurance boundary (plan
//! `clock-ad-checker-independence-plan-2026-07-20-en.md` §2) and must not be
//! shared with the producer.
//!
//! `verify_vector_clock_ad_plan_after_vector_plan` is called by BOTH the
//! producer's terminal verification (`build_vector_clock_ad_plan`) and the
//! standalone checker (`verify_vector_clock_ad_plan`), so every admission
//! guard it hosts — including `reject_unadopted_stateful_reads` — remains on
//! both paths (the shared-guard rule of the producer/checker doctrine in
//! `vector/mod.rs`). Because the island/transport/
//! reverse-fallback checks below are now independently re-derived rather
//! than replayed from the producer, this path is a genuine producer-vs-
//! checker cross-check.

use super::model::*;
use crate::clk_env::annotate;
use crate::signal_fir::vector::analysis::{DepKind, EffectAtom, StateCell, StateResource};
use crate::signal_fir::vector::decoration_verify::{
    CanonicalSigType, VerifiedDecorationCertificate,
};
use crate::signal_fir::vector::verify::{LoopKind, Placement, VectorPlan, verify_vector_plan};
use crate::signal_prepare::VerifiedPreparedSignals;
use propagate::{ClockDomainKind, ClockDomainTable};
use signals::{SigId, SigMatch, match_sig};
use sigtype::Nature;
use std::collections::{BTreeMap, BTreeSet};
use tlib::{match_sym_rec, match_sym_ref};

/// Recomputes all island, transport, and reverse-fallback obligations.
pub fn verify_vector_clock_ad_plan(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VectorPlan,
    clock_ad_plan: &VectorClockAdPlan,
) -> Result<(), VectorClockAdError> {
    verify_vector_plan(vector_plan)?;
    verify_vector_clock_ad_plan_after_vector_plan(
        prepared,
        domains,
        decorations,
        vector_plan,
        clock_ad_plan,
    )
}
pub(super) fn verify_vector_clock_ad_plan_after_vector_plan(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    vector_plan: &VectorPlan,
    clock_ad_plan: &VectorClockAdPlan,
) -> Result<(), VectorClockAdError> {
    if clock_ad_plan.schema_version != VECTOR_CLOCK_AD_PLAN_VERSION {
        return Err(VectorClockAdError::UnsupportedSchema {
            found: clock_ad_plan.schema_version,
        });
    }
    if clock_ad_plan.vec_size != vector_plan.vec_size {
        return Err(VectorClockAdError::VecSizeMismatch {
            declared: clock_ad_plan.vec_size,
            actual: vector_plan.vec_size,
        });
    }
    verify_source_alignment(prepared, domains, decorations, vector_plan)?;
    verify_clock_islands(
        prepared,
        domains,
        decorations,
        vector_plan,
        &clock_ad_plan.clock_islands,
    )?;
    reject_unadopted_stateful_reads(
        prepared,
        decorations,
        vector_plan,
        &clock_ad_plan.clock_islands,
    )?;
    verify_transport_policies(prepared, vector_plan, &clock_ad_plan.transports)?;
    if clock_ad_plan.forward_ad != ForwardAdPolicy::ExpandedSignalGraph {
        return Err(VectorClockAdError::ReverseAdCoverageMismatch);
    }
    verify_reverse_fallbacks(
        prepared,
        decorations,
        vector_plan,
        &clock_ad_plan.reverse_ad_fallbacks,
    )?;
    Ok(())
}
/// Checker-owned property check for the clock-island facts.
///
/// For every domain of the table (in table iteration order) this verifies,
/// straight against the sources: kind/parent vs the table entry; that
/// `wrapper_signal_id` is the arena's unique wrapper of that kind for the
/// domain, with the declared `clock_signal_id` matching; the guard implied
/// by the clock record's canonical type; that `boundary_loop_id` is the
/// wrapper's owned loop and that loop is a serial island; `signal_ids` as
/// the ordered set of decoration records in this domain; membership and
/// completeness of `clock_state_signal_ids`; and `nested_loop_ids` as the
/// set of loops rooted in this domain's clock id. It also rejects wrappers
/// that reference a domain outside the table.
pub(super) fn verify_clock_islands(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
    declared: &[ClockIsland],
) -> Result<(), VectorClockAdError> {
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

    // One scan of the decoration certificate builds the two source-of-truth
    // maps every island's obligations are checked against: which decoration
    // record is a wrapper for which domain, and which clock-state signal
    // belongs to which domain. Neither map trusts the declared artifact.
    let mut wrappers_by_domain = BTreeMap::<u64, Vec<(u64, u32, ClockDomainKind)>>::new();
    let mut clock_state_by_domain = BTreeMap::<u64, Vec<u64>>::new();
    for record in &decorations.certificate().records {
        let sig = ids[&record.signal_id];
        if let Some((domain, clock, kind)) = checker_wrapper_domain_and_clock(prepared, sig) {
            wrappers_by_domain.entry(domain).or_default().push((
                u64::from(record.signal_id),
                clock.as_u32(),
                kind,
            ));
        }
        if record
            .effects
            .iter()
            .any(|effect| checker_is_owned_clock_effect(effect, record.signal_id))
        {
            if matches!(match_sig(prepared.arena(), sig), SigMatch::Clocked(token, _)
                if prepared.arena().is_nil(token))
            {
                continue;
            }
            let domain = checker_clock_state_domain(prepared, sig).ok_or(
                VectorClockAdError::ClockStateDomainUnknown {
                    signal_id: u64::from(record.signal_id),
                },
            )?;
            clock_state_by_domain
                .entry(domain)
                .or_default()
                .push(u64::from(record.signal_id));
        }
    }

    if declared.len() != domains.len() {
        return Err(VectorClockAdError::IslandCoverageMismatch);
    }
    let mut remaining_clock_state = clock_state_by_domain;
    for ((domain_id, domain), island) in domains.iter().zip(declared) {
        let raw = u64::from(domain_id.as_u32());
        if island.domain_id != raw {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }
        let domain_wrappers = wrappers_by_domain.get(&raw).cloned().unwrap_or_default();
        if domain_wrappers.len() != 1 {
            return Err(VectorClockAdError::WrapperCoverageMismatch { domain_id: raw });
        }
        let (wrapper_signal_id, clock_signal, wrapper_kind) = domain_wrappers[0];
        if wrapper_kind != domain.kind {
            return Err(VectorClockAdError::WrapperKindMismatch {
                domain_id: raw,
                table: domain.kind,
                signal: wrapper_kind,
            });
        }
        if island.kind != domain.kind
            || island.parent_domain != domain.parent.map(|parent| u64::from(parent.as_u32()))
        {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }
        if island.wrapper_signal_id != wrapper_signal_id
            || island.clock_signal_id != u64::from(clock_signal)
        {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }

        let clock_record =
            records
                .get(&clock_signal)
                .ok_or(VectorClockAdError::ClockSignalUnknown {
                    domain_id: raw,
                    signal_id: island.clock_signal_id,
                })?;
        let expected_guard = checker_guard_for(raw, domain.kind, &clock_record.sig_type)?;
        if island.guard != expected_guard {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }

        let wrapper_signal =
            signals
                .get(&wrapper_signal_id)
                .ok_or(VectorClockAdError::BoundaryNotOwned {
                    domain_id: raw,
                    signal_id: wrapper_signal_id,
                })?;
        let Placement::Owned(boundary_loop_id) = wrapper_signal.placement else {
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
        if island.boundary_loop_id != boundary_loop_id {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }

        let expected_signal_ids = decorations
            .certificate()
            .records
            .iter()
            .filter(|record| record.clock_domain == Some(domain_id.as_u32()))
            .map(|record| u64::from(record.signal_id))
            .collect::<Vec<_>>();
        if island.signal_ids != expected_signal_ids {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }

        let expected_clock_state = remaining_clock_state.remove(&raw).unwrap_or_default();
        if island.clock_state_signal_ids != expected_clock_state {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }

        let domain_clock_id = raw + 1;
        let expected_nested_loop_ids = plan
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
        if island.nested_loop_ids != expected_nested_loop_ids {
            return Err(VectorClockAdError::IslandCoverageMismatch);
        }
    }

    // Completeness: no wrapper references a domain outside the table, and
    // no clock-state signal's owned effect resolves to a domain no island
    // in the table claimed.
    if wrappers_by_domain
        .keys()
        .any(|domain| *domain >= domains.len() as u64)
        || !remaining_clock_state.is_empty()
    {
        return Err(VectorClockAdError::IslandCoverageMismatch);
    }
    Ok(())
}
/// Checker-owned property check for the transport policies: a
/// bijection with `plan.transports`, in transport order, where each mode is
/// independently derived from the decision table (fused-group membership,
/// then the outer clock id, then arena `PermVar`-ness) and compared to the
/// declared mode.
pub(super) fn verify_transport_policies(
    prepared: &VerifiedPreparedSignals,
    plan: &VectorPlan,
    declared: &[ClockTransportPolicy],
) -> Result<(), VectorClockAdError> {
    if declared.len() != plan.transports.len() {
        return Err(VectorClockAdError::TransportCoverageMismatch);
    }
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
    for (policy, transport) in declared.iter().zip(&plan.transports) {
        if policy.transport_id != transport.transport_id {
            return Err(VectorClockAdError::TransportCoverageMismatch);
        }
        let signal = signals.get(&transport.signal_id).ok_or(
            VectorClockAdError::SignalCoverageMismatch {
                signal_id: transport.signal_id,
            },
        )?;
        let expected_mode = if let Some(&group_id) = fused_transports.get(&transport.transport_id) {
            ClockTransportMode::FusedScalar { group_id }
        } else if signal.clock_id == 0 {
            ClockTransportMode::OuterChunk
        } else if ids
            .get(&u32::try_from(signal.signal_id).map_err(|_| {
                VectorClockAdError::SignalCoverageMismatch {
                    signal_id: signal.signal_id,
                }
            })?)
            .is_some_and(|&sig| matches!(match_sig(prepared.arena(), sig), SigMatch::PermVar(_)))
        {
            ClockTransportMode::HeldOutput {
                domain_id: signal.clock_id - 1,
            }
        } else {
            ClockTransportMode::IslandScalar {
                domain_id: signal.clock_id - 1,
            }
        };
        if policy.mode != expected_mode {
            return Err(VectorClockAdError::TransportCoverageMismatch);
        }
    }
    Ok(())
}
/// Checker-owned property check for the reverse-AD fallbacks: a
/// bijection, in decoration-record order, with the arena's
/// `ReverseTimeRec`/`BlockReverseAD` records, each verified against the
/// signal's owned placement, its arena-derived kind, and the fixed
/// `[Forward, Reverse]` epochs and scalar-window diagnostic.
pub(super) fn verify_reverse_fallbacks(
    prepared: &VerifiedPreparedSignals,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
    declared: &[ReverseAdFallback],
) -> Result<(), VectorClockAdError> {
    let ids = crate::signal_fir::vector::common::ids::prepared_signal_ids(prepared);
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut declared_iter = declared.iter();
    let mut matched = 0usize;
    for record in &decorations.certificate().records {
        let sig = ids[&record.signal_id];
        let expected_kind = match match_sig(prepared.arena(), sig) {
            SigMatch::ReverseTimeRec(_) => ReverseAdKind::ReverseTimeRec,
            SigMatch::BlockReverseAD { .. } => ReverseAdKind::BlockReverseAd,
            _ => continue,
        };
        matched += 1;
        let signal_id = u64::from(record.signal_id);
        let fallback = declared_iter
            .next()
            .ok_or(VectorClockAdError::ReverseAdCoverageMismatch)?;
        if fallback.signal_id != signal_id || fallback.kind != expected_kind {
            return Err(VectorClockAdError::ReverseAdCoverageMismatch);
        }
        let signal = signals
            .get(&signal_id)
            .ok_or(VectorClockAdError::SignalCoverageMismatch { signal_id })?;
        let Placement::Owned(owner_loop_id) = signal.placement else {
            return Err(VectorClockAdError::ReverseCarrierNotOwned { signal_id });
        };
        if fallback.owner_loop_id != owner_loop_id
            || fallback.epochs != [AdEpoch::Forward, AdEpoch::Reverse]
            || fallback.diagnostic != ReverseAdDiagnostic::ScalarReverseWindowRequired
        {
            return Err(VectorClockAdError::ReverseAdCoverageMismatch);
        }
    }
    if matched != declared.len() || declared_iter.next().is_some() {
        return Err(VectorClockAdError::ReverseAdCoverageMismatch);
    }
    Ok(())
}
/// Rejects fire-time reads of audio-rate stateful values.
///
/// Clock inference is rate-polymorphic: a recursion or delay whose inputs are
/// all constants infers the bottom environment, so the plan hoists it into an
/// outer-rate loop even when it is written under a clock wrapper (e.g.
/// `upsampling(1 : (+ ~ *(0.5)))`). Its state then advances once per outer
/// sample instead of once per fire — a miscompile the reference semantics
/// reject. Until such groups are adopted into their wrapper's domain, any
/// value dependency from an in-domain consumer to an audio-rate stateful
/// producer fails closed. The one legal crossing is the `ZeroPad` value
/// operand: propagation inserts it exactly where an outer-rate producer is
/// zero-stuffed into an upsampled domain.
pub(super) fn reject_unadopted_stateful_reads(
    prepared: &VerifiedPreparedSignals,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
    islands: &[ClockIsland],
) -> Result<(), VectorClockAdError> {
    let ids = crate::signal_fir::vector::common::ids::prepared_signal_ids(prepared);
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let domain_by_clock = islands
        .iter()
        .map(|island| (island.domain_id + 1, island.domain_id))
        .collect::<BTreeMap<_, _>>();
    for dependency in &decorations.certificate().dependencies {
        if !matches!(
            dependency.kind,
            DepKind::Immediate | DepKind::Delayed { .. }
        ) {
            continue;
        }
        let Some(consumer) = signals.get(&u64::from(dependency.from)) else {
            continue;
        };
        let Some(producer) = signals.get(&u64::from(dependency.to)) else {
            continue;
        };
        if producer.clock_id != 0 {
            continue;
        }
        let Some(&domain_id) = domain_by_clock.get(&consumer.clock_id) else {
            continue;
        };
        let Some(&producer_sig) = ids.get(&dependency.to) else {
            continue;
        };
        if !is_stateful_producer(prepared, producer_sig) {
            continue;
        }
        let consumer_sig = ids.get(&dependency.from).copied();
        if consumer_sig
            .is_some_and(|sig| matches!(match_sig(prepared.arena(), sig), SigMatch::ZeroPad(_, _)))
        {
            continue;
        }
        return Err(VectorClockAdError::UnadoptedStatefulRead {
            domain_id,
            consumer: u64::from(dependency.from),
            producer: u64::from(dependency.to),
        });
    }
    Ok(())
}
/// Whether the signal carries per-sample state whose advance rate is
/// observable: symbolic recursion (group, reference, or projection) and
/// explicit delays.
pub(super) fn is_stateful_producer(prepared: &VerifiedPreparedSignals, sig: SigId) -> bool {
    let arena = prepared.arena();
    if match_sym_rec(arena, sig).is_some() || match_sym_ref(arena, sig).is_some() {
        return true;
    }
    match match_sig(arena, sig) {
        SigMatch::Delay1(_) | SigMatch::Delay(_, _) | SigMatch::Prefix(_, _) => true,
        SigMatch::Proj(_, group) => {
            match_sym_rec(arena, group).is_some() || match_sym_ref(arena, group).is_some()
        }
        _ => false,
    }
}
pub(super) fn verify_source_alignment(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
) -> Result<(), VectorClockAdError> {
    let records = &decorations.certificate().records;
    if records.len() != plan.signals.len() {
        return Err(VectorClockAdError::SignalCoverageMismatch {
            signal_id: u64::MAX,
        });
    }
    let clocks = annotate(prepared.arena(), domains, prepared.outputs())?;
    let ids = crate::signal_fir::vector::common::ids::prepared_signal_ids(prepared);
    for (record, signal) in records.iter().zip(&plan.signals) {
        let signal_id = u64::from(record.signal_id);
        if signal.signal_id != signal_id {
            return Err(VectorClockAdError::SignalCoverageMismatch { signal_id });
        }
        let sig = ids
            .get(&record.signal_id)
            .copied()
            .ok_or(VectorClockAdError::SignalCoverageMismatch { signal_id })?;
        let inferred = clocks
            .env(sig)
            .ok_or(VectorClockAdError::ClockFactMismatch { signal_id })?
            .map(|domain| domain.as_u32());
        if record.clock_domain != inferred
            || signal.clock_id != inferred.map_or(0, |domain| u64::from(domain) + 1)
        {
            return Err(VectorClockAdError::ClockFactMismatch { signal_id });
        }
    }
    verify_domain_tree(domains)
}
pub(super) fn verify_domain_tree(domains: &ClockDomainTable) -> Result<(), VectorClockAdError> {
    for (domain_id, domain) in domains.iter() {
        let raw = u64::from(domain_id.as_u32());
        if let Some(parent) = domain.parent
            && domains.get(parent).is_none()
        {
            return Err(VectorClockAdError::DomainParentUnknown {
                domain_id: raw,
                parent_id: u64::from(parent.as_u32()),
            });
        }
        let mut seen = BTreeSet::new();
        let mut cursor = Some(domain_id);
        while let Some(current) = cursor {
            if !seen.insert(current.as_u32()) {
                return Err(VectorClockAdError::DomainCycle { domain_id: raw });
            }
            cursor = domains.get(current).and_then(|entry| entry.parent);
        }
    }
    Ok(())
}
// The five leaf helpers below are checker-owned duplicates of `build.rs`'s
// `wrapper_domain_and_clock`, `clock_state_domain`, `is_owned_clock_effect`,
// `guard_for`, and `scalar_clock_facts`. They must NOT be merged or shared
// with the producer's copies: the duplication is the assurance boundary
// (plan provenance: §2) that lets `verify_clock_islands` catch a defect that lives only
// inside `build.rs`'s versions.
fn checker_wrapper_domain_and_clock(
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
fn checker_clock_state_domain(prepared: &VerifiedPreparedSignals, sig: SigId) -> Option<u64> {
    match match_sig(prepared.arena(), sig) {
        SigMatch::Clocked(token, _) => {
            let SigMatch::ClockEnvToken(domain) = match_sig(prepared.arena(), token) else {
                return None;
            };
            Some(u64::from(domain))
        }
        SigMatch::OnDemand(_) | SigMatch::Upsampling(_) | SigMatch::Downsampling(_) => {
            checker_wrapper_domain_and_clock(prepared, sig).map(|(domain, _, _)| domain)
        }
        _ => None,
    }
}
fn checker_is_owned_clock_effect(effect: &EffectAtom, signal_id: u32) -> bool {
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
fn checker_guard_for(
    domain_id: u64,
    kind: ClockDomainKind,
    ty: &CanonicalSigType,
) -> Result<ClockGuard, VectorClockAdError> {
    let (nature, boolean_interval) = checker_scalar_clock_facts(ty)
        .ok_or(VectorClockAdError::UnsupportedClockType { domain_id, kind })?;
    match kind {
        ClockDomainKind::OnDemand if boolean_interval => Ok(ClockGuard::BooleanOnDemand),
        ClockDomainKind::OnDemand if nature == Nature::Int => Ok(ClockGuard::CountedOnDemand),
        ClockDomainKind::Upsampling if nature == Nature::Int => Ok(ClockGuard::CountedUpsampling),
        ClockDomainKind::Downsampling if nature == Nature::Int => Ok(ClockGuard::DownsampleModulo),
        _ => Err(VectorClockAdError::UnsupportedClockType { domain_id, kind }),
    }
}
fn checker_scalar_clock_facts(ty: &CanonicalSigType) -> Option<(Nature, bool)> {
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
