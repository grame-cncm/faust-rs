//! Clock-island / AD policy vocabulary and error taxonomy (schema v1).

use crate::clk_env::ClkEnvError;
use crate::signal_fir::vector::analysis::{EffectAtom, StateCell, StateResource};
#[cfg(test)]
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::verify::{VectorPlan, VectorPlanError};
use propagate::ClockDomainKind;
use std::collections::BTreeSet;
use std::fmt;

/// Current canonical clock/AD-plan schema.
pub const VECTOR_CLOCK_AD_PLAN_VERSION: u32 = 1;
/// Concrete guard shape used to implement `fires(c, i)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClockGuard {
    /// On-demand domain gated by a boolean clock: fires once when the clock is true.
    BooleanOnDemand,
    /// On-demand domain gated by an integer clock: fires `count` times per outer sample.
    CountedOnDemand,
    /// Upsampling domain gated by an integer clock: fires `count` times per outer sample.
    CountedUpsampling,
    /// Downsampling domain: fires when the running counter modulo the factor is zero.
    DownsampleModulo,
}
/// Whether a planned chunk transport may use the outer chunk index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClockTransportMode {
    /// Audio-rate value: the routed `i0 - vindex` chunk route remains valid.
    OuterChunk,
    /// Per-sample value crossing logical loops in one certified fused group.
    /// The route is a scalar stack temporary inside the group's sole sample
    /// envelope, either a top-rate loop or one exact guarded clock island,
    /// never a chunk array filled by a preceding loop.
    FusedScalar {
        /// Identifier of the certified fused group that owns the scalar route.
        group_id: u64,
    },
    /// Domain-rate value: route below the serial guard, one fire at a time.
    IslandScalar {
        /// Identifier of the clock domain whose guard encloses the route.
        domain_id: u64,
    },
    /// Persistent `PermVar` value exported from a domain to an ancestor.
    HeldOutput {
        /// Identifier of the clock domain that produces the held value.
        domain_id: u64,
    },
}
/// Complete policy for one existing planned transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockTransportPolicy {
    /// Identifier of the planned transport this policy applies to.
    pub transport_id: u64,
    /// Route mode chosen for the transport under clock/AD planning.
    pub mode: ClockTransportMode,
}
/// One serial OD/US/DS region and the planned loops nested below it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockIsland {
    /// Stable identifier of this clock domain.
    pub domain_id: u64,
    /// Enclosing clock domain, or `None` for a top-level island.
    pub parent_domain: Option<u64>,
    /// Domain kind (on-demand, upsampling, or downsampling).
    pub kind: ClockDomainKind,
    /// Signal computing the clock value that drives `fires(c, i)`.
    pub clock_signal_id: u64,
    /// Clock wrapper signal marking the domain boundary in Signal IR.
    pub wrapper_signal_id: u64,
    /// Serial planned loop that owns the domain boundary.
    pub boundary_loop_id: u64,
    /// Concrete guard shape implementing the domain's fire condition.
    pub guard: ClockGuard,
    /// Exact signals whose inferred clock environment is this domain.
    pub signal_ids: Vec<u64>,
    /// Signals that own clock guard/counter state for this domain.
    pub clock_state_signal_ids: Vec<u64>,
    /// Existing planned loops with at least one root in this domain.
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
    /// Reverse-time recursion carrier iterating samples backwards.
    ReverseTimeRec,
    /// Block-level reverse-mode AD carrier over a forward/tape/reverse window.
    BlockReverseAd,
}
/// Semantically fixed reverse-mode epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdEpoch {
    /// Forward pass computing primal values and recording the tape.
    Forward,
    /// Reverse pass consuming the tape to accumulate adjoints.
    Reverse,
}
/// Stable reason why reverse-mode execution is not admitted to vector mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReverseAdDiagnostic {
    /// The carrier needs a scalar forward/tape/reverse window; vector chunk semantics are refused.
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
    /// Signal-IR identifier of the reverse-mode carrier.
    pub signal_id: u64,
    /// Scalar loop that owns the carrier's execution.
    pub owner_loop_id: u64,
    /// Which reverse-mode carrier shape triggered the fallback.
    pub kind: ReverseAdKind,
    /// Ordered execution epochs the scalar window must run.
    pub epochs: Vec<AdEpoch>,
    /// Stable reason the carrier is excluded from vector mode.
    pub diagnostic: ReverseAdDiagnostic,
}
/// Canonical finite clock/AD-plan artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorClockAdPlan {
    /// Schema version; must equal [`VECTOR_CLOCK_AD_PLAN_VERSION`].
    pub schema_version: u32,
    /// Vector chunk size; must match the underlying vector plan.
    pub vec_size: u64,
    /// Exact set of serial clock islands, one per clock domain.
    pub clock_islands: Vec<ClockIsland>,
    /// One routing policy per existing planned transport, in transport order.
    pub transports: Vec<ClockTransportPolicy>,
    /// Forward-mode AD policy at the prepared-signal boundary.
    pub forward_ad: ForwardAdPolicy,
    /// Exact set of scalar fallbacks for reverse-mode carriers.
    pub reverse_ad_fallbacks: Vec<ReverseAdFallback>,
}
/// Opaque evidence that clock/AD-plan construction passed its checker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVectorClockAdPlan {
    pub(super) plan: VectorClockAdPlan,
    pub(super) vector_plan: VectorPlan,
}
impl VerifiedVectorClockAdPlan {
    /// Returns the checked clock/AD plan.
    #[must_use]
    pub fn plan(&self) -> &VectorClockAdPlan {
        &self.plan
    }

    /// Returns the underlying vector plan the clock/AD plan was checked against.
    #[must_use]
    pub fn vector_plan(&self) -> &VectorPlan {
        &self.vector_plan
    }

    /// Consumes the evidence and returns the checked clock/AD plan.
    #[must_use]
    pub fn into_plan(self) -> VectorClockAdPlan {
        self.plan
    }

    /// State resources whose execution is completely owned by clock/AD-plan guards or
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
/// Typed producer/checker failure at the clock/AD-plan boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorClockAdError {
    /// The underlying vector plan failed verification.
    Plan(VectorPlanError),
    /// Clock-environment inference over the Signal IR failed.
    ClockInference(ClkEnvError),
    /// The plan declares a schema version other than the canonical one.
    UnsupportedSchema {
        /// Schema version found in the plan.
        found: u32,
    },
    /// The plan's vector size differs from the vector plan's.
    VecSizeMismatch {
        /// Vector size declared by the clock/AD plan.
        declared: u64,
        /// Vector size of the underlying vector plan.
        actual: u64,
    },
    /// A vector-plan signal is not covered by the clock/AD source facts.
    SignalCoverageMismatch {
        /// Signal missing from the source facts.
        signal_id: u64,
    },
    /// Independently derived clock facts disagree for a signal.
    ClockFactMismatch {
        /// Signal with conflicting clock facts.
        signal_id: u64,
    },
    /// A clock domain names a parent that is not itself a known domain.
    DomainParentUnknown {
        /// Domain naming the unknown parent.
        domain_id: u64,
        /// Parent identifier that does not exist.
        parent_id: u64,
    },
    /// The clock-domain parent hierarchy contains a cycle.
    DomainCycle {
        /// Domain on the detected cycle.
        domain_id: u64,
    },
    /// A clock domain names a clock signal absent from the plan.
    ClockSignalUnknown {
        /// Domain naming the unknown clock signal.
        domain_id: u64,
        /// Clock signal identifier that does not exist.
        signal_id: u64,
    },
    /// A domain's clock signal admits no supported boolean/integer guard.
    UnsupportedClockType {
        /// Domain whose clock type is unsupported.
        domain_id: u64,
        /// Kind of the offending clock domain.
        kind: ClockDomainKind,
    },
    /// A clock domain does not have exactly one matching wrapper signal.
    WrapperCoverageMismatch {
        /// Domain with zero or multiple wrappers.
        domain_id: u64,
    },
    /// The domain table and Signal IR disagree on a wrapper's domain kind.
    WrapperKindMismatch {
        /// Domain whose kinds disagree.
        domain_id: u64,
        /// Kind recorded in the domain table.
        table: ClockDomainKind,
        /// Kind carried by the Signal-IR wrapper.
        signal: ClockDomainKind,
    },
    /// A clock-state signal carries no decodable domain token.
    ClockStateDomainUnknown {
        /// Clock-state signal without a domain token.
        signal_id: u64,
    },
    /// A domain boundary signal has no owned loop in the vector plan.
    BoundaryNotOwned {
        /// Domain whose boundary is unowned.
        domain_id: u64,
        /// Boundary signal without an owned loop.
        signal_id: u64,
    },
    /// A domain boundary is owned by a loop that is not serial.
    BoundaryNotSerial {
        /// Domain whose boundary loop is not serial.
        domain_id: u64,
        /// Non-serial loop owning the boundary.
        loop_id: u64,
    },
    /// Declared clock islands do not exactly match the derived island facts.
    IslandCoverageMismatch,
    /// Declared transport policies do not exactly match the planned transports.
    TransportCoverageMismatch,
    /// A reverse-mode carrier has no owned scalar loop.
    ReverseCarrierNotOwned {
        /// Reverse-mode carrier signal without an owned scalar loop.
        signal_id: u64,
    },
    /// Declared reverse-AD fallbacks do not exactly match the derived carrier facts.
    ReverseAdCoverageMismatch,
    /// A runtime clock value has the wrong shape for the domain's guard.
    ClockValueKindMismatch {
        /// Guard the clock value failed to match.
        guard: ClockGuard,
    },
    /// A downsampling guard received a non-positive factor.
    InvalidDownsampleFactor {
        /// Offending downsampling factor.
        factor: i64,
    },
    /// A domain signal reads audio-rate stateful data from fire time; such state
    /// is not adopted into the domain and would advance at the wrong rate.
    UnadoptedStatefulRead {
        /// Domain in which the read occurs.
        domain_id: u64,
        /// Domain signal performing the read.
        consumer: u64,
        /// Audio-rate stateful signal being read.
        producer: u64,
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
            Self::UnadoptedStatefulRead {
                domain_id,
                consumer,
                producer,
            } => {
                write!(
                    f,
                    "clock domain {domain_id}: signal {consumer} reads audio-rate stateful \
                     signal {producer} from fire time; rate-polymorphic state under a clock \
                     wrapper is not adopted into the domain yet and would advance at the \
                     wrong rate"
                )
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
