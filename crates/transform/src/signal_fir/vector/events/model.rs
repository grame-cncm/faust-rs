//! Event-order vocabulary: limits, event/dependency DTOs, certificates,
//! error taxonomy, and total helper conversions shared by producer and
//! checker (plan provenance: §4.6: vocabulary and total conversions are shareable).

use crate::signal_fir::vector::analysis::EffectAtom;
use crate::signal_fir::vector::route::{RoutedUseSource, VectorRegion};
use crate::signal_fir::vector::state::VectorStateAction;
use crate::signal_fir::vector::verify::VectorPlanError;
use std::collections::BTreeSet;
use std::fmt;
/// Suggested upper bound for focused production and differential checks.
pub const DEFAULT_EVENT_LIMIT: usize = 4096;
/// Upper bound for a production two-sample compact basis.
///
/// Unlike [`DEFAULT_EVENT_LIMIT`], this bound never permits complete chunk
/// expansion. It was selected after the general routed-plan sweep showed that
/// the largest measured qualifying basis (`reverb_designer.dsp`, f64)
/// contains 28,843 events. The release compile-budget gate guards the
/// resulting work.
pub const DEFAULT_COMPACT_EVENT_LIMIT: usize = 32_768;
/// Separate finite budgets for complete and compact event evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventLimits {
    pub(super) complete: usize,
    pub(super) compact: usize,
}
impl EventLimits {
    /// Creates explicit complete-expansion and compact-basis limits.
    #[must_use]
    pub const fn new(complete: usize, compact: usize) -> Self {
        Self { complete, compact }
    }

    /// Applies one bound to both forms, primarily for focused boundary tests.
    #[must_use]
    pub const fn uniform(limit: usize) -> Self {
        Self::new(limit, limit)
    }
}
/// Production event budgets approved for the general compact rollout.
pub const DEFAULT_EVENT_LIMITS: EventLimits =
    EventLimits::new(DEFAULT_EVENT_LIMIT, DEFAULT_COMPACT_EVENT_LIMIT);
/// One sample checks the repeated body and the second checks every adjacent
/// carried dependence. Every routed plan must pass the same independently
/// reconstructed repetition checks before compact evidence is accepted.
pub(super) const COMPACT_EVENT_SAMPLE_BASIS: usize = 2;
/// Stable region containing a bounded dynamic event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventRegion {
    /// Fixed control computation, executed once before vector epochs.
    Control,
    /// An epoch boundary rather than a loop body operation.
    Epoch(u64),
    /// One loop's chunk-entry state transition phase.
    LoopPre(u64),
    /// One scheduled vector-plan loop.
    Loop(u64),
    /// One loop's chunk-exit state transition phase.
    LoopPost(u64),
}
/// Canonical source identity for a routed signal use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventUseSource {
    /// A definition in fixed control scope.
    Control,
    /// A definition in the same loop scope.
    Loop(u64),
    /// A named planned chunk transport.
    Transport(u64),
}
/// One operation represented by the bounded event model.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorEventKind {
    /// A routed signal definition.
    Definition {
        /// Routed signal being defined.
        signal_id: u64,
    },
    /// One exact signal-level effect atom attached to a definition.
    Effect {
        /// Routed signal whose definition carries the effect.
        signal_id: u64,
        /// Position of the atom in the definition's effect list.
        effect_index: u64,
        /// The exact effect atom.
        effect: EffectAtom,
    },
    /// Producer-side write into a named chunk transport.
    TransportStore {
        /// Named chunk transport being written.
        transport_id: u64,
    },
    /// Consumer-side read from a named chunk transport.
    TransportLoad {
        /// Named chunk transport being read.
        transport_id: u64,
    },
    /// One routed signal use. `occurrence` distinguishes repeated equal uses.
    Use {
        /// Routed signal being used.
        signal_id: u64,
        /// Where the used definition comes from.
        source: EventUseSource,
        /// Index distinguishing repeated equal uses of the same source.
        occurrence: u64,
    },
    /// Fixed entry barrier for one epoch.
    EpochEnter {
        /// Epoch being entered.
        epoch_id: u64,
    },
    /// Fixed exit barrier for one epoch.
    EpochExit {
        /// Epoch being exited.
        epoch_id: u64,
    },
    /// One checked state-plan delay or recursion transition.
    StateTransition {
        /// The exact checked state transition.
        action: VectorStateAction,
    },
}
/// One canonical static or sample-indexed event.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VectorEvent {
    /// Dense canonical identity, independent of `-ss`.
    pub event_id: u64,
    /// Region that executes the event.
    pub region: EventRegion,
    /// Chunk-relative sample for loop events; absent for control/barriers.
    pub sample: Option<u64>,
    /// Exact operation identity.
    pub kind: VectorEventKind,
}
/// Directed dynamic dependence `before -> after`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventDependency {
    /// Event that must execute first.
    pub before: u64,
    /// Event that must execute second.
    pub after: u64,
}
/// Canonical bounded witness containing `<scalar`, `<vec`, and `D`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventOrderCertificate {
    /// Logical chunk length certified by this witness.
    pub(super) sample_count: u64,
    /// Number of concrete samples retained in the finite event table. This is
    /// equal to `sample_count` for the expanded model and two for a repeated
    /// induction basis that checks both one sample and the `n -> n + 1`
    /// carried-state boundary.
    pub(super) checked_sample_count: u64,
    pub(super) events: Vec<VectorEvent>,
    pub(super) scalar_order: Vec<u64>,
    pub(super) vector_order: Vec<u64>,
    pub(super) dependencies: Vec<EventDependency>,
}
impl EventOrderCertificate {
    /// Logical number of chunk samples covered by the model.
    #[must_use]
    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// Number of samples materialized in the finite event table.
    #[must_use]
    pub fn checked_sample_count(&self) -> u64 {
        self.checked_sample_count
    }

    /// Whether the certificate uses the compact two-sample repetition basis.
    #[must_use]
    pub fn is_compact(&self) -> bool {
        self.checked_sample_count < self.sample_count
    }

    /// Canonical event table.
    #[must_use]
    pub fn events(&self) -> &[VectorEvent] {
        &self.events
    }

    /// Sample-major scalar reference order.
    #[must_use]
    pub fn scalar_order(&self) -> &[u64] {
        &self.scalar_order
    }

    /// Epoch-major, scheduled-loop-major vector order.
    #[must_use]
    pub fn vector_order(&self) -> &[u64] {
        &self.vector_order
    }

    /// Complete finite dependence relation used by the checker.
    #[must_use]
    pub fn dependencies(&self) -> &[EventDependency] {
        &self.dependencies
    }
}
/// Opaque evidence that [`verify_event_order_certificate`](super::check::verify_event_order_certificate) accepted the certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedEventOrderCertificate {
    pub(super) certificate: EventOrderCertificate,
}
impl VerifiedEventOrderCertificate {
    /// Returns the checked finite certificate.
    #[must_use]
    pub fn certificate(&self) -> &EventOrderCertificate {
        &self.certificate
    }

    /// Consumes the wrapper and returns the checked finite certificate.
    #[must_use]
    pub fn into_certificate(self) -> EventOrderCertificate {
        self.certificate
    }
}
/// Typed producer/checker failure for the event-order gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorEventError {
    /// The vector plan is not independently valid.
    Plan(VectorPlanError),
    /// The routed artifact was produced from a different plan.
    RoutedPlanMismatch,
    /// The state artifact was produced from a different vector plan.
    StatePlanMismatch,
    /// The route layout does not exactly and topologically cover the plan.
    InvalidLayout {
        /// Which layout invariant was violated.
        detail: &'static str,
        /// The scheduled loop at which the violation was found.
        loop_id: u64,
    },
    /// The selected complete or compact finite table is larger than its bound.
    EventBoundExceeded {
        /// Number of events the table would need.
        needed: usize,
        /// The applicable event-count bound.
        limit: usize,
    },
    /// A route-independent lower bound already exceeds both applicable bounds.
    EventLowerBoundExceeded {
        /// The route-independent lower bound on the event count.
        minimum: usize,
        /// The larger of the two applicable bounds.
        limit: usize,
    },
    /// Event-count arithmetic exceeded the host representation.
    EventCountOverflow,
    /// The event table differs from independent reconstruction.
    EventTableMismatch,
    /// A compact certificate does not use the canonical two-sample basis or
    /// its per-sample templates are not translation invariant.
    CompactRepetitionMismatch,
    /// The scalar order differs from independent reconstruction.
    ScalarOrderMismatch,
    /// The vector order differs from the accepted routed layout.
    VectorOrderMismatch,
    /// The dependence relation differs from independent reconstruction.
    DependencyMismatch,
    /// An order is not a permutation of the event table.
    InvalidOrder {
        /// Which order (`"scalar"` or `"vector"`) is invalid.
        which: &'static str,
    },
    /// A reconstructed dependence contradicts scalar execution.
    ScalarDependenceViolation {
        /// Event required to execute first.
        before: u64,
        /// Event required to execute second.
        after: u64,
    },
    /// A scalar-ordered dependence is reversed by vector execution.
    FissionSafeViolation {
        /// Event required to execute first.
        before: u64,
        /// Event required to execute second.
        after: u64,
    },
}
impl fmt::Display for VectorEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => write!(f, "vector plan verification failed: {error}"),
            Self::RoutedPlanMismatch => {
                write!(
                    f,
                    "routed FIR and event certificate use different vector plans"
                )
            }
            Self::StatePlanMismatch => {
                write!(f, "vector state and event certificates use different plans")
            }
            Self::InvalidLayout { detail, loop_id } => {
                write!(f, "invalid routed layout for loop {loop_id}: {detail}")
            }
            Self::EventBoundExceeded { needed, limit } => {
                write!(
                    f,
                    "bounded event model needs {needed} events, limit is {limit}"
                )
            }
            Self::EventLowerBoundExceeded { minimum, limit } => write!(
                f,
                "bounded event model requires at least {minimum} events, limit is {limit}"
            ),
            Self::EventCountOverflow => write!(f, "bounded event count overflowed"),
            Self::EventTableMismatch => write!(f, "event table does not match routed FIR"),
            Self::CompactRepetitionMismatch => {
                write!(f, "compact event repetition basis is not canonical")
            }
            Self::ScalarOrderMismatch => write!(f, "scalar event order is not canonical"),
            Self::VectorOrderMismatch => write!(f, "vector event order does not match routing"),
            Self::DependencyMismatch => write!(f, "event dependence relation is incomplete"),
            Self::InvalidOrder { which } => {
                write!(f, "{which} order is not a permutation of the event table")
            }
            Self::ScalarDependenceViolation { before, after } => write!(
                f,
                "dependence {before} -> {after} contradicts scalar execution"
            ),
            Self::FissionSafeViolation { before, after } => write!(
                f,
                "vector execution reverses scalar dependence {before} -> {after}"
            ),
        }
    }
}
impl std::error::Error for VectorEventError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            _ => None,
        }
    }
}
impl From<VectorPlanError> for VectorEventError {
    fn from(value: VectorPlanError) -> Self {
        Self::Plan(value)
    }
}
pub(super) type EventKey = (EventRegion, Option<u64>, VectorEventKind);
pub(super) fn event_region(region: VectorRegion) -> EventRegion {
    match region {
        VectorRegion::Control => EventRegion::Control,
        VectorRegion::Loop(loop_id) => EventRegion::Loop(loop_id),
    }
}
pub(super) fn event_use_source(source: RoutedUseSource) -> EventUseSource {
    match source {
        RoutedUseSource::Direct(VectorRegion::Control) => EventUseSource::Control,
        RoutedUseSource::Direct(VectorRegion::Loop(loop_id)) => EventUseSource::Loop(loop_id),
        RoutedUseSource::Transport(transport_id) => EventUseSource::Transport(transport_id),
    }
}

pub(super) fn add_dependency(
    dependencies: &mut BTreeSet<EventDependency>,
    before: u64,
    after: u64,
) {
    if before != after {
        dependencies.insert(EventDependency { before, after });
    }
}
