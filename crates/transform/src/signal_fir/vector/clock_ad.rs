//! Verified P6.2 clock-island and automatic-differentiation execution policy.
//!
//! # C++ provenance and adaptation
//! Clock guards follow `compile_scal.cpp::generateOD` and the clocked
//! `CodeIFblock`/`SimpleForLoop` lowering: boolean on-demand executes zero or
//! one inner transition, integer on-demand and upsampling execute a counted
//! number of transitions, and downsampling executes through a persistent
//! modulo counter. Domain-owned state advances in fire time and `PermVar`
//! outputs hold their last value when a domain does not fire.
//!
//! Rust composes the already verified signal-level [`VectorPlan`] with the
//! propagation-owned [`propagate::ClockDomainTable`] and a freshly recomputed
//! [`crate::clk_env::ClkEnvMap`]. Every wrapper becomes one serial island with
//! explicit parentage, member signals, nested loops, and transport policy.
//! Only top-rate transports retain P5's outer-chunk indexing; domain-rate
//! transports are marked island-scalar and must be rematerialized below the
//! guard by the later FIR assembly step.
//!
//! Forward AD has no Signal-IR carrier after propagation: its primal and
//! tangent lanes are ordinary prepared signals and use the normal vector
//! plan. `ReverseTimeRec` and `BlockReverseAD`, by contrast, are certified as
//! scalar fallbacks with immutable `Forward < Reverse` epochs. This module
//! does not claim vector reverse-window semantics and cannot activate a
//! backend path by itself.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use propagate::{ClockDomainKind, ClockDomainTable};
use signals::{SigId, SigMatch, match_sig};
use sigtype::Nature;

use crate::clk_env::{ClkEnvError, annotate};
use crate::signal_prepare::VerifiedPreparedSignals;

use super::decoration_verify::{CanonicalSigType, VerifiedDecorationCertificate};
use super::vector_analysis::{EffectAtom, StateCell, StateResource};
use super::vector_plan::VerifiedVectorPlan;
use super::vector_verify::{LoopKind, Placement, VectorPlan, VectorPlanError, verify_vector_plan};

/// Current canonical P6.2 clock/AD-plan schema.
pub const VECTOR_CLOCK_AD_PLAN_VERSION: u32 = 1;

/// Concrete guard shape used to implement `fires(c, i)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClockGuard {
    BooleanOnDemand,
    CountedOnDemand,
    CountedUpsampling,
    DownsampleModulo,
}

/// Whether a pre-planned P5 transport may use the outer chunk index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClockTransportMode {
    /// Audio-rate value: the P5 `i0 - vindex` chunk route remains valid.
    OuterChunk,
    /// Per-sample value crossing logical loops in one certified fused group.
    /// The route is a scalar stack temporary inside the group's sole sample
    /// envelope, either a top-rate loop or one exact guarded clock island,
    /// never a chunk array filled by a preceding loop.
    FusedScalar { group_id: u64 },
    /// Domain-rate value: route below the serial guard, one fire at a time.
    IslandScalar { domain_id: u64 },
    /// Persistent `PermVar` value exported from a domain to an ancestor.
    HeldOutput { domain_id: u64 },
}

/// Complete policy for one existing P5 transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockTransportPolicy {
    pub transport_id: u64,
    pub mode: ClockTransportMode,
}

/// One serial OD/US/DS region and the P4 loops nested below it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockIsland {
    pub domain_id: u64,
    pub parent_domain: Option<u64>,
    pub kind: ClockDomainKind,
    pub clock_signal_id: u64,
    pub wrapper_signal_id: u64,
    pub boundary_loop_id: u64,
    pub guard: ClockGuard,
    /// Exact signals whose inferred clock environment is this domain.
    pub signal_ids: Vec<u64>,
    /// Signals that own clock guard/counter state for this domain.
    pub clock_state_signal_ids: Vec<u64>,
    /// Existing P4 loops with at least one root in this domain.
    pub nested_loop_ids: Vec<u64>,
}

/// FAD policy at the prepared-signal boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardAdPolicy {
    /// Propagation expanded FAD into ordinary primal/tangent signals.
    ExpandedSignalGraph,
}

/// Reverse-mode carrier requiring a block-local reverse window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReverseAdKind {
    ReverseTimeRec,
    BlockReverseAd,
}

/// Semantically fixed reverse-mode epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdEpoch {
    Forward,
    Reverse,
}

/// Stable reason why reverse-mode execution is not admitted to vector mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReverseAdDiagnostic {
    ScalarReverseWindowRequired,
}

impl ReverseAdDiagnostic {
    /// Stable internal diagnostic code for a future CLI/backend activation gate.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ScalarReverseWindowRequired => "FRS-VEC-RAD-SCALAR",
        }
    }

    /// User-facing reason that vector reverse-mode execution is refused.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::ScalarReverseWindowRequired => {
                "reverse AD requires a scalar forward/tape/reverse window; vector chunk semantics are not enabled"
            }
        }
    }
}

/// One explicit scalar fallback for a reverse-time Signal-IR carrier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseAdFallback {
    pub signal_id: u64,
    pub owner_loop_id: u64,
    pub kind: ReverseAdKind,
    pub epochs: Vec<AdEpoch>,
    pub diagnostic: ReverseAdDiagnostic,
}

/// Canonical finite P6.2 artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorClockAdPlan {
    pub schema_version: u32,
    pub vec_size: u64,
    pub clock_islands: Vec<ClockIsland>,
    pub transports: Vec<ClockTransportPolicy>,
    pub forward_ad: ForwardAdPolicy,
    pub reverse_ad_fallbacks: Vec<ReverseAdFallback>,
}

/// Opaque evidence that P6.2 construction passed its checker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVectorClockAdPlan {
    plan: VectorClockAdPlan,
    vector_plan: VectorPlan,
}

impl VerifiedVectorClockAdPlan {
    #[must_use]
    pub fn plan(&self) -> &VectorClockAdPlan {
        &self.plan
    }

    #[must_use]
    pub fn vector_plan(&self) -> &VectorPlan {
        &self.vector_plan
    }

    #[must_use]
    pub fn into_plan(self) -> VectorClockAdPlan {
        self.plan
    }

    /// State resources whose execution is completely owned by P6.2 guards or
    /// persistent held-output transports.
    #[must_use]
    pub fn managed_state_resources(&self) -> BTreeSet<StateResource> {
        self.vector_plan
            .signals
            .iter()
            .flat_map(|signal| signal.effects.iter())
            .filter_map(|effect| match effect {
                EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource)
                    if matches!(
                        resource,
                        StateResource::Signal {
                            cell: StateCell::Clock | StateCell::Hold,
                            ..
                        }
                    ) =>
                {
                    Some(resource.clone())
                }
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) fn verified_vector_clock_ad_plan_for_test(
    plan: VectorClockAdPlan,
    vector_plan: &VerifiedVectorPlan,
) -> VerifiedVectorClockAdPlan {
    assert_eq!(plan.schema_version, VECTOR_CLOCK_AD_PLAN_VERSION);
    assert_eq!(plan.vec_size, vector_plan.plan().vec_size);
    assert_eq!(plan.transports.len(), vector_plan.plan().transports.len());
    for (policy, transport) in plan.transports.iter().zip(&vector_plan.plan().transports) {
        assert_eq!(policy.transport_id, transport.transport_id);
    }
    VerifiedVectorClockAdPlan {
        plan,
        vector_plan: vector_plan.plan().clone(),
    }
}

/// Typed producer/checker failure at the P6.2 boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorClockAdError {
    Plan(VectorPlanError),
    ClockInference(ClkEnvError),
    UnsupportedSchema {
        found: u32,
    },
    VecSizeMismatch {
        declared: u64,
        actual: u64,
    },
    SignalCoverageMismatch {
        signal_id: u64,
    },
    ClockFactMismatch {
        signal_id: u64,
    },
    DomainParentUnknown {
        domain_id: u64,
        parent_id: u64,
    },
    DomainCycle {
        domain_id: u64,
    },
    ClockSignalUnknown {
        domain_id: u64,
        signal_id: u64,
    },
    UnsupportedClockType {
        domain_id: u64,
        kind: ClockDomainKind,
    },
    WrapperCoverageMismatch {
        domain_id: u64,
    },
    WrapperKindMismatch {
        domain_id: u64,
        table: ClockDomainKind,
        signal: ClockDomainKind,
    },
    ClockStateDomainUnknown {
        signal_id: u64,
    },
    BoundaryNotOwned {
        domain_id: u64,
        signal_id: u64,
    },
    BoundaryNotSerial {
        domain_id: u64,
        loop_id: u64,
    },
    IslandCoverageMismatch,
    TransportCoverageMismatch,
    ReverseCarrierNotOwned {
        signal_id: u64,
    },
    ReverseAdCoverageMismatch,
    ClockValueKindMismatch {
        guard: ClockGuard,
    },
    InvalidDownsampleFactor {
        factor: i64,
    },
}

impl fmt::Display for VectorClockAdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => write!(f, "vector plan verification failed: {error}"),
            Self::ClockInference(error) => write!(f, "clock inference failed: {error}"),
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported vector clock/AD schema {found}")
            }
            Self::VecSizeMismatch { declared, actual } => write!(
                f,
                "clock/AD plan vector size {declared} differs from vector plan {actual}"
            ),
            Self::SignalCoverageMismatch { signal_id } => {
                write!(f, "clock/AD source facts do not cover signal {signal_id}")
            }
            Self::ClockFactMismatch { signal_id } => {
                write!(f, "clock facts disagree for signal {signal_id}")
            }
            Self::DomainParentUnknown {
                domain_id,
                parent_id,
            } => write!(
                f,
                "clock domain {domain_id} names unknown parent {parent_id}"
            ),
            Self::DomainCycle { domain_id } => {
                write!(
                    f,
                    "clock domain hierarchy cycles through domain {domain_id}"
                )
            }
            Self::ClockSignalUnknown {
                domain_id,
                signal_id,
            } => write!(
                f,
                "clock domain {domain_id} names unknown clock signal {signal_id}"
            ),
            Self::UnsupportedClockType { domain_id, kind } => write!(
                f,
                "clock domain {domain_id} ({kind:?}) has no supported boolean/integer guard"
            ),
            Self::WrapperCoverageMismatch { domain_id } => write!(
                f,
                "clock domain {domain_id} does not have exactly one matching wrapper"
            ),
            Self::WrapperKindMismatch {
                domain_id,
                table,
                signal,
            } => write!(
                f,
                "clock domain {domain_id} is {table:?} in the table but {signal:?} in Signal IR"
            ),
            Self::ClockStateDomainUnknown { signal_id } => write!(
                f,
                "clock-state signal {signal_id} has no decodable domain token"
            ),
            Self::BoundaryNotOwned {
                domain_id,
                signal_id,
            } => write!(
                f,
                "clock boundary {domain_id} signal {signal_id} has no owned loop"
            ),
            Self::BoundaryNotSerial { domain_id, loop_id } => write!(
                f,
                "clock boundary {domain_id} is owned by non-serial loop {loop_id}"
            ),
            Self::IslandCoverageMismatch => write!(f, "clock-island facts are not exact"),
            Self::TransportCoverageMismatch => {
                write!(f, "clock transport policies are not exact")
            }
            Self::ReverseCarrierNotOwned { signal_id } => write!(
                f,
                "reverse-mode carrier {signal_id} has no owned scalar loop"
            ),
            Self::ReverseAdCoverageMismatch => {
                write!(f, "reverse-mode fallback facts are not exact")
            }
            Self::ClockValueKindMismatch { guard } => {
                write!(f, "runtime clock value does not match {guard:?}")
            }
            Self::InvalidDownsampleFactor { factor } => {
                write!(f, "downsampling factor must be positive, got {factor}")
            }
        }
    }
}

impl std::error::Error for VectorClockAdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            Self::ClockInference(error) => Some(error),
            _ => None,
        }
    }
}

impl From<VectorPlanError> for VectorClockAdError {
    fn from(value: VectorPlanError) -> Self {
        Self::Plan(value)
    }
}

impl From<ClkEnvError> for VectorClockAdError {
    fn from(value: ClkEnvError) -> Self {
        Self::ClockInference(value)
    }
}

/// Builds and independently checks the P6.2 island/AD policy.
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

fn verify_vector_clock_ad_plan_after_vector_plan(
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
    let expected_islands = derive_clock_islands(prepared, domains, decorations, vector_plan)?;
    if clock_ad_plan.clock_islands != expected_islands {
        return Err(VectorClockAdError::IslandCoverageMismatch);
    }
    let expected_transports = derive_transport_policies(prepared, vector_plan)?;
    if clock_ad_plan.transports != expected_transports {
        return Err(VectorClockAdError::TransportCoverageMismatch);
    }
    if clock_ad_plan.forward_ad != ForwardAdPolicy::ExpandedSignalGraph {
        return Err(VectorClockAdError::ReverseAdCoverageMismatch);
    }
    let expected_reverse = derive_reverse_fallbacks(prepared, decorations, vector_plan)?;
    if clock_ad_plan.reverse_ad_fallbacks != expected_reverse {
        return Err(VectorClockAdError::ReverseAdCoverageMismatch);
    }
    Ok(())
}

fn signal_ids(prepared: &VerifiedPreparedSignals) -> BTreeMap<u32, SigId> {
    prepared
        .sig_types_map()
        .keys()
        .map(|&sig| (sig.as_u32(), sig))
        .collect()
}

fn verify_source_alignment(
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
    let ids = signal_ids(prepared);
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

fn verify_domain_tree(domains: &ClockDomainTable) -> Result<(), VectorClockAdError> {
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

fn derive_clock_islands(
    prepared: &VerifiedPreparedSignals,
    domains: &ClockDomainTable,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
) -> Result<Vec<ClockIsland>, VectorClockAdError> {
    let ids = signal_ids(prepared);
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

fn derive_transport_policies(
    prepared: &VerifiedPreparedSignals,
    plan: &VectorPlan,
) -> Result<Vec<ClockTransportPolicy>, VectorClockAdError> {
    let ids = signal_ids(prepared);
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

fn derive_reverse_fallbacks(
    prepared: &VerifiedPreparedSignals,
    decorations: &VerifiedDecorationCertificate,
    plan: &VectorPlan,
) -> Result<Vec<ReverseAdFallback>, VectorClockAdError> {
    let ids = signal_ids(prepared);
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

/// Runtime clock value used by the executable `ClockStep` reference model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockValue {
    Boolean(bool),
    Integer(i64),
}

/// Minimal concrete state needed to test fire-time and held-output semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockRuntime<S, O> {
    pub state: S,
    pub held_output: O,
    pub downsample_counter: u64,
}

/// Applies `Step_c` exactly `fires(c,i)` times for one outer sample.
///
/// The held output is changed only by an inner transition. Consequently zero
/// fires preserve both domain state and the previous output.
pub fn simulate_clock_step<S, O, F>(
    guard: ClockGuard,
    clock: ClockValue,
    runtime: &mut ClockRuntime<S, O>,
    mut transition: F,
) -> Result<u64, VectorClockAdError>
where
    F: FnMut(&S, u64) -> (S, O),
{
    let fires = match (guard, clock) {
        (ClockGuard::BooleanOnDemand, ClockValue::Boolean(active)) => u64::from(active),
        (
            ClockGuard::CountedOnDemand | ClockGuard::CountedUpsampling,
            ClockValue::Integer(count),
        ) => u64::try_from(count).unwrap_or(0),
        (ClockGuard::DownsampleModulo, ClockValue::Integer(factor)) => {
            if factor <= 0 {
                return Err(VectorClockAdError::InvalidDownsampleFactor { factor });
            }
            let fires = u64::from(runtime.downsample_counter == 0);
            let factor = u64::try_from(factor).expect("positive i64 fits u64");
            runtime.downsample_counter = (runtime.downsample_counter + 1) % factor;
            fires
        }
        (guard, _) => return Err(VectorClockAdError::ClockValueKindMismatch { guard }),
    };
    for fire in 0..fires {
        let (next_state, output) = transition(&runtime.state, fire);
        runtime.state = next_state;
        runtime.held_output = output;
    }
    Ok(fires)
}

/// Executes one scalar reverse window with immutable `Forward < Reverse` order.
pub fn execute_reverse_ad_window<S, P, T, A, Forward, Reverse>(
    initial_state: S,
    forward: Forward,
    reverse: Reverse,
) -> (S, P, A)
where
    Forward: FnOnce(S) -> (S, P, T),
    Reverse: FnOnce(S, T) -> (S, A),
{
    let (forward_state, primal, tape) = forward(initial_state);
    let (reverse_state, adjoints) = reverse(forward_state, tape);
    (reverse_state, primal, adjoints)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use propagate::{ClockDomain, ClockDomainTable};
    use signals::{BlockRevPolicy, SigBuilder};
    use tlib::TreeArena;

    use super::*;
    use crate::clk_env::annotate;
    use crate::signal_fir::decoration_verify::certify_decorations;
    use crate::signal_fir::vector_plan::build_vector_plan;
    use crate::signal_prepare::prepare_signals_for_fir_verified;

    fn clock_fixture(
        kind: ClockDomainKind,
    ) -> (
        VerifiedPreparedSignals,
        ClockDomainTable,
        VerifiedDecorationCertificate,
        VerifiedVectorPlan,
    ) {
        let mut arena = TreeArena::new();
        let wrapper_box = arena.nil();
        let (clock, input) = {
            let mut builder = SigBuilder::new(&mut arena);
            (builder.int(2), builder.input(0))
        };
        let mut domains = ClockDomainTable::new();
        let domain = domains.alloc(ClockDomain {
            parent: None,
            kind,
            clock,
            wrapper_box,
            inputs: vec![input],
        });
        let root = {
            let mut builder = SigBuilder::new(&mut arena);
            let token = builder.clock_env_token(domain.as_u32());
            let guarded_clock = builder.clocked(token, clock);
            let guarded_value = builder.clocked(token, input);
            let hold = builder.perm_var(guarded_value);
            let wrapper = match kind {
                ClockDomainKind::OnDemand => builder.on_demand(&[guarded_clock, hold]),
                ClockDomainKind::Upsampling => builder.upsampling(&[guarded_clock, hold]),
                ClockDomainKind::Downsampling => builder.downsampling(&[guarded_clock, hold]),
            };
            builder.seq(wrapper, hold)
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
        let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        let vector_plan = build_vector_plan(&decorations, 8).unwrap();
        (prepared, domains, decorations, vector_plan)
    }

    #[test]
    fn production_clock_plan_builds_serial_island_and_refines_transports() {
        for (kind, guard) in [
            (ClockDomainKind::OnDemand, ClockGuard::CountedOnDemand),
            (ClockDomainKind::Upsampling, ClockGuard::CountedUpsampling),
            (ClockDomainKind::Downsampling, ClockGuard::DownsampleModulo),
        ] {
            let (prepared, domains, decorations, vector_plan) = clock_fixture(kind);
            let verified =
                build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan)
                    .unwrap();
            assert_eq!(verified.plan().clock_islands.len(), 1);
            let island = &verified.plan().clock_islands[0];
            assert_eq!(island.domain_id, 0);
            assert_eq!(island.kind, kind);
            assert_eq!(island.guard, guard);
            assert!(matches!(
                vector_plan.plan().loops[island.boundary_loop_id as usize].kind,
                LoopKind::Island(_)
            ));
            assert!(!island.clock_state_signal_ids.is_empty());
            assert!(verified.plan().transports.iter().all(|policy| {
                matches!(
                    policy.mode,
                    ClockTransportMode::OuterChunk
                        | ClockTransportMode::IslandScalar { domain_id: 0 }
                        | ClockTransportMode::HeldOutput { domain_id: 0 }
                )
            }));
            assert!(verified.plan().transports.iter().any(|policy| matches!(
                policy.mode,
                ClockTransportMode::HeldOutput { domain_id: 0 }
            )));
        }
    }

    #[test]
    fn nested_clock_islands_preserve_domain_parentage() {
        let mut arena = TreeArena::new();
        let wrapper_box = arena.nil();
        let (outer_clock, inner_clock, input) = {
            let mut builder = SigBuilder::new(&mut arena);
            (builder.int(1), builder.int(2), builder.input(0))
        };
        let mut domains = ClockDomainTable::new();
        let outer = domains.alloc(ClockDomain {
            parent: None,
            kind: ClockDomainKind::OnDemand,
            clock: outer_clock,
            wrapper_box,
            inputs: vec![input],
        });
        let inner = domains.alloc(ClockDomain {
            parent: Some(outer),
            kind: ClockDomainKind::Upsampling,
            clock: inner_clock,
            wrapper_box,
            inputs: vec![input],
        });
        let root = {
            let mut builder = SigBuilder::new(&mut arena);
            let outer_token = builder.clock_env_token(outer.as_u32());
            let inner_token = builder.clock_env_token(inner.as_u32());
            let inner_clock_at_outer = builder.clocked(outer_token, inner_clock);
            let inner_guard = builder.clocked(inner_token, inner_clock_at_outer);
            let inner_value = builder.clocked(inner_token, input);
            let inner_hold = builder.perm_var(inner_value);
            let inner_wrapper = builder.upsampling(&[inner_guard, inner_hold]);
            let inner_result = builder.seq(inner_wrapper, inner_hold);

            let outer_guard = builder.clocked(outer_token, outer_clock);
            let outer_value = builder.clocked(outer_token, inner_result);
            let outer_hold = builder.perm_var(outer_value);
            let outer_wrapper = builder.on_demand(&[outer_guard, outer_hold]);
            builder.seq(outer_wrapper, outer_hold)
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
        let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        let vector_plan = build_vector_plan(&decorations, 8).unwrap();
        let verified =
            build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();

        assert_eq!(verified.plan().clock_islands.len(), 2);
        assert_eq!(verified.plan().clock_islands[0].parent_domain, None);
        assert_eq!(verified.plan().clock_islands[1].parent_domain, Some(0));
        assert_ne!(
            verified.plan().clock_islands[0].wrapper_signal_id,
            verified.plan().clock_islands[1].wrapper_signal_id
        );
    }

    #[test]
    fn checker_rejects_clock_island_and_transport_mutations() {
        let (prepared, domains, decorations, vector_plan) =
            clock_fixture(ClockDomainKind::Upsampling);
        let verified =
            build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();

        let mut island = verified.plan().clone();
        island.clock_islands[0].guard = ClockGuard::DownsampleModulo;
        assert_eq!(
            verify_vector_clock_ad_plan(
                &prepared,
                &domains,
                &decorations,
                vector_plan.plan(),
                &island,
            ),
            Err(VectorClockAdError::IslandCoverageMismatch)
        );

        if let Some(index) = verified
            .plan()
            .transports
            .iter()
            .position(|policy| !matches!(policy.mode, ClockTransportMode::OuterChunk))
        {
            let mut transport = verified.plan().clone();
            transport.transports[index].mode = ClockTransportMode::OuterChunk;
            assert_eq!(
                verify_vector_clock_ad_plan(
                    &prepared,
                    &domains,
                    &decorations,
                    vector_plan.plan(),
                    &transport,
                ),
                Err(VectorClockAdError::TransportCoverageMismatch)
            );
        }

        let mut mismatched_domains = ClockDomainTable::new();
        let placeholder = prepared.outputs()[0];
        mismatched_domains.alloc(ClockDomain {
            parent: None,
            kind: ClockDomainKind::Downsampling,
            clock: placeholder,
            wrapper_box: placeholder,
            inputs: Vec::new(),
        });
        assert!(matches!(
            verify_vector_clock_ad_plan(
                &prepared,
                &mismatched_domains,
                &decorations,
                vector_plan.plan(),
                verified.plan(),
            ),
            Err(VectorClockAdError::WrapperKindMismatch { .. })
        ));
    }

    #[test]
    fn block_reverse_ad_is_forced_to_fixed_scalar_epochs() {
        let mut arena = TreeArena::new();
        let root = {
            let mut builder = SigBuilder::new(&mut arena);
            let body = builder.input(0);
            let seed = builder.input(1);
            let cotangent = builder.real(1.0);
            let carrier =
                builder.block_reverse_ad(&[body], &[seed], &[cotangent], BlockRevPolicy::TapeFull);
            builder.proj(1, carrier)
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
        let domains = ClockDomainTable::new();
        let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        let vector_plan = build_vector_plan(&decorations, 8).unwrap();
        let verified =
            build_vector_clock_ad_plan(&prepared, &domains, &decorations, &vector_plan).unwrap();
        assert_eq!(verified.plan().reverse_ad_fallbacks.len(), 1);
        let fallback = &verified.plan().reverse_ad_fallbacks[0];
        assert_eq!(fallback.kind, ReverseAdKind::BlockReverseAd);
        assert_eq!(fallback.epochs, vec![AdEpoch::Forward, AdEpoch::Reverse]);
        assert_eq!(
            fallback.diagnostic,
            ReverseAdDiagnostic::ScalarReverseWindowRequired
        );
        assert_eq!(fallback.diagnostic.code(), "FRS-VEC-RAD-SCALAR");
        assert!(
            fallback
                .diagnostic
                .message()
                .contains("forward/tape/reverse")
        );
    }

    #[test]
    fn clock_step_preserves_state_and_hold_when_domain_does_not_fire() {
        let mut runtime = ClockRuntime {
            state: 7_i32,
            held_output: 11_i32,
            downsample_counter: 0,
        };
        let fires = simulate_clock_step(
            ClockGuard::BooleanOnDemand,
            ClockValue::Boolean(false),
            &mut runtime,
            |state, _| (*state + 1, *state + 10),
        )
        .unwrap();
        assert_eq!(fires, 0);
        assert_eq!(runtime.state, 7);
        assert_eq!(runtime.held_output, 11);
    }

    #[test]
    fn counted_and_downsample_guards_match_scalar_reference_equations() {
        let mut counted = ClockRuntime {
            state: 0_i32,
            held_output: -1_i32,
            downsample_counter: 0,
        };
        assert_eq!(
            simulate_clock_step(
                ClockGuard::CountedUpsampling,
                ClockValue::Integer(3),
                &mut counted,
                |state, _| (*state + 1, *state + 1),
            )
            .unwrap(),
            3
        );
        assert_eq!((counted.state, counted.held_output), (3, 3));
        assert_eq!(
            simulate_clock_step(
                ClockGuard::CountedOnDemand,
                ClockValue::Integer(-2),
                &mut counted,
                |state, _| (*state + 1, *state + 1),
            )
            .unwrap(),
            0
        );
        assert_eq!((counted.state, counted.held_output), (3, 3));

        let mut downsample = ClockRuntime {
            state: 0_i32,
            held_output: -1_i32,
            downsample_counter: 0,
        };
        let mut observed = Vec::new();
        for _ in 0..7 {
            observed.push(
                simulate_clock_step(
                    ClockGuard::DownsampleModulo,
                    ClockValue::Integer(3),
                    &mut downsample,
                    |state, _| (*state + 1, *state + 1),
                )
                .unwrap(),
            );
        }
        assert_eq!(observed, vec![1, 0, 0, 1, 0, 0, 1]);
        assert_eq!((downsample.state, downsample.held_output), (3, 3));
    }

    #[test]
    fn reverse_window_cannot_interleave_fixed_epochs() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let forward_trace = Rc::clone(&trace);
        let reverse_trace = Rc::clone(&trace);
        let (state, primal, adjoint) = execute_reverse_ad_window(
            2_i32,
            move |state| {
                forward_trace.borrow_mut().push("forward");
                (state + 3, state * 2, vec![state, state + 1])
            },
            move |state, tape: Vec<i32>| {
                reverse_trace.borrow_mut().push("reverse");
                (state + 5, tape.into_iter().sum::<i32>())
            },
        );
        assert_eq!(&*trace.borrow(), &["forward", "reverse"]);
        assert_eq!((state, primal, adjoint), (10, 4, 5));
    }
}
